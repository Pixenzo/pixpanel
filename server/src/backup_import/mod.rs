//! Başka panelin yedek dosyasından içe aktarma (cPanel, Plesk, HestiaCP).
//!
//! Akış iki adımdır ve ikisi de yalnızca yönetici içindir:
//!
//! 1. **İnceleme** (`start_inspect`): yedek arşivi (ya da açılmış klasör) `<veri>/tmp/import-<kimlik>/raw`
//!    altına güvenli biçimde açılır (bkz. `extract`), biçimi tanınır ve içindekiler `manifest.json` olarak
//!    yazılır. Kullanıcı bulunan alan adlarını ve veritabanlarını önizlemede görür.
//! 2. **İçe aktarma** (`start_import`): seçilen her alan adı için site (yoksa) hesapta açılır, dosyalar
//!    yayın klasörüne hesabın kendi kullanıcısıyla yazılır, veritabanları oluşturulup dökümler panelin
//!    kısıtlı içe aktarma yolundan yüklenir, WordPress'in `wp-config.php`'si (ya da `.env`) yeni
//!    veritabanına çevrilir. Yarıda kalırsa bu işlemde oluşturulan siteler ve veritabanları geri alınır.
//!
//! Güvenlik:
//! * Yedekten gelen hiçbir yol ya da ad doğrudan kullanılmaz; içe aktarma yalnızca incelemede üretilen
//!   manifestteki yolları, yeniden doğrulayarak çözer (`extract::resolve_inside`).
//! * Dosyalar siteye `tar` boru hattıyla, hesabın kendi uid'siyle açılarak yazılır; kullanıcının klasörüne
//!   root olarak yazılmaz. setuid/setgid bitleri ve bağlar açma aşamasında atılır.
//! * Döküm, `dbms::import` yolundan (yetkisiz kullanıcı + sınırlı veritabanı hesabı) yüklenir.
//! * Geçici klasör yalnızca root'a açıktır; 24 saat sonra kendiliğinden silinir.
//!
//! Posta kutuları ve DNS içe aktarılmaz; kutu adresleri önizlemede yalnızca bilgi olarak gösterilir.

pub mod cpanel;
pub mod extract;
pub mod hestia;
pub mod manifest;
pub mod plesk;
mod xml;

use std::collections::HashSet;
use std::io::Read;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::auth::{self, AuthUser};
use crate::config::Config;
use crate::db::now;
use crate::dbms::{self, Engine};
use crate::error::{AppError, AppResult};
use crate::hosting::php;
use crate::hosting::sites::{self, NewSite, Site};
use crate::hosting::sysuser::{self, SysUser};
use crate::jobs::JobCtx;
use crate::state::AppState;
use crate::sys::exec::base_command;
use crate::sys::runas;
use crate::wordpress::{self, ConfigDatabase};
use manifest::{BackupManifest, ManifestDatabase, ManifestDomain};

/// Aynı anda tutulabilecek en çok inceleme (her biri disk kaplar).
const MAX_INSPECTIONS: usize = 4;
/// Geçici klasörün ömrü.
const EXPIRE_SECS: i64 = 24 * 3600;
/// İçe aktarma kilidinin bu süreden sonra bayat sayıldığı (panel yarıda kapanmıştır).
const LOCK_STALE: Duration = Duration::from_secs(12 * 3600);
const MAX_ITEMS: usize = 100;
const QUICK: Duration = Duration::from_secs(60);

fn bad(message: impl Into<String>) -> AppError {
    AppError::bad(message.into())
}

// ---- Geçici klasör ve üst veri ----

pub fn work_root(cfg: &Config) -> PathBuf {
    cfg.data_dir.join("tmp")
}

/// İnceleme kimliği: `auth::random_token` çıktısı (küçük harfli onaltılık). Yola girdiği için sıkı denetlenir.
pub fn valid_id(id: &str) -> bool {
    (16..=64).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn inspect_dir(cfg: &Config, id: &str) -> AppResult<PathBuf> {
    if !valid_id(id) {
        return Err(AppError::NotFound);
    }
    let dir = work_root(cfg).join(format!("import-{id}"));
    if dir.is_dir() { Ok(dir) } else { Err(AppError::NotFound) }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Meta {
    user_id: i64,
    source: String,
    created_at: i64,
    /// `running`, `ready` ya da `failed`.
    status: String,
    error: Option<String>,
}

fn read_meta(dir: &Path) -> Option<Meta> {
    serde_json::from_str(&std::fs::read_to_string(dir.join("meta.json")).ok()?).ok()
}

fn write_meta(dir: &Path, meta: &Meta) -> AppResult<()> {
    let text = serde_json::to_string(meta).map_err(|err| AppError::internal(err.to_string()))?;
    let staging = dir.join("meta.json.new");
    std::fs::write(&staging, text)?;
    std::fs::rename(&staging, dir.join("meta.json"))?;
    Ok(())
}

/// İnceleme görevinin kimliği ayrı dosyada tutulur: görev üst veriyi güncellerken yarışmasın.
fn read_job_id(dir: &Path) -> Option<i64> {
    std::fs::read_to_string(dir.join("job")).ok()?.trim().parse().ok()
}

fn read_manifest(dir: &Path) -> Option<BackupManifest> {
    serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).ok()?).ok()
}

