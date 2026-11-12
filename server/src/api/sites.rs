//! Site yönetimi uçları.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{self, AuthUser, Role};
use crate::error::{AppError, AppResult};
use crate::hosting::nginx::ProxyRule;
use crate::hosting::node::{self, NodeSettings};
use crate::hosting::php::{self, PhpSettings};
use crate::sys::runas;
use crate::hosting::sites::{self, NewSite, Site};
use crate::deploy;
use crate::hosting::{domain, nginx, ssl};
use crate::state::AppState;

const AREA: &str = "sites";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/options", get(options))
        .route("/summary", get(summary))
        .route("/{id}", get(detail).patch(update).delete(remove))
        .route("/{id}/clone", axum::routing::post(clone_site))
        .route("/{id}/cache/purge", axum::routing::post(purge_cache))
        .route("/{id}/migrate/probe", axum::routing::post(migrate_probe))
        .route("/{id}/migrate", axum::routing::post(migrate))
        .route("/{id}/uptime", get(uptime))
        .route("/{id}/stats", get(stats))
        .route("/{id}/node", get(node_status))
        .route("/{id}/node/run", axum::routing::post(node_run))
        .route("/{id}/node/{action}", axum::routing::post(node_action))
}

async fn list(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Vec<Site>>> {
    user.require(AREA)?;
    Ok(Json(sites::list(&state.db, &user)?))
}

#[derive(serde::Deserialize)]
struct StatsQuery {
    /// Kaç günlük dönem (varsayılan 30, en çok 365).
    days: Option<i64>,
}

/// Sitenin ziyaret ve trafik istatistikleri.
async fn stats(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, axum::extract::Query(query): axum::extract::Query<StatsQuery>) -> AppResult<Json<crate::stats::SiteStats>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    // Sayfa açıldığında son dakikaların istekleri de görünsün diye günlüğün yeni satırları hemen işlenir.
    let db = state.db.clone();
    let fresh = site.clone();
    let _ = tokio::task::spawn_blocking(move || crate::stats::collect_site(&db, &fresh)).await;
    Ok(Json(crate::stats::site_stats(&state.db, site.id, query.days.unwrap_or(30).clamp(1, 365))?))
}

/// Sitenin son 24 saatlik erişilebilirlik geçmişi.
async fn uptime(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<crate::uptime::History>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    Ok(Json(crate::uptime::history(&state.db, site.id)?))
}

