//! Dosya yöneticisinin işlemlerini yürüten yardımcı süreç.
//!
//! Panel, kullanıcı dosyalarına kendi (root) yetkisiyle dokunmaz. Her işlem
//! için bu yardımcıyı başlatır; yardımcı önce hesabın klasörüne `chroot` yapar,
//! sonra hesabın uid/gid'sine düşer ve ancak ondan sonra isteği işler. Böylece:
//!
//! - yol denetimindeki bir hata ya da sembolik bağ, klasörün dışına çıkaramaz
//!   (çekirdek dışarıyı göstermez),
//! - dosya izinlerini panel değil çekirdek uygular.
//!
//! İletişim: standart girdinin ilk satırı JSON zarfıdır; yanıt standart
//! çıktıya tek satır JSON olarak yazılır. İndirmede bu satırı dosyanın ham
//! içeriği, yüklemede zarfı dosyanın ham içeriği izler.

use std::cell::Cell;
use std::fs::{self, File};
use std::io::{self, BufRead, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Düzenleyicide açılabilecek en büyük metin dosyası.
const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub struct Envelope {
    /// İşlemlerin hapsedileceği klasör (`/` ise hapis ve yetki düşürme uygulanmaz).
    pub root: String,
    pub uid: u32,
    pub gid: u32,
    /// Hesabın kalan disk kotası (bayt); kota yoksa boş. Yardımcı süreç veritabanını bilmediği için
    /// arşiv açma sınırı için paneli yürüten taraf verir. Kullanıcı isteğinden gelmez: zarfı yalnızca
    /// panel kurar (`Request` içinde böyle bir alan yoktur).
    #[serde(default)]
    pub disk_budget: Option<u64>,
    pub request: Request,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveFormat {
    Zip,
    TarGz,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    List { path: String },
    Read { path: String },
    Write { path: String, content: String, #[serde(default)] create_new: bool },
    Mkdir { path: String },
    Rename { from: String, to: String },
    Move { paths: Vec<String>, dest: String },
    Copy { paths: Vec<String>, dest: String },
    Delete { paths: Vec<String> },
    Chmod { paths: Vec<String>, mode: u32, #[serde(default)] recursive: bool },
    Compress { paths: Vec<String>, dest: String, format: ArchiveFormat },
    Extract { path: String, dest: String },
    /// Dosyayı (klasörse tar.gz olarak) standart çıktıya akıtır.
    Download { path: String },
    /// Standart girdiden gelen içeriği `dir/name` olarak kaydeder.
    Upload { dir: String, name: String, #[serde(default)] overwrite: bool },
}

#[derive(Debug, Serialize)]
struct Entry {
    name: String,
    /// file | dir | symlink | other
    kind: &'static str,
    size: u64,
    mode: u32,
    mtime: i64,
    uid: u32,
    gid: u32,
    /// Sembolik bağın gösterdiği yer.
    target: Option<String>,
}

type OpResult<T> = Result<T, String>;

/// G/Ç hatalarını kullanıcının anlayacağı Türkçe iletilere çevirir.
fn describe(err: io::Error, path: &Path) -> String {
    let shown = path.display();
    match err.kind() {
        io::ErrorKind::NotFound => format!("{shown} bulunamadı"),
        io::ErrorKind::PermissionDenied => format!("{shown} için yetkiniz yok"),
        io::ErrorKind::AlreadyExists => format!("{shown} zaten var"),
        io::ErrorKind::DirectoryNotEmpty => format!("{shown} boş değil"),
        io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => "Diskte yer kalmadı".to_string(),
        _ => format!("{shown}: {err}"),
    }
}

/// İstekteki yolu doğrular: mutlak olmalı, `..` içermemeli.
fn resolve(path: &str) -> OpResult<PathBuf> {
    let candidate = Path::new(path);
    if !candidate.is_absolute() || path.contains('\0') {
        return Err("Geçersiz yol".into());
    }
    let mut resolved = PathBuf::from("/");
    for component in candidate.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(part) => resolved.push(part),
            _ => return Err("Yol “..” içeremez".into()),
        }
    }
    Ok(resolved)
}

/// Dosya ya da klasör adı: tek bir yol parçası olmalı.
fn validate_name(name: &str) -> OpResult<()> {
    let valid = !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\0') && name.len() <= 255;
    if valid { Ok(()) } else { Err(format!("Geçersiz ad: {name}")) }
}

fn file_name(path: &Path) -> OpResult<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .ok_or_else(|| "Kök klasör üzerinde bu işlem yapılamaz".to_string())
}

fn entry_of(path: &Path, name: String) -> OpResult<Entry> {
    let meta = fs::symlink_metadata(path).map_err(|err| describe(err, path))?;
    let file_type = meta.file_type();
    let (kind, target) = if file_type.is_symlink() {
        ("symlink", fs::read_link(path).ok().map(|target| target.to_string_lossy().into_owned()))
    } else if file_type.is_dir() {
        ("dir", None)
    } else if file_type.is_file() {
        ("file", None)
    } else {
        ("other", None)
    };
    Ok(Entry { name, kind, size: meta.len(), mode: meta.mode() & 0o7777, mtime: meta.mtime(), uid: meta.uid(), gid: meta.gid(), target })
}

fn list(path: &str) -> OpResult<Value> {
    let dir = resolve(path)?;
    let mut entries = Vec::new();
    for item in fs::read_dir(&dir).map_err(|err| describe(err, &dir))? {
        let item = item.map_err(|err| describe(err, &dir))?;
        let name = item.file_name().to_string_lossy().into_owned();
        // Aynı anda silinen bir dosya listelemeyi bozmamalı.
        if let Ok(entry) = entry_of(&item.path(), name) {
            entries.push(entry);
        }
    }
    entries.sort_by(|a, b| (a.kind != "dir").cmp(&(b.kind != "dir")).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(json!({ "path": dir.to_string_lossy(), "entries": entries }))
}

fn read(path: &str) -> OpResult<Value> {
    let path = resolve(path)?;
    let meta = fs::metadata(&path).map_err(|err| describe(err, &path))?;
    if !meta.is_file() {
        return Err("Bu bir dosya değil".into());
    }
    if meta.len() > MAX_TEXT_BYTES {
        return Err("Dosya düzenleyicide açmak için çok büyük (en fazla 2 MB); indirip düzenleyin".into());
    }
    let bytes = fs::read(&path).map_err(|err| describe(err, &path))?;
    let content = String::from_utf8(bytes).map_err(|_| "Bu dosya metin değil; düzenleyicide açılamaz".to_string())?;
    Ok(json!({ "content": content, "mode": meta.mode() & 0o7777, "mtime": meta.mtime() }))
}

fn write(path: &str, content: &str, create_new: bool) -> OpResult<Value> {
    let path = resolve(path)?;
    if create_new {
        let mut file =
            fs::OpenOptions::new().write(true).create_new(true).mode(0o644).open(&path).map_err(|err| describe(err, &path))?;
        file.write_all(content.as_bytes()).map_err(|err| describe(err, &path))?;
    } else {
        // Var olan dosyanın izinleri ve sahipliği korunsun diye yerinde yazılır.
        fs::write(&path, content.as_bytes()).map_err(|err| describe(err, &path))?;
    }
    Ok(json!({}))
}

fn mkdir(path: &str) -> OpResult<Value> {
    let path = resolve(path)?;
    fs::create_dir(&path).map_err(|err| describe(err, &path))?;
    Ok(json!({}))
}

fn rename(from: &str, to: &str) -> OpResult<Value> {
    let (from, to) = (resolve(from)?, resolve(to)?);
    if fs::symlink_metadata(&to).is_ok() {
        return Err(format!("{} zaten var", to.display()));
    }
    fs::rename(&from, &to).map_err(|err| describe(err, &from))?;
    Ok(json!({}))
}

/// Hedef klasörün, taşınan/kopyalanan klasörün kendi içinde olmadığını denetler.
fn ensure_not_inside(source: &Path, dest_dir: &Path) -> OpResult<()> {
    if dest_dir.starts_with(source) {
        Err(format!("{} kendi içine taşınamaz ya da kopyalanamaz", source.display()))
    } else {
        Ok(())
    }
}

fn move_into(paths: &[String], dest: &str) -> OpResult<Value> {
    let dest_dir = resolve(dest)?;
    for path in paths {
        let source = resolve(path)?;
        ensure_not_inside(&source, &dest_dir)?;
        let target = dest_dir.join(file_name(&source)?);
        if fs::symlink_metadata(&target).is_ok() {
            return Err(format!("{} zaten var", target.display()));
        }
        match fs::rename(&source, &target) {
            Ok(()) => {}
            // Farklı dosya sistemleri arasında taşıma: kopyala, sonra sil.
            Err(err) if err.raw_os_error() == Some(18) => {
                copy_tree(&source, &target)?;
                remove(&source)?;
            }
            Err(err) => return Err(describe(err, &source)),
        }
    }
    Ok(json!({}))
}

fn copy_tree(source: &Path, target: &Path) -> OpResult<()> {
    let meta = fs::symlink_metadata(source).map_err(|err| describe(err, source))?;
    if meta.file_type().is_symlink() {
        let link = fs::read_link(source).map_err(|err| describe(err, source))?;
        std::os::unix::fs::symlink(link, target).map_err(|err| describe(err, target))?;
    } else if meta.is_dir() {
        fs::create_dir(target).map_err(|err| describe(err, target))?;
        for item in fs::read_dir(source).map_err(|err| describe(err, source))? {
            let item = item.map_err(|err| describe(err, source))?;
            copy_tree(&item.path(), &target.join(item.file_name()))?;
        }
        let _ = fs::set_permissions(target, fs::Permissions::from_mode(meta.mode() & 0o7777));
    } else if meta.is_file() {
        fs::copy(source, target).map_err(|err| describe(err, source))?;
    }
    Ok(())
}

/// Hedefte aynı ad varsa `ad (2).uzantı` biçiminde boş bir ad bulur.
fn unique_target(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if fs::symlink_metadata(&first).is_err() {
        return first;
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    (2..).map(|n| dir.join(format!("{stem} ({n}){extension}"))).find(|candidate| fs::symlink_metadata(candidate).is_err()).expect("boş ad bulunur")
}

fn copy_into(paths: &[String], dest: &str) -> OpResult<Value> {
    let dest_dir = resolve(dest)?;
    for path in paths {
        let source = resolve(path)?;
        ensure_not_inside(&source, &dest_dir)?;
        let target = unique_target(&dest_dir, &file_name(&source)?);
        copy_tree(&source, &target)?;
    }
    Ok(json!({}))
}

fn remove(path: &Path) -> OpResult<()> {
    let meta = fs::symlink_metadata(path).map_err(|err| describe(err, path))?;
    let result = if meta.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) };
    result.map_err(|err| describe(err, path))
}

fn delete(paths: &[String]) -> OpResult<Value> {
    for path in paths {
        let path = resolve(path)?;
        if path == Path::new("/") {
            return Err("Kök klasör silinemez".into());
        }
        remove(&path)?;
    }
    Ok(json!({}))
}

fn chmod_tree(path: &Path, mode: u32, recursive: bool) -> OpResult<()> {
    let meta = fs::symlink_metadata(path).map_err(|err| describe(err, path))?;
    // Sembolik bağın kendisinin izni olmaz; gösterdiği yere de dokunulmaz.
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    // Özyinelemede klasörlere, dosyada okuma izni olan her sınıf için geçiş (x) izni de eklenir;
    // yoksa 644 uygulanan klasörlere girilemez.
    let effective = if recursive && meta.is_dir() { mode | ((mode & 0o444) >> 2) } else { mode };
    fs::set_permissions(path, fs::Permissions::from_mode(effective)).map_err(|err| describe(err, path))?;
    if recursive && meta.is_dir() {
        for item in fs::read_dir(path).map_err(|err| describe(err, path))? {
            chmod_tree(&item.map_err(|err| describe(err, path))?.path(), mode, true)?;
        }
    }
    Ok(())
}

fn chmod(paths: &[String], mode: u32, recursive: bool) -> OpResult<Value> {
    if mode > 0o777 {
        return Err("İzin değeri 000-777 arasında olmalı".into());
    }
    for path in paths {
        chmod_tree(&resolve(path)?, mode, recursive)?;
    }
    Ok(json!({}))
}

/// Klasör ağacını gezip her dosya ve klasör için (gerçek yol, arşiv içi ad) üretir.
fn walk(path: &Path, archive_name: &Path, visit: &mut dyn FnMut(&Path, &Path, &fs::Metadata) -> OpResult<()>) -> OpResult<()> {
    let meta = fs::symlink_metadata(path).map_err(|err| describe(err, path))?;
    // Sembolik bağlar arşive alınmaz; arşivi açan tarafta beklenmedik yerlere yazılmasını önler.
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    visit(path, archive_name, &meta)?;
    if meta.is_dir() {
        for item in fs::read_dir(path).map_err(|err| describe(err, path))? {
            let item = item.map_err(|err| describe(err, path))?;
            walk(&item.path(), &archive_name.join(item.file_name()), visit)?;
        }
    }
    Ok(())
}

fn write_zip(sources: &[PathBuf], output: File) -> OpResult<()> {
    let failed = |err: zip::result::ZipError| format!("Arşiv oluşturulamadı: {err}");
    let mut writer = zip::ZipWriter::new(output);
    for source in sources {
        let top = PathBuf::from(file_name(source)?);
        walk(source, &top, &mut |path, name, meta| {
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(meta.mode() & 0o777)
                .large_file(meta.len() > u32::MAX as u64);
            let name = name.to_string_lossy();
            if meta.is_dir() {
                writer.add_directory(name, options).map_err(failed)?;
            } else if meta.is_file() {
                writer.start_file(name, options).map_err(failed)?;
                let mut file = File::open(path).map_err(|err| describe(err, path))?;
                io::copy(&mut file, &mut writer).map_err(|err| describe(err, path))?;
            }
            Ok(())
        })?;
    }
    writer.finish().map_err(failed)?;
    Ok(())
}

fn write_tar_gz<W: Write>(sources: &[PathBuf], output: W) -> OpResult<()> {
    let failed = |err: io::Error| format!("Arşiv oluşturulamadı: {err}");
    let encoder = flate2::write::GzEncoder::new(output, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    builder.follow_symlinks(false);
    for source in sources {
        let top = PathBuf::from(file_name(source)?);
        walk(source, &top, &mut |path, name, meta| {
            if meta.is_dir() {
                builder.append_dir(name, path).map_err(failed)
            } else if meta.is_file() {
                let mut file = File::open(path).map_err(|err| describe(err, path))?;
                builder.append_file(name, &mut file).map_err(failed)
            } else {
                Ok(())
            }
        })?;
    }
    builder.into_inner().and_then(|encoder| encoder.finish()).map_err(failed)?;
    Ok(())
}

fn compress(paths: &[String], dest: &str, format: ArchiveFormat) -> OpResult<Value> {
    if paths.is_empty() {
        return Err("Sıkıştırılacak bir şey seçilmedi".into());
    }
    let dest = resolve(dest)?;
    let sources: Vec<PathBuf> = paths.iter().map(|path| resolve(path)).collect::<OpResult<_>>()?;
    if sources.iter().any(|source| dest.starts_with(source)) {
        return Err("Arşiv, sıkıştırılan klasörün içine yazılamaz".into());
    }
    let output = fs::OpenOptions::new().write(true).create_new(true).mode(0o644).open(&dest).map_err(|err| describe(err, &dest))?;
    let result = match format {
        ArchiveFormat::Zip => write_zip(&sources, output),
        ArchiveFormat::TarGz => write_tar_gz(&sources, output),
    };
    if result.is_err() {
        let _ = fs::remove_file(&dest);
    }
    result.map(|()| json!({}))
}

// ---- Arşiv açma ----
//
// Zip bombasına karşı üç sınır vardır: toplam açılmış boyut (en çok 10 GB ve hesabın kalan disk
// kotası), girdi sayısı ve zip'te tek girdinin sıkıştırma oranı. Zip'in merkezi dizini hiçbir şey
// yazılmadan önce okunur; beyanlar yalan olabileceği için açma sırasında gerçek bayt sayısı da
// sayılır. Arşiv önce hedef klasörün içindeki geçici bir klasöre açılır, ancak sınırlar aşılmadan
// bittiyse hedefe taşınır; bu yüzden reddedilen ya da yarıda kesilen arşivden geride dosya kalmaz.
//
// Arşivdeki `..` içeren girdiler, zip'te sembolik bağlar ve hedef dışına çıkan yollar eskiden olduğu
// gibi atlanır (`enclosed_name` ve tar'ın `unpack_in` kuralları); bu davranış değişmedi.

/// Tek seferde açılabilecek en büyük toplam boyut.
const MAX_EXTRACT_BYTES: u64 = 10 * 1024 * 1024 * 1024;
/// Bir arşivden açılabilecek en çok girdi (dosya ve klasör).
const MAX_EXTRACT_ENTRIES: u64 = 200_000;
/// Zip'te tek girdinin açılmış/sıkıştırılmış boyut oranı sınırı.
const MAX_ZIP_RATIO: u64 = 1000;
/// Oran denetimi bundan küçük girdilere uygulanmaz (küçük bir metin dosyası yüksek oranla sıkışabilir;
/// toplam boyut ve girdi sayısı sınırları zaten tüm arşivi sınırlar).
const RATIO_MIN_SIZE: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq)]
struct ExtractLimits {
    max_bytes: u64,
    max_entries: u64,
    max_ratio: u64,
    ratio_min_size: u64,
    /// `max_bytes`, hesabın kalan disk kotasından geliyor (genel 10 GB sınırından küçük).
    quota_bound: bool,
}

/// Bayt sayısını okunur yazar (1,5 GB gibi).
fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let value = bytes as f64;
    if value >= KB * KB * KB {
        format!("{:.1} GB", value / (KB * KB * KB))
    } else if value >= KB * KB {
        format!("{:.1} MB", value / (KB * KB))
    } else if value >= KB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{bytes} bayt")
    }
}