/// Çalışan bir içe aktarmanın kilidi; düşünce silinir.
struct ImportLock(PathBuf);

impl ImportLock {
    fn path(dir: &Path) -> PathBuf {
        dir.join("import.lock")
    }

    fn is_active(dir: &Path) -> bool {
        std::fs::metadata(Self::path(dir)).and_then(|meta| meta.modified()).ok().and_then(|time| time.elapsed().ok()).is_some_and(|age| age < LOCK_STALE)
    }

    fn acquire(dir: &Path) -> AppResult<Self> {
        let path = Self::path(dir);
        if Self::is_active(dir) {
            return Err(AppError::conflict("Bu yedek için bir içe aktarma zaten sürüyor"));
        }
        let _ = std::fs::remove_file(&path);
        std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|_| AppError::conflict("Bu yedek için bir içe aktarma zaten sürüyor"))?;
        Ok(Self(path))
    }
}

impl Drop for ImportLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// ---- İnceleme ----

/// İncelenecek yedek: sunucudaki mutlak yol (dosya ya da klasör).
fn validate_source(cfg: &Config, path: &str) -> AppResult<PathBuf> {
    let path = path.trim();
    if path.is_empty() || path.len() > 4096 || path.contains('\0') {
        return Err(bad("Yedek dosyasının yolunu yazın"));
    }
    let candidate = Path::new(path);
    if !candidate.is_absolute() || candidate.components().any(|component| matches!(component, Component::ParentDir)) {
        return Err(bad("Yol mutlak olmalı (ör. /home/hesap/yedek.tar.gz) ve “..” içermemeli"));
    }
    let resolved = std::fs::canonicalize(candidate).map_err(|_| bad(format!("{path} bulunamadı ya da okunamıyor")))?;
    let meta = std::fs::metadata(&resolved).map_err(|_| bad(format!("{path} okunamıyor")))?;
    if !(meta.is_file() || meta.is_dir()) {
        return Err(bad("Yol bir dosya ya da klasör olmalı"));
    }
    // Panelin kendi verisi ve ayarları yedek olarak okunmaz.
    let protected = [std::fs::canonicalize(&cfg.data_dir).unwrap_or_else(|_| cfg.data_dir.clone()), PathBuf::from("/etc/pixpanel")];
    if protected.iter().any(|dir| resolved.starts_with(dir)) {
        return Err(bad("Panelin kendi klasörleri yedek olarak kullanılamaz"));
    }
    Ok(resolved)
}

/// Disk tutan (başarısız olmayan) inceleme sayısı; başarısız incelemenin açılmış verisi zaten silinir.
fn count_inspections(cfg: &Config) -> usize {
    std::fs::read_dir(work_root(cfg))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("import-"))
                .filter(|entry| read_meta(&entry.path()).is_none_or(|meta| meta.status != "failed"))
                .count()
        })
        .unwrap_or(0)
}

/// İnceleme görevini başlatır; (inceleme kimliği, görev kimliği) döner. Yönetici denetimi çağırana aittir.
pub fn start_inspect(state: &AppState, user: &AuthUser, path: &str) -> AppResult<(String, i64)> {
    let source = validate_source(&state.cfg, path)?;
    let root = work_root(&state.cfg);
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&root)?;
    if count_inspections(&state.cfg) >= MAX_INSPECTIONS {
        return Err(AppError::conflict(format!("Aynı anda en çok {MAX_INSPECTIONS} yedek incelemesi tutulur; işiniz biten incelemeleri silin")));
    }
    let id = auth::random_token(12);
    let dir = root.join(format!("import-{id}"));
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    let meta = Meta { user_id: user.id, source: source.to_string_lossy().into_owned(), created_at: now(), status: "running".into(), error: None };
    write_meta(&dir, &meta)?;

    let name = source.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "yedek".into());
    let title = format!("Yedek inceleme: {name}");
    let (job_state, job_id_value, job_source) = (state.clone(), id.clone(), source);
    let job = state.jobs.start("migrate.backup-inspect", &title, Some(user.id), move |ctx| async move { inspect_job(ctx, job_state, job_id_value, job_source).await });
    match job {
        Ok(job_id) => {
            std::fs::write(dir.join("job"), job_id.to_string())?;
            Ok((id, job_id))
        }
        Err(err) => {
            let _ = std::fs::remove_dir_all(&dir);
            Err(err)
        }
    }
}

