//! Posta yönetimi uçları: alan adları, kutular, takma adlar, kuyruk ve genel ayarlar.

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::{self, AuthUser};
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::hosting::domain as domain_rules;
use crate::mail::sieve::{self, AutoReply, Filter};
use crate::mail::{self, DnsRecord, MailAlias, MailDomain, Mailbox, SpamSettings};
use crate::state::AppState;
use crate::sys::{exec, services};

const AREA: &str = "mail";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(overview))
        .route("/domains", post(create_domain))
        .route("/domains/{id}", get(domain_detail).patch(update_domain).delete(remove_domain))
        .route("/domains/{id}/dns", get(check_dns))
        .route("/domains/{id}/dns/apply", post(apply_dns))
        .route("/domains/{id}/client-settings", get(client_settings))
        .route("/domains/{id}/mailboxes", post(create_mailbox))
        .route("/domains/{id}/aliases", post(create_alias))
        .route("/mailboxes/{id}", patch(update_mailbox).delete(remove_mailbox))
        .route("/aliases/{id}", patch(update_alias).delete(remove_alias))
        .route("/queue", get(queue))
        .route("/queue/flush", post(flush_queue))
        .route("/queue/{id}", delete(remove_queued))
        .route("/settings", get(settings).put(save_settings))
        .route("/limits", get(limit_settings).put(save_limit_settings))
        .route("/limit-hits", get(limit_hits))
        .route("/setup", post(run_setup))
        .route("/activity", get(activity))
        // Oturum istemeyen uçlar: posta istemcilerinin otomatik ayarı (nginx `autoconfig.`/`autodiscover.`
        // adlarından ve sitelerin `.well-known` yolundan buraya vekil olur). Yalnızca alan adına bakarlar.
        .route("/public/autoconfig", get(public_autoconfig))
        .route("/public/mobileconfig", get(public_mobileconfig))
        .route("/public/autodiscover", post(public_autodiscover))
}

fn ensure_installed() -> AppResult<()> {
    if mail::is_installed() { Ok(()) } else { Err(AppError::bad("Posta sunucusu kurulu değil; önce Bileşenler sayfasından kurun")) }
}

#[derive(Serialize)]
struct ServiceState {
    name: &'static str,
    label: &'static str,
    running: bool,
}

#[derive(Serialize)]
struct OwnerOption {
    id: i64,
    username: String,
}

#[derive(Serialize)]
struct Overview {
    installed: bool,
    services: Vec<ServiceState>,
    hostname: String,
    /// Web postası (Roundcube) kurulu mu? Kuruluysa `/webmail/` yolunda yayınlanır.
    webmail: bool,
    domains: Vec<MailDomain>,
    /// Posta açılabilecek alan adları (hesabın siteleri; yönetici her adı yazabilir).
    available_domains: Vec<String>,
    owners: Vec<OwnerOption>,
}

/// Hesabın sitelerinin alan adları ve takma adları.
fn site_names(state: &AppState, owner_id: i64) -> AppResult<Vec<String>> {
    state.db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT domain FROM sites WHERE user_id = ?1
             UNION SELECT a.name FROM site_aliases a JOIN sites s ON s.id = a.site_id WHERE s.user_id = ?1
             ORDER BY 1",
        )?;
        stmt.query_map(params![owner_id], |row| row.get(0))?.collect()
    })
}