/// Arşiv adını iletide güvenle göstermek için kısaltır ve denetim karakterlerini atar.
fn shown_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(80).collect()
}

impl ExtractLimits {
    /// `disk_budget`: hesabın kalan disk kotası (kota yoksa `None`).
    fn new(disk_budget: Option<u64>) -> Self {
        let max_bytes = disk_budget.map_or(MAX_EXTRACT_BYTES, |budget| budget.min(MAX_EXTRACT_BYTES));
        Self { max_bytes, max_entries: MAX_EXTRACT_ENTRIES, max_ratio: MAX_ZIP_RATIO, ratio_min_size: RATIO_MIN_SIZE, quota_bound: disk_budget.is_some_and(|budget| budget < MAX_EXTRACT_BYTES) }
    }

    fn check_entries(&self, count: u64) -> OpResult<()> {
        if count > self.max_entries {
            return Err(format!("Arşivde çok fazla dosya var; tek seferde en çok {} girdi açılabilir.", self.max_entries));
        }
        Ok(())
    }

    /// Arşivin beyan ettiği toplam açılmış boyutu denetler.
    fn check_declared(&self, total: u64) -> OpResult<()> {
        if total <= self.max_bytes {
            return Ok(());
        }
        Err(if self.quota_bound {
            format!("Arşivin açılmış boyutu en az {} olacak; hesabınızın kalan disk kotası {}. Yer açın ya da paketi yükseltin.", human_size(total), human_size(self.max_bytes))
        } else {
            format!("Arşivin açılmış boyutu en az {} olacak; tek seferde en çok {} açılabilir.", human_size(total), human_size(self.max_bytes))
        })
    }

