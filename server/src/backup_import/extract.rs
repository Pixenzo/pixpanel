//! Yedek arşivlerinin ve dökümlerin güvenli açılması.
//!
//! Yedek dosyası güvenilmeyen girdidir. Bu yüzden arşiv root olarak ama kısıtlı biçimde açılır:
//!
//! * Yol kaçışı (`..`) olan arşiv bütünüyle reddedilir; baştaki `/` atılır.
//! * Sembolik bağ, sabit bağ, aygıt dosyası ve boru hiçbir zaman oluşturulmaz (atlanır ve sayılır),
//!   dosyalar `O_NOFOLLOW` ile yazılır; böylece açılan ağaçta bağ bulunamaz.
//! * setuid/setgid/sticky bitleri atılır; sahiplik arşivden alınmaz (hepsi root'a ait olur,
//!   siteye kopyalanırken hesabın kullanıcısına geçer).
//! * `fsop` (dosya yöneticisi) ile aynı sınırlar: toplam boyut, girdi sayısı ve sıkıştırma oranı;
//!   ayrıca boyut sınırı sunucudaki boş yerin bir kısmıyla da kısılır.
//!
//! `.gz` dahilen, `.zst`, `.bz2` ve `.xz` sistemdeki `zstd`, `bzip2`, `xz` komutlarıyla açılır.

use std::cell::Cell;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;

use crate::fsop::{human_size, MAX_EXTRACT_BYTES, MAX_EXTRACT_ENTRIES, MAX_ZIP_RATIO, RATIO_MIN_SIZE};

/// Bir tar girdisinin başlığı ve dolgusu için bayt payı (fsop ile aynı hesap).
const HEADER_ALLOWANCE: u64 = 2048;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_bytes: u64,
    pub max_entries: u64,
    /// Sıkıştırılmış dosyanın boyutu; verilirse açılan veri bunun `MAX_ZIP_RATIO` katını aşamaz.
    pub compressed_len: Option<u64>,
}

impl Limits {
    /// Genel sınırlar; `dir` klasörünün bulunduğu diskte boş yerin en çok %80'i kullanılabilir.
    pub fn standard(dir: &Path) -> Self {
        let max_bytes = available_bytes(dir).map_or(MAX_EXTRACT_BYTES, |free| (free / 5 * 4).min(MAX_EXTRACT_BYTES));
        Self { max_bytes, max_entries: MAX_EXTRACT_ENTRIES, compressed_len: None }
    }

    fn overflow_message(&self) -> String {
        format!("Arşivden çıkan veri {} sınırını aştı; açma durduruldu (sunucudaki boş yer ya da genel sınır).", human_size(self.max_bytes))
    }
}

/// Klasörün bulunduğu diskte root dışı kullanıcıların kullanabileceği boş bayt sayısı.
pub fn available_bytes(dir: &Path) -> Option<u64> {
    let stat = nix::sys::statvfs::statvfs(dir).ok()?;
    Some((stat.blocks_available() as u64).saturating_mul(stat.fragment_size() as u64))
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Stats {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    /// Oluşturulmayan sembolik ve sabit bağlar.
    pub skipped_links: u64,
    /// Oluşturulmayan aygıt dosyası, boru gibi özel girdiler.
    pub skipped_special: u64,
}

impl Stats {
    pub fn summary(&self) -> String {
        let mut text = format!("{} dosya, {} klasör, {}", self.files, self.dirs, human_size(self.bytes));
        if self.skipped_links + self.skipped_special > 0 {
            text.push_str(&format!(" ({} bağ ve {} özel dosya güvenlik için atlandı)", self.skipped_links, self.skipped_special));
        }
        text
    }
}

// ---- Sıkıştırma ----

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Codec {
    Plain,
    Gzip,
    Zstd,
    Bzip2,
    Xz,
}

pub fn sniff(head: &[u8]) -> Codec {
    if head.starts_with(&[0x1f, 0x8b]) {
        Codec::Gzip
    } else if head.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Codec::Zstd
    } else if head.starts_with(b"BZh") {
        Codec::Bzip2
    } else if head.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
        Codec::Xz
    } else {
        Codec::Plain
    }
}

/// Sıkıştırması açılmış akış. Harici bir açıcı (`zstd` vb.) kullanıldıysa düşünce süreç sonlandırılır.
pub struct Decoded {
    reader: Box<dyn Read>,
    child: Option<Child>,
    pub compressed_len: u64,
}