async fn overview(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Overview>> {
    user.require(AREA)?;
    let installed = mail::is_installed();
    let mut service_states = Vec::new();
    for (name, label) in mail::SERVICES {
        service_states.push(ServiceState { name, label, running: installed && services::is_active(name).await });
    }
    let domains = mail::list_domains(&state.db, &user)?;
    let taken: Vec<&str> = domains.iter().map(|d| d.domain.as_str()).collect();
    // `www.` ile başlayan adlar posta için anlamsızdır.
    let available_domains =
        site_names(&state, user.owner_id())?.into_iter().filter(|name| !name.starts_with("www.") && !taken.contains(&name.as_str())).collect();
    let owners = if user.is_admin() {
        state.db.call(|conn| {
            let mut stmt = conn.prepare("SELECT id, username FROM users WHERE role != 'member' ORDER BY username")?;
            stmt.query_map([], |row| Ok(OwnerOption { id: row.get(0)?, username: row.get(1)? }))?.collect()
        })?
    } else {
        Vec::new()
    };
    let webmail = crate::dbtools::installed().roundcube;
    if webmail {
        // Paket panel dışında kurulduysa yapılandırma ilk bakışta tamamlanır.
        if let Err(err) = crate::dbtools::ensure_runtime(&state.cfg).await {
            tracing::warn!(error = %err, "web postası yapılandırılamadı");
        }
    }
    Ok(Json(Overview { installed, services: service_states, hostname: mail::hostname(&state.db), webmail, domains, available_domains, owners }))
}

// ---- Alan adları ----

#[derive(Deserialize)]
struct CreateDomainBody {
    domain: String,
    /// Yalnızca yönetici: alan adının atanacağı hesap.
    user_id: Option<i64>,
}

async fn create_domain(State(state): State<AppState>, user: AuthUser, Json(body): Json<CreateDomainBody>) -> AppResult<Json<MailDomain>> {
    user.require(AREA)?;
    ensure_installed()?;
    let domain = domain_rules::normalize(&body.domain)?;
    let owner_id = match body.user_id.filter(|id| *id != user.owner_id()) {
        Some(target) => {
            user.require_admin()?;
            target
        }
        None => user.owner_id(),
    };
    crate::quota::check(&state.db, owner_id, crate::quota::Resource::MailDomain)?;
    // Paylaşımlı sunucuda bir hesap başkasının (ya da dış dünyanın) alan adını
    // yerel posta alanı yapıp o adrese giden postayı ele geçirebilirdi; bu yüzden
    // yönetici dışındakiler yalnızca kendi sitelerinin adlarını kullanabilir.
    if !user.is_admin() && !site_names(&state, owner_id)?.contains(&domain) {
        return Err(AppError::bad("Posta yalnızca hesabınızdaki bir sitenin alan adı için açılabilir"));
    }
    // Site açmak alan adının sahibi olmayı kanıtlamaz (herkes istediği adla site açabilir). Posta
    // alanı açılınca bu sunucudaki bütün uygulamaların o adrese gönderdiği posta yerel teslim
    // edilir; bu yüzden alan adının gerçekten bu sunucuya yönlendirilmiş olması aranır.
    if !user.is_admin() && !crate::addresses::domain_points_here(&state.db, &domain).await {
        return Err(AppError::bad(format!(
            "{domain} henüz bu sunucuyu göstermiyor. Önce alan adının DNS kayıtlarını (A kaydı) bu sunucuya yönlendirin; kayıt yayıldıktan sonra postayı açabilirsiniz. Alan adını henüz taşımadıysanız yöneticiniz postayı sizin adınıza açabilir."
        )));
    }
    let _guard = state.sys_lock.lock().await;
    let dkim_record = mail::ensure_dkim(&domain).await?;
    let inserted = state.db.call(|conn| {
        conn.execute(
            "INSERT OR IGNORE INTO mail_domains (user_id, domain, dkim_record, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![owner_id, domain, dkim_record, now()],
        )?;
        Ok((conn.changes() > 0).then(|| conn.last_insert_rowid()))
    })?;
    let id = inserted.ok_or_else(|| AppError::conflict(format!("{domain} için posta zaten açık")))?;
    mail::sync(&state.db).await?;
    // Alan adının DNS'i panelde yönetiliyorsa posta kayıtları kendiliğinden eklenir.
    for line in crate::dns::add_mail_records(&state, owner_id, &domain, &mail::hostname(&state.db), mail::DKIM_SELECTOR, &dkim_record).await {
        tracing::info!(domain = %domain, "DNS: {line}");
    }
    auth::audit(&state.db, &user, "mail.domain.create", &domain, "");
    Ok(Json(mail::get_domain(&state.db, &user, id)?))
}

#[derive(Serialize)]
struct DomainDetail {
    domain: MailDomain,
    mailboxes: Vec<Mailbox>,
    aliases: Vec<MailAlias>,
    dns: Vec<DnsRecord>,
    hostname: String,
    /// Alan adının DNS'i panelde yönetiliyor mu? Öyleyse kayıtlar tek tıkla eklenebilir.
    dns_managed: bool,
    /// Web postası (Roundcube) kurulu mu? Kuruluysa kutuların yanında açma bağlantısı gösterilir.
    webmail: bool,
}

async fn domain_detail(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<DomainDetail>> {
    user.require(AREA)?;
    let domain = mail::get_domain(&state.db, &user, id)?;
    let mut mailboxes = mail::list_mailboxes(&state.db, Some(id))?;
    if mail::is_installed() && !mailboxes.is_empty() {
        let usage = mail::usage().await;
        for mailbox in &mut mailboxes {
            mailbox.used_bytes = usage.get(&mailbox.address).copied();
        }
    }
    let hostname = mail::hostname(&state.db);
    Ok(Json(DomainDetail {
        dns_managed: crate::dns::zone_for(&state.db, domain.user_id, &domain.domain)?.is_some(),
        dns: mail::dns_records(&domain, &hostname),
        aliases: mail::list_aliases(&state.db, Some(id))?,
        mailboxes,
        domain,
        hostname,
        webmail: crate::dbtools::installed().roundcube,
    }))
}

async fn check_dns(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Vec<DnsRecord>>> {
    user.require(AREA)?;
    let domain = mail::get_domain(&state.db, &user, id)?;
    let mut records = mail::dns_records(&domain, &mail::hostname(&state.db));
    mail::check_dns(&mut records).await;
    Ok(Json(records))
}

/// Posta kayıtlarını (MX, SPF, DKIM, DMARC) alan adının panelde yönetilen DNS bölgesine yazar.
async fn apply_dns(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let domain = mail::get_domain(&state.db, &user, id)?;
    if crate::dns::zone_for(&state.db, domain.user_id, &domain.domain)?.is_none() {
        return Err(AppError::bad("Bu alan adının DNS'i panelde yönetilmiyor; kayıtları DNS sağlayıcınıza elle ekleyin ya da DNS sayfasından bölgeyi bağlayın"));
    }
    let report =
        crate::dns::add_mail_records(&state, domain.user_id, &domain.domain, &mail::hostname(&state.db), mail::DKIM_SELECTOR, &domain.dkim_record).await;
    auth::audit(&state.db, &user, "mail.domain.dns", &domain.domain, "");
    Ok(Json(json!({ "report": report })))
}

#[derive(Deserialize)]
struct UpdateDomainBody {
    /// Boş dize "hepsini yakala" adresini kaldırır; verilmezse değişmez.
    catch_all: Option<String>,
    /// Saatlik alıcı sınırı (yalnızca yönetici/bayi): -1 varsayılana döner, 0 sınırsız.
    send_limit: Option<i64>,
}

/// Gönderim sınırı isteğini doğrular: -1 → `None` (varsayılan), aksi hâlde geçerli sınır. Değer
/// kaydedilenden farklıysa yalnızca yönetici ve bayi değiştirebilir (müşteri kendi sınırını yükseltemez).
fn resolve_send_limit(user: &AuthUser, requested: i64, current: Option<i64>) -> AppResult<Option<i64>> {
    let wanted = if requested == -1 { None } else { Some(mail::policy::validate_limit(requested)?) };
    if wanted != current && !(user.is_admin() || user.role == auth::Role::Reseller) {
        return Err(AppError::Forbidden);
    }
    Ok(wanted)
}

async fn update_domain(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateDomainBody>,
) -> AppResult<Json<MailDomain>> {
    user.require(AREA)?;
    let domain = mail::get_domain(&state.db, &user, id)?;
    let catch_all = match body.catch_all.as_deref() {
        None => None,
        Some(value) if value.trim().is_empty() => Some(String::new()),
        Some(value) => Some(mail::validate_address(value)?),
    };
    let send_limit = match body.send_limit {
        Some(requested) => Some(resolve_send_limit(&user, requested, domain.send_limit)?),
        None => None,
    };
    let _guard = state.sys_lock.lock().await;
    if let Some(catch_all) = &catch_all {
        state.db.call(|conn| conn.execute("UPDATE mail_domains SET catch_all = ?1 WHERE id = ?2", params![catch_all, id]))?;
    }
    if let Some(send_limit) = send_limit {
        state.db.call(|conn| conn.execute("UPDATE mail_domains SET send_limit = ?1 WHERE id = ?2", params![send_limit, id]))?;
        mail::policy::invalidate();
    }
    mail::sync(&state.db).await?;
    let detail = match send_limit {
        Some(limit) => format!("saatlik sınır: {}", limit.map(|value| value.to_string()).unwrap_or_else(|| "varsayılan".into())),
        None => String::new(),
    };
    auth::audit(&state.db, &user, "mail.domain.update", &domain.domain, &detail);
    Ok(Json(mail::get_domain(&state.db, &user, id)?))
}

#[derive(Deserialize)]
struct RemoveQuery {
    /// Kutulardaki postalar da silinsin mi?
    #[serde(default)]
    delete_mail: bool,
}

async fn remove_domain(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Query(query): Query<RemoveQuery>,
) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let domain = mail::get_domain(&state.db, &user, id)?;
    let _guard = state.sys_lock.lock().await;
    state.db.call(|conn| conn.execute("DELETE FROM mail_domains WHERE id = ?1", params![id]))?;
    mail::sync(&state.db).await?;
    mail::remove_dkim(&domain.domain);
    if query.delete_mail {
        mail::remove_mail_data(&domain.domain, None).await?;
    }
    auth::audit(&state.db, &user, "mail.domain.delete", &domain.domain, if query.delete_mail { "postalarla birlikte" } else { "" });
    Ok(Json(json!({ "ok": true })))
}

// ---- Kutular ----

#[derive(Deserialize)]
struct CreateMailboxBody {
    local: String,
    password: String,
    #[serde(default = "default_quota")]
    quota_mb: i64,
}

fn default_quota() -> i64 {
    1024
}

/// Adresin bu alan adında kutu ya da takma ad olarak kullanılmadığını denetler.
fn ensure_address_free(state: &AppState, domain_id: i64, local: &str) -> AppResult<()> {
    let taken: bool = state.db.call(|conn| {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM mailboxes WHERE domain_id = ?1 AND local = ?2)
                 OR EXISTS(SELECT 1 FROM mail_aliases WHERE domain_id = ?1 AND local = ?2)",
            params![domain_id, local],
            |row| row.get(0),
        )
    })?;
    if taken { Err(AppError::conflict("Bu adres zaten kullanılıyor")) } else { Ok(()) }
}