async fn detail(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Site>> {
    user.require(AREA)?;
    Ok(Json(sites::get(&state.db, &user, id)?))
}

#[derive(Serialize)]
struct GitSummary {
    branch: String,
    running: bool,
    last_status: Option<String>,
    last_at: Option<i64>,
    last_subject: Option<String>,
}

/// Site kartlarında gösterilen canlı durum özeti.
#[derive(Serialize)]
struct SiteSummary {
    site_id: i64,
    ssl_expires_at: Option<i64>,
    git: Option<GitSummary>,
    /// Sitenin sahibi olan hesabın veritabanı ve zamanlanmış görev sayıları.
    databases: i64,
    cron_jobs: i64,
    /// Panelin bu sitede fark ettiği sorunlar (en önemlisi başta).
    issues: Vec<crate::health::SiteIssue>,
    /// Son 24 saatin erişilebilirlik yüzdesi; site yoklanmıyorsa `None`.
    uptime_day: Option<f64>,
    /// Son başarılı yoklamanın yanıt süresi (ms).
    response_millis: Option<u32>,
    /// Site klasörünün boyutu (bayt; saatte bir ölçülür, henüz ölçülmediyse `None`).
    disk_bytes: Option<u64>,
    /// Bu ayki trafik (bayt).
    month_bytes: i64,
}

async fn summary(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Vec<SiteSummary>>> {
    user.require(AREA)?;
    let mut list = Vec::new();
    for site in sites::list(&state.db, &user)? {
        let ssl_expires_at = if site.ssl_mode == "none" { None } else { ssl::read_info(&site.domain).map(|info| info.not_after) };
        let git = match deploy::load_config(&state.db, site.id)? {
            Some(config) => {
                let last = deploy::list_deployments(&state.db, site.id, 1)?.into_iter().next();
                Some(GitSummary {
                    branch: config.branch,
                    running: deploy::is_running(site.id),
                    last_status: last.as_ref().map(|d| d.status.clone()),
                    last_at: last.as_ref().map(|d| d.finished_at.unwrap_or(d.started_at)),
                    last_subject: last.map(|d| d.commit_subject),
                })
            }
            None => None,
        };
        let (databases, cron_jobs) = state.db.call(|conn| {
            Ok((
                conn.query_row("SELECT COUNT(*) FROM databases WHERE user_id = ?1", params![site.user_id], |row| row.get(0))?,
                conn.query_row("SELECT COUNT(*) FROM cron_jobs WHERE run_as = ?1", params![site.owner], |row| row.get(0))?,
            ))
        })?;
        let (uptime_day, response_millis) = crate::uptime::glance(&state.db, site.id)?;
        list.push(SiteSummary {
            site_id: site.id,
            ssl_expires_at,
            git,
            databases,
            cron_jobs,
            issues: state.health.site_issues(site.id),
            uptime_day,
            response_millis,
            disk_bytes: crate::quota::site_size(site.id),
            month_bytes: crate::stats::site_month_bytes(&state.db, site.id)?,
        });
    }
    Ok(Json(list))
}

#[derive(Serialize)]
struct OwnerOption {
    id: i64,
    username: String,
}

#[derive(Serialize)]
struct Options {
    nginx_installed: bool,
    php_versions: Vec<String>,
    /// Sitenin atanabileceği hesaplar (yönetici ve bayiler için).
    owners: Vec<OwnerOption>,
    /// Sürüm başına genel PHP ayarları; site formunda devralınan değerleri göstermek için.
    php_defaults: std::collections::BTreeMap<String, php::PhpGlobal>,
    node_versions: Vec<String>,
    /// Sistemdeki Python sürümü; kurulu değilse boş (Python sitesi açılamaz).
    python_versions: Vec<String>,
    /// Yöneticinin uv ile kurduğu Python sürümleri; Python siteleri bunlardan birini seçebilir.
    python_runtimes: Vec<String>,
    /// Hesabın sitelerini bağlayabileceği sunucu adresleri (yöneticide hepsi, diğerlerinde ayrılmış olanlar).
    ips: Vec<IpOption>,
}

#[derive(Serialize)]
struct IpOption {
    address: String,
    label: String,
    /// Adresin ayrıldığı hesap; yönetici, siteyi sahibine ayrılmamış adrese de bağlayabilir.
    user_id: Option<i64>,
}

#[derive(Deserialize)]
struct CloneBody {
    domain: String,
}

/// Siteyi yeni bir alan adına kopyalar: ayarlar hemen, dosyalar (ve WordPress ise veritabanı)
/// arka plan görevinde çoğaltılır.
async fn clone_site(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<CloneBody>) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let source = sites::get(&state.db, &user, id)?;
    let (job_id, site_id) = start_clone(&state, &user, source, &body.domain, false).await?;
    Ok(Json(json!({ "job_id": job_id, "site_id": site_id })))
}

/// Kopyalama görevini başlatır; (görev, yeni site) kimliklerini döner. `staging` verilirse kopya
/// asıl sitenin WordPress hazırlık ortamı olarak işaretlenir.
pub(crate) async fn start_clone(state: &AppState, user: &AuthUser, source: Site, domain_text: &str, staging: bool) -> AppResult<(i64, i64)> {
    if source.status != "active" {
        return Err(AppError::bad("Askıdaki site kopyalanamaz"));
    }
    crate::quota::check(&state.db, source.user_id, crate::quota::Resource::Site)?;
    let domain_name = domain::normalize(domain_text)?;
    sites::check_names_allowed(&state.db, user, source.user_id, std::slice::from_ref(&domain_name))?;
    let wordpress = crate::wordpress::is_wordpress(&source).await;
    if staging && !wordpress {
        return Err(AppError::bad("Hazırlık ortamı yalnızca WordPress kurulu sitelerde açılır"));
    }
    if wordpress {
        crate::quota::check(&state.db, source.user_id, crate::quota::Resource::Database)?;
    }

    let target = {
        let _guard = state.sys_lock.lock().await;
        let new_id = sites::insert(
            &state.db,
            &NewSite {
                user_id: source.user_id,
                domain: domain_name.clone(),
                aliases: Vec::new(),
                kind: source.kind.clone(),
                php_version: source.php_version.clone(),
                proxy_target: source.proxy_target.clone(),
                node: source.node.clone(),
            },
        )?;
        let mut target = sites::load(&state.db, new_id)?.ok_or(AppError::NotFound)?;
        crate::clone::inherit_settings(&source, &mut target);
        let prepared = match sites::save(&state.db, &target) {
            Ok(()) => sites::provision(&target).await,
            Err(err) => Err(err),
        };
        if let Err(err) = prepared {
            if let Err(cleanup) = sites::deprovision(&target, true).await {
                tracing::warn!(error = %cleanup, domain = %target.domain, "yarım kopya temizlenemedi");
            }
            sites::delete_row(&state.db, new_id)?;
            return Err(err);
        }
        if staging {
            sites::set_staging_of(&state.db, new_id, Some(source.id))?;
            target.staging_of = Some(source.id);
        }
        target
    };

    auth::audit(&state.db, user, if staging { "wordpress.staging.create" } else { "site.clone" }, &source.domain, &domain_name);
    let job_state = state.clone();
    let title = if staging { format!("Hazırlık ortamı: {} → {}", source.domain, domain_name) } else { format!("Site kopyalama: {} → {}", source.domain, domain_name) };
    let target_id = target.id;
    let job_id =
        state.jobs.start("site.clone", &title, Some(user.id), move |ctx| async move { crate::clone::run(&ctx, &job_state, &source, &target, wordpress, staging).await })?;
    Ok((job_id, target_id))
}

#[derive(Deserialize)]
struct MigrateBody {
    host: String,
    port: Option<u16>,
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    private_key: String,
    path: String,
    #[serde(default)]
    host_key: String,
    #[serde(default)]
    database: bool,
    /// WordPress dışındaki uygulamalar için karşı sunucudaki veritabanının bağlantı bilgileri.
    remote_database: Option<RemoteDatabase>,
}

#[derive(Deserialize)]
struct RemoteDatabase {
    name: String,
    user: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    host: String,
}

impl MigrateBody {
    fn source(&self) -> AppResult<crate::migrate::Source> {
        let mut source = crate::migrate::Source {
            host: self.host.clone(),
            port: self.port.unwrap_or(22),
            username: self.username.clone(),
            password: self.password.clone(),
            private_key: self.private_key.clone(),
            path: self.path.clone(),
        };
        source.validate()?;
        Ok(source)
    }
}

/// Taşınacak sitenin bulunduğu sunucuya bağlanmayı dener; sunucu anahtarını ve bulduklarını döner.
async fn migrate_probe(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<MigrateBody>) -> AppResult<Json<crate::migrate::Probe>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    let source = body.source()?;
    Ok(Json(crate::migrate::probe(&site, &source).await?))
}

