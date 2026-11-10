//! Site sertifikası uçları: kendinden imzalı, yüklenen ve Let's Encrypt.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::hosting::sites::{self, Site};
use crate::hosting::ssl::{self, CertInfo};
use crate::hosting::{acme, nginx};
use crate::state::AppState;

const AREA: &str = "ssl";
/// Sertifika ve anahtar metinleri için üst sınır.
const MAX_PEM_BYTES: usize = 128 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{id}/ssl", get(status).delete(disable))
        .route("/{id}/ssl/self-signed", post(self_signed))
        .route("/{id}/ssl/custom", post(custom))
        .route("/{id}/ssl/letsencrypt", post(letsencrypt))
}

#[derive(Serialize)]
struct SslStatus {
    mode: String,
    force_https: bool,
    hsts: bool,
    certificate: Option<CertInfo>,
}

fn status_of(site: &Site) -> SslStatus {
    SslStatus {
        mode: site.ssl_mode.clone(),
        force_https: site.force_https,
        hsts: site.hsts,
        certificate: if site.ssl_mode == "none" { None } else { ssl::read_info(&site.domain) },
    }
}

async fn status(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<SslStatus>> {
    user.require(AREA)?;
    Ok(Json(status_of(&sites::get(&state.db, &user, id)?)))
}

fn site_names(site: &Site) -> Vec<String> {
    let mut names = vec![site.domain.clone()];
    names.extend(site.aliases.iter().cloned());
    names
}

async fn self_signed(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<SslStatus>> {
    user.require(AREA)?;
    let _guard = state.sys_lock.lock().await;
    let site = sites::get(&state.db, &user, id)?;
    let (certificate, private_key) = ssl::generate_self_signed(&site_names(&site))?;
    let updated = sites::activate_ssl(&state.db, &site, "selfsigned", &certificate, &private_key).await?;
    auth::audit(&state.db, &user, "ssl.self_signed", &site.domain, "");
    Ok(Json(status_of(&updated)))
}

#[derive(Deserialize)]
struct CustomBody {
    /// Sertifika ve varsa ara sertifikalar (PEM).
    certificate: String,
    private_key: String,
}

async fn custom(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<CustomBody>,
) -> AppResult<Json<SslStatus>> {
    user.require(AREA)?;
    if body.certificate.len() > MAX_PEM_BYTES || body.private_key.len() > MAX_PEM_BYTES {
        return Err(AppError::bad("Sertifika ya da anahtar çok büyük"));
    }
    let _guard = state.sys_lock.lock().await;
    let site = sites::get(&state.db, &user, id)?;
    ssl::validate_custom(&site.domain, &body.certificate, &body.private_key)?;
    let certificate = format!("{}\n", body.certificate.trim());
    let private_key = format!("{}\n", body.private_key.trim());
    let updated = sites::activate_ssl(&state.db, &site, "custom", &certificate, &private_key)
        .await
        .map_err(|err| match err {
            // nginx'in en sık reddettiği durum anahtarın sertifikaya ait olmamasıdır.
            AppError::BadRequest(detail) if detail.contains("key values mismatch") => {
                AppError::bad("Özel anahtar bu sertifikaya ait değil")
            }
            other => other,
        })?;
    auth::audit(&state.db, &user, "ssl.custom", &site.domain, "");
    Ok(Json(status_of(&updated)))
}

#[derive(Deserialize)]
struct LetsEncryptBody {
    /// Takma adlar da sertifikaya eklensin mi?
    #[serde(default = "default_true")]
    include_aliases: bool,
    /// Doğrulama DNS üzerinden yapılsın mı? (Alan adının DNS'i panelde yönetiliyor olmalı.)
    #[serde(default)]
    dns: bool,
    /// Joker sertifika (`*.alan`); her zaman DNS ile doğrulanır.
    #[serde(default)]
    wildcard: bool,
}

fn default_true() -> bool {
    true
}

async fn letsencrypt(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<LetsEncryptBody>,
) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    if !nginx::is_installed() {
        return Err(AppError::bad("nginx kurulu değil"));
    }
    let mode = if body.wildcard {
        "letsencrypt_wildcard"
    } else if body.dns {
        "letsencrypt_dns"
    } else {
        "letsencrypt"
    };
    if mode != "letsencrypt" && crate::dns::zone_for(&state.db, site.user_id, &site.domain)?.is_none() {
        return Err(AppError::bad("DNS ile doğrulama için alan adının DNS'i panelde yönetiliyor olmalı (DNS sayfasından bölgeyi ekleyin ya da Cloudflare'e bağlayın)"));
    }
    let names = issue_names(&site, mode, body.include_aliases);
    let job_id = start_issue_job(&state, Some(user.id), site.id, &site.domain, names, mode)?;
    auth::audit(&state.db, &user, "ssl.letsencrypt", &site.domain, "");
    Ok(Json(json!({ "job_id": job_id })))
}

/// Sertifikaya yazılacak adlar: joker kipinde alan adı ve `*.alan adı`.
pub fn issue_names(site: &Site, mode: &str, include_aliases: bool) -> Vec<String> {
    if mode == "letsencrypt_wildcard" {
        return vec![site.domain.clone(), format!("*.{}", site.domain)];
    }
    if include_aliases { site_names(site) } else { vec![site.domain.clone()] }
}

/// Sertifika alma görevini başlatır. Otomatik yenileme de bunu kullanır.
/// `mode`: `letsencrypt` (HTTP), `letsencrypt_dns` ya da `letsencrypt_wildcard`.
pub fn start_issue_job(
    state: &AppState,
    user_id: Option<i64>,
    site_id: i64,
    domain: &str,
    names: Vec<String>,
    mode: &'static str,
) -> AppResult<i64> {
    let title = format!("{domain} için Let's Encrypt sertifikası");
    let state = state.clone();
    let jobs = state.jobs.clone();
    let domain = domain.to_string();
    jobs.start("ssl.letsencrypt", &title, user_id, move |ctx| async move {
        let outcome = async {
            let solver = match mode {
                "letsencrypt" => None,
                _ => {
                    let owner = sites::load(&state.db, site_id)?.ok_or_else(|| AppError::bad("Site bu sırada silinmiş"))?.user_id;
                    Some(crate::dns::AcmeSolver::new(state.clone(), owner))
                }
            };
            let (chain, private_key) = acme::issue(&ctx, &state.db, &state.cfg, &names, solver).await?;
            // Sertifika alınırken site değişmiş ya da silinmiş olabilir; güncel kayıt okunur.
            let _guard = state.sys_lock.lock().await;
            let site = sites::load(&state.db, site_id)?.ok_or_else(|| AppError::bad("Site bu sırada silinmiş"))?;
            sites::activate_ssl(&state.db, &site, mode, &chain, &private_key).await?;
            ctx.log("Sertifika kuruldu; site artık HTTPS ile açılıyor");
            Ok(())
        }
        .await;
        // Kullanıcının başlattığı istekte hata zaten ekranda görünür; yalnızca
        // kendiliğinden yenileme başarısız olursa site sahibine haber verilir.
        if let (Err(err), None) = (&outcome, user_id) {
            if let Ok(Some(site)) = sites::load(&state.db, site_id) {
                let body = format!("Site: {domain}
Hata: {err}

Sertifikanın süresi dolmadan önce panel yeniden deneyecek.");
                crate::notify::send_event(&state, site.user_id, "ssl_failed", &format!("{domain} sertifikası yenilenemedi"), &body).await;
            }
        }
        outcome
    })
}

async fn disable(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<SslStatus>> {
    user.require(AREA)?;
    let _guard = state.sys_lock.lock().await;
    let site = sites::get(&state.db, &user, id)?;
    let mut updated = site.clone();
    updated.ssl_mode = "none".into();
    updated.force_https = false;
    updated.hsts = false;
    sites::save(&state.db, &updated)?;
    // Önce nginx sertifikayı bırakır, dosyalar ondan sonra silinir.
    if let Err(err) = sites::provision(&updated).await {
        sites::save(&state.db, &site)?;
        return Err(err);
    }
    ssl::remove(&site.domain)?;
    auth::audit(&state.db, &user, "ssl.disable", &site.domain, "");
    Ok(Json(status_of(&updated)))
}