async fn create_mailbox(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<CreateMailboxBody>,
) -> AppResult<Json<Mailbox>> {
    user.require(AREA)?;
    ensure_installed()?;
    let domain = mail::get_domain(&state.db, &user, id)?;
    crate::quota::check(&state.db, domain.user_id, crate::quota::Resource::Mailbox)?;
    let local = mail::validate_local(&body.local)?;
    auth::validate_password(&body.password)?;
    mail::validate_quota(body.quota_mb)?;
    ensure_address_free(&state, id, &local)?;
    let hash = auth::hash_password(&body.password)?;

    let _guard = state.sys_lock.lock().await;
    let mailbox_id = state.db.call(|conn| {
        conn.execute(
            "INSERT INTO mailboxes (domain_id, local, password_hash, quota_mb, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, local, hash, body.quota_mb, now()],
        )?;
        Ok(conn.last_insert_rowid())
    })?;
    mail::sync(&state.db).await?;
    auth::audit(&state.db, &user, "mail.mailbox.create", &format!("{local}@{}", domain.domain), "");
    Ok(Json(mail::load_mailbox(&state.db, mailbox_id)?))
}

/// Kutuyu okur ve isteği yapanın alan adına erişimi olduğunu doğrular.
fn authorized_mailbox(state: &AppState, user: &AuthUser, id: i64) -> AppResult<Mailbox> {
    let mailbox = mail::load_mailbox(&state.db, id)?;
    mail::get_domain(&state.db, user, mailbox.domain_id)?;
    Ok(mailbox)
}