/// Başka sunucudaki siteyi bu siteye çeker (arka plan görevi).
async fn migrate(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(mut body): Json<MigrateBody>) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    if site.status != "active" {
        return Err(AppError::bad("Askıdaki siteye taşıma yapılamaz"));
    }
    if site.kind == "proxy" {
        return Err(AppError::bad("Ters vekil sitelerinin dosyası yoktur; taşıma yapılamaz"));
    }
    let source = body.source()?;
    let manual = match body.remote_database.take().filter(|_| body.database) {
        Some(remote) => {
            let fits = |value: &str, max: usize| !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control);
            if !fits(remote.name.trim(), 64) || !fits(remote.user.trim(), 80) || remote.password.len() > 256 || remote.host.len() > 255 {
                return Err(AppError::bad("Karşı sunucudaki veritabanının adı ve kullanıcısı gerekli"));
            }
            let host = if remote.host.trim().is_empty() { "localhost".to_string() } else { remote.host.trim().to_string() };
            Some(crate::wordpress::ConfigDatabase { name: remote.name.trim().to_string(), user: remote.user.trim().to_string(), password: remote.password, host })
        }
        None => None,
    };
    let with_database = body.database;
    if manual.is_some() {
        crate::dbms::Engine::Mariadb.require_installed()?;
        crate::quota::check(&state.db, site.user_id, crate::quota::Resource::Database)?;
    } else if with_database {
        if site.kind != "php" {
            return Err(AppError::bad("Veritabanı yalnızca PHP sitelerine (WordPress) taşınabilir"));
        }
        crate::dbms::Engine::Mariadb.require_installed()?;
        crate::quota::check(&state.db, site.user_id, crate::quota::Resource::Database)?;
    }
    auth::audit(&state.db, &user, "site.migrate", &site.domain, &format!("{}:{}", source.host, source.path));
    let job_state = state.clone();
    let title = format!("Site taşıma: {} → {}", source.host, site.domain);
    let host_key = body.host_key;
    let job_id = state.jobs.start("site.migrate", &title, Some(user.id), move |ctx| async move {
        crate::migrate::run(&ctx, &job_state, &site, &source, &host_key, with_database, manual).await
    })?;
    Ok(Json(json!({ "job_id": job_id })))
}