async fn inspect_job(ctx: JobCtx, state: AppState, id: String, source: PathBuf) -> AppResult<()> {
    let dir = inspect_dir(&state.cfg, &id)?;
    ctx.log(format!("Yedek inceleniyor: {}", source.display()));
    let worker = {
        let (ctx, dir, source) = (ctx.clone(), dir.clone(), source.clone());
        tokio::task::spawn_blocking(move || -> Result<BackupManifest, String> {
            let raw = dir.join("raw");
            let limits = extract::Limits::standard(&dir);
            ctx.log(format!("Açma sınırı: en çok {} ({} girdi)", crate::fsop::human_size(limits.max_bytes), limits.max_entries));
            let stats = if source.is_dir() { extract::copy_tree(&source, &raw, &limits)? } else { extract::extract_file(&source, &raw, limits)? };
            ctx.log(format!("Yedek açıldı: {}", stats.summary()));
            let manifest = manifest::parse_backup(&raw)?;
            let text = serde_json::to_string(&manifest).map_err(|err| err.to_string())?;
            std::fs::write(dir.join("manifest.json"), text).map_err(|err| format!("Önizleme yazılamadı: {err}"))?;
            Ok(manifest)
        })
        .await
    };
    let outcome = match worker {
        Ok(result) => result,
        Err(err) => Err(format!("İnceleme beklenmedik biçimde sonlandı: {err}")),
    };
    match outcome {
        Ok(manifest) => {
            ctx.log(format!(
                "Biçim: {}. Bulunan: {} alan adı, {} veritabanı, {} posta kutusu.",
                manifest.format,
                manifest.domains.len(),
                manifest.databases.len(),
                manifest.mailboxes.len()
            ));
            for note in &manifest.notes {
                ctx.log(format!("Not: {note}"));
            }
            if let Some(mut meta) = read_meta(&dir) {
                meta.status = "ready".into();
                write_meta(&dir, &meta)?;
            }
            Ok(())
        }
        Err(message) => {
            // Başarısız incelemenin açılmış verisi diski tutmasın.
            let _ = std::fs::remove_dir_all(dir.join("raw"));
            if let Some(mut meta) = read_meta(&dir) {
                meta.status = "failed".into();
                meta.error = Some(message.clone());
                let _ = write_meta(&dir, &meta);
            }
            Err(AppError::bad(message))
        }
    }
}

#[derive(Serialize)]
pub struct Inspection {
    pub inspect_id: String,
    /// `running`, `ready` ya da `failed`.
    pub status: String,
    pub error: Option<String>,
    pub job_id: Option<i64>,
    pub source: String,
    pub created_at: i64,
    pub manifest: Option<BackupManifest>,
}

pub fn get_inspection(state: &AppState, id: &str) -> AppResult<Inspection> {
    let dir = inspect_dir(&state.cfg, id)?;
    let meta = read_meta(&dir).ok_or(AppError::NotFound)?;
    let (mut status, mut error) = (meta.status.clone(), meta.error.clone());
    if status == "running" {
        // Panel yeniden başladıysa görev yarıda kesilmiştir; sonsuza dek "çalışıyor" görünmesin.
        if let Some(job_id) = read_job_id(&dir) {
            if let Ok((job, _)) = state.jobs.get(job_id) {
                if job.status == "failed" {
                    status = "failed".into();
                    error = Some("İnceleme görevi tamamlanamadı; görev günlüğüne bakın".into());
                }
            }
        }
    }
    let manifest = if status == "ready" { read_manifest(&dir) } else { None };
    Ok(Inspection { inspect_id: id.to_string(), status, error, job_id: read_job_id(&dir), source: meta.source, created_at: meta.created_at, manifest })
}

pub fn delete_inspection(state: &AppState, id: &str) -> AppResult<()> {
    let dir = inspect_dir(&state.cfg, id)?;
    if ImportLock::is_active(&dir) {
        return Err(AppError::conflict("Bu yedek için bir içe aktarma sürüyor; bitince silin"));
    }
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

/// Ömrünü doldurmuş geçici klasörleri siler (periyodik temizlik döngüsünden çağrılır).
pub fn cleanup_expired(cfg: &Config) -> usize {
    let Ok(entries) = std::fs::read_dir(work_root(cfg)) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("import-") {
            continue;
        }
        let dir = entry.path();
        let created = read_meta(&dir)
            .map(|meta| meta.created_at)
            .or_else(|| entry.metadata().ok().and_then(|meta| meta.modified().ok()).and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map(|age| age.as_secs() as i64))
            .unwrap_or(0);
        if now() - created > EXPIRE_SECS && !ImportLock::is_active(&dir) && std::fs::remove_dir_all(&dir).is_ok() {
            removed += 1;
        }
    }
    removed
}

// ---- İçe aktarma isteği ----