#[derive(Deserialize)]
struct UpdateMailboxBody {
    password: Option<String>,
    quota_mb: Option<i64>,
    enabled: Option<bool>,
    forward_to: Option<Vec<String>>,
    forward_keep: Option<bool>,
    auto_reply: Option<AutoReply>,
    filters: Option<Vec<Filter>>,
    /// Saatlik alıcı sınırı (yalnızca yönetici/bayi): -1 varsayılana döner, 0 sınırsız.
    send_limit: Option<i64>,
}

async fn update_mailbox(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateMailboxBody>,
) -> AppResult<Json<Mailbox>> {
    user.require(AREA)?;
    ensure_installed()?;
    let previous = authorized_mailbox(&state, &user, id)?;
    let mut mailbox = previous.clone();

    if let Some(password) = &body.password {
        auth::validate_password(password)?;
        mailbox.password_hash = auth::hash_password(password)?;
    }
    if let Some(quota_mb) = body.quota_mb {
        mail::validate_quota(quota_mb)?;
        mailbox.quota_mb = quota_mb;
    }
    if let Some(enabled) = body.enabled {
        mailbox.enabled = enabled;
    }
    if let Some(forward_to) = &body.forward_to {
        mailbox.forward_to = mail::validate_forwards(forward_to)?;
        if mailbox.forward_to.contains(&mailbox.address) {
            return Err(AppError::bad("Kutu kendi adresine yönlendirilemez"));
        }
    }
    if let Some(forward_keep) = body.forward_keep {
        mailbox.forward_keep = forward_keep;
    }
    if let Some(auto_reply) = body.auto_reply {
        auto_reply.validate()?;
        mailbox.auto_reply = auto_reply;
    }
    if let Some(filters) = body.filters {
        sieve::validate_filters(&filters, mail::validate_address)?;
        mailbox.filters = filters;
    }
    if let Some(requested) = body.send_limit {
        mailbox.send_limit = resolve_send_limit(&user, requested, previous.send_limit)?;
    }

    let _guard = state.sys_lock.lock().await;
    let rules_changed = mailbox.filters != previous.filters
        || mailbox.auto_reply != previous.auto_reply
        || mailbox.forward_to != previous.forward_to
        || mailbox.forward_keep != previous.forward_keep;
    if rules_changed {
        // Betik derlenemezse kayıt değişmez; eski kurallar yürürlükte kalır.
        mail::apply_sieve(&mailbox).await?;
    }
    mail::save_mailbox(&state.db, &mailbox)?;
    mail::sync(&state.db).await?;
    if previous.enabled && !mailbox.enabled {
        mail::kick(&mailbox.address).await;
    }
    let detail = if mailbox.send_limit != previous.send_limit {
        format!("saatlik sınır: {}", mailbox.send_limit.map(|value| value.to_string()).unwrap_or_else(|| "varsayılan".into()))
    } else {
        String::new()
    };
    auth::audit(&state.db, &user, "mail.mailbox.update", &mailbox.address, &detail);
    mail::policy::invalidate();
    let mut updated = mailbox;
    mail::policy::decorate_mailboxes(&state.db, std::slice::from_mut(&mut updated));
    Ok(Json(updated))
}

