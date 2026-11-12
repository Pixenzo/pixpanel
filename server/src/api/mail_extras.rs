//! Posta ekleri: virüs taraması (ClamAV), alan adı başına sertifika (`mail.<alan>`, `webmail.<alan>`)
//! ve posta listeleri uçları. Yollar `api::mail` yönlendiricisine bağlıdır (`/mail/...`).

use axum::extract::{Path, State};
use axum::Json;
use futures_util::future::join_all;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::{self, AuthUser};
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::hosting::ssl::CertInfo;
use crate::mail::lists::{self, ListMember, MailList};
use crate::mail::{self, antivirus, tls};
use crate::state::AppState;

const AREA: &str = "mail";

fn ensure_installed() -> AppResult<()> {
    if mail::is_installed() { Ok(()) } else { Err(AppError::bad("Posta sunucusu kurulu değil; önce Bileşenler sayfasından kurun")) }
}

fn default_true() -> bool {
    true
}

// ---- Virüs taraması (yalnızca yönetici) ----

pub(super) async fn antivirus_status(user: AuthUser) -> AppResult<Json<antivirus::AntivirusStatus>> {
    user.require_admin()?;
    Ok(Json(antivirus::status().await))
}

/// Rspamd'nin virüs taraması yapılandırmasını yeniden yazar (bileşen paneller dışında kurulduysa ya da
/// yapılandırma elle bozulduysa onarmak için).
pub(super) async fn antivirus_apply(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<antivirus::AntivirusStatus>> {
    user.require_admin()?;
    ensure_installed()?;
    if !antivirus::is_installed() {
        return Err(AppError::bad("ClamAV kurulu değil; önce Bileşenler sayfasından kurun"));
    }
    let _guard = state.sys_lock.lock().await;
    antivirus::apply_config().await?;
    auth::audit(&state.db, &user, "mail.antivirus.apply", "", "");
    Ok(Json(antivirus::status().await))
}

// ---- Alan adı başına sertifika ----

#[derive(Serialize)]
struct NameView {
    name: String,
    /// `mail`, `autoconfig`, `autodiscover` ya da `webmail`.
    role: &'static str,
    /// Ad şu an bu sunucunun bir adresine çözülüyor mu?
    resolves_here: bool,
    /// Alan adının sertifikası bu adı kapsıyor mu?
    in_certificate: bool,
}

#[derive(Serialize)]
struct SuggestedRecord {
    kind: &'static str,
    name: String,
    content: String,
}

#[derive(Serialize)]
pub(super) struct OwnNameStatus {
    enabled: bool,
    webmail: bool,
    dns: bool,
    mail_host: String,
    webmail_host: String,
    names: Vec<NameView>,
    certificate: Option<CertInfo>,
    last_error: String,
    issued_at: Option<i64>,
    dns_managed: bool,
    mail_installed: bool,
    nginx_installed: bool,
    webmail_installed: bool,
    /// Alan adının DNS'i panelde yönetilmiyorsa elle eklenecek kayıtlar.
    suggested_records: Vec<SuggestedRecord>,
}

fn role_of(domain: &str, name: &str) -> &'static str {
    match name.strip_suffix(&format!(".{domain}")) {
        Some("mail") => "mail",
        Some("autoconfig") => "autoconfig",
        Some("autodiscover") => "autodiscover",
        _ => "webmail",
    }
}