impl Read for Decoded {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buffer)
    }
}

impl Drop for Decoded {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Decoded {
    /// Dosyayı sıkıştırma türünü ilk baytlarından tanıyarak açar.
    pub fn open(path: &Path) -> Result<Self, String> {
        let describe = |err: io::Error| format!("{} okunamadı: {err}", path.display());
        let mut file = File::open(path).map_err(describe)?;
        let compressed_len = file.metadata().map_err(describe)?.len();
        let mut head = [0u8; 8];
        let read = read_full(&mut file, &mut head).map_err(describe)?;
        let codec = sniff(&head[..read]);
        drop(file);
        let file = File::open(path).map_err(describe)?;
        match codec {
            Codec::Plain => Ok(Self { reader: Box::new(file), child: None, compressed_len }),
            Codec::Gzip => Ok(Self { reader: Box::new(flate2::read::MultiGzDecoder::new(io::BufReader::new(file))), child: None, compressed_len }),
            Codec::Zstd | Codec::Bzip2 | Codec::Xz => {
                drop(file);
                let (tool, package) = match codec {
                    Codec::Zstd => ("zstd", "zstd"),
                    Codec::Bzip2 => ("bzip2", "bzip2"),
                    _ => ("xz", "xz-utils"),
                };
                let mut command = Command::new(tool);
                command.arg("-dc").arg("--").arg(path).env_clear().env("PATH", "/usr/local/bin:/usr/bin:/bin").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
                let mut child = command.spawn().map_err(|err| {
                    if err.kind() == io::ErrorKind::NotFound {
                        format!("Bu arşiv {tool} ile sıkıştırılmış ama sunucuda {tool} komutu kurulu değil; {package} paketini kurup yeniden deneyin")
                    } else {
                        format!("{tool} başlatılamadı: {err}")
                    }
                })?;
                let stdout = child.stdout.take().ok_or_else(|| format!("{tool} çıktısı okunamadı"))?;
                Ok(Self { reader: Box::new(io::BufReader::new(stdout)), child: Some(child), compressed_len })
            }
        }
    }
}

/// Tampon dolana ya da akış bitene kadar okur (tek `read` az bayt dönebilir).
pub fn read_full<R: Read>(reader: &mut R, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    Ok(filled)
}

/// Başlık bloğu bir tar arşivine mi ait (`ustar` imzası)?
pub fn looks_like_tar(head: &[u8]) -> bool {
    head.len() >= 262 && &head[257..262] == b"ustar"
}

// ---- Yol denetimi ----

/// Arşiv içindeki yolu hedefe göre göreli ve güvenli yola çevirir. `..` içeren yol hata sayılır;
/// baştaki `/` ve `.` atılır.
pub fn safe_relative(path: &Path) -> Result<PathBuf, String> {
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir | Component::RootDir => {}
            Component::ParentDir | Component::Prefix(_) => {
                return Err(format!("Arşivde klasörün dışına çıkan bir yol var ({}); arşiv güvenli değil, açılmadı.", shown(path)));
            }
        }
    }
    if clean.as_os_str().len() > 4096 {
        return Err("Arşivde aşırı uzun bir yol var; arşiv açılmadı.".into());
    }
    Ok(clean)
}

fn shown(path: &Path) -> String {
    path.to_string_lossy().chars().filter(|ch| !ch.is_control()).take(80).collect()
}

/// Yedek içindeki göreli (`/` ayraçlı) yolu `base` altında güvenle çözer: `..` ve mutlak yol reddedilir,
/// bütün parçalar gerçek dosya/klasör olmalı (sembolik bağ yok). Var olan yolu döner.
pub fn resolve_inside(base: &Path, relative: &str) -> Result<PathBuf, String> {
    let rel = Path::new(relative);
    if rel.is_absolute() || relative.contains('\0') {
        return Err("Geçersiz yol".into());
    }
    let mut current = base.to_path_buf();
    for component in rel.components() {
        match component {
            Component::Normal(part) => current.push(part),
            Component::CurDir => {}
            _ => return Err("Geçersiz yol".into()),
        }
        let meta = fs::symlink_metadata(&current).map_err(|_| "Yedekte bu yol bulunamadı".to_string())?;
        if meta.file_type().is_symlink() {
            return Err("Geçersiz yol".into());
        }
    }
    Ok(current)
}

