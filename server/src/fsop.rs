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

use std::fs::{self, File};
use std::io::{self, BufRead, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

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

fn extract_zip(archive: &Path, dest: &Path) -> OpResult<()> {
    let failed = |err: zip::result::ZipError| format!("Arşiv açılamadı: {err}");
    let file = File::open(archive).map_err(|err| describe(err, archive))?;
    let mut zip = zip::ZipArchive::new(file).map_err(failed)?;
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
        let mut output =
            fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&target).map_err(|err| describe(err, &target))?;
        io::copy(&mut entry, &mut output).map_err(|err| describe(err, &target))?;
    }
    Ok(())
}

fn extract_tar<R: Read>(reader: R, dest: &Path) -> OpResult<()> {
    let mut archive = tar::Archive::new(reader);
    archive.set_preserve_permissions(false);
    archive.set_preserve_ownerships(false);
    archive.set_overwrite(true);
    // `unpack`, hedef klasörün dışına yazmaya çalışan girdileri reddeder.
    archive.unpack(dest).map_err(|err| format!("Arşiv açılamadı: {err}"))
}

fn extract(path: &str, dest: &str) -> OpResult<Value> {
    let archive = resolve(path)?;
    let dest = resolve(dest)?;
    fs::create_dir_all(&dest).map_err(|err| describe(err, &dest))?;
    let name = archive.to_string_lossy().to_lowercase();
    let open = || File::open(&archive).map_err(|err| describe(err, &archive));
    if name.ends_with(".zip") {
        extract_zip(&archive, &dest)?;
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        extract_tar(flate2::read::GzDecoder::new(open()?), &dest)?;
    } else if name.ends_with(".tar") {
        extract_tar(open()?, &dest)?;
    } else {
        return Err("Desteklenen arşiv türleri: .zip, .tar.gz, .tgz, .tar".into());
    }
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
        Request::Extract { path, dest } => extract(path, dest),
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
