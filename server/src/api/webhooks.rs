//! Olay kancaları (web kancaları) yönetimi: hesabın kendi adreslerini tanımlaması.
//!
//! Kancalar hesaba aittir; başkasının kancası `404` döner. Alt hesaplar kanca yönetemez.
//! Gizli anahtar yalnızca oluşturulurken ve yenilenirken bir kez gösterilir.

use axum::extract::{Path, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{self, AuthUser, Role};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::webhooks::{self, Delivery, Hook};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(index).post(create))
        .route("/{id}", put(update).delete(remove))
        .route("/{id}/secret", post(rotate_secret))
        .route("/{id}/test", post(test))
        .route("/{id}/deliveries", get(deliveries))
}

/// Alt hesaplar kanca yönetemez: kancalar hesabın bütün olaylarını dışarı taşır.
fn owner(user: &AuthUser) -> AppResult<i64> {
    if user.role == Role::Member {
        return Err(AppError::Forbidden);
    }
    Ok(user.id)
}

#[derive(Serialize)]
struct EventOption {
    id: &'static str,
    label: &'static str,
}

/// Gizli anahtarı içermeyen görünüm.
#[derive(Serialize)]
struct HookView {
    id: i64,
    url: String,
    events: Vec<String>,
    enabled: bool,
    created_at: i64,
    last_delivery: Option<Delivery>,
}

fn hook_view(state: &AppState, hook: Hook) -> AppResult<HookView> {
    let last_delivery = webhooks::deliveries(&state.db, hook.id)?.into_iter().next();
    Ok(HookView { id: hook.id, url: hook.url, events: hook.events, enabled: hook.enabled, created_at: hook.created_at, last_delivery })
}

#[derive(Serialize)]
struct Index {
    hooks: Vec<HookView>,
    available_events: Vec<EventOption>,
    max_hooks: i64,
    /// Yalnızca yönetici özel ağ adreslerine kanca tanımlayabilir.
    private_allowed: bool,
}

async fn index(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Index>> {
    let owner = owner(&user)?;
    let hooks = webhooks::list(&state.db, owner)?.into_iter().map(|hook| hook_view(&state, hook)).collect::<AppResult<Vec<_>>>()?;
    Ok(Json(Index {
        hooks,
        available_events: webhooks::EVENTS.iter().map(|(id, label)| EventOption { id, label }).collect(),
        max_hooks: webhooks::MAX_HOOKS,
        private_allowed: user.is_admin(),
    }))
}

#[derive(Deserialize)]
struct CreateBody {
    url: String,
    events: Vec<String>,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

async fn create(State(state): State<AppState>, user: AuthUser, Json(body): Json<CreateBody>) -> AppResult<Json<serde_json::Value>> {
    let owner = owner(&user)?;
    let events = webhooks::validate_events(&body.events)?;
    let target = webhooks::resolve_target(&body.url, user.is_admin()).await?;
    let secret = webhooks::new_secret();
    let hook = webhooks::create(&state.db, owner, target.url.as_str(), &events, &secret, body.enabled)?;
    auth::audit(&state.db, &user, "webhook.create", &webhooks::display_origin(&hook.url), "");
    // Gizli anahtar yalnızca burada, bir kez gösterilir.
    Ok(Json(json!({ "hook": hook_view(&state, hook)?, "secret": secret })))
}

#[derive(Deserialize)]
struct UpdateBody {
    url: String,
    events: Vec<String>,
    enabled: bool,
}

async fn update(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<UpdateBody>) -> AppResult<Json<HookView>> {
    let owner = owner(&user)?;
    let mut hook = webhooks::get(&state.db, owner, id)?;
    hook.events = webhooks::validate_events(&body.events)?;
    // Adres değiştiyse (ya da hâlâ geçerli mi diye) her kayıtta yeniden çözülüp doğrulanır.
    let target = webhooks::resolve_target(&body.url, user.is_admin()).await?;
    hook.url = target.url.to_string();
    hook.enabled = body.enabled;
    webhooks::update(&state.db, &hook)?;
    auth::audit(&state.db, &user, "webhook.update", &webhooks::display_origin(&hook.url), "");
    Ok(Json(hook_view(&state, hook)?))
}

async fn remove(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<serde_json::Value>> {
    let owner = owner(&user)?;
    let hook = webhooks::get(&state.db, owner, id)?;
    webhooks::delete(&state.db, owner, id)?;
    auth::audit(&state.db, &user, "webhook.delete", &webhooks::display_origin(&hook.url), "");
    Ok(Json(json!({ "ok": true })))
}

async fn rotate_secret(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<serde_json::Value>> {
    let owner = owner(&user)?;
    let mut hook = webhooks::get(&state.db, owner, id)?;
    hook.secret = webhooks::new_secret();
    webhooks::update(&state.db, &hook)?;
    auth::audit(&state.db, &user, "webhook.secret.rotate", &webhooks::display_origin(&hook.url), "");
    Ok(Json(json!({ "secret": hook.secret })))
}

/// Sınama gönderimi: tek deneme yapılır, sonuç hemen döner.
async fn test(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Delivery>> {
    let owner = owner(&user)?;
    let hook = webhooks::get(&state.db, owner, id)?;
    Ok(Json(webhooks::send_test(&state.db, &hook).await?))
}

async fn deliveries(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Vec<Delivery>>> {
    let owner = owner(&user)?;
    let hook = webhooks::get(&state.db, owner, id)?;
    Ok(Json(webhooks::deliveries(&state.db, hook.id)?))
}