async fn remove_mailbox(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Query(query): Query<RemoveQuery>,
) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let mailbox = authorized_mailbox(&state, &user, id)?;
    let _guard = state.sys_lock.lock().await;
    state.db.call(|conn| conn.execute("DELETE FROM mailboxes WHERE id = ?1", params![id]))?;
    mail::sync(&state.db).await?;
    mail::kick(&mailbox.address).await;
    if query.delete_mail {
        if let Some((local, domain)) = mailbox.address.split_once('@') {
            mail::remove_mail_data(domain, Some(local)).await?;
        }
    }
    auth::audit(&state.db, &user, "mail.mailbox.delete", &mailbox.address, if query.delete_mail { "postalarla birlikte" } else { "" });
    Ok(Json(json!({ "ok": true })))
}

// ---- Takma adlar ----

#[derive(Deserialize)]
struct AliasBody {
    #[serde(default)]
    local: String,
    destinations: Vec<String>,
}

async fn create_alias(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<AliasBody>) -> AppResult<Json<MailAlias>> {
    user.require(AREA)?;
    ensure_installed()?;
    let domain = mail::get_domain(&state.db, &user, id)?;
    let local = mail::validate_local(&body.local)?;
    let destinations = mail::validate_destinations(&body.destinations)?;
    ensure_address_free(&state, id, &local)?;
    let _guard = state.sys_lock.lock().await;
    let alias_id = state.db.call(|conn| {
        conn.execute(
            "INSERT INTO mail_aliases (domain_id, local, destinations, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, local, destinations.join(","), now()],
        )?;
        Ok(conn.last_insert_rowid())
    })?;
    mail::sync(&state.db).await?;
    auth::audit(&state.db, &user, "mail.alias.create", &format!("{local}@{}", domain.domain), "");
    Ok(Json(mail::load_alias(&state.db, alias_id)?))
}

