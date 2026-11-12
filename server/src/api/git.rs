//! Git ile yayınlama uçları ve depo sağlayıcılarının çağırdığı web kancası.

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{self, AuthUser};
use crate::deploy::{self, Deployment, GitConfig, Started};
use crate::error::{AppError, AppResult};
use crate::hosting::sites;
use crate::state::AppState;

const AREA: &str = "sites";
/// Depo sağlayıcılarının gönderdiği gövde için üst sınır (GitHub en fazla 25 MB yollar).
const MAX_WEBHOOK_BYTES: usize = 26 * 1024 * 1024;

/// `/sites` altına eklenen uçlar.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{id}/git", get(show).put(save).delete(remove))
        .route("/{id}/git/deploy", post(deploy_now))
        .route("/{id}/git/webhook-token", post(rotate_token))
        .route("/{id}/git/deployments/{deployment}", get(deployment_detail))
}

/// Kimlik doğrulaması istemeyen web kancası; adresteki anahtar yetkidir.
pub fn hooks_router() -> Router<AppState> {
    Router::new().route("/deploy/{token}", post(webhook))
}

#[derive(Serialize)]
struct View {
    config: Option<GitConfig>,
    /// Depo sağlayıcısına tanımlanacak adresin yolu; arayüz başına panel adresini ekler.
    webhook_path: Option<String>,
    deployments: Vec<Deployment>,
    running: bool,
    /// İmza gizli anahtarının kendisi geri gönderilmez; yalnızca tanımlı olup olmadığı.
    signature_secret_set: bool,
}

fn view(state: &AppState, site_id: i64) -> AppResult<View> {
    let config = deploy::load_config(&state.db, site_id)?;
    let signature_secret_set = config.as_ref().is_some_and(|config| !config.signature_secret.is_empty());
    Ok(View {
        webhook_path: config.as_ref().map(|config| format!("/api/v1/hooks/deploy/{}", config.webhook_token)),
        config,
        deployments: deploy::list_deployments(&state.db, site_id, 30)?,
        running: deploy::is_running(site_id),
        signature_secret_set,
    })
}

async fn show(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<View>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    Ok(Json(view(&state, site.id)?))
}

#[derive(Deserialize)]
struct SaveBody {
    repo_url: String,
    branch: String,
    #[serde(default = "default_true")]
    auto_deploy: bool,
    #[serde(default)]
    script: String,
    /// Verilmezse kayıtlı imza gizli anahtarı korunur; boş metin anahtarı kaldırır.
    #[serde(default)]
    signature_secret: Option<String>,
}

fn default_true() -> bool {
    true
}

async fn save(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<SaveBody>,
) -> AppResult<Json<View>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    let repo_url = deploy::validate_repo_url(&body.repo_url)?;
    let branch = deploy::validate_branch(&body.branch)?;
    let script = deploy::validate_script(&body.script)?;

    let existing = deploy::load_config(&state.db, site.id)?;
    let previous_secret = existing.as_ref().map(|c| c.signature_secret.clone()).unwrap_or_default();
    let signature_secret = match body.signature_secret.as_deref().map(str::trim) {
        None => previous_secret.clone(),
        Some("") => String::new(),
        Some(value) => deploy::validate_signature_secret(value)?,
    };
    let public_key = deploy::ensure_deploy_key(&site).await?;
    let config = GitConfig {
        site_id: site.id,
        repo_url,
        branch,
        auto_deploy: body.auto_deploy,
        script,
        webhook_token: existing.as_ref().map(|c| c.webhook_token.clone()).unwrap_or_else(deploy::new_webhook_token),
        signature_secret: signature_secret.clone(),
        public_key,
        created_at: existing.map(|c| c.created_at).unwrap_or_else(crate::db::now),
    };
    deploy::save_config(&state.db, &config)?;
    auth::audit(&state.db, &user, "git.update", &site.domain, &config.repo_url);
    if signature_secret != previous_secret {
        let action = if signature_secret.is_empty() { "git.signature.clear" } else { "git.signature.set" };
        auth::audit(&state.db, &user, action, &site.domain, "");
    }
    Ok(Json(view(&state, site.id)?))
}

async fn remove(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<View>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    if deploy::is_running(site.id) {
        return Err(AppError::conflict("Süren bir yayın var; bitmesini bekleyin"));
    }
    deploy::delete_config(&state.db, site.id)?;
    deploy::remove_deploy_key(&site).await;
    auth::audit(&state.db, &user, "git.remove", &site.domain, "");
    Ok(Json(view(&state, site.id)?))
}

async fn rotate_token(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<View>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    let mut config = deploy::load_config(&state.db, site.id)?.ok_or(AppError::NotFound)?;
    config.webhook_token = deploy::new_webhook_token();
    deploy::save_config(&state.db, &config)?;
    auth::audit(&state.db, &user, "git.webhook.rotate", &site.domain, "");
    Ok(Json(view(&state, site.id)?))
}