async fn options(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Options>> {
    user.require(AREA)?;
    let owners = match user.role {
        Role::Admin | Role::Reseller => {
            let (all, me) = (user.is_admin(), user.id);
            state.db.call(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, username FROM users
                     WHERE role != 'member' AND (?1 OR id = ?2 OR parent_id = ?2) ORDER BY username",
                )?;
                let rows = stmt.query_map(params![all, me], |row| Ok(OwnerOption { id: row.get(0)?, username: row.get(1)? }))?;
                rows.collect()
            })?
        }
        _ => Vec::new(),
    };
    let php_versions = php::installed_versions();
    let php_defaults = php_versions.iter().map(|version| (version.clone(), php::load_global(version))).collect();
    let ips = crate::addresses::selectable(&state.db, user.is_admin(), user.owner_id())
        .await?
        .into_iter()
        .map(|item| IpOption { address: item.address, label: item.label, user_id: item.user_id })
        .collect();
    Ok(Json(Options { nginx_installed: nginx::is_installed(), php_versions, owners, php_defaults, node_versions: node::installed_versions(), python_versions: node::python_versions(), python_runtimes: crate::hosting::pyenv::installed_versions(), ips }))
}

#[derive(Deserialize)]
struct CreateBody {
    domain: String,
    kind: String,
    php_version: Option<String>,
    proxy_target: Option<String>,
    node_version: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    /// Sitenin sahibi; verilmezse isteği yapan hesap.
    user_id: Option<i64>,
}

/// Sitenin hangi hesaba açılacağını belirler ve isteği yapanın buna yetkisini denetler.
fn resolve_owner(state: &AppState, user: &AuthUser, requested: Option<i64>) -> AppResult<i64> {
    let Some(target) = requested.filter(|id| *id != user.owner_id()) else {
        return Ok(user.owner_id());
    };
    let found = state.db.call(|conn| {
        conn.query_row("SELECT role, parent_id FROM users WHERE id = ?1", params![target], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
        })
        .optional()
    })?;
    let (role, parent_id) = found.ok_or_else(|| AppError::bad("Seçilen hesap bulunamadı"))?;
    let allowed = match user.role {
        Role::Admin => true,
        Role::Reseller => parent_id == Some(user.id),
        _ => false,
    };
    if !allowed || role == "member" {
        return Err(AppError::Forbidden);
    }
    Ok(target)
}