#[derive(Debug, Deserialize)]
pub struct ImportItem {
    /// Yedekteki alan adı (manifestteki ad).
    pub domain: String,
    /// Sitenin bu sunucuda açılacağı ad; verilmezse `domain`.
    pub target_domain: Option<String>,
    /// İçe aktarılacak veritabanları (manifestteki adlar).
    #[serde(default)]
    pub databases: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ImportRequest {
    pub inspect_id: String,
    pub owner_user_id: i64,
    pub items: Vec<ImportItem>,
}

struct PlanItem {
    domain: ManifestDomain,
    target: String,
    databases: Vec<ManifestDatabase>,
    /// Hedef ad hesapta zaten site olarak varsa o site (dosyalar içine kopyalanır).
    existing: Option<Site>,
}

struct Plan {
    dir: PathBuf,
    owner_id: i64,
    items: Vec<PlanItem>,
}

/// İsteği doğrular ve içe aktarma görevini başlatır; görev kimliğini döner. Yönetici denetimi çağırana aittir.
pub async fn start_import(state: &AppState, user: &AuthUser, request: ImportRequest) -> AppResult<i64> {
    let dir = inspect_dir(&state.cfg, &request.inspect_id)?;
    let meta = read_meta(&dir).ok_or(AppError::NotFound)?;
    if meta.status != "ready" {
        return Err(bad("Yedek henüz incelenmedi ya da inceleme başarısız oldu"));
    }
    let manifest = read_manifest(&dir).ok_or_else(|| bad("Yedeğin önizlemesi bulunamadı; yedeği yeniden inceleyin"))?;
    if request.items.is_empty() || request.items.len() > MAX_ITEMS {
        return Err(bad("İçe aktarılacak en az bir alan adı seçin"));
    }

    let owner_id = request.owner_user_id;
    let (owner_name, owner_role): (String, String) = state
        .db
        .call(|conn| conn.query_row("SELECT username, role FROM users WHERE id = ?1", params![owner_id], |row| Ok((row.get(0)?, row.get(1)?))))
        .map_err(|_| bad("Seçilen hesap bulunamadı"))?;
    if owner_role == "member" {
        return Err(bad("Alt hesaba içe aktarılamaz; ana hesabı seçin"));
    }

    let mut items = Vec::new();
    let (mut targets, mut taken_databases): (HashSet<String>, HashSet<String>) = (HashSet::new(), HashSet::new());
    let mut wants_database = false;
    let mut new_sites = 0usize;
    let existing_sites = sites::list_by_user(&state.db, owner_id)?;
    for item in &request.items {
        let domain = manifest.domains.iter().find(|known| known.name == item.domain).ok_or_else(|| bad(format!("{} yedekte yok", item.domain)))?.clone();
        if domain.docroot_path.is_none() {
            return Err(bad(format!("{} için yedekte site içeriği bulunamadı; bu alan adı içe aktarılamaz", domain.name)));
        }
        let target = crate::hosting::domain::normalize(item.target_domain.as_deref().filter(|name| !name.trim().is_empty()).unwrap_or(&item.domain)).map_err(|_| {
            bad(format!("“{}” geçerli bir alan adı değil; hedef alan adını yazın", item.target_domain.as_deref().filter(|name| !name.trim().is_empty()).unwrap_or(&item.domain)))
        })?;
        if !targets.insert(target.clone()) {
            return Err(bad(format!("{target} birden çok kez seçildi")));
        }
        sites::check_names_allowed(&state.db, user, owner_id, std::slice::from_ref(&target))?;
        let existing = existing_sites.iter().find(|site| site.domain == target).cloned();
        if let Some(site) = &existing {
            if site.status != "active" || site.kind == "proxy" {
                return Err(bad(format!("{target} sitesine içe aktarılamaz (askıda ya da ters vekil sitesi)")));
            }
        } else {
            let taken_elsewhere: bool = state.db.call(|conn| {
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sites WHERE domain = ?1) OR EXISTS(SELECT 1 FROM site_aliases WHERE name = ?1)",
                    params![target],
                    |row| row.get(0),
                )
            })?;
            if taken_elsewhere {
                return Err(AppError::conflict(format!("{target} başka bir hesapta zaten kullanılıyor")));
            }
            new_sites += 1;
        }
        let mut databases = Vec::new();
        for name in &item.databases {
            let database = manifest.databases.iter().find(|known| &known.name == name).ok_or_else(|| bad(format!("{name} veritabanı yedekte yok")))?;
            if !taken_databases.insert(name.clone()) {
                return Err(bad(format!("{name} veritabanı birden çok alan adına seçildi")));
            }
            if database.engine == "mysql" && database.dump_path.is_some() {
                wants_database = true;
            }
            databases.push(database.clone());
        }
        items.push(PlanItem { domain, target, databases, existing });
    }
    if wants_database {
        Engine::Mariadb.require_installed()?;
        crate::quota::check(&state.db, owner_id, crate::quota::Resource::Database)?;
    }
    if new_sites > 0 {
        crate::quota::check(&state.db, owner_id, crate::quota::Resource::Site)?;
    }

    let lock = ImportLock::acquire(&dir)?;
    let summary: Vec<String> = items.iter().map(|item| format!("{} → {}", item.domain.name, item.target)).collect();
    let database_names: Vec<&str> = items.iter().flat_map(|item| item.databases.iter().map(|db| db.name.as_str())).collect();
    auth::audit(&state.db, user, "migrate.backup.import", &meta.source, &format!("hesap: {owner_name}; alanlar: {}; veritabanları: {}; biçim: {}", summary.join(", "), database_names.join(", "), manifest.format));

    let plan = Plan { dir, owner_id, items };
    let title = format!("Yedekten içe aktarma ({}): {}", manifest.format, summary.join(", "));
    let (job_state, actor) = (state.clone(), user.clone());
    state.jobs.start("migrate.backup-import", &title, Some(user.id), move |ctx| async move {
        let _lock = lock;
        import_job(&ctx, &job_state, &actor, &plan).await
    })
}