fn authorized_alias(state: &AppState, user: &AuthUser, id: i64) -> AppResult<MailAlias> {
    let alias = mail::load_alias(&state.db, id)?;
    mail::get_domain(&state.db, user, alias.domain_id)?;
    Ok(alias)
}

async fn update_alias(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<AliasBody>) -> AppResult<Json<MailAlias>> {
    user.require(AREA)?;
    let alias = authorized_alias(&state, &user, id)?;
    let destinations = mail::validate_destinations(&body.destinations)?;
    let _guard = state.sys_lock.lock().await;
    state.db.call(|conn| conn.execute("UPDATE mail_aliases SET destinations = ?1 WHERE id = ?2", params![destinations.join(","), id]))?;
    mail::sync(&state.db).await?;
    auth::audit(&state.db, &user, "mail.alias.update", &alias.address, "");
    Ok(Json(mail::load_alias(&state.db, id)?))
}

async fn remove_alias(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let alias = authorized_alias(&state, &user, id)?;
    let _guard = state.sys_lock.lock().await;
    state.db.call(|conn| conn.execute("DELETE FROM mail_aliases WHERE id = ?1", params![id]))?;
    mail::sync(&state.db).await?;
    auth::audit(&state.db, &user, "mail.alias.delete", &alias.address, "");
    Ok(Json(json!({ "ok": true })))
}

// ---- Kuyruk (yalnızca yönetici) ----

async fn queue(user: AuthUser) -> AppResult<Json<Vec<Value>>> {
    user.require_admin()?;
    ensure_installed()?;
    let output = exec::run("postqueue", &["-j"]).await?;
    let entries = output
        .stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .map(|entry| {
            json!({
                "id": entry["queue_id"],
                "queue": entry["queue_name"],
                "arrived_at": entry["arrival_time"],
                "size": entry["message_size"],
                "sender": entry["sender"],
                "recipients": entry["recipients"].as_array().map(|list| {
                    list.iter().map(|r| json!({ "address": r["address"], "reason": r["delay_reason"] })).collect::<Vec<_>>()
                }),
            })
        })
        .collect();
    Ok(Json(entries))
}

async fn flush_queue(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    user.require_admin()?;
    ensure_installed()?;
    exec::run_ok("postqueue", &["-f"]).await?;
    auth::audit(&state.db, &user, "mail.queue.flush", "", "");
    Ok(Json(json!({ "ok": true })))
}

async fn remove_queued(State(state): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    ensure_installed()?;
    // Kuyruk kimliği onaltılık (kısa) ya da harf-rakam (uzun) biçimdedir; `ALL` gibi özel değerler kabul edilmez.
    let valid = (6..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric()) && id != "ALL";
    if !valid {
        return Err(AppError::bad("Geçersiz kuyruk kimliği"));
    }
    exec::run_ok("postsuper", &["-d", &id]).await?;
    auth::audit(&state.db, &user, "mail.queue.delete", &id, "");
    Ok(Json(json!({ "ok": true })))
}

// ---- Genel ayarlar ve etkinlik (yalnızca yönetici) ----

#[derive(Serialize)]
struct Settings {
    hostname: String,
    spam: SpamSettings,
}