fn validate_kind(kind: &str) -> AppResult<()> {
    if sites::KINDS.contains(&kind) { Ok(()) } else { Err(AppError::bad("Geçersiz site türü")) }
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateBody>,
) -> AppResult<Json<Site>> {
    user.require(AREA)?;
    validate_kind(&body.kind)?;
    let owner_id = resolve_owner(&state, &user, body.user_id)?;
    crate::quota::check(&state.db, owner_id, crate::quota::Resource::Site)?;
    let domain_name = domain::normalize(&body.domain)?;
    let aliases = sites::normalize_aliases(&domain_name, &body.aliases)?;
    let mut all_names = vec![domain_name.clone()];
    all_names.extend(aliases.iter().cloned());
    sites::check_names_allowed(&state.db, &user, owner_id, &all_names)?;

    let php_version = match body.kind.as_str() {
        "php" => {
            let version = body.php_version.ok_or_else(|| AppError::bad("PHP sürümü seçin"))?;
            php::validate_version(&version)?;
            Some(version)
        }
        _ => None,
    };
    let proxy_target = match body.kind.as_str() {
        "proxy" => {
            let target = body.proxy_target.ok_or_else(|| AppError::bad("Hedef adresi girin"))?;
            Some(sites::validate_proxy_target(&target, user.is_admin(), state.cfg.listen_port)?)
        }
        _ => None,
    };

    let node_settings = match body.kind.as_str() {
        "node" => {
            let settings = NodeSettings { version: body.node_version.ok_or_else(|| AppError::bad("Node.js sürümü seçin"))?, ..NodeSettings::default() };
            settings.validate("node")?;
            settings
        }
        "python" => node::python_defaults()?,
        _ => NodeSettings::default(),
    };

    let new = NewSite { user_id: owner_id, domain: domain_name, aliases, kind: body.kind, php_version, proxy_target, node: node_settings };
    Ok(Json(create_site(&state, &user, new).await?))
}

/// Doğrulanmış site kaydını ekler, sisteme uygular (kullanıcı, klasörler, PHP havuzu, nginx) ve
/// alan adının DNS'i panelde yönetiliyorsa adres kaydını açar. Yarıda kalırsa geride kayıt bırakmaz.
/// Yedekten içe aktarma da siteyi aynı yoldan açar.
pub(crate) async fn create_site(state: &AppState, user: &AuthUser, new: NewSite) -> AppResult<Site> {
    let owner_id = new.user_id;
    let domain_name = new.domain.clone();
    // Sistem dosyalarına dokunan işlemler sırayla yürür.
    let _guard = state.sys_lock.lock().await;
    let id = sites::insert(&state.db, &new)?;
    let mut site = sites::get(&state.db, user, id)?;
    // Hesaba ayrılmış adres varsa yeni site ona bağlanarak açılır.
    let dedicated: Option<String> = state.db.call(|conn| {
        conn.query_row("SELECT address FROM server_ips WHERE user_id = ?1 ORDER BY created_at, address LIMIT 1", params![owner_id], |row| row.get(0)).optional()
    })?;
    if let Some(address) = dedicated {
        site.ip = address;
        sites::save(&state.db, &site)?;
    }
    if let Err(err) = sites::provision(&site).await {
        // Yarım kalan kurulum geri alınır; kullanıcının dosyalarına dokunulmaz.
        if let Err(cleanup) = sites::deprovision(&site, false).await {
            tracing::warn!(error = %cleanup, domain = %site.domain, "yarım site temizlenemedi");
        }
        sites::delete_row(&state.db, id)?;
        return Err(err);
    }
    // Alan adının DNS'i panelde yönetiliyorsa adres kaydı kendiliğinden eklenir.
    let mut names = vec![site.domain.clone()];
    names.extend(site.aliases.iter().cloned());
    for line in crate::dns::add_site_records(state, site.user_id, &names).await {
        tracing::info!(domain = %site.domain, "DNS: {line}");
    }
    auth::audit(&state.db, user, "site.create", &domain_name, &site.kind);
    Ok(site)
}

#[derive(Deserialize)]
struct UpdateBody {
    kind: Option<String>,
    aliases: Option<Vec<String>>,
    public_dir: Option<String>,
    php_version: Option<String>,
    php_settings: Option<PhpSettings>,
    proxy_target: Option<String>,
    proxy_websocket: Option<bool>,
    force_https: Option<bool>,
    hsts: Option<bool>,
    nginx_custom: Option<String>,
    status: Option<String>,
    node: Option<NodeSettings>,
    proxy_rules: Option<Vec<ProxyRule>>,
    template: Option<String>,
    /// Sitenin bağlanacağı sunucu adresi; boş metin bütün adresler demektir.
    ip: Option<String>,
    /// Site kurallarının tamamı (yönlendirmeler, parola koruması…); verilirse öncekinin yerini alır.
    rules: Option<crate::hosting::rules::SiteRules>,
}