// ---- İçe aktarma görevi ----

/// Bu içe aktarmada oluşturulanlar; hata olursa geri alınır.
#[derive(Default)]
struct Created {
    sites: Vec<Site>,
    databases: Vec<String>,
}

async fn import_job(ctx: &JobCtx, state: &AppState, actor: &AuthUser, plan: &Plan) -> AppResult<()> {
    let work = plan.dir.join("work");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::DirBuilder::new().mode(0o700).create(&work)?;
    let mut created = Created::default();
    let outcome = run_items(ctx, state, actor, plan, &work, &mut created).await;
    let _ = std::fs::remove_dir_all(&work);
    match outcome {
        Ok(()) => {
            ctx.log("İçe aktarma tamamlandı. Alan adlarının DNS kayıtlarını bu sunucuya çevirmeden önce siteleri denetleyin. Posta kutuları aktarılmadı; IMAP ile ayrıca aktarabilirsiniz.");
            Ok(())
        }
        Err(err) => {
            if !created.sites.is_empty() || !created.databases.is_empty() {
                ctx.log(format!("İçe aktarma başarısız oldu; bu işlemde oluşturulan {} site ve {} veritabanı geri alınıyor.", created.sites.len(), created.databases.len()));
            }
            rollback(ctx, state, &mut created).await;
            Err(err)
        }
    }
}

async fn rollback(ctx: &JobCtx, state: &AppState, created: &mut Created) {
    while let Some(name) = created.databases.pop() {
        wordpress::drop_database(state, &name, true).await;
        ctx.log(format!("Geri alındı: veritabanı {name}"));
    }
    while let Some(site) = created.sites.pop() {
        let _guard = state.sys_lock.lock().await;
        if let Err(err) = sites::deprovision(&site, true).await {
            tracing::warn!(error = %err, domain = %site.domain, "yarım içe aktarma sitesi temizlenemedi");
        }
        let _ = sites::delete_row(&state.db, site.id);
        ctx.log(format!("Geri alındı: site {}", site.domain));
    }
}

fn highest_php() -> Option<String> {
    php::installed_versions().into_iter().next()
}

