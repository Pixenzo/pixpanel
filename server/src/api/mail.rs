//! Posta yönetimi uçları: alan adları, kutular, takma adlar, kuyruk ve genel ayarlar.

use std::time::Duration;

use axum::extract::{Path, Query, State};
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
        .route("/domains/{id}/mailboxes", post(create_mailbox))
        .route("/domains/{id}/aliases", post(create_alias))
        .route("/mailboxes/{id}", patch(update_mailbox).delete(remove_mailbox))
        .route("/aliases/{id}", patch(update_alias).delete(remove_alias))
        .route("/queue", get(queue))
        .route("/queue/flush", post(flush_queue))
        .route("/queue/{id}", delete(remove_queued))
        .route("/settings", get(settings).put(save_settings))
        .route("/setup", post(run_setup))
        .route("/activity", get(activity))
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
    // Paylaşımlı sunucuda bir hesap başkasının (ya da dış dünyanın) alan adını
    // yerel posta alanı yapıp o adrese giden postayı ele geçirebilirdi; bu yüzden
    // yönetici dışındakiler yalnızca kendi sitelerinin adlarını kullanabilir.
    if !user.is_admin() && !site_names(&state, owner_id)?.contains(&domain) {
        return Err(AppError::bad("Posta yalnızca hesabınızdaki bir sitenin alan adı için açılabilir"));
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
    /// Boş dize "hepsini yakala" adresini kaldırır.
    catch_all: String,
}

async fn update_domain(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateDomainBody>,
) -> AppResult<Json<MailDomain>> {
    user.require(AREA)?;
    let domain = mail::get_domain(&state.db, &user, id)?;
    let catch_all = if body.catch_all.trim().is_empty() { String::new() } else { mail::validate_address(&body.catch_all)? };
    let _guard = state.sys_lock.lock().await;
    state.db.call(|conn| conn.execute("UPDATE mail_domains SET catch_all = ?1 WHERE id = ?2", params![catch_all, id]))?;
    mail::sync(&state.db).await?;
    auth::audit(&state.db, &user, "mail.domain.update", &domain.domain, "");
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
    auth::audit(&state.db, &user, "mail.mailbox.update", &mailbox.address, "");
    Ok(Json(mailbox))
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