// ---- tar açma ----

#[derive(Clone, Copy, PartialEq)]
enum Overflow {
    None,
    Bytes,
    Ratio,
}

struct Counted<R> {
    inner: R,
    total: u64,
    max: u64,
    ratio_cap: Option<u64>,
    flag: Rc<Cell<Overflow>>,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.total = self.total.saturating_add(read as u64);
        if self.total > self.max {
            self.flag.set(Overflow::Bytes);
            return Err(io::Error::other("açılan veri sınırı aştı"));
        }
        if self.ratio_cap.is_some_and(|cap| self.total > RATIO_MIN_SIZE && self.total > cap) {
            self.flag.set(Overflow::Ratio);
            return Err(io::Error::other("sıkıştırma oranı sınırı aştı"));
        }
        Ok(read)
    }
}

fn dir_mode(mode: u32) -> u32 {
    (mode & 0o777) | 0o700
}

fn file_mode(mode: u32) -> u32 {
    (mode & 0o777) | 0o600
}

fn ensure_dir(path: &Path, mode: u32) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|err| format!("{} oluşturulamadı: {err}", shown(path)))?;
    fs::set_permissions(path, fs::Permissions::from_mode(dir_mode(mode))).map_err(|err| format!("{} izinleri ayarlanamadı: {err}", shown(path)))
}

/// tar akışını `dest` altına açar. Sınırlar ve güvenlik kuralları için modül açıklamasına bakın.
pub fn extract_tar_stream<R: Read>(reader: R, dest: &Path, limits: &Limits) -> Result<Stats, String> {
    let flag = Rc::new(Cell::new(Overflow::None));
    let ratio_cap = limits.compressed_len.map(|length| length.max(1).saturating_mul(MAX_ZIP_RATIO));
    let counting = Counted { inner: reader, total: 0, max: limits.max_bytes.saturating_add(limits.max_entries.saturating_mul(HEADER_ALLOWANCE)), ratio_cap, flag: Rc::clone(&flag) };
    let failed = |err: io::Error| match flag.get() {
        Overflow::Bytes => limits.overflow_message(),
        Overflow::Ratio => format!("Arşivin sıkıştırma oranı {MAX_ZIP_RATIO}:1'i aşıyor (zip bombası olabilir); açılmadı."),
        Overflow::None => format!("Arşiv açılamadı: {err}"),
    };
    let mut archive = tar::Archive::new(counting);
    let mut stats = Stats::default();
    let mut declared_total = 0u64;
    let mut entries = 0u64;
    for entry in archive.entries().map_err(failed)? {
        let mut entry = entry.map_err(failed)?;
        entries += 1;
        if entries > limits.max_entries {
            return Err(format!("Arşivde çok fazla dosya var; en çok {} girdi açılabilir.", limits.max_entries));
        }
        let kind = entry.header().entry_type();
        let path = entry.path().map_err(failed)?.into_owned();
        let relative = safe_relative(&path)?;
        let mode = entry.header().mode().unwrap_or(0o644);
        match kind {
            tar::EntryType::Directory => {
                if relative.as_os_str().is_empty() {
                    continue;
                }
                ensure_dir(&dest.join(&relative), mode)?;
                stats.dirs += 1;
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                if relative.as_os_str().is_empty() {
                    continue;
                }
                declared_total = declared_total.saturating_add(entry.size());
                if declared_total > limits.max_bytes {
                    return Err(limits.overflow_message());
                }
                let target = dest.join(&relative);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|err| format!("{} oluşturulamadı: {err}", shown(parent)))?;
                }
                let mut output = fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .custom_flags(nix::libc::O_NOFOLLOW)
                    .mode(0o600)
                    .open(&target)
                    .map_err(|err| format!("{} yazılamadı: {err}", shown(&relative)))?;
                let written = io::copy(&mut entry, &mut output).map_err(failed)?;
                drop(output);
                fs::set_permissions(&target, fs::Permissions::from_mode(file_mode(mode))).map_err(|err| format!("{} izinleri ayarlanamadı: {err}", shown(&relative)))?;
                stats.files += 1;
                stats.bytes += written;
            }
            tar::EntryType::Symlink | tar::EntryType::Link => stats.skipped_links += 1,
            tar::EntryType::XGlobalHeader | tar::EntryType::XHeader | tar::EntryType::GNULongName | tar::EntryType::GNULongLink => {}
            _ => stats.skipped_special += 1,
        }
    }
    Ok(stats)
}