async fn own_name_view(state: &AppState, domain_id: i64, domain: &mail::MailDomain) -> AppResult<OwnNameStatus> {
    let row = tls::get(&state.db, domain_id)?;
    let nginx_installed = crate::hosting::nginx::is_installed();
    let webmail_installed = crate::dbtools::installed().roundcube;
    let webmail = row.as_ref().map(|row| row.webmail).unwrap_or(webmail_installed && nginx_installed);
    let wanted = tls::certificate_names(&domain.domain, webmail);
    let certificate = tls::certificate_info(&domain.domain);
    let resolved = join_all(wanted.iter().map(|name| tls::resolves_here(&state.db, name))).await;
    let names: Vec<NameView> = wanted
        .iter()
        .zip(resolved)
        .map(|(name, resolves_here)| NameView {
            name: name.clone(),
            role: role_of(&domain.domain, name),
            resolves_here,
            in_certificate: certificate.as_ref().is_some_and(|info| info.names.iter().any(|pattern| pattern == name || pattern.strip_prefix("*.").is_some_and(|suffix| name.split_once('.').is_some_and(|(_, rest)| rest == suffix)))),
        })
        .collect();
    let dns_managed = crate::dns::zone_for(&state.db, domain.user_id, &domain.domain)?.is_some();
    let server_ip = crate::dns::server_ip(&state.db);
    let suggested_records = if dns_managed {
        Vec::new()
    } else {
        wanted.iter().map(|name| SuggestedRecord { kind: "A", name: name.clone(), content: server_ip.clone() }).collect()
    };
    Ok(OwnNameStatus {
        enabled: row.is_some(),
        webmail: row.as_ref().map(|row| row.webmail).unwrap_or(false),
        dns: row.as_ref().map(|row| row.dns).unwrap_or(false),
        mail_host: tls::mail_host(&domain.domain),
        webmail_host: tls::webmail_host(&domain.domain),
        names,
        certificate,
        last_error: row.as_ref().map(|row| row.last_error.clone()).unwrap_or_default(),
        issued_at: row.as_ref().and_then(|row| row.issued_at),
        dns_managed,
        mail_installed: mail::is_installed(),
        nginx_installed,
        webmail_installed,
        suggested_records,
    })
}