async fn run_items(ctx: &JobCtx, state: &AppState, actor: &AuthUser, plan: &Plan, work: &Path, created: &mut Created) -> AppResult<()> {
    let raw = plan.dir.join("raw");
    for (index, item) in plan.items.iter().enumerate() {
        ctx.log(format!("== {} → {} ==", item.domain.name, item.target));
        let item_work = work.join(format!("item-{index}"));
        std::fs::DirBuilder::new().mode(0o700).create(&item_work)?;
        let result = import_item(ctx, state, actor, plan, &raw, &item_work, item, created).await;
        let _ = std::fs::remove_dir_all(&item_work);
        result?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn import_item(ctx: &JobCtx, state: &AppState, actor: &AuthUser, plan: &Plan, raw: &Path, work: &Path, item: &PlanItem, created: &mut Created) -> AppResult<()> {
    let content_rel = item.domain.docroot_path.as_deref().ok_or_else(|| bad("Site içeriği bulunamadı"))?;
    let content = materialize_content(ctx, raw, work, content_rel).await?;
    let (is_php, wordpress_found) = {
        let content = content.clone();
        tokio::task::spawn_blocking(move || inspect_content(&content)).await.map_err(|err| AppError::internal(err.to_string()))?
    };

    let site = match &item.existing {
        Some(site) => {
            ctx.log(format!("{} sitesi zaten var; dosyalar içine kopyalanır (aynı adlı dosyaların üzerine yazılır).", site.domain));
            site.clone()
        }
        None => {
            crate::quota::check(&state.db, plan.owner_id, crate::quota::Resource::Site)?;
            let php_version = if is_php { highest_php() } else { None };
            if is_php && php_version.is_none() {
                ctx.log("UYARI: sitede PHP dosyaları var ama bu sunucuda PHP kurulu değil; site statik olarak açılıyor. PHP'yi Bileşenler sayfasından kurup site türünü değiştirin.");
            }
            let kind = if php_version.is_some() { "php" } else { "static" };
            let new = NewSite {
                user_id: plan.owner_id,
                domain: item.target.clone(),
                aliases: Vec::new(),
                kind: kind.into(),
                php_version,
                proxy_target: None,
                node: Default::default(),
            };
            let mut site = crate::api::sites::create_site(state, actor, new).await?;
            created.sites.push(site.clone());
            ctx.log(format!("Site oluşturuldu: {} ({})", site.domain, if kind == "php" { format!("PHP {}", site.php_version.as_deref().unwrap_or("")) } else { "statik".to_string() }));
            if wordpress_found && site.kind == "php" {
                site.template = "wordpress".into();
                let _guard = state.sys_lock.lock().await;
                let applied = async {
                    sites::save(&state.db, &site)?;
                    sites::provision(&site).await
                }
                .await;
                match applied {
                    Ok(()) => ctx.log("WordPress web sunucusu şablonu uygulandı."),
                    Err(err) => ctx.log(format!("UYARI: WordPress şablonu uygulanamadı ({err}); site ayarlarından elle seçebilirsiniz.")),
                }
            }
            site
        }
    };
    let sys_user = sysuser::lookup(&site.owner).ok_or_else(|| AppError::internal("Hesabın Linux kullanıcısı bulunamadı"))?;

    copy_into_site(ctx, &sys_user, &site, &content).await?;

    if item.databases.is_empty() {
        return Ok(());
    }
    // Dosyalar yerine geldi; WordPress'in beklediği veritabanı adı ayar dosyasından okunur.
    let wp_config: Option<ConfigDatabase> = if site.kind == "php" && wordpress_found {
        match wordpress::config_database(&site).await {
            Ok(found) => found,
            Err(err) => {
                ctx.log(format!("UYARI: wp-config.php okunamadı ({err}); veritabanları WordPress olarak bağlanamayacak."));
                None
            }
        }
    } else {
        None
    };
    for (db_index, database) in item.databases.iter().enumerate() {
        import_database(ctx, state, plan, raw, work, &sys_user, &site, database, wp_config.as_ref(), db_index, created).await?;
    }
    Ok(())
}

/// İçerikte PHP dosyası ve WordPress var mı (ilk iki klasör derinliğine bakar).
fn inspect_content(content: &Path) -> (bool, bool) {
    let wordpress = content.join("wp-config.php").is_file();
    let (files, _) = manifest::walk(content, 2);
    let php = wordpress || files.iter().any(|path| path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("php")));
    (php, wordpress)
}

/// Yedekteki site içeriğini (klasör ya da arşiv) kopyalanabilir bir klasöre çevirir.
async fn materialize_content(ctx: &JobCtx, raw: &Path, work: &Path, relative: &str) -> AppResult<PathBuf> {
    let source = extract::resolve_inside(raw, relative).map_err(bad)?;
    if source.is_dir() {
        return Ok(if source.file_name().is_some_and(|name| name == "domain_data") { content_root(source) } else { source });
    }
    let out = work.join("content");
    let (task_ctx, out_path, task_work) = (ctx.clone(), out.clone(), work.to_path_buf());
    let stats = tokio::task::spawn_blocking(move || {
        task_ctx.log(format!("Site içeriği açılıyor: {}", source.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()));
        let limits = extract::Limits::standard(&task_work);
        extract::extract_file(&source, &out_path, limits)
    })
    .await
    .map_err(|err| AppError::internal(err.to_string()))?
    .map_err(bad)?;
    ctx.log(format!("Site içeriği açıldı: {}", stats.summary()));
    Ok(content_root(out))
}

/// Açılmış içerikte yayın klasörü varsa (`httpdocs`, `public_html`) kök odur.
fn content_root(dir: PathBuf) -> PathBuf {
    for name in ["httpdocs", "public_html"] {
        let candidate = dir.join(name);
        if candidate.is_dir() {
            return candidate;
        }
    }
    dir
}

/// İçeriği yayın klasörüne hesabın kendi kullanıcısıyla yazar: root `tar` ile okur, hesabın uid'siyle açılan
/// `tar`a akıtır. Ardından izinler düzeltilir (grup okuyabilir, başkası yazamaz; özel bitler zaten atılmıştır).
async fn copy_into_site(ctx: &JobCtx, user: &SysUser, site: &Site, content: &Path) -> AppResult<()> {
    let docroot = site.docroot();
    // Panelin karşılama sayfası tek başına duruyorsa kaldırılır; yoksa içe aktarılan sitenin index.php'sinin önüne geçerdi.
    let listing = runas::output(runas::command(user, &docroot, "find", &[".", "-mindepth", "1", "-maxdepth", "1", "-not", "-name", "index.html", "-print", "-quit"]), QUICK).await?;
    if listing.success() && listing.stdout.trim().is_empty() {
        let _ = runas::output(runas::command(user, &docroot, "rm", &["-f", "--", "index.html"]), QUICK).await;
    }
    ctx.log(format!("Dosyalar kopyalanıyor → {}", docroot.display()));
    let mut producer = base_command("tar");
    producer.arg("-C").arg(content).args(["-cf", "-", "."]);
    let consumer = runas::command(user, &docroot, "tar", &["-xf", "-", "--no-same-owner", "--no-same-permissions", "-C", "."]);
    let (read_code, write_code, _) = crate::migrate::pipe(ctx, producer, consumer).await?;
    if read_code != 0 || write_code != 0 {
        return Err(bad("Dosyalar yayın klasörüne kopyalanamadı (ayrıntı yukarıda); disk dolmuş ya da kota aşılmış olabilir"));
    }
    let names: Vec<String> = std::fs::read_dir(content)?.flatten().filter_map(|entry| entry.file_name().into_string().ok()).collect();
    for chunk in names.chunks(100) {
        let targets: Vec<String> = chunk.iter().map(|name| format!("./{name}")).collect();
        let mut args: Vec<&str> = vec!["-R", "u+rwX,g+rX,g-w,o-w", "--"];
        args.extend(targets.iter().map(String::as_str));
        let _ = runas::output(runas::command(user, &docroot, "chmod", &args), Duration::from_secs(600)).await;
    }
    if let Ok(output) = runas::output(runas::command(user, &docroot, "du", &["-sh", "--", "."]), Duration::from_secs(300)).await {
        if let Some(total) = output.stdout.split_whitespace().next() {
            ctx.log(format!("Yayın klasörünün boyutu: {total}"));
        }
    }
    Ok(())
}

fn owner_databases(state: &AppState, owner_id: i64) -> AppResult<HashSet<String>> {
    state.db.call(|conn| {
        let mut stmt = conn.prepare("SELECT name FROM databases WHERE user_id = ?1 AND engine = 'mariadb'")?;
        let rows = stmt.query_map(params![owner_id], |row| row.get::<_, String>(0))?;
        rows.collect()
    })
}

/// Dökümü yayına hazır düz `.sql` dosyasına çevirir (sıkıştırma açılır, arşivse içindeki `.sql` bulunur,
/// `USE`/`CREATE DATABASE` satırları atılır).
async fn prepare_dump(ctx: &JobCtx, raw: &Path, work: &Path, relative: &str, index: usize) -> AppResult<PathBuf> {
    let source = extract::resolve_inside(raw, relative).map_err(bad)?;
    let out = work.join(format!("dump-{index}.sql"));
    let (task_work, task_out) = (work.to_path_buf(), out.clone());
    let written = tokio::task::spawn_blocking(move || -> Result<u64, String> {
        if !source.is_file() {
            return Err("Döküm dosyası yedekte bulunamadı".into());
        }
        let mut decoded = extract::Decoded::open(&source)?;
        let compressed_len = decoded.compressed_len;
        let mut head = vec![0u8; 512];
        let read = extract::read_full(&mut decoded, &mut head).map_err(|err| format!("Döküm okunamadı: {err}"))?;
        head.truncate(read);
        let mut limits = extract::Limits::standard(&task_work);
        let stream = std::io::Cursor::new(head.clone()).chain(decoded);
        if extract::looks_like_tar(&head) {
            limits.compressed_len = Some(compressed_len);
            let folder = task_work.join(format!("dump-{index}"));
            extract::extract_tar_stream(stream, &folder, &limits)?;
            let (files, _) = manifest::walk(&folder, 8);
            let pick = files.iter().find(|path| path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("sql"))).or_else(|| if files.len() == 1 { files.first() } else { None }).cloned().ok_or("Döküm arşivinde .sql dosyası bulunamadı")?;
            let file = std::fs::File::open(folder.join(&pick)).map_err(|err| format!("Döküm okunamadı: {err}"))?;
            let written = extract::write_sql(file, &task_out, limits.max_bytes)?;
            let _ = std::fs::remove_dir_all(&folder);
            Ok(written)
        } else {
            extract::write_sql(stream, &task_out, limits.max_bytes)
        }
    })
    .await
    .map_err(|err| AppError::internal(err.to_string()))?
    .map_err(bad)?;
    ctx.log(format!("Döküm hazırlandı ({})", crate::fsop::human_size(written)));
    Ok(out)
}