/// tar arşivini (düz, gzip, zstd, bzip2 ya da xz) `dest` altına açar.
pub fn extract_file(archive: &Path, dest: &Path, mut limits: Limits) -> Result<Stats, String> {
    fs::create_dir_all(dest).map_err(|err| format!("{} oluşturulamadı: {err}", shown(dest)))?;
    let mut decoded = Decoded::open(archive)?;
    limits.compressed_len = Some(decoded.compressed_len);
    let mut head = vec![0u8; 512];
    let read = read_full(&mut decoded, &mut head).map_err(|err| format!("Arşiv okunamadı: {err}"))?;
    head.truncate(read);
    if !looks_like_tar(&head) {
        return Err("Dosya bir tar arşivi değil (tar, tar.gz, tar.zst ya da tgz bekleniyordu).".into());
    }
    extract_tar_stream(io::Cursor::new(head).chain(decoded), dest, &limits)
}

// ---- Klasör kopyalama ----

/// Zaten açılmış bir yedek klasörünü (ör. çıkarılmış Plesk yapısı) güvenli biçimde `dest` altına kopyalar:
/// bağlar ve özel dosyalar atlanır, sınırlar aynıdır.
pub fn copy_tree(src: &Path, dest: &Path, limits: &Limits) -> Result<Stats, String> {
    let mut stats = Stats::default();
    let mut entries = 0u64;
    fs::create_dir_all(dest).map_err(|err| format!("{} oluşturulamadı: {err}", shown(dest)))?;
    copy_level(src, dest, limits, &mut stats, &mut entries)?;
    Ok(stats)
}

fn copy_level(src: &Path, dest: &Path, limits: &Limits, stats: &mut Stats, entries: &mut u64) -> Result<(), String> {
    for item in fs::read_dir(src).map_err(|err| format!("{} okunamadı: {err}", shown(src)))? {
        let item = item.map_err(|err| format!("{} okunamadı: {err}", shown(src)))?;
        *entries += 1;
        if *entries > limits.max_entries {
            return Err(format!("Klasörde çok fazla dosya var; en çok {} girdi alınabilir.", limits.max_entries));
        }
        let meta = fs::symlink_metadata(item.path()).map_err(|err| format!("{} okunamadı: {err}", shown(&item.path())))?;
        let target = dest.join(item.file_name());
        let kind = meta.file_type();
        if kind.is_symlink() {
            stats.skipped_links += 1;
        } else if kind.is_dir() {
            ensure_dir(&target, meta.permissions().mode())?;
            stats.dirs += 1;
            copy_level(&item.path(), &target, limits, stats, entries)?;
        } else if kind.is_file() {
            if stats.bytes.saturating_add(meta.len()) > limits.max_bytes {
                return Err(limits.overflow_message());
            }
            let mut input = fs::OpenOptions::new().read(true).custom_flags(nix::libc::O_NOFOLLOW).open(item.path()).map_err(|err| format!("{} okunamadı: {err}", shown(&item.path())))?;
            let mut output = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&target).map_err(|err| format!("{} yazılamadı: {err}", shown(&target)))?;
            let written = io::copy(&mut input, &mut output).map_err(|err| format!("{} kopyalanamadı: {err}", shown(&target)))?;
            drop(output);
            fs::set_permissions(&target, fs::Permissions::from_mode(file_mode(meta.permissions().mode()))).map_err(|err| err.to_string())?;
            stats.files += 1;
            stats.bytes += written;
        } else {
            stats.skipped_special += 1;
        }
    }
    Ok(())
}

// ---- SQL dökümü ----

/// Döküm satırlarından yönetici yetkisi gerektirenler (veritabanı seçme/oluşturma/silme) çıkarılır:
/// içe aktarma yalnızca hedef veritabanına yetkili sınırlı hesapla yapılır ve zaten doğru veritabanına bağlıdır.
const DROPPED_PREFIXES: [&[u8]; 3] = [b"USE `", b"CREATE DATABASE", b"DROP DATABASE"];