pub(super) async fn own_name_status(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<OwnNameStatus>> {
    user.require(AREA)?;
    let domain = mail::get_domain(&state.db, &user, id)?;
    Ok(Json(own_name_view(&state, id, &domain).await?))
}

#[derive(Deserialize)]
pub(super) struct OwnNameBody {
    enabled: bool,
    /// `webmail.<alan>` adresi de sunulsun mu?
    #[serde(default)]
    webmail: bool,
    /// Doğrulama DNS ile yapılsın mı (alan adının DNS'i panelde yönetiliyor olmalı)?
    #[serde(default)]
    dns: bool,
    /// DNS panelde yönetiliyorsa eksik A kayıtları kendiliğinden eklensin mi?
    #[serde(default = "default_true")]
    add_dns: bool,
}

pub(super) async fn own_name_save(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<OwnNameBody>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    ensure_installed()?;
    let domain = mail::get_domain(&state.db, &user, id)?;
    if !body.enabled {
        let _guard = state.sys_lock.lock().await;
        tls::disable(&state.db, id)?;
        tls::remove_files(&domain.domain);
        tls::apply_all(&state).await?;
        auth::audit(&state.db, &user, "mail.domain.own_name.disable", &domain.domain, "");
        return Ok(Json(json!({ "job_id": null, "dns_report": [] })));
    }

    let nginx_installed = crate::hosting::nginx::is_installed();
    if body.webmail && !crate::dbtools::installed().roundcube {
        return Err(AppError::bad("Web postası (Roundcube) kurulu değil; önce Bileşenler sayfasından kurun"));
    }
    if body.webmail && !nginx_installed {
        return Err(AppError::bad("webmail adresi için nginx gerekir; önce Bileşenler sayfasından kurun"));
    }
    let dns_managed = crate::dns::zone_for(&state.db, domain.user_id, &domain.domain)?.is_some();
    if body.dns && !dns_managed {
        return Err(AppError::bad("DNS ile doğrulama için alan adının DNS'i panelde yönetiliyor olmalı (DNS sayfasından bölgeyi ekleyin ya da Cloudflare'e bağlayın)"));
    }
    if !body.dns && !nginx_installed {
        return Err(AppError::bad("Web üzerinden doğrulama için nginx kurulu olmalı; DNS ile doğrulamayı deneyin"));
    }
    let names = tls::certificate_names(&domain.domain, body.webmail);
    if let Some(name) = tls::foreign_name(&state.db, domain.user_id, &names) {
        return Err(AppError::bad(format!("{name} adı başka bir hesabın sitesine ait; bu ad kullanılamaz")));
    }

    let _guard = state.sys_lock.lock().await;
    tls::enable(&state.db, id, body.webmail, body.dns)?;
    let dns_report = if body.add_dns && dns_managed { crate::dns::add_address_records(&state, domain.user_id, &names).await } else { Vec::new() };
    let job_id = if tls::needs_issue(tls::certificate_info(&domain.domain).as_ref(), &names, now()) {
        Some(tls::start_issue_job(&state, Some(user.id), id)?)
    } else {
        // Sertifika zaten tüm adları kapsıyor: yalnızca yapılandırmalar yenilenir.
        tls::apply_all(&state).await?;
        None
    };
    let detail = format!("{}{}", if body.webmail { "webmail, " } else { "" }, if body.dns { "DNS doğrulaması" } else { "web doğrulaması" });
    auth::audit(&state.db, &user, "mail.domain.own_name.enable", &domain.domain, &detail);
    Ok(Json(json!({ "job_id": job_id, "dns_report": dns_report })))
}

/// Sertifikayı hemen yeniden alır (adlar değiştiyse ya da yenileme beklenmeden).
pub(super) async fn own_name_renew(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    ensure_installed()?;
    let domain = mail::get_domain(&state.db, &user, id)?;
    if tls::get(&state.db, id)?.is_none() {
        return Err(AppError::bad("Bu alan adı için kendi adıyla bağlanma açık değil"));
    }
    let job_id = tls::start_issue_job(&state, Some(user.id), id)?;
    auth::audit(&state.db, &user, "mail.domain.own_name.renew", &domain.domain, "");
    Ok(Json(json!({ "job_id": job_id })))
}

// ---- Posta listeleri ----

/// Alan adına erişimi doğrular ve listenin o alan adına ait olduğunu denetler.
fn authorized_list(state: &AppState, user: &AuthUser, domain_id: i64, list_id: i64) -> AppResult<(mail::MailDomain, MailList)> {
    let domain = mail::get_domain(&state.db, user, domain_id)?;
    let list = lists::load_list(&state.db, list_id)?;
    if list.domain_id != domain_id {
        return Err(AppError::NotFound);
    }
    Ok((domain, list))
}

pub(super) async fn list_lists(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Vec<MailList>>> {
    user.require(AREA)?;
    mail::get_domain(&state.db, &user, id)?;
    Ok(Json(lists::list_lists(&state.db, id)?))
}

#[derive(Deserialize)]
pub(super) struct CreateListBody {
    local: String,
    #[serde(default)]
    mode: Option<String>,
}

pub(super) async fn create_list(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<CreateListBody>) -> AppResult<Json<MailList>> {
    user.require(AREA)?;
    ensure_installed()?;
    let domain = mail::get_domain(&state.db, &user, id)?;
    let local = mail::validate_local(&body.local)?;
    let mode = lists::validate_mode(body.mode.as_deref().unwrap_or("members"))?;
    super::mail::ensure_address_free(&state, id, &local)?;
    let _guard = state.sys_lock.lock().await;
    let list_id = state.db.call(|conn| {
        conn.execute("INSERT INTO mail_lists (domain_id, local, mode, created_at) VALUES (?1, ?2, ?3, ?4)", params![id, local, mode, now()])?;
        Ok(conn.last_insert_rowid())
    })?;
    mail::sync(&state.db).await?;
    auth::audit(&state.db, &user, "mail.list.create", &format!("{local}@{}", domain.domain), &mode);
    Ok(Json(lists::load_list(&state.db, list_id)?))
}

#[derive(Serialize)]
pub(super) struct ListDetail {
    list: MailList,
    members: Vec<ListMember>,
}

pub(super) async fn list_detail(State(state): State<AppState>, user: AuthUser, Path((id, list_id)): Path<(i64, i64)>) -> AppResult<Json<ListDetail>> {
    user.require(AREA)?;
    let (_, list) = authorized_list(&state, &user, id, list_id)?;
    Ok(Json(ListDetail { members: lists::list_members(&state.db, list_id)?, list }))
}

#[derive(Deserialize)]
pub(super) struct UpdateListBody {
    mode: String,
}

pub(super) async fn update_list(State(state): State<AppState>, user: AuthUser, Path((id, list_id)): Path<(i64, i64)>, Json(body): Json<UpdateListBody>) -> AppResult<Json<MailList>> {
    user.require(AREA)?;
    let (_, list) = authorized_list(&state, &user, id, list_id)?;
    let mode = lists::validate_mode(&body.mode)?;
    let _guard = state.sys_lock.lock().await;
    state.db.call(|conn| conn.execute("UPDATE mail_lists SET mode = ?1 WHERE id = ?2", params![mode, list_id]))?;
    mail::sync(&state.db).await?;
    auth::audit(&state.db, &user, "mail.list.update", &list.address, &mode);
    Ok(Json(lists::load_list(&state.db, list_id)?))
}

pub(super) async fn remove_list(State(state): State<AppState>, user: AuthUser, Path((id, list_id)): Path<(i64, i64)>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let (_, list) = authorized_list(&state, &user, id, list_id)?;
    let _guard = state.sys_lock.lock().await;
    state.db.call(|conn| conn.execute("DELETE FROM mail_lists WHERE id = ?1", params![list_id]))?;
    mail::sync(&state.db).await?;
    auth::audit(&state.db, &user, "mail.list.delete", &list.address, "");
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub(super) struct MembersBody {
    /// Toplu yapıştırılan adresler (satır, boşluk, virgül ya da noktalı virgülle ayrılmış).
    text: String,
    /// Eklenen/işaretlenen üyeler yönetici mi?
    #[serde(default)]
    admin: bool,
}

pub(super) async fn add_members(State(state): State<AppState>, user: AuthUser, Path((id, list_id)): Path<(i64, i64)>, Json(body): Json<MembersBody>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    ensure_installed()?;
    let (_, list) = authorized_list(&state, &user, id, list_id)?;
    let addresses = lists::parse_member_text(&body.text)?;
    lists::ensure_no_loop(&list.address, &addresses)?;
    let _guard = state.sys_lock.lock().await;
    let (added, existing) = lists::add_members(&state.db, list_id, &addresses, body.admin)?;
    mail::sync(&state.db).await?;
    auth::audit(&state.db, &user, "mail.list.members.add", &list.address, &format!("{added} yeni, {existing} zaten üye"));
    Ok(Json(json!({ "added": added, "existing": existing, "list": lists::load_list(&state.db, list_id)? })))
}

pub(super) async fn remove_members(State(state): State<AppState>, user: AuthUser, Path((id, list_id)): Path<(i64, i64)>, Json(body): Json<MembersBody>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    ensure_installed()?;
    let (_, list) = authorized_list(&state, &user, id, list_id)?;
    let addresses = lists::parse_member_text(&body.text)?;
    let _guard = state.sys_lock.lock().await;
    let removed = lists::remove_members(&state.db, list_id, &addresses)?;
    mail::sync(&state.db).await?;
    auth::audit(&state.db, &user, "mail.list.members.remove", &list.address, &format!("{removed} üye"));
    Ok(Json(json!({ "removed": removed, "list": lists::load_list(&state.db, list_id)? })))
}

/// Üyelerin yönetici işaretini değiştirir (`admin` true: yönetici yap, false: kaldır).
pub(super) async fn set_members_role(State(state): State<AppState>, user: AuthUser, Path((id, list_id)): Path<(i64, i64)>, Json(body): Json<MembersBody>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    ensure_installed()?;
    let (_, list) = authorized_list(&state, &user, id, list_id)?;
    let addresses = lists::parse_member_text(&body.text)?;
    let _guard = state.sys_lock.lock().await;
    let changed = lists::set_admin(&state.db, list_id, &addresses, body.admin)?;
    mail::sync(&state.db).await?;
    auth::audit(&state.db, &user, "mail.list.members.role", &list.address, &format!("{changed} üye, yönetici: {}", body.admin));
    Ok(Json(json!({ "changed": changed, "list": lists::load_list(&state.db, list_id)? })))
}