    /// Zip girdisinin sıkıştırma oranını denetler.
    fn check_ratio(&self, name: &str, size: u64, compressed: u64) -> OpResult<()> {
        if size > self.ratio_min_size && size > compressed.max(1).saturating_mul(self.max_ratio) {
            return Err(format!(
                "“{}” girdisinin sıkıştırma oranı {}:1; {}:1'i aşan arşivler açılmaz (zip bombası olabilir).",
                shown_name(name),
                size / compressed.max(1),
                self.max_ratio
            ));
        }
        Ok(())
    }

    /// Açma sırasında gerçek veri sınırı aşıldığında verilen ileti.
    fn overflow_message(&self) -> String {
        let reason = if self.quota_bound { " Sınır, hesabınızın kalan disk kotasıdır." } else { "" };
        format!(
            "Arşivden çıkan gerçek veri {} sınırını aştı (arşivin beyan ettiği boyut gerçeği yansıtmıyor); açma durduruldu ve çıkarılan dosyalar silindi.{reason}",
            human_size(self.max_bytes)
        )
    }
}

/// Açma sırasında gerçekten yazılan bayt sayacı.
struct Written<'a> {
    limits: &'a ExtractLimits,
    total: u64,
}

impl Written<'_> {
    fn add(&mut self, bytes: u64) -> OpResult<()> {
        self.total = self.total.saturating_add(bytes);
        if self.total > self.limits.max_bytes { Err(self.limits.overflow_message()) } else { Ok(()) }
    }
}