async fn load_sql(ctx: &JobCtx, database: &str, sql: &Path) -> AppResult<()> {
    let file = std::fs::File::open(sql).map_err(|err| AppError::internal(format!("döküm açılamadı: {err}")))?;
    dbms::import::run(ctx, Engine::Mariadb, database, Stdio::from(file)).await
}

#[allow(clippy::too_many_arguments)]
async fn import_database(
    ctx: &JobCtx,
    state: &AppState,
    plan: &Plan,
    raw: &Path,
    work: &Path,
    sys_user: &SysUser,
    site: &Site,
    database: &ManifestDatabase,
    wp_config: Option<&ConfigDatabase>,
    index: usize,
    created: &mut Created,
) -> AppResult<()> {
    if database.engine != "mysql" {
        ctx.log(format!("{} veritabanı PostgreSQL; içe aktarılmadı (yedekteki dökümü Veritabanları sayfasından elle yükleyin).", database.name));
        return Ok(());
    }
    let Some(dump) = database.dump_path.as_deref() else {
        ctx.log(format!("{} veritabanının dökümü yedekte bulunamadı; atlandı.", database.name));
        return Ok(());
    };
    ctx.log(format!("Veritabanı içe aktarılıyor: {}", database.name));
    crate::quota::check(&state.db, plan.owner_id, crate::quota::Resource::Database)?;
    let sql = prepare_dump(ctx, raw, work, dump, index).await?;

    let before = owner_databases(state, plan.owner_id)?;
    let outcome = if wp_config.is_some_and(|config| config.name == database.name) {
        let sql = sql.clone();
        wordpress::adopt_database(ctx, state, site, |new_database| async move { load_sql(ctx, &new_database, &sql).await }).await
    } else {
        async {
            let (new_database, db_user, db_password) = wordpress::create_site_database(state, site, "db").await?;
            if let Err(err) = load_sql(ctx, &new_database, &sql).await {
                wordpress::drop_database(state, &new_database, true).await;
                return Err(err);
            }
            ctx.log(format!("Veritabanı yüklendi: {new_database} (kullanıcı: {db_user})"));
            crate::migrate::finish_plain_database(ctx, sys_user, site, &database.name, &new_database, &db_user, &db_password).await;
            Ok(())
        }
        .await
    };
    // Bu adımda açılan veritabanları (başarı ya da hata) izlenir; sonraki bir hata hepsini geri alır.
    if let Ok(after) = owner_databases(state, plan.owner_id) {
        created.databases.extend(after.difference(&before).cloned());
    }
    let _ = std::fs::remove_file(&sql);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_ids_are_strict() {
        let id = auth::random_token(12);
        assert!(valid_id(&id));
        for bad in ["", "abc", "../../etc/passwd0000000000", "ABCDEF0123456789ABCDEF", "0123456789abcdef/../x", &"a".repeat(65)] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn content_root_prefers_publish_folder() {
        let dir = manifest::testing::fresh_dir("root");
        manifest::testing::put(&dir, "httpdocs/index.php", "x");
        manifest::testing::put(&dir, "cgi-bin/a", "x");
        assert_eq!(content_root(dir.clone()), dir.join("httpdocs"));
        std::fs::remove_dir_all(dir.join("httpdocs")).unwrap();
        manifest::testing::put(&dir, "public_html/index.php", "x");
        assert_eq!(content_root(dir.clone()), dir.join("public_html"));
        std::fs::remove_dir_all(&dir).unwrap();
        let flat = manifest::testing::fresh_dir("flat");
        manifest::testing::put(&flat, "index.php", "x");
        assert_eq!(content_root(flat.clone()), flat);
        assert_eq!(inspect_content(&flat), (true, false));
        std::fs::remove_dir_all(&flat).unwrap();
    }

    #[test]
    fn expired_inspections_are_cleaned_but_running_imports_are_kept() {
        let data = manifest::testing::fresh_dir("cleanup");
        let cfg = Config { data_dir: data.clone(), ..Config::default() };
        let make = |id: &str, age: i64| {
            let dir = work_root(&cfg).join(format!("import-{id}"));
            std::fs::create_dir_all(dir.join("raw")).unwrap();
            let meta = Meta { user_id: 1, source: "/x".into(), created_at: now() - age, status: "ready".into(), error: None };
            write_meta(&dir, &meta).unwrap();
            dir
        };
        let fresh = make("aaaaaaaaaaaaaaaa", 60);
        let old = make("bbbbbbbbbbbbbbbb", EXPIRE_SECS + 60);
        let busy = make("cccccccccccccccc", EXPIRE_SECS + 60);
        let lock = ImportLock::acquire(&busy).unwrap();
        // İnceleme klasörü olmayan başka bir klasöre dokunulmaz.
        std::fs::create_dir_all(work_root(&cfg).join("baska")).unwrap();
        assert_eq!(cleanup_expired(&cfg), 1);
        assert!(fresh.is_dir() && busy.is_dir() && !old.exists());
        assert!(work_root(&cfg).join("baska").is_dir());
        drop(lock);
        assert_eq!(cleanup_expired(&cfg), 1);
        assert!(!busy.exists() && fresh.is_dir());
        std::fs::remove_dir_all(&data).unwrap();
    }

    #[test]
    fn meta_roundtrip_and_lock() {
        let dir = manifest::testing::fresh_dir("meta");
        let meta = Meta { user_id: 1, source: "/x/y.tar.gz".into(), created_at: 5, status: "running".into(), error: None };
        write_meta(&dir, &meta).unwrap();
        assert_eq!(read_meta(&dir).unwrap().created_at, 5);
        let lock = ImportLock::acquire(&dir).unwrap();
        assert!(ImportLock::is_active(&dir));
        assert!(ImportLock::acquire(&dir).is_err());
        drop(lock);
        assert!(!ImportLock::is_active(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