#[derive(Deserialize)]
struct DeployBody {
    /// Verilirse dalın ucu yerine bu sürüm yayınlanır (eski bir sürüme dönmek için).
    commit: Option<String>,
}

async fn deploy_now(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<DeployBody>,
) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    let commit = body.commit.as_deref().map(deploy::validate_commit).transpose()?;
    let source = if commit.is_some() { "rollback" } else { "manual" };
    let started = deploy::start(&state, &site, source, commit, user.id)?;
    auth::audit(&state.db, &user, "git.deploy", &site.domain, source);
    Ok(Json(match started {
        Started::Now(deployment) => json!({ "deployment": deployment }),
        Started::Queued => json!({ "queued": true }),
    }))
}

async fn deployment_detail(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, deployment)): Path<(i64, i64)>,
) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    let deployment = deploy::get_deployment(&state.db, site.id, deployment)?;
    let log = match deployment.job_id {
        Some(job_id) => state.jobs.get(job_id).map(|(_, log)| log).unwrap_or_default(),
        None => String::new(),
    };
    Ok(Json(json!({ "deployment": deployment, "log": log })))
}

/// Gönderim bildiriminin yalnızca ilgilendiğimiz alanı.
#[derive(Deserialize)]
struct PushPayload {
    #[serde(rename = "ref")]
    git_ref: Option<String>,
}

/// Web kancasının reddedilme nedenleri; imza hatası ayrı bir iletiyle 401 döner.
enum Reject {
    BadSignature,
    App(AppError),
}

impl From<AppError> for Reject {
    fn from(err: AppError) -> Self {
        Self::App(err)
    }
}

impl IntoResponse for Reject {
    fn into_response(self) -> Response {
        match self {
            Self::App(err) => err.into_response(),
            Self::BadSignature => (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": { "code": "invalid_signature", "message": "İmza doğrulanamadı: gizli anahtar eşleşmiyor ya da imza başlığı yok" } })),
            )
                .into_response(),
        }
    }
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

async fn read_body(request: Request) -> AppResult<axum::body::Bytes> {
    axum::body::to_bytes(request.into_body(), MAX_WEBHOOK_BYTES)
        .await
        .map_err(|_| AppError::bad("İstek gövdesi okunamadı ya da çok büyük"))
}

async fn webhook(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    request: Request,
) -> Result<Json<serde_json::Value>, Reject> {
    // Anahtar gövde okunmadan önce denetlenir; tanımayan istekler için bellek harcanmaz.
    let token_ok = (16..=128).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !token_ok {
        return Err(AppError::NotFound.into());
    }
    let config = deploy::config_by_token(&state.db, &token)?.ok_or(AppError::NotFound)?;
    let site = sites::load(&state.db, config.site_id)?.ok_or(AppError::NotFound)?;

    // İmza gizli anahtarı tanımlıysa her istek (GitHub'ın "ping"i dahil) imzalı olmalıdır;
    // imza gövdeye bağlı olduğundan gövde baştan okunur. Tanımlı değilse eski davranış sürer.
    let mut request = Some(request);
    let mut body = None;
    if !config.signature_secret.is_empty() {
        let bytes = read_body(request.take().ok_or(AppError::NotFound)?).await?;
        let verified = deploy::verify_hook_signature(
            &config.signature_secret,
            header_text(&headers, "x-hub-signature-256"),
            header_text(&headers, "x-gitlab-token"),
            &bytes,
        );
        if !verified {
            tracing::warn!(site_id = site.id, "web kancası imzası doğrulanamadı");
            return Err(Reject::BadSignature);
        }
        body = Some(bytes);
    }

    // GitHub kancayı kaydederken bir "ping" gönderir; bu bir gönderim değildir.
    if header_text(&headers, "x-github-event") == Some("ping") {
        return Ok(Json(json!({ "ok": true, "result": "pong" })));
    }
    if !config.auto_deploy {
        return Ok(Json(json!({ "ok": true, "result": "skipped", "reason": "Otomatik yayınlama kapalı" })));
    }

    let body = match body {
        Some(bytes) => bytes,
        None => read_body(request.take().ok_or(AppError::NotFound)?).await?,
    };
    // Gövde JSON değilse (ya da dal bilgisi taşımıyorsa) her çağrı yayın başlatır.
    let pushed_ref = serde_json::from_slice::<PushPayload>(&body).ok().and_then(|payload| payload.git_ref);
    if let Some(pushed) = &pushed_ref {
        if pushed != &format!("refs/heads/{}", config.branch) {
            return Ok(Json(json!({ "ok": true, "result": "skipped", "reason": "Gönderim başka bir dala yapıldı" })));
        }
    }

    let result = match deploy::start(&state, &site, "webhook", None, site.user_id)? {
        Started::Now(deployment) => json!({ "ok": true, "result": "started", "deployment": deployment }),
        Started::Queued => json!({ "ok": true, "result": "queued" }),
    };
    Ok(Json(result))
}