/// Girdiyi hedefe kopyalar; hem girdinin beyan ettiği boyutu hem toplam sınırı aşan veriyi reddeder.
fn copy_counted(entry: &mut dyn Read, output: &mut File, declared: u64, written: &mut Written, target: &Path) -> OpResult<()> {
    let mut buffer = vec![0u8; 64 * 1024];
    let mut entry_total = 0u64;
    loop {
        let read = match entry.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(read) => read,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(describe(err, target)),
        };
        entry_total += read as u64;
        if entry_total > declared {
            return Err(format!("“{}” beyan edilenden büyük çıktı; arşiv bozuk ya da zararlı olabilir.", shown_name(&target.to_string_lossy())));
        }
        written.add(read as u64)?;
        output.write_all(&buffer[..read]).map_err(|err| describe(err, target))?;
    }
}

fn extract_zip(archive: &Path, dest: &Path, limits: &ExtractLimits) -> OpResult<()> {
    let failed = |err: zip::result::ZipError| format!("Arşiv açılamadı: {err}");
    let file = File::open(archive).map_err(|err| describe(err, archive))?;
    let mut zip = zip::ZipArchive::new(file).map_err(failed)?;
    limits.check_entries(zip.len() as u64)?;

    // Önce merkezi dizindeki beyanlar denetlenir; hiçbir şey yazılmadan reddedilir.
    let mut declared_total = 0u64;
    for index in 0..zip.len() {
        let entry = zip.by_index_raw(index).map_err(failed)?;
        if entry.is_dir() || entry.is_symlink() || entry.enclosed_name().is_none() {
            continue;
        }
        limits.check_ratio(entry.name(), entry.size(), entry.compressed_size())?;
        declared_total = declared_total.saturating_add(entry.size());
        limits.check_declared(declared_total)?;
    }

    let mut written = Written { limits, total: 0 };
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(failed)?;
        // `enclosed_name`, hedef klasörün dışına çıkmaya çalışan adları (../) eler.
        let Some(relative) = entry.enclosed_name() else { continue };
        let target = dest.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|err| describe(err, &target))?;
            continue;
        }
        if entry.is_symlink() {
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|err| describe(err, parent))?;
        }
        // setuid gibi özel bitler arşivden alınmaz.
        let mode = entry.unix_mode().map(|mode| mode & 0o777).unwrap_or(0o644);
        let declared = entry.size();
        let mut output =
            fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&target).map_err(|err| describe(err, &target))?;
        copy_counted(&mut entry, &mut output, declared, &mut written, &target)?;
    }
    Ok(())
}