async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateBody>,
) -> AppResult<Json<Site>> {
    user.require(AREA)?;
    let _guard = state.sys_lock.lock().await;
    let previous = sites::get(&state.db, &user, id)?;
    let mut site = previous.clone();

    if let Some(kind) = body.kind {
        validate_kind(&kind)?;
        site.kind = kind;
    }
    if let Some(aliases) = &body.aliases {
        site.aliases = sites::normalize_aliases(&site.domain, aliases)?;
        // Yalnızca yeni eklenen adlar denetlenir; var olanlar eski kurala göre kabul edilmişti.
        let added: Vec<String> = site.aliases.iter().filter(|name| !previous.aliases.contains(name)).cloned().collect();
        sites::check_names_allowed(&state.db, &user, site.user_id, &added)?;
    }
    if let Some(public_dir) = &body.public_dir {
        site.public_dir = sites::validate_public_dir(public_dir)?;
    }
    if let Some(version) = body.php_version {
        php::validate_version(&version)?;
        site.php_version = Some(version);
    }
    if let Some(settings) = body.php_settings {
        settings.validate()?;
        site.php_settings = settings;
    }
    if let Some(target) = &body.proxy_target {
        site.proxy_target = Some(sites::validate_proxy_target(target, user.is_admin(), state.cfg.listen_port)?);
    }
    if let Some(websocket) = body.proxy_websocket {
        site.proxy_websocket = websocket;
    }
    if let Some(force_https) = body.force_https {
        site.force_https = force_https;
    }
    if let Some(hsts) = body.hsts {
        site.hsts = hsts;
    }
    if let Some(template) = body.template {
        if !crate::hosting::nginx::template_allowed(&site.kind, &template) {
            return Err(AppError::bad("Bu şablon bu site türünde kullanılamaz"));
        }
        site.template = template;
    }
    if let Some(rules) = body.rules {
        let mut names = vec![site.domain.clone()];
        names.extend(site.aliases.iter().cloned());
        site.rules = crate::hosting::rules::normalize(rules, &previous.rules, &names, &site.kind).await?;
    }
    if let Some(ip) = &body.ip {
        site.ip = crate::addresses::validate_for_site(&state.db, user.is_admin(), site.user_id, ip).await?;
    }
    if let Some(custom) = body.nginx_custom {
        // Ham nginx yönergeleri sunucunun tamamını etkileyebilir.
        user.require_admin()?;
        if custom.len() > 16 * 1024 {
            return Err(AppError::bad("Özel yönergeler 16 KB'ı aşamaz"));
        }
        site.nginx_custom = custom;
    }
    if let Some(status) = body.status {
        if !matches!(status.as_str(), "active" | "suspended") {
            return Err(AppError::bad("Geçersiz durum"));
        }
        // Site sahibi, askıya alınmış sitesini kendi kendine açamaz.
        if !user.is_admin() && !(user.role == Role::Reseller && site.user_id != user.id) {
            return Err(AppError::Forbidden);
        }
        site.status = status;
    }
    if let Some(settings) = body.node {
        site.node = settings;
    }
    if let Some(rules) = &body.proxy_rules {
        site.proxy_rules = sites::validate_proxy_rules(rules, user.is_admin(), state.cfg.listen_port)?;
    }
    if node::is_app(&site.kind) {
        // Tür yeni değiştiyse ayarlar öteki çalışma ortamından kalmış olabilir; yenisinin başlangıç değerleri verilir.
        // Python yorumlayıcısı değişmişse sitenin sanal ortamı, servis yazılırken (`node::apply`) yeni yorumlayıcıyla yeniden kurulur.
        if site.kind == "python" {
            site.node.reconcile_python(previous.kind == "python")?;
        }
        site.node.validate(&site.kind)?;
    }
    if site.kind != "python" {
        site.node.python.clear();
    }
    if site.kind == "php" && site.php_version.is_none() {
        return Err(AppError::bad("PHP sürümü seçin"));
    }
    if site.kind == "proxy" && site.proxy_target.is_none() {
        return Err(AppError::bad("Hedef adresi girin"));
    }
    if (site.force_https || site.hsts) && !sites::ssl_ready(&site) {
        return Err(AppError::bad("HTTPS yönlendirmesi için önce siteye bir SSL sertifikası tanımlayın"));
    }

    sites::save(&state.db, &site)?;
    if let Err(err) = sites::provision(&site).await {
        // Yeni ayarlar uygulanamadıysa kayıt ve sistem eski hâline döndürülür.
        sites::save(&state.db, &previous)?;
        if let Err(restore) = sites::provision(&previous).await {
            tracing::error!(error = %restore, domain = %previous.domain, "site eski ayarlarına döndürülemedi");
        }
        return Err(err);
    }
    if site.ip != previous.ip {
        // Adres değişti: panelin yönettiği DNS kayıtları yeni adrese çevrilir.
        let mut names = vec![site.domain.clone()];
        names.extend(site.aliases.iter().cloned());
        let old = crate::dns::site_address_record(&state.db, &site.domain, &previous.ip);
        for line in crate::dns::move_site_records(&state, site.user_id, &names, &old).await {
            tracing::info!(domain = %site.domain, "DNS: {line}");
        }
    }
    auth::audit(&state.db, &user, "site.update", &site.domain, "");
    Ok(Json(site))
}

