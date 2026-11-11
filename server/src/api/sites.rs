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
        list.push(SiteSummary { site_id: site.id, ssl_expires_at, git, databases, cron_jobs, issues: state.health.site_issues(site.id), uptime_day, response_millis });
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
    Ok(Json(Options { nginx_installed: nginx::is_installed(), php_versions, owners, php_defaults, node_versions: node::installed_versions(), ips }))
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
            Some(sites::validate_proxy_target(&target)?)
        }
        _ => None,
    };

    let node_settings = match body.kind.as_str() {
        "node" => {
            let settings = NodeSettings { version: body.node_version.ok_or_else(|| AppError::bad("Node.js sürümü seçin"))?, ..NodeSettings::default() };
            settings.validate()?;
            settings
        }
        _ => NodeSettings::default(),
    };

    // Sistem dosyalarına dokunan işlemler sırayla yürür.
    let _guard = state.sys_lock.lock().await;
    let id = sites::insert(
        &state.db,
        &NewSite { user_id: owner_id, domain: domain_name.clone(), aliases, kind: body.kind, php_version, proxy_target, node: node_settings },
    )?;
    let mut site = sites::get(&state.db, &user, id)?;
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
    for line in crate::dns::add_site_records(&state, site.user_id, &names).await {
        tracing::info!(domain = %site.domain, "DNS: {line}");
    }
    auth::audit(&state.db, &user, "site.create", &domain_name, &site.kind);
    Ok(Json(site))
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
        site.proxy_target = Some(sites::validate_proxy_target(target)?);
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
        site.rules = crate::hosting::rules::normalize(rules, &previous.rules, &names).await?;
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
        settings.validate()?;
        site.node = settings;
    }
    if let Some(rules) = &body.proxy_rules {
        site.proxy_rules = sites::validate_proxy_rules(rules)?;
    }
    if site.kind == "node" {
        site.node.validate()?;
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

/// Node.js sitesini okur; site başka türdeyse hata döner.
fn node_site(state: &AppState, user: &AuthUser, id: i64) -> AppResult<Site> {
    let site = sites::get(&state.db, user, id)?;
    if site.kind == "node" { Ok(site) } else { Err(AppError::bad("Bu site bir Node.js uygulaması değil")) }
}

async fn node_status(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<node::Status>> {
    user.require(AREA)?;
    let site = node_site(&state, &user, id)?;
    Ok(Json(node::status(site.id).await?))
}

async fn node_action(State(state): State<AppState>, user: AuthUser, Path((id, action)): Path<(i64, String)>) -> AppResult<Json<node::Status>> {
    user.require(AREA)?;
    let site = node_site(&state, &user, id)?;
    if site.status != "active" && action != "stop" {
        return Err(AppError::bad("Askıdaki sitenin uygulaması başlatılamaz"));
    }
    node::action(site.id, &action).await?;
    auth::audit(&state.db, &user, &format!("node.{action}"), &site.domain, "");
    Ok(Json(node::status(site.id).await?))
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