/// Döküm akışını `output` dosyasına yazar; `USE`, `CREATE DATABASE` ve `DROP DATABASE` satırlarını atar.
/// Yazılan bayt sayısını döner; `max_bytes` aşılırsa hata verir.
pub fn write_sql<R: Read>(mut reader: R, output: &Path, max_bytes: u64) -> Result<u64, String> {
    #[derive(PartialEq)]
    enum Mode {
        Start,
        Pass,
        Skip,
    }
    let mut out = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(output).map_err(|err| format!("{} yazılamadı: {err}", shown(output)))?;
    let io_error = |err: io::Error| format!("Döküm yazılamadı: {err}");
    let longest = DROPPED_PREFIXES.iter().map(|prefix| prefix.len()).max().unwrap_or(0);
    let mut buffer = vec![0u8; 256 * 1024];
    let (mut mode, mut prefix) = (Mode::Start, Vec::<u8>::new());
    let mut written = 0u64;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(format!("Döküm okunamadı: {err}")),
        };
        let mut index = 0;
        while index < read {
            match mode {
                Mode::Pass => {
                    let end = buffer[index..read].iter().position(|byte| *byte == b'\n').map_or(read, |at| index + at + 1);
                    out.write_all(&buffer[index..end]).map_err(io_error)?;
                    written += (end - index) as u64;
                    if buffer[end - 1] == b'\n' {
                        mode = Mode::Start;
                    }
                    index = end;
                }
                Mode::Skip => {
                    match buffer[index..read].iter().position(|byte| *byte == b'\n') {
                        Some(at) => {
                            index += at + 1;
                            mode = Mode::Start;
                        }
                        None => index = read,
                    }
                }
                Mode::Start => {
                    let byte = buffer[index];
                    index += 1;
                    prefix.push(byte);
                    if byte == b'\n' || prefix.len() >= longest {
                        if DROPPED_PREFIXES.iter().any(|dropped| prefix.starts_with(dropped)) {
                            mode = if byte == b'\n' { Mode::Start } else { Mode::Skip };
                        } else {
                            out.write_all(&prefix).map_err(io_error)?;
                            written += prefix.len() as u64;
                            mode = if byte == b'\n' { Mode::Start } else { Mode::Pass };
                        }
                        prefix.clear();
                    }
                }
            }
            if written > max_bytes {
                return Err(format!("Döküm {} sınırını aştı; yüklenmedi.", human_size(max_bytes)));
            }
        }
    }
    // Sonu satır sonuyla bitmeyen kısa döküm.
    if !prefix.is_empty() && !DROPPED_PREFIXES.iter().any(|dropped| prefix.starts_with(dropped)) {
        out.write_all(&prefix).map_err(io_error)?;
        written += prefix.len() as u64;
    }
    out.flush().map_err(io_error)?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("pixpanel-bimp-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn limits() -> Limits {
        Limits { max_bytes: 1024 * 1024, max_entries: 100, compressed_len: None }
    }

    fn file_entry(builder: &mut tar::Builder<Vec<u8>>, path: &str, data: &[u8], mode: u32) {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(mode);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder.append_data(&mut header, path, data).unwrap();
    }

    fn raw_entry(builder: &mut tar::Builder<Vec<u8>>, raw_name: &str, kind: tar::EntryType, data: &[u8], link: Option<&str>) {
        let mut header = tar::Header::new_gnu();
        let name = raw_name.as_bytes();
        header.as_old_mut().name[..name.len()].copy_from_slice(name);
        if let Some(link) = link {
            header.as_old_mut().linkname[..link.len()].copy_from_slice(link.as_bytes());
        }
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(kind);
        header.set_cksum();
        builder.append(&header, data).unwrap();
    }

    #[test]
    fn extracts_files_and_normalizes_modes() {
        let mut builder = tar::Builder::new(Vec::new());
        raw_entry(&mut builder, "site/", tar::EntryType::Directory, b"", None);
        file_entry(&mut builder, "site/index.php", b"<?php", 0o4755);
        file_entry(&mut builder, "site/readme.txt", b"merhaba", 0o000);
        let bytes = builder.into_inner().unwrap();
        let dest = fresh_dir("ok");
        let stats = extract_tar_stream(io::Cursor::new(bytes), &dest, &limits()).unwrap();
        assert_eq!(stats.files, 2);
        assert_eq!(fs::read_to_string(dest.join("site/index.php")).unwrap(), "<?php");
        // setuid atılır, sahibe okuma/yazma garanti edilir.
        assert_eq!(fs::metadata(dest.join("site/index.php")).unwrap().permissions().mode() & 0o7777, 0o755 | 0o600);
        assert_eq!(fs::metadata(dest.join("site/readme.txt")).unwrap().permissions().mode() & 0o7777, 0o600);
        fs::remove_dir_all(&dest).unwrap();
    }

    #[test]
    fn path_escape_rejects_whole_archive() {
        for name in ["../evil.txt", "a/../../evil.txt"] {
            let mut builder = tar::Builder::new(Vec::new());
            file_entry(&mut builder, "iyi.txt", b"x", 0o644);
            raw_entry(&mut builder, name, tar::EntryType::Regular, b"kotu", None);
            let dest = fresh_dir("escape");
            let err = extract_tar_stream(io::Cursor::new(builder.into_inner().unwrap()), &dest, &limits()).unwrap_err();
            assert!(err.contains("dışına çıkan"), "{err}");
            assert!(!dest.parent().unwrap().join("evil.txt").exists());
            fs::remove_dir_all(&dest).unwrap();
        }
    }

    #[test]
    fn absolute_paths_are_stripped_not_followed() {
        let mut builder = tar::Builder::new(Vec::new());
        raw_entry(&mut builder, "/tmp/pixpanel-bimp-mutlak.txt", tar::EntryType::Regular, b"x", None);
        let dest = fresh_dir("abs");
        extract_tar_stream(io::Cursor::new(builder.into_inner().unwrap()), &dest, &limits()).unwrap();
        assert!(dest.join("tmp/pixpanel-bimp-mutlak.txt").is_file());
        assert!(!Path::new("/tmp/pixpanel-bimp-mutlak.txt").exists());
        fs::remove_dir_all(&dest).unwrap();
    }

    #[test]
    fn links_and_devices_are_never_created() {
        let mut builder = tar::Builder::new(Vec::new());
        raw_entry(&mut builder, "bag", tar::EntryType::Symlink, b"", Some("/etc/passwd"));
        raw_entry(&mut builder, "sabit", tar::EntryType::Link, b"", Some("bag"));
        raw_entry(&mut builder, "aygit", tar::EntryType::Char, b"", None);
        raw_entry(&mut builder, "boru", tar::EntryType::Fifo, b"", None);
        // Bağın ardından aynı adla dosya yazma girişimi bağı izlememeli.
        file_entry(&mut builder, "bag", b"duz dosya", 0o644);
        let dest = fresh_dir("links");
        let stats = extract_tar_stream(io::Cursor::new(builder.into_inner().unwrap()), &dest, &limits()).unwrap();
        assert_eq!((stats.skipped_links, stats.skipped_special, stats.files), (2, 2, 1));
        assert!(!fs::symlink_metadata(dest.join("bag")).unwrap().file_type().is_symlink());
        assert!(!dest.join("aygit").exists() && !dest.join("boru").exists() && !dest.join("sabit").exists());
        fs::remove_dir_all(&dest).unwrap();
    }

    #[test]
    fn entry_and_size_limits_apply() {
        let mut builder = tar::Builder::new(Vec::new());
        for index in 0..5 {
            file_entry(&mut builder, &format!("f{index}"), b"x", 0o644);
        }
        let bytes = builder.into_inner().unwrap();
        let dest = fresh_dir("entries");
        let small = Limits { max_entries: 3, ..limits() };
        assert!(extract_tar_stream(io::Cursor::new(bytes.clone()), &dest, &small).unwrap_err().contains("çok fazla"));
        let tiny = Limits { max_bytes: 3, ..limits() };
        assert!(extract_tar_stream(io::Cursor::new(bytes), &dest, &tiny).is_err());
        fs::remove_dir_all(&dest).unwrap();
    }

    #[test]
    fn gzip_bomb_ratio_is_rejected() {
        use flate2::write::GzEncoder;
        let mut builder = tar::Builder::new(Vec::new());
        file_entry(&mut builder, "sifir.bin", &vec![0u8; 8 * 1024 * 1024], 0o644);
        let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(&builder.into_inner().unwrap()).unwrap();
        let gz = encoder.finish().unwrap();
        let dir = fresh_dir("bomb");
        let archive = dir.join("bomba.tar.gz");
        fs::write(&archive, &gz).unwrap();
        // Gerçek oran 1000'in altında kalsa da küçültülmüş sınırla denenir: dosya boyutu yapay olarak küçük sayılır.
        let mut limited = limits();
        limited.max_bytes = 64 * 1024 * 1024;
        let mut decoded = Decoded::open(&archive).unwrap();
        limited.compressed_len = Some(1024);
        let err = extract_tar_stream(&mut decoded, &dir.join("out"), &limited).unwrap_err();
        assert!(err.contains("sıkıştırma oranı"), "{err}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extract_file_reads_gzip_and_rejects_non_tar() {
        use flate2::write::GzEncoder;
        let mut builder = tar::Builder::new(Vec::new());
        file_entry(&mut builder, "a/b.txt", b"merhaba", 0o644);
        let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&builder.into_inner().unwrap()).unwrap();
        let dir = fresh_dir("file");
        fs::write(dir.join("yedek.tar.gz"), encoder.finish().unwrap()).unwrap();
        fs::write(dir.join("duz.txt"), b"tar degil").unwrap();
        let stats = extract_file(&dir.join("yedek.tar.gz"), &dir.join("out"), limits()).unwrap();
        assert_eq!(stats.files, 1);
        assert_eq!(fs::read_to_string(dir.join("out/a/b.txt")).unwrap(), "merhaba");
        assert!(extract_file(&dir.join("duz.txt"), &dir.join("out2"), limits()).unwrap_err().contains("tar arşivi değil"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resolve_inside_rejects_escape_and_links() {
        let dir = fresh_dir("resolve");
        fs::create_dir_all(dir.join("a")).unwrap();
        fs::write(dir.join("a/b.txt"), "x").unwrap();
        std::os::unix::fs::symlink("/etc", dir.join("bag")).unwrap();
        assert!(resolve_inside(&dir, "a/b.txt").is_ok());
        for bad in ["../x", "a/../../x", "/etc/passwd", "bag", "bag/passwd", "yok"] {
            assert!(resolve_inside(&dir, bad).is_err(), "{bad}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sql_filter_drops_database_statements_only() {
        let dump = "-- baslik\nCREATE DATABASE /*!32312 IF NOT EXISTS*/ `x`;\nUSE `x`;\nCREATE TABLE t (a int);\nINSERT INTO t VALUES ('USE `y`;');\nDROP DATABASE x;\nUSE\n";
        let dir = fresh_dir("sql");
        let out = dir.join("d.sql");
        // Küçük parçalarla beslemek arabellek sınırlarını da sınar.
        struct Slow<'a>(&'a [u8]);
        impl Read for Slow<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                let take = self.0.len().min(buffer.len()).min(3);
                buffer[..take].copy_from_slice(&self.0[..take]);
                self.0 = &self.0[take..];
                Ok(take)
            }
        }
        write_sql(Slow(dump.as_bytes()), &out, 10_000).unwrap();
        assert_eq!(fs::read_to_string(&out).unwrap(), "-- baslik\nCREATE TABLE t (a int);\nINSERT INTO t VALUES ('USE `y`;');\nUSE\n");
        assert!(write_sql(io::Cursor::new(vec![b'a'; 100]), &dir.join("b.sql"), 10).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_tree_skips_links() {
        let dir = fresh_dir("copy");
        fs::create_dir_all(dir.join("src/d")).unwrap();
        fs::write(dir.join("src/d/f.txt"), "x").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", dir.join("src/bag")).unwrap();
        let stats = copy_tree(&dir.join("src"), &dir.join("dst"), &limits()).unwrap();
        assert_eq!((stats.files, stats.dirs, stats.skipped_links), (1, 1, 1));
        assert!(dir.join("dst/d/f.txt").is_file() && !dir.join("dst/bag").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn codec_is_sniffed() {
        assert_eq!(sniff(&[0x1f, 0x8b, 8]), Codec::Gzip);
        assert_eq!(sniff(&[0x28, 0xb5, 0x2f, 0xfd, 0]), Codec::Zstd);
        assert_eq!(sniff(b"BZh9"), Codec::Bzip2);
        assert_eq!(sniff(b"SELECT 1"), Codec::Plain);
    }
}