/// Sitenin sunucu önbelleğinin içeriğini siler (önbellek kapalıysa bir şey yapılmaz).
async fn purge_cache(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let _guard = state.sys_lock.lock().await;
    let site = sites::get(&state.db, &user, id)?;
    crate::hosting::rules::purge_cache(site.id).await?;
    auth::audit(&state.db, &user, "site.cache.purge", &site.domain, "");
    Ok(Json(json!({ "ok": true })))
}

/// Node.js sitesini okur; site başka türdeyse hata döner.
fn node_site(state: &AppState, user: &AuthUser, id: i64) -> AppResult<Site> {
    let site = sites::get(&state.db, user, id)?;
    if node::is_app(&site.kind) { Ok(site) } else { Err(AppError::bad("Bu site bir uygulama sitesi değil")) }
}

async fn node_status(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<node::Status>> {
    user.require(AREA)?;
    let site = node_site(&state, &user, id)?;
    Ok(Json(app_status(&site).await?))
}

/// Uygulamanın durumu; Python sitelerinde bağımlılık dosyası da eklenir.
async fn app_status(site: &Site) -> AppResult<node::Status> {
    let mut status = node::status(site.id).await?;
    if site.kind == "python" {
        status.dependencies = node::dependency_file(&site.root());
    }
    Ok(status)
}

async fn node_action(State(state): State<AppState>, user: AuthUser, Path((id, action)): Path<(i64, String)>) -> AppResult<Json<node::Status>> {
    user.require(AREA)?;
    let site = node_site(&state, &user, id)?;
    if site.status != "active" && action != "stop" {
        return Err(AppError::bad("Askıdaki sitenin uygulaması başlatılamaz"));
    }
    node::action(site.id, &action).await?;
    auth::audit(&state.db, &user, &format!("node.{action}"), &site.domain, "");
    Ok(Json(app_status(&site).await?))
}

/// Python sitesinin bağımlılıklarını sanal ortama kurar: `requirements.txt` varsa `pip install -r`,
/// yoksa `pyproject.toml` için `pip install .`. Komut sitenin kullanıcısıyla çalışır.
fn python_install(state: &AppState, user: &AuthUser, site: Site, restart: bool) -> AppResult<Json<serde_json::Value>> {
    let account = crate::hosting::sysuser::lookup(&site.owner).ok_or_else(|| AppError::bad("Hesabın Linux kullanıcısı bulunamadı"))?;
    let root = site.root();
    let (shown, args): (&str, Vec<&str>) = match node::dependency_file(&root) {
        Some("requirements.txt") => ("pip install -r requirements.txt", vec!["install", "--no-input", "--disable-pip-version-check", "-r", "requirements.txt"]),
        Some(_) => ("pip install .", vec!["install", "--no-input", "--disable-pip-version-check", "."]),
        None => return Err(AppError::bad("Site klasöründe requirements.txt ya da pyproject.toml yok; bağımlılıklarınızı bu dosyalardan birine yazın")),
    };
    let venv_bin = root.join(node::VENV_DIR).join("bin");
    let title = format!("pip install ({})", site.domain);
    let site_id = site.id;
    let job_id = state.jobs.start("python.pip", &title, Some(user.id), move |ctx| async move {
        ctx.log(format!("$ {shown}"));
        let mut command = runas::command(&account, &root, "pip", &args);
        command.env("PATH", format!("{}:{}", venv_bin.display(), runas::USER_PATH)).env("VIRTUAL_ENV", venv_bin.parent().unwrap_or(&root));
        ctx.run_streaming(&mut command, "pip", None).await?;
        if restart {
            ctx.log("Uygulama yeniden başlatılıyor");
            node::action(site_id, "restart").await?;
        }
        Ok(())
    })?;
    auth::audit(&state.db, user, "python.pip", &site.domain, "");
    Ok(Json(serde_json::json!({ "job_id": job_id })))
}

#[derive(Deserialize)]
struct NodeRunBody {
    /// `install`, `ci` ya da package.json içindeki bir betiğin adı (ör. `build`).
    script: String,
    /// Bitince uygulama yeniden başlatılsın mı?
    #[serde(default)]
    restart: bool,
}

/// Site klasöründe, sitenin kullanıcısıyla bir npm komutu çalıştırır (bağımlılık kurma, derleme).
async fn node_run(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<NodeRunBody>,
) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let site = node_site(&state, &user, id)?;
    if site.kind == "python" {
        return python_install(&state, &user, site, body.restart);
    }
    let args: Vec<String> = match body.script.as_str() {
        "install" => vec!["install".into()],
        "ci" => vec!["ci".into()],
        script => {
            let valid = !script.is_empty() && script.len() <= 64 && script.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'_' | b'-' | b'.'));
            if !valid {
                return Err(AppError::bad("Geçersiz betik adı"));
            }
            vec!["run".into(), script.to_string()]
        }
    };
    let account = crate::hosting::sysuser::lookup(&site.owner).ok_or_else(|| AppError::bad("Hesabın Linux kullanıcısı bulunamadı"))?;
    let title = format!("npm {} ({})", args.join(" "), site.domain);
    let (root, bin, site_id, restart) = (site.root(), node::bin_dir(&site.node.version), site.id, body.restart);
    let job_id = state.jobs.start("node.npm", &title, Some(user.id), move |ctx| async move {
        ctx.log(format!("$ npm {}", args.join(" ")));
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut command = runas::command(&account, &root, "npm", &args);
        command.env("PATH", format!("{}:{}", bin.display(), runas::USER_PATH)).env("NODE_ENV", "production").env("CI", "1");
        ctx.run_streaming(&mut command, "npm", None).await?;
        if restart {
            ctx.log("Uygulama yeniden başlatılıyor");
            node::action(site_id, "restart").await?;
        }
        Ok(())
    })?;
    auth::audit(&state.db, &user, "node.npm", &site.domain, &body.script);
    Ok(Json(serde_json::json!({ "job_id": job_id })))
}

#[derive(Deserialize)]
struct RemoveQuery {
    #[serde(default)]
    delete_files: bool,
}

async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Query(query): Query<RemoveQuery>,
) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let _guard = state.sys_lock.lock().await;
    let site = sites::get(&state.db, &user, id)?;
    super::ftp::remove_for_site(&state, site.id).await;
    sites::deprovision(&site, query.delete_files).await?;
    sites::delete_row(&state.db, id)?;
    let detail = if query.delete_files { "dosyalarla birlikte" } else { "" };
    auth::audit(&state.db, &user, "site.delete", &site.domain, detail);
    Ok(Json(json!({ "ok": true })))
}