async fn settings(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Settings>> {
    user.require_admin()?;
    Ok(Json(Settings { hostname: mail::hostname(&state.db), spam: mail::load_spam(&state.db)? }))
}

#[derive(Deserialize)]
struct SettingsBody {
    hostname: String,
    spam: SpamSettings,
}

async fn save_settings(State(state): State<AppState>, user: AuthUser, Json(body): Json<SettingsBody>) -> AppResult<Json<Settings>> {
    user.require_admin()?;
    ensure_installed()?;
    let spam = body.spam.normalized()?;
    let _guard = state.sys_lock.lock().await;
    let previous_hostname = mail::hostname(&state.db);
    let hostname = mail::set_hostname(&state.db, &body.hostname)?;
    mail::save_spam(&state.db, &spam).await?;
    if hostname != previous_hostname {
        // Sunucu adı Postfix ve sertifika seçimini etkiler; temel yapılandırma yeniden yazılır.
        mail::setup(&state.db, &state.cfg).await?;
    }
    auth::audit(&state.db, &user, "mail.settings", &hostname, "");
    Ok(Json(Settings { hostname, spam }))
}

/// Temel yapılandırmayı yeniden yazar ve servisleri yeniden başlatır. Paketler
/// panel dışında kurulduysa ya da yapılandırma elle bozulduysa onarmak için.
async fn run_setup(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    user.require_admin()?;
    ensure_installed()?;
    let _guard = state.sys_lock.lock().await;
    mail::setup(&state.db, &state.cfg).await?;
    auth::audit(&state.db, &user, "mail.setup", "", "");
    Ok(Json(json!({ "ok": true })))
}

/// Rspamd'nin denetleyicisinden özet sayılar ve son işlenen iletiler.
async fn activity(user: AuthUser) -> AppResult<Json<Value>> {
    user.require_admin()?;
    ensure_installed()?;
    let client = reqwest::Client::builder().timeout(Duration::from_secs(8)).build().map_err(|err| AppError::internal(err.to_string()))?;
    let fetch = |path: &'static str| {
        let client = client.clone();
        async move {
            let response = client.get(format!("http://127.0.0.1:11334{path}")).send().await.ok()?;
            response.json::<Value>().await.ok()
        }
    };
    let stat = fetch("/stat").await.unwrap_or(Value::Null);
    let history = fetch("/history").await.unwrap_or(Value::Null);
    let rows: Vec<Value> = history["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .take(100)
        .map(|row| {
            json!({
                "time": row["unix_time"],
                "sender": row["sender_smtp"],
                "recipients": row["rcpt_smtp"],
                "subject": row["subject"],
                "action": row["action"],
                "score": row["score"],
                "required_score": row["required_score"],
                "ip": row["ip"],
                "size": row["size"],
            })
        })
        .collect();
    Ok(Json(json!({
        "scanned": stat["scanned"],
        "actions": stat["actions"],
        "learned": stat["learned"],
        "messages": rows,
    })))
}

// ---- Giden posta sınırı ----

#[derive(Serialize, Deserialize)]
struct LimitSettings {
    /// Kutu başına saatlik alıcı sınırının sunucu geneli varsayılanı (0 sınırsız).
    default_limit: i64,
    /// Alan adı sınırı, ayrıca belirtilmemişse varsayılanın bu kadar katıdır (salt okunur).
    #[serde(default)]
    domain_factor: i64,
}

async fn limit_settings(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<LimitSettings>> {
    user.require_admin()?;
    Ok(Json(LimitSettings { default_limit: mail::policy::default_limit(&state.db), domain_factor: mail::policy::DOMAIN_FACTOR }))
}

async fn save_limit_settings(State(state): State<AppState>, user: AuthUser, Json(body): Json<LimitSettings>) -> AppResult<Json<LimitSettings>> {
    user.require_admin()?;
    mail::policy::save_default_limit(&state.db, body.default_limit)?;
    auth::audit(&state.db, &user, "mail.limit.settings", "", &format!("saatlik sınır: {}", body.default_limit));
    Ok(Json(LimitSettings { default_limit: body.default_limit, domain_factor: mail::policy::DOMAIN_FACTOR }))
}

/// Son günlerde sınıra takılan kutu ve alan adları (kullanıcı yalnızca kendi alan adlarını görür).
async fn limit_hits(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Vec<mail::policy::LimitHit>>> {
    user.require(AREA)?;
    let hits = mail::policy::list_hits(&state.db, 500)?;
    if user.is_admin() {
        return Ok(Json(hits));
    }
    let visible: Vec<String> = mail::list_domains(&state.db, &user)?.into_iter().map(|domain| domain.domain).collect();
    Ok(Json(hits.into_iter().filter(|hit| visible.contains(&hit.domain)).collect()))
}

// ---- İstemci ayarları ----

async fn client_settings(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let domain = mail::get_domain(&state.db, &user, id)?;
    let names = mail::autoconfig::names_status(&state.db, &domain.domain);
    let nginx_ready = crate::hosting::nginx::is_installed();
    let scheme = |index: usize| if names.get(index).is_some_and(|name| name.https) { "https" } else { "http" };
    Ok(Json(json!({
        "hostname": mail::hostname(&state.db),
        "imap_port": 993,
        "submission_port": 587,
        "smtps_port": 465,
        "nginx": nginx_ready,
        "names": names,
        "autoconfig_url": format!("{}://autoconfig.{}/mail/config-v1.1.xml", scheme(0), domain.domain),
        "autodiscover_url": format!("{}://autodiscover.{}/autodiscover/autodiscover.xml", scheme(1), domain.domain),
        "mobileconfig_url": format!("{}://autoconfig.{}/mail/mobileconfig?email=ADRES", scheme(0), domain.domain),
    })))
}

// ---- Genel uçlar (oturumsuz): otomatik istemci ayarı ----
//
// Bu uçlar bilerek `AuthUser` almaz (posta istemcileri oturum açamaz). Yalnızca alan adının bu
// sunucuda posta alan adı olup olmadığına bakarlar; adresin var olup olmadığını ele vermezler.

fn xml_response(content_type: &'static str, body: String) -> Response {
    let mut response = body.into_response();
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Alan adının bu sunucuda posta alan adı olduğunu doğrular; değilse 404.
fn require_mail_domain(state: &AppState, domain: &str) -> AppResult<()> {
    if mail::autoconfig::is_mail_domain(&state.db, domain) { Ok(()) } else { Err(AppError::NotFound) }
}

#[derive(Deserialize)]
struct AutoconfigQuery {
    emailaddress: Option<String>,
}

async fn public_autoconfig(State(state): State<AppState>, headers: HeaderMap, Query(query): Query<AutoconfigQuery>) -> AppResult<Response> {
    let domain = match query.emailaddress.as_deref() {
        Some(value) => mail::autoconfig::parse_email(value)?.1,
        // Adres verilmediyse alan adı istek yapılan `autoconfig.<alan>` adından alınır.
        None => headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .and_then(mail::autoconfig::domain_from_host)
            .ok_or_else(|| AppError::bad("E-posta adresi gerekli"))?,
    };
    require_mail_domain(&state, &domain)?;
    Ok(xml_response("application/xml; charset=utf-8", mail::autoconfig::render_autoconfig(&domain, &mail::hostname(&state.db))))
}

#[derive(Deserialize)]
struct MobileconfigQuery {
    email: Option<String>,
    emailaddress: Option<String>,
}

async fn public_mobileconfig(State(state): State<AppState>, Query(query): Query<MobileconfigQuery>) -> AppResult<Response> {
    let value = query.email.or(query.emailaddress).ok_or_else(|| AppError::bad("E-posta adresi gerekli"))?;
    let (local, domain) = mail::autoconfig::parse_email(&value)?;
    require_mail_domain(&state, &domain)?;
    let mut response = xml_response(
        "application/x-apple-aspen-config; charset=utf-8",
        mail::autoconfig::render_mobileconfig(&format!("{local}@{domain}"), &mail::hostname(&state.db)),
    );
    response.headers_mut().insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment; filename=\"mail.mobileconfig\""));
    Ok(response)
}

async fn public_autodiscover(State(state): State<AppState>, body: Bytes) -> AppResult<Response> {
    let raw = mail::autoconfig::extract_autodiscover_email(&body)?;
    let (local, domain) = mail::autoconfig::parse_email(&raw)?;
    require_mail_domain(&state, &domain)?;
    Ok(xml_response("text/xml; charset=utf-8", mail::autoconfig::render_autodiscover(&format!("{local}@{domain}"), &mail::hostname(&state.db))))
}