/// Sıkıştırması açılmış tar akışında okunan baytları sayar; sınırı aşınca okumayı hata ile keser.
struct CountingReader<R> {
    inner: R,
    total: u64,
    limit: u64,
    exceeded: Rc<Cell<bool>>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.total = self.total.saturating_add(read as u64);
        if self.total > self.limit {
            self.exceeded.set(true);
            return Err(io::Error::other("açılan veri sınırı aştı"));
        }
        Ok(read)
    }
}

/// tar akışında önceden bir beyan listesi yoktur; başlıklar okundukça boyut ve girdi sayısı sayılır.
fn extract_tar<R: Read>(reader: R, dest: &Path, limits: &ExtractLimits) -> OpResult<()> {
    let exceeded = Rc::new(Cell::new(false));
    // Akıştaki başlık ve dolgu baytları da sayılır; verinin kendisi ayrıca başlıklardaki boyutlarla sınırlanır.
    let counting = CountingReader { inner: reader, total: 0, limit: limits.max_bytes.saturating_add(limits.max_entries.saturating_mul(2048)), exceeded: Rc::clone(&exceeded) };
    let mut archive = tar::Archive::new(counting);
    archive.set_preserve_permissions(false);
    archive.set_preserve_ownerships(false);
    archive.set_overwrite(true);
    let failed = |err: io::Error| if exceeded.get() { limits.overflow_message() } else { format!("Arşiv açılamadı: {err}") };

    // `Archive::unpack` ile aynı sıra: klasör girdileri en sona bırakılır ki kısıtlı izinli bir klasör
    // içindeki dosyaların yazılmasını engellemesin. `unpack_in`, hedef dışına çıkan yolları atlar.
    let (mut count, mut declared) = (0u64, 0u64);
    let mut directories = Vec::new();
    for entry in archive.entries().map_err(&failed)? {
        let mut entry = entry.map_err(&failed)?;
        count += 1;
        limits.check_entries(count)?;
        declared = declared.saturating_add(entry.size());
        limits.check_declared(declared)?;
        if entry.header().entry_type() == tar::EntryType::Directory {
            directories.push(entry);
        } else {
            entry.unpack_in(dest).map_err(&failed)?;
        }
    }
    directories.sort_by(|a, b| b.path_bytes().cmp(&a.path_bytes()));
    for mut directory in directories {
        directory.unpack_in(dest).map_err(&failed)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ArchiveKind {
    Zip,
    TarGz,
    Tar,
}

fn archive_kind(name: &str) -> Option<ArchiveKind> {
    let name = name.to_lowercase();
    if name.ends_with(".zip") {
        Some(ArchiveKind::Zip)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(ArchiveKind::TarGz)
    } else if name.ends_with(".tar") {
        Some(ArchiveKind::Tar)
    } else {
        None
    }
}

/// Geçici klasörün içeriğini hedefe taşır. Aynı adlı klasörler birleşir; aynı adlı dosya (ya da
/// sembolik bağ) yenisiyle değişir. Hedefteki sembolik bağın gösterdiği yere yazılmaz, bağın kendisi değişir.
fn merge_into(source: &Path, target: &Path) -> OpResult<()> {
    for item in fs::read_dir(source).map_err(|err| describe(err, source))? {
        let item = item.map_err(|err| describe(err, source))?;
        let (from, to) = (item.path(), target.join(item.file_name()));
        let from_is_dir = fs::symlink_metadata(&from).map(|meta| meta.is_dir()).unwrap_or(false);
        match fs::symlink_metadata(&to) {
            Ok(existing) if from_is_dir && existing.is_dir() => {
                merge_into(&from, &to)?;
                continue;
            }
            Ok(_) => remove(&to)?,
            Err(_) => {}
        }
        fs::rename(&from, &to).map_err(|err| describe(err, &to))?;
    }
    Ok(())
}

/// Var olmayan hedef yolun, ilk eksik olan (oluşturulacak) üst klasörü.
fn first_missing_ancestor(dest: &Path) -> Option<PathBuf> {
    let mut missing = None;
    let mut current = dest;
    while fs::symlink_metadata(current).is_err() {
        missing = Some(current.to_path_buf());
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }
    missing
}

fn extract_with(archive: &Path, dest: &Path, kind: ArchiveKind, limits: &ExtractLimits) -> OpResult<()> {
    let created = first_missing_ancestor(dest);
    fs::create_dir_all(dest).map_err(|err| describe(err, dest))?;
    let staging = dest.join(format!(".pixpanel-acma-{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir(&staging).map_err(|err| describe(err, &staging))?;
    let open = || File::open(archive).map_err(|err| describe(err, archive));
    let extracted = match kind {
        ArchiveKind::Zip => extract_zip(archive, &staging, limits),
        ArchiveKind::TarGz => open().and_then(|file| extract_tar(flate2::read::GzDecoder::new(file), &staging, limits)),
        ArchiveKind::Tar => open().and_then(|file| extract_tar(file, &staging, limits)),
    };
    let result = extracted.and_then(|()| merge_into(&staging, dest));
    let _ = fs::remove_dir_all(&staging);
    if result.is_err() {
        // Başarısız açma, kendi oluşturduğu klasörleri de geride bırakmaz.
        if let Some(new_dir) = created {
            let _ = fs::remove_dir_all(new_dir);
        }
    }
    result
}

fn extract(path: &str, dest: &str, disk_budget: Option<u64>) -> OpResult<Value> {
    let archive = resolve(path)?;
    let dest = resolve(dest)?;
    let kind = archive_kind(&archive.to_string_lossy()).ok_or_else(|| "Desteklenen arşiv türleri: .zip, .tar.gz, .tgz, .tar".to_string())?;
    extract_with(&archive, &dest, kind, &ExtractLimits::new(disk_budget))?;
    Ok(json!({}))
}

fn respond(value: &Value) {
    let mut stdout = io::stdout().lock();
    let _ = serde_json::to_writer(&mut stdout, value);
    let _ = stdout.write_all(b"\n");
    let _ = stdout.flush();
}

/// Başlık satırını, ardından dosyanın (klasörse tar.gz arşivinin) içeriğini yazar.
fn download(path: &str) -> OpResult<()> {
    let path = resolve(path)?;
    let meta = fs::metadata(&path).map_err(|err| describe(err, &path))?;
    let name = file_name(&path).unwrap_or_else(|_| "dosyalar".into());
    if meta.is_dir() {
        respond(&json!({ "ok": true, "data": { "name": format!("{name}.tar.gz"), "size": null } }));
        // Başlıktan sonra hata bildirilemez; arşiv yarım kalırsa istemci bozuk dosyadan anlar.
        let _ = write_tar_gz(&[path], io::stdout().lock());
    } else {
        let mut file = File::open(&path).map_err(|err| describe(err, &path))?;
        respond(&json!({ "ok": true, "data": { "name": name, "size": meta.len() } }));
        let _ = io::copy(&mut file, &mut io::stdout().lock());
    }
    Ok(())
}

fn upload(dir: &str, name: &str, overwrite: bool, input: &mut dyn Read) -> OpResult<Value> {
    validate_name(name)?;
    let dir = resolve(dir)?;
    let target = dir.join(name);
    if !overwrite && fs::symlink_metadata(&target).is_ok() {
        return Err(format!("{name} zaten var"));
    }
    // Yarım kalan yükleme hedef dosyayı bozmasın diye önce geçici ada yazılır.
    let temp = dir.join(format!(".{name}.pixpanel-yukleme-{}", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o644).open(&temp).map_err(|err| describe(err, &temp))?;
        let size = io::copy(input, &mut file).map_err(|err| describe(err, &target))?;
        fs::rename(&temp, &target).map_err(|err| describe(err, &target))?;
        Ok(json!({ "size": size }))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Hesabın klasörüne hapsolur ve yetkilerini kalıcı olarak düşürür.
fn confine(envelope: &Envelope) -> OpResult<()> {
    use nix::unistd::{chdir, chroot, setgid, setgroups, setuid, Gid, Uid};
    let failed = |what: &str, err: nix::Error| format!("{what} başarısız: {err}");
    nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(0o022));
    if envelope.root == "/" && envelope.uid == 0 {
        return chdir("/").map_err(|err| failed("chdir", err));
    }
    // Hapis ve yetki düşürme birbirinden ayrılmaz: biri olmadan diğeri uygulanmaz.
    if envelope.uid == 0 || envelope.gid == 0 || envelope.root == "/" {
        return Err("Geçersiz çalışma kapsamı".into());
    }
    chroot(Path::new(&envelope.root)).map_err(|err| failed("chroot", err))?;
    chdir("/").map_err(|err| failed("chdir", err))?;
    // Sıra önemli: ek gruplar ve gid, uid bırakılmadan önce değiştirilmelidir.
    setgroups(&[]).map_err(|err| failed("setgroups", err))?;
    setgid(Gid::from_raw(envelope.gid)).map_err(|err| failed("setgid", err))?;
    setuid(Uid::from_raw(envelope.uid)).map_err(|err| failed("setuid", err))?;
    // Yetkinin geri alınamadığı doğrulanır.
    if setuid(Uid::from_raw(0)).is_ok() {
        return Err("Yetkiler düşürülemedi".into());
    }
    Ok(())
}

/// Yardımcı sürecin giriş noktası; çıkış kodunu döner.
pub fn run_helper() -> i32 {
    let mut stdin = io::stdin().lock();
    let mut header = String::new();
    let envelope: Envelope = match stdin.read_line(&mut header).ok().and_then(|_| serde_json::from_str(&header).ok()) {
        Some(envelope) => envelope,
        None => {
            respond(&json!({ "ok": false, "error": "Geçersiz istek" }));
            return 2;
        }
    };
    if let Err(error) = confine(&envelope) {
        respond(&json!({ "ok": false, "error": error }));
        return 2;
    }

    let result = match &envelope.request {
        Request::List { path } => list(path),
        Request::Read { path } => read(path),
        Request::Write { path, content, create_new } => write(path, content, *create_new),
        Request::Mkdir { path } => mkdir(path),
        Request::Rename { from, to } => rename(from, to),
        Request::Move { paths, dest } => move_into(paths, dest),
        Request::Copy { paths, dest } => copy_into(paths, dest),
        Request::Delete { paths } => delete(paths),
        Request::Chmod { paths, mode, recursive } => chmod(paths, *mode, *recursive),
        Request::Compress { paths, dest, format } => compress(paths, dest, *format),
        Request::Extract { path, dest } => extract(path, dest, envelope.disk_budget),
        Request::Upload { dir, name, overwrite } => upload(dir, name, *overwrite, &mut stdin),
        Request::Download { path } => match download(path) {
            // İndirme yanıtını kendisi yazar.
            Ok(()) => return 0,
            Err(error) => Err(error),
        },
    };
    match result {
        Ok(data) => {
            respond(&json!({ "ok": true, "data": data }));
            0
        }
        Err(error) => {
            respond(&json!({ "ok": false, "error": error }));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sınırları küçültülmüş denetim: üretimdeki sabitler küçük arşivlerle sınanamaz, bu yüzden
    /// sınama sabitleri değil `ExtractLimits` değerini değiştirir (üretimde yapılandırma yoktur).
    fn small_limits() -> ExtractLimits {
        ExtractLimits { max_bytes: 10_000, max_entries: 5, max_ratio: 20, ratio_min_size: 1000, quota_bound: false }
    }

    /// Her sınama için boş, benzersiz bir klasör.
    fn fresh_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pixpanel-fsop-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Sıkıştırılamayan sahte rastgele veri (xorshift).
    fn noise(len: usize, seed: u64) -> Vec<u8> {
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect()
    }

    fn make_zip(path: &Path, entries: &[(&str, Vec<u8>)]) {
        let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }

    fn make_tar_gz(path: &Path, entries: &[(&str, Vec<u8>)]) {
        let encoder = flate2::write::GzEncoder::new(File::create(path).unwrap(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, data.as_slice()).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names = Vec::new();
        fn walk_names(dir: &Path, prefix: &str, names: &mut Vec<String>) {
            for item in fs::read_dir(dir).unwrap() {
                let item = item.unwrap();
                let name = format!("{prefix}{}", item.file_name().to_string_lossy());
                if item.file_type().unwrap().is_dir() {
                    walk_names(&item.path(), &format!("{name}/"), names);
                } else {
                    names.push(name);
                }
            }
        }
        walk_names(dir, "", &mut names);
        names.sort();
        names
    }

    #[test]
    fn limits_use_the_smaller_of_cap_and_quota() {
        let unlimited = ExtractLimits::new(None);
        assert_eq!((unlimited.max_bytes, unlimited.quota_bound), (10 * 1024 * 1024 * 1024, false));
        assert_eq!((unlimited.max_entries, unlimited.max_ratio), (200_000, 1000));
        let tight = ExtractLimits::new(Some(5 * 1024 * 1024));
        assert_eq!((tight.max_bytes, tight.quota_bound), (5 * 1024 * 1024, true));
        let roomy = ExtractLimits::new(Some(50 * 1024 * 1024 * 1024));
        assert_eq!((roomy.max_bytes, roomy.quota_bound), (10 * 1024 * 1024 * 1024, false));
        assert!(tight.check_declared(6 * 1024 * 1024).unwrap_err().contains("kota"));
        assert!(unlimited.check_declared(11 * 1024 * 1024 * 1024).unwrap_err().contains("en çok"));
        assert!(unlimited.check_declared(10 * 1024 * 1024 * 1024).is_ok());
    }

    #[test]
    fn entry_count_and_ratio_limits() {
        let limits = ExtractLimits::new(None);
        assert!(limits.check_entries(200_000).is_ok());
        assert!(limits.check_entries(200_001).is_err());
        // 100 MB açılmış, 90 KB sıkıştırılmış: oran ~1138:1.
        assert!(limits.check_ratio("a.bin", 100 * 1024 * 1024, 90 * 1024).unwrap_err().contains("zip bombası"));
        assert!(limits.check_ratio("a.bin", 100 * 1024 * 1024, 5 * 1024 * 1024).is_ok());
        // Küçük girdilerde oran denetlenmez; sıkıştırılmış boyutu sıfır olan büyük girdi reddedilir.
        assert!(limits.check_ratio("a.txt", 500 * 1024, 10).is_ok());
        assert!(limits.check_ratio("a.bin", 2 * 1024 * 1024, 0).is_err());
    }

    #[test]
    fn counter_stops_data_longer_than_declared() {
        let dir = fresh_dir("counter");
        let limits = small_limits();
        let mut output = File::create(dir.join("out")).unwrap();
        let mut written = Written { limits: &limits, total: 0 };
        let mut long = io::Cursor::new(vec![7u8; 100]);
        assert!(copy_counted(&mut long, &mut output, 50, &mut written, &dir.join("out")).unwrap_err().contains("beyan edilenden"));
        // Toplam sınır da ayrıca uygulanır.
        let mut written = Written { limits: &limits, total: 9_990 };
        let mut data = io::Cursor::new(vec![7u8; 100]);
        assert!(copy_counted(&mut data, &mut output, 100, &mut written, &dir.join("out")).unwrap_err().contains("gerçek veri"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ordinary_zip_is_extracted_and_merged() {
        let dir = fresh_dir("zip-ok");
        make_zip(&dir.join("a.zip"), &[("a.txt", b"merhaba".to_vec()), ("alt/b.txt", b"dunya".to_vec())]);
        let dest = dir.join("hedef");
        fs::create_dir_all(dest.join("alt")).unwrap();
        fs::write(dest.join("alt/eski.txt"), "kalir").unwrap();
        extract_with(&dir.join("a.zip"), &dest, ArchiveKind::Zip, &small_limits()).unwrap();
        assert_eq!(files_in(&dest), vec!["a.txt", "alt/b.txt", "alt/eski.txt"]);
        assert_eq!(fs::read_to_string(dest.join("a.txt")).unwrap(), "merhaba");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejected_zip_leaves_nothing_behind() {
        let dir = fresh_dir("zip-bad");
        // Girdi sayısı sınırı.
        let many: Vec<(String, Vec<u8>)> = (0..6).map(|n| (format!("f{n}.txt"), b"x".to_vec())).collect();
        let many_refs: Vec<(&str, Vec<u8>)> = many.iter().map(|(name, data)| (name.as_str(), data.clone())).collect();
        make_zip(&dir.join("cok.zip"), &many_refs);
        // Toplam boyut sınırı (iki dosya, ikisi de tek başına sınırın altında).
        // Sıkışmayan veri kullanılır ki oran sınırı değil toplam boyut sınırı devreye girsin.
        make_zip(&dir.join("buyuk.zip"), &[("a.bin", noise(6000, 1)), ("b.bin", noise(6000, 2))]);
        // Oran sınırı: 100 KB'lık sıfır dosyası.
        make_zip(&dir.join("bomba.zip"), &[("sifir.bin", vec![0u8; 100 * 1024])]);
        for (name, wanted) in [("cok.zip", "çok fazla"), ("buyuk.zip", "açılmış boyutu"), ("bomba.zip", "sıkıştırma oranı")] {
            let dest = dir.join(format!("hedef-{name}"));
            let error = extract_with(&dir.join(name), &dest, ArchiveKind::Zip, &small_limits()).unwrap_err();
            assert!(error.contains(wanted), "{name}: {error}");
            assert!(!dest.exists(), "{name}: hedef klasör geride kalmamalı");
        }
        // Var olan hedefe de dosya bırakılmaz; geçici klasör de silinir.
        let dest = dir.join("var");
        fs::create_dir_all(&dest).unwrap();
        assert!(extract_with(&dir.join("bomba.zip"), &dest, ArchiveKind::Zip, &small_limits()).is_err());
        assert!(files_in(&dest).is_empty() && fs::read_dir(&dest).unwrap().count() == 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn zip_entries_cannot_escape_the_destination() {
        let dir = fresh_dir("zip-escape");
        make_zip(&dir.join("kotu.zip"), &[("../disari.txt", b"x".to_vec()), ("/mutlak.txt", b"x".to_vec()), ("iyi.txt", b"x".to_vec())]);
        let dest = dir.join("hedef");
        extract_with(&dir.join("kotu.zip"), &dest, ArchiveKind::Zip, &small_limits()).unwrap();
        assert_eq!(files_in(&dest), vec!["iyi.txt"]);
        assert!(!dir.join("disari.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tar_gz_is_counted_while_extracting() {
        let dir = fresh_dir("tar");
        make_tar_gz(&dir.join("a.tar.gz"), &[("a.txt", b"merhaba".to_vec()), ("alt/b.txt", b"dunya".to_vec())]);
        let dest = dir.join("hedef");
        extract_with(&dir.join("a.tar.gz"), &dest, ArchiveKind::TarGz, &small_limits()).unwrap();
        assert_eq!(files_in(&dest), vec!["a.txt", "alt/b.txt"]);

        let many: Vec<(String, Vec<u8>)> = (0..6).map(|n| (format!("f{n}.txt"), b"x".to_vec())).collect();
        let many_refs: Vec<(&str, Vec<u8>)> = many.iter().map(|(name, data)| (name.as_str(), data.clone())).collect();
        make_tar_gz(&dir.join("cok.tar.gz"), &many_refs);
        make_tar_gz(&dir.join("buyuk.tar.gz"), &[("a.bin", vec![1u8; 6000]), ("b.bin", vec![2u8; 6000])]);
        for (name, wanted) in [("cok.tar.gz", "çok fazla"), ("buyuk.tar.gz", "açılmış boyutu")] {
            let dest = dir.join(format!("hedef-{name}"));
            let error = extract_with(&dir.join(name), &dest, ArchiveKind::TarGz, &small_limits()).unwrap_err();
            assert!(error.contains(wanted), "{name}: {error}");
            assert!(!dest.exists(), "{name}: hedef klasör geride kalmamalı");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn counting_reader_cuts_off_decoded_stream() {
        let exceeded = Rc::new(Cell::new(false));
        let mut reader = CountingReader { inner: io::Cursor::new(vec![0u8; 100]), total: 0, limit: 50, exceeded: Rc::clone(&exceeded) };
        let mut sink = Vec::new();
        assert!(io::copy(&mut reader, &mut sink).is_err());
        assert!(exceeded.get());
    }

    #[test]
    fn archive_kinds_are_recognised_case_insensitively() {
        assert_eq!(archive_kind("/a/B.ZIP"), Some(ArchiveKind::Zip));
        assert_eq!(archive_kind("/a/b.tar.gz"), Some(ArchiveKind::TarGz));
        assert_eq!(archive_kind("/a/b.tgz"), Some(ArchiveKind::TarGz));
        assert_eq!(archive_kind("/a/b.tar"), Some(ArchiveKind::Tar));
        assert_eq!(archive_kind("/a/b.rar"), None);
    }
}
