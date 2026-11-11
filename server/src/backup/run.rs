//! Yedek alma ve geri yükleme görevleri.

use std::path::{Component, Path, PathBuf};

use rusqlite::{params, OptionalExtension};
use serde::Deserialize;

use super::restic::{self, Snapshot};
use super::{resolve, DatabaseSource, Item, MailSource, Plan, Scope, Target, RUN_LOCK};
use crate::db::now;
use crate::dbms::{self, Engine};
use crate::error::{AppError, AppResult};
use crate::hosting::nginx::{self, ProxyRule};
use crate::hosting::node::{self, NodeSettings};
use crate::hosting::php::{self, PhpSettings};
use crate::hosting::sites::{self, NewSite, Site};
use crate::hosting::{domain, sysuser};
use crate::jobs::JobCtx;
use crate::mail;
use crate::state::AppState;
use crate::sys::exec;

fn backup_dir(state: &AppState) -> PathBuf {
    state.cfg.data_dir.join("backup")
}

/// Yalnızca root'un okuyabildiği klasörü (yoksa) oluşturur.
fn private_dir(path: &Path) -> AppResult<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn tags(kind: &str, name: &str, owner: &str, plan: Option<&Plan>) -> Vec<String> {
    let mut tags = vec![format!("kind:{kind}"), format!("name:{name}")];
    if !owner.is_empty() {
        tags.push(format!("owner:{owner}"));
    }
    tags.push(match plan {
        Some(plan) => format!("plan:{}", plan.id),
        None => "manual".to_string(),
    });
    tags
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

// ---- Yedek alma ----

fn manifest_path(state: &AppState, domain: &str) -> PathBuf {
    backup_dir(state).join("manifests").join(format!("{domain}.json"))
}

async fn backup_site(state: &AppState, ctx: &JobCtx, target: &Target, site: &Site, plan: Option<&Plan>) -> AppResult<()> {
    let root = site.root();
    if !root.is_dir() {
        return Err(AppError::internal("site klasörü bulunamadı"));
    }
    // Silinen site yedekten yeniden kurulabilsin diye ayarları da yedeğe girer.
    let manifest = manifest_path(state, &site.domain);
    private_dir(manifest.parent().unwrap_or(Path::new("/")))?;
    let json = serde_json::to_vec_pretty(site).map_err(|err| AppError::internal(err.to_string()))?;
    crate::hosting::atomic_write(&manifest, &json, 0o600)?;

    let mut paths = vec![text(&root)];
    let ssl_dir = Path::new(nginx::SSL_DIR).join(&site.domain);
    if ssl_dir.is_dir() {
        paths.push(text(&ssl_dir));
    }
    paths.push(text(&manifest));
    restic::backup_paths(ctx, target, &paths, &tags("site", &site.domain, &site.owner, plan)).await
}

fn dump_file(engine: &str, name: &str) -> String {
    format!("/{engine}-{name}.sql")
}

async fn backup_database(ctx: &JobCtx, target: &Target, database: &DatabaseSource, plan: Option<&Plan>) -> AppResult<()> {
    let engine = Engine::parse(&database.engine)?;
    engine.require_installed()?;
    dbms::validate_name(&database.name, "Veritabanı adı")?;
    let mut tags = tags("db", &database.name, &database.owner, plan);
    tags.push(format!("engine:{}", database.engine));
    let dump = dbms::dump_command(engine, &database.name);
    restic::backup_command(ctx, target, &dump_file(&database.engine, &database.name), &tags, &dump).await
}

async fn backup_mail(ctx: &JobCtx, target: &Target, source: &MailSource, plan: Option<&Plan>) -> AppResult<()> {
    let root = Path::new(mail::MAIL_ROOT).join(&source.domain);
    if !root.is_dir() {
        ctx.log("Bu alan adında henüz posta yok; atlandı.");
        return Ok(());
    }
    restic::backup_paths(ctx, target, &[text(&root)], &tags("mail", &source.domain, &source.owner, plan)).await
}

async fn backup_panel(state: &AppState, ctx: &JobCtx, target: &Target, plan: Option<&Plan>) -> AppResult<()> {
    let dir = backup_dir(state).join("panel");
    private_dir(&dir)?;
    // Çalışan veritabanının tutarlı bir kopyası alınır; asıl dosya yazılırken yedeklenmez.
    let copy = dir.join("pixpanel.db");
    let _ = std::fs::remove_file(&copy);
    let copy_str = text(&copy);
    state.db.call(|conn| conn.execute("VACUUM INTO ?1", [&copy_str]))?;
    let outcome = restic::backup_paths(ctx, target, &[copy_str, "/etc/pixpanel".to_string()], &tags("panel", "panel", "", plan)).await;
    let _ = std::fs::remove_file(&copy);
    outcome
}

async fn backup_item(state: &AppState, ctx: &JobCtx, target: &Target, item: &Item, plan: Option<&Plan>) -> AppResult<()> {
    match item {
        Item::Site(site) => backup_site(state, ctx, target, site, plan).await,
        Item::Database(database) => backup_database(ctx, target, database, plan).await,
        Item::Mail(source) => backup_mail(ctx, target, source, plan).await,
        Item::Panel => backup_panel(state, ctx, target, plan).await,
    }
}

async fn run_backup(state: &AppState, ctx: &JobCtx, target: &Target, scope: &Scope, plan: Option<&Plan>) -> AppResult<()> {
    if RUN_LOCK.try_lock().is_err() {
        ctx.log("Başka bir yedekleme ya da geri yükleme sürüyor; bitmesi bekleniyor…");
    }
    let _guard = RUN_LOCK.lock().await;
    restic::require_supported().await?;
    ctx.log(format!("Hedef: {}", target.name));
    let checked = restic::check(target).await;
    super::save_check(&state.db, target.id, checked.as_ref().err().map(String::as_str))?;
    if let Err(reason) = checked {
        return Err(AppError::internal(format!("Yedek hedefine ulaşılamadı: {reason}")));
    }

    let items = resolve(&state.db, scope)?;
    if items.is_empty() {
        return Err(AppError::internal("Yedeklenecek bir şey bulunamadı (seçilen kayıtlar silinmiş olabilir)"));
    }
    let mut failed = Vec::new();
    for item in &items {
        ctx.log(String::new());
        ctx.log(format!("== {}", item.label()));
        if let Err(err) = backup_item(state, ctx, target, item, plan).await {
            ctx.log(format!("HATA: {err}"));
            failed.push(item.label());
        }
    }

    if let Some(plan) = plan {
        if plan.keep_daily + plan.keep_weekly + plan.keep_monthly > 0 {
            ctx.log(String::new());
            ctx.log("== Eski yedekler temizleniyor");
            if let Err(err) = restic::apply_retention(ctx, target, plan.id, plan.keep_daily, plan.keep_weekly, plan.keep_monthly).await {
                ctx.log(format!("HATA: {err}"));
                failed.push("eski yedeklerin temizlenmesi".to_string());
            }
        }
    }

    ctx.log(String::new());
    if failed.is_empty() {
        ctx.log(format!("{} birim yedeklendi.", items.len()));
        Ok(())
    } else {
        Err(AppError::internal(format!("{} / {} birim yedeklenemedi: {}", failed.len(), items.len(), failed.join(", "))))
    }
}

/// Başka bir görevin içinden, o görevin günlüğüne yazarak yedek alır (ör. güncellemeden
/// hemen önce). Yedek "elle alınmış" sayılır; saklama ilkeleri onu silmez.
pub async fn run_inline(state: &AppState, ctx: &JobCtx, target: &Target, scope: &Scope) -> AppResult<()> {
    run_backup(state, ctx, target, scope, None).await
}

/// Yedekleme görevini başlatır. `plan` verilirse sonucu plana işlenir ve saklama
/// ilkesi uygulanır; verilmezse yedek "elle alınmış" sayılır ve kendiliğinden silinmez.
pub fn start_backup(state: &AppState, user_id: Option<i64>, target: Target, scope: Scope, plan: Option<Plan>) -> AppResult<i64> {
    let title = match &plan {
        Some(plan) => format!("Yedekleme: {}", plan.name),
        None => format!("Yedekleme (elle): {}", target.name),
    };
    let notify_user = plan.as_ref().map(|plan| plan.user_id).or(user_id);
    let job_state = state.clone();
    let job_title = title.clone();
    state.jobs.start("backup", &title, user_id, move |ctx| async move {
        let outcome = run_backup(&job_state, &ctx, &target, &scope, plan.as_ref()).await;
        if let Some(plan) = &plan {
            let status = if outcome.is_ok() { "success" } else { "failed" };
            if let Err(err) = super::set_plan_status(&job_state.db, plan.id, status) {
                tracing::warn!(error = %err, "plan durumu kaydedilemedi");
            }
        }
        if let (Err(err), Some(user_id)) = (&outcome, notify_user) {
            let body = format!("{err}\n\nSon satırlar:\n{}", ctx.tail(15));
            crate::notify::send_event(&job_state, user_id, "backup_failed", &format!("{job_title} başarısız oldu"), &body).await;
        }
        outcome
    })
}

pub fn start_plan(state: &AppState, user_id: Option<i64>, plan: Plan) -> AppResult<i64> {
    let target = super::load_target(&state.db, plan.target_id)?;
    let plan_id = plan.id;
    let job_id = start_backup(state, user_id, target, plan.scope.clone(), Some(plan))?;
    super::set_plan_started(&state.db, plan_id, job_id)?;
    Ok(job_id)
}

/// Zamanı gelen planları başlatır ve bir sonraki çalışma zamanlarını yazar.
pub async fn start_due_plans(state: &AppState) -> AppResult<()> {
    if !super::is_installed() {
        return Ok(());
    }
    let offset = super::local_offset().await;
    let current = now();
    for plan in super::list_plans(&state.db)? {
        let next = plan.next_run(current, offset);
        match plan.next_run_at {
            Some(due) if due <= current && next.is_some() => {
                super::set_plan_next(&state.db, plan.id, next)?;
                // Önceki koşu hâlâ sürüyorsa üstüne yenisi başlatılmaz.
                let still_running = plan.last_job_id.and_then(|id| state.jobs.get(id).ok()).is_some_and(|(info, _)| info.status == "running");
                if still_running {
                    tracing::warn!(plan = %plan.name, "önceki yedekleme sürdüğü için bu koşu atlandı");
                    continue;
                }
                tracing::info!(plan = %plan.name, "zamanlanmış yedekleme başlatılıyor");
                if let Err(err) = start_plan(state, None, plan) {
                    tracing::error!(error = %err, "zamanlanmış yedekleme başlatılamadı");
                }
            }
            stored if stored != next && !(stored.is_some() && next.is_some()) => super::set_plan_next(&state.db, plan.id, next)?,
            _ => {}
        }
    }
    Ok(())
}

// ---- Geri yükleme ----

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct RestoreOptions {
    /// Yalnızca bu dosya ve klasörler geri yüklenir; boşsa hepsi.
    pub paths: Vec<String>,
    /// Hedefte olup yedekte olmayan dosyalar silinsin mi?
    pub delete_extra: bool,
    /// Site yedeğini başka bir sitenin klasörüne yükler.
    pub destination_site_id: Option<i64>,
    /// Veritabanı yedeğini başka bir veritabanına yükler.
    pub destination_database_id: Option<i64>,
}

/// Seçilen yolların `root` altında kaldığını doğrular; hiç seçilmediyse kökün kendisini döner.
fn validate_includes(paths: &[String], root: &Path) -> AppResult<Vec<String>> {
    if paths.is_empty() {
        return Ok(vec![text(root)]);
    }
    if paths.len() > 200 {
        return Err(AppError::bad("Tek seferde en fazla 200 yol seçilebilir"));
    }
    paths
        .iter()
        .map(|path| {
            let candidate = Path::new(path);
            let inside = candidate.starts_with(root)
                && candidate.components().all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
                && path.len() < 4096
                && !path.chars().any(char::is_control);
            if inside { Ok(path.clone()) } else { Err(AppError::bad(format!("Bu yol yedeğin kapsamı dışında: {path}"))) }
        })
        .collect()
}

fn site_by_domain(state: &AppState, name: &str) -> AppResult<Option<Site>> {
    let id: Option<i64> = state.db.call(|conn| conn.query_row("SELECT id FROM sites WHERE domain = ?1", [name], |row| row.get(0)).optional())?;
    match id {
        Some(id) => sites::load(&state.db, id),
        None => Ok(None),
    }
}

fn user_id_of(state: &AppState, username: &str) -> AppResult<i64> {
    state
        .db
        .call(|conn| conn.query_row("SELECT id FROM users WHERE username = ?1", [username], |row| row.get(0)).optional())?
        .ok_or_else(|| AppError::bad(format!("Yedeğin sahibi olan “{username}” hesabı artık yok; önce hesabı yeniden oluşturun")))
}

/// Yedeğe konan site ayarları (`Site` yapısının geri okunabilen alanları).
#[derive(Deserialize)]
struct SiteManifest {
    owner: String,
    domain: String,
    #[serde(default)]
    aliases: Vec<String>,
    kind: String,
    public_dir: String,
    php_version: Option<String>,
    #[serde(default)]
    php_settings: PhpSettings,
    proxy_target: Option<String>,
    #[serde(default)]
    proxy_websocket: bool,
    #[serde(default)]
    ssl_mode: String,
    #[serde(default)]
    force_https: bool,
    #[serde(default)]
    hsts: bool,
    #[serde(default)]
    nginx_custom: String,
    #[serde(default)]
    node: NodeSettings,
    #[serde(default)]
    proxy_rules: Vec<ProxyRule>,
}

/// Silinmiş siteyi yedekteki ayarlarından yeniden kurar (dosyalar ayrıca yüklenir).
/// Sertifika dosyaları henüz yerinde olmadığından site önce SSL'siz kurulur.
async fn recreate_site(state: &AppState, ctx: &JobCtx, manifest: &SiteManifest) -> AppResult<Site> {
    let user_id = user_id_of(state, &manifest.owner)?;
    let domain_name = domain::normalize(&manifest.domain)?;
    if domain_name != manifest.domain || !sites::KINDS.contains(&manifest.kind.as_str()) {
        return Err(AppError::bad("Yedekteki site ayarları geçersiz"));
    }
    let aliases = sites::normalize_aliases(&domain_name, &manifest.aliases)?;
    let public_dir = sites::validate_public_dir(&manifest.public_dir)?;
    let php_version = match (manifest.kind.as_str(), &manifest.php_version) {
        ("php", Some(version)) => {
            php::validate_version(version)?;
            Some(version.clone())
        }
        ("php", None) => return Err(AppError::bad("Yedekteki site ayarlarında PHP sürümü yok")),
        _ => None,
    };
    let proxy_target = match (manifest.kind.as_str(), &manifest.proxy_target) {
        ("proxy", Some(target)) => Some(sites::validate_proxy_target(target)?),
        ("proxy", None) => return Err(AppError::bad("Yedekteki site ayarlarında hedef adres yok")),
        _ => None,
    };
    if node::is_app(&manifest.kind) {
        manifest.node.validate(&manifest.kind)?;
    }
    let proxy_rules = sites::validate_proxy_rules(&manifest.proxy_rules)?;

    ctx.log(format!("Site artık yok; yedekteki ayarlarla yeniden kuruluyor ({}).", manifest.kind));
    let _guard = state.sys_lock.lock().await;
    let id = sites::insert(
        &state.db,
        &NewSite { user_id, domain: domain_name, aliases, kind: manifest.kind.clone(), php_version, proxy_target, node: manifest.node.clone() },
    )?;
    let mut site = sites::load(&state.db, id)?.ok_or(AppError::NotFound)?;
    site.public_dir = public_dir;
    site.php_settings = manifest.php_settings.clone();
    site.proxy_websocket = manifest.proxy_websocket;
    site.nginx_custom = manifest.nginx_custom.clone();
    site.proxy_rules = proxy_rules;
    let applied = match sites::save(&state.db, &site) {
        Ok(()) => sites::provision(&site).await,
        Err(err) => Err(err),
    };
    if let Err(err) = applied {
        if let Err(cleanup) = sites::deprovision(&site, false).await {
            tracing::warn!(error = %cleanup, domain = %site.domain, "yarım site temizlenemedi");
        }
        sites::delete_row(&state.db, id)?;
        return Err(err);
    }
    Ok(site)
}

async fn restore_site(state: &AppState, ctx: &JobCtx, target: &Target, snapshot: &Snapshot, options: &RestoreOptions) -> AppResult<()> {
    let source_root = snapshot.paths.iter().find(|path| path.starts_with("/home/")).cloned().ok_or_else(|| AppError::bad("Yedekte site klasörü yok"))?;
    let existing = site_by_domain(state, &snapshot.name)?;

    // Başka bir siteye (klon) yükleme.
    if let Some(destination_id) = options.destination_site_id.filter(|id| existing.as_ref().map(|site| site.id) != Some(*id)) {
        let destination = sites::load(&state.db, destination_id)?.ok_or(AppError::NotFound)?;
        let user = sysuser::lookup(&destination.owner).ok_or_else(|| AppError::internal("hedef sitenin kullanıcısı bulunamadı"))?;
        let root = destination.root();
        ctx.log(format!("Dosyalar {} sitesinin klasörüne yükleniyor.", destination.domain));
        restic::restore(ctx, target, &snapshot.id, Some(&source_root), &text(&root), &[], options.delete_extra).await?;
        // Dosyalar yedekteki sahibin kimliğiyle gelir; hedef sitenin sahibine devredilir.
        exec::run_ok("chown", &["-R", "-h", &user.uid.to_string(), "--", &text(&root)]).await?;
        let logs = root.join("logs");
        if logs.is_dir() {
            std::os::unix::fs::lchown(&logs, Some(user.uid), Some(user.gid))?;
        }
        return restart_app(ctx, &destination).await;
    }

    let (site, recreated, manifest) = match existing {
        Some(site) => (site, false, None),
        None => {
            let manifest_file = snapshot.paths.iter().find(|path| path.ends_with(".json")).ok_or_else(|| AppError::bad("Yedekte site ayarları yok; site silinmişse önce aynı alan adıyla yeniden oluşturun"))?;
            let raw = restic::read_file(target, &snapshot.id, manifest_file).await?;
            let manifest: SiteManifest = serde_json::from_str(&raw).map_err(|_| AppError::bad("Yedekteki site ayarları okunamadı"))?;
            if manifest.domain != snapshot.name {
                return Err(AppError::bad("Yedekteki site ayarları bu yedeğe ait değil"));
            }
            (recreate_site(state, ctx, &manifest).await?, true, Some(manifest))
        }
    };
    let root = site.root();
    if text(&root) != source_root {
        return Err(AppError::bad("Sitenin sahibi yedek alındığından beri değişmiş; “başka siteye yükle” seçeneğini kullanın"));
    }

    let ssl_dir = Path::new(nginx::SSL_DIR).join(&site.domain);
    let (includes, delete) = if recreated {
        // Yeniden kurulan sitede karşılama sayfası yerine yedekteki her şey gelir.
        (vec![source_root.clone(), text(&ssl_dir)], true)
    } else {
        (validate_includes(&options.paths, &root)?, options.delete_extra)
    };
    ctx.log("Dosyalar geri yükleniyor.");
    restic::restore(ctx, target, &snapshot.id, None, "/", &includes, delete).await?;

    if let Some(manifest) = manifest {
        // Sertifika dosyaları da geri geldiyse HTTPS eski ayarlarıyla açılır.
        if manifest.ssl_mode != "none" && ssl_dir.join("fullchain.pem").exists() {
            let _guard = state.sys_lock.lock().await;
            let mut secured = site.clone();
            secured.ssl_mode = manifest.ssl_mode.clone();
            secured.force_https = manifest.force_https;
            secured.hsts = manifest.hsts;
            sites::save(&state.db, &secured)?;
            match sites::provision(&secured).await {
                Ok(()) => ctx.log("Sertifika geri yüklendi; HTTPS açıldı."),
                Err(err) => {
                    ctx.log(format!("UYARI: HTTPS açılamadı ({err}); site HTTP ile yayında, sertifikayı yeniden alın."));
                    sites::save(&state.db, &site)?;
                    sites::provision(&site).await?;
                }
            }
        } else if manifest.ssl_mode != "none" {
            ctx.log("UYARI: yedekte sertifika yok; SSL sekmesinden yeniden alın.");
        }
    }
    restart_app(ctx, &site).await
}

/// Node.js sitelerinde uygulama yeni dosyalarla yeniden başlatılır.
async fn restart_app(ctx: &JobCtx, site: &Site) -> AppResult<()> {
    if node::is_app(&site.kind) && site.status == "active" {
        if let Err(err) = node::action(site.id, "restart").await {
            ctx.log(format!("UYARI: uygulama yeniden başlatılamadı: {err}"));
        }
    }
    Ok(())
}

async fn restore_database(state: &AppState, ctx: &JobCtx, target: &Target, snapshot: &Snapshot, options: &RestoreOptions) -> AppResult<()> {
    let engine = Engine::parse(&snapshot.engine)?;
    engine.require_installed()?;
    dbms::validate_name(&snapshot.name, "Veritabanı adı")?;
    let find = |id: Option<i64>| {
        state.db.call(|conn| match id {
            Some(id) => conn.query_row("SELECT engine, name FROM databases WHERE id = ?1", [id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))).optional(),
            None => conn
                .query_row("SELECT engine, name FROM databases WHERE engine = ?1 AND name = ?2", params![snapshot.engine, snapshot.name], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional(),
        })
    };
    let destination = match options.destination_database_id {
        Some(id) => {
            let (found_engine, name) = find(Some(id))?.ok_or(AppError::NotFound)?;
            if found_engine != snapshot.engine {
                return Err(AppError::bad("Yedek ile hedef veritabanının türü farklı"));
            }
            name
        }
        None => match find(None)? {
            Some((_, name)) => name,
            None => {
                let user_id = user_id_of(state, &snapshot.owner)?;
                ctx.log("Veritabanı artık yok; yeniden oluşturuluyor.");
                dbms::create_database(engine, &snapshot.name).await?;
                state.db.call(|conn| {
                    conn.execute("INSERT INTO databases (user_id, engine, name, created_at) VALUES (?1, ?2, ?3, ?4)", params![user_id, snapshot.engine, snapshot.name, now()])
                })?;
                ctx.log("UYARI: veritabanı kullanıcıları yedekte yer almaz; Veritabanları sayfasından yeniden yetkilendirin.");
                snapshot.name.clone()
            }
        },
    };
    dbms::validate_name(&destination, "Veritabanı adı")?;
    ctx.log(format!("Döküm {destination} veritabanına yükleniyor."));
    let mut import = dbms::import_command(engine, &destination);
    restic::pipe_file(ctx, target, &snapshot.id, &dump_file(&snapshot.engine, &snapshot.name), &mut import, "veritabanı istemcisi").await
}

async fn restore_mail(state: &AppState, ctx: &JobCtx, target: &Target, snapshot: &Snapshot, options: &RestoreOptions) -> AppResult<()> {
    let exists: bool = state.db.call(|conn| conn.query_row("SELECT EXISTS (SELECT 1 FROM mail_domains WHERE domain = ?1)", [&snapshot.name], |row| row.get(0)))?;
    if !exists {
        return Err(AppError::bad("Bu posta alanı artık yok; önce Posta sayfasından alan adını ve kutuları yeniden oluşturun"));
    }
    let root = Path::new(mail::MAIL_ROOT).join(&snapshot.name);
    let includes = validate_includes(&options.paths, &root)?;
    ctx.log("Postalar geri yükleniyor.");
    restic::restore(ctx, target, &snapshot.id, None, "/", &includes, options.delete_extra).await
}

pub fn start_restore(state: &AppState, user_id: i64, target: Target, snapshot: Snapshot, options: RestoreOptions) -> AppResult<i64> {
    if snapshot.kind == "panel" {
        return Err(AppError::bad("Panel yedeği arayüzden geri yüklenmez; adımlar docs/infra/felaket-kurtarma.md belgesinde"));
    }
    let what = match snapshot.kind.as_str() {
        "site" => "site",
        "db" => "veritabanı",
        "mail" => "posta",
        _ => return Err(AppError::bad("Bu yedek türü geri yüklenemez")),
    };
    let title = format!("Geri yükleme: {what} {}", snapshot.name);
    let job_state = state.clone();
    state.jobs.start("restore", &title, Some(user_id), move |ctx| async move {
        if RUN_LOCK.try_lock().is_err() {
            ctx.log("Başka bir yedekleme ya da geri yükleme sürüyor; bitmesi bekleniyor…");
        }
        let _guard = RUN_LOCK.lock().await;
        restic::require_supported().await?;
        match snapshot.kind.as_str() {
            "site" => restore_site(&job_state, &ctx, &target, &snapshot, &options).await?,
            "db" => restore_database(&job_state, &ctx, &target, &snapshot, &options).await?,
            _ => restore_mail(&job_state, &ctx, &target, &snapshot, &options).await?,
        }
        ctx.log("Geri yükleme tamamlandı.");
        Ok(())
    })
}

/// Tek bir yedeği depodan siler.
pub fn start_forget(state: &AppState, user_id: i64, target: Target, snapshot: Snapshot) -> AppResult<i64> {
    let title = format!("Yedek siliniyor: {} {}", snapshot.kind, snapshot.name);
    state.jobs.start("backup", &title, Some(user_id), move |ctx| async move {
        let _guard = RUN_LOCK.lock().await;
        restic::forget(&ctx, &target, &snapshot.id).await
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_paths_must_stay_inside_the_site() {
        let root = Path::new("/home/ali/sites/deneme.test");
        assert_eq!(validate_includes(&[], root).unwrap(), vec!["/home/ali/sites/deneme.test".to_string()]);
        assert!(validate_includes(&["/home/ali/sites/deneme.test/public/index.php".into()], root).is_ok());
        assert!(validate_includes(&["/home/ali/sites/deneme.test/../baska.test".into()], root).is_err());
        assert!(validate_includes(&["/home/ali/sites/deneme.test.evil/x".into()], root).is_err());
        assert!(validate_includes(&["/etc/shadow".into()], root).is_err());
    }
}
