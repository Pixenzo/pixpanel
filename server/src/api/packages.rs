//! Hizmet paketleri ve hesap kullanımı.
//!
//! Yönetici her paketi, bayi yalnızca kendi tanımladıklarını yönetir. Bayinin
//! paketi, bayinin kendi paketinin sınırlarını aşamaz.

use axum::extract::{Path, State};
use axum::routing::{get, patch};
use axum::{Json, Router};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{self, AuthUser, Role};
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::hosting::resource_limits;
use crate::quota::{self, Limits, Package, Usage};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/usage", get(own_usage))
        .route("/limits-support", get(limits_support))
        .route("/{id}", patch(update).delete(remove))
}

/// Paket tanımlayabilenler: yönetici ve bayi.
fn require_manager(user: &AuthUser) -> AppResult<()> {
    if matches!(user.role, Role::Admin | Role::Reseller) { Ok(()) } else { Err(AppError::Forbidden) }
}

/// Bayinin tanımladığı paket kendi sınırlarını aşamaz.
fn check_ceiling(state: &AppState, user: &AuthUser, limits: &Limits) -> AppResult<()> {
    if user.is_admin() {
        return Ok(());
    }
    if let Some((_, own)) = quota::limits_for(&state.db, user.id)? {
        limits.within(&own).map_err(|field| AppError::bad(format!("Paketin {field} sınırı sizin paketinizin sınırını aşıyor")))?;
    }
    Ok(())
}

/// Paketi okur ve isteği yapanın onu yönetebildiğini denetler.
fn load_manageable(state: &AppState, user: &AuthUser, id: i64) -> AppResult<Package> {
    let package = quota::load_package(&state.db, id)?;
    if user.is_admin() || package.owner_id == user.id { Ok(package) } else { Err(AppError::NotFound) }
}

async fn list(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Vec<Package>>> {
    require_manager(&user)?;
    let owner = if user.is_admin() { None } else { Some(user.id) };
    Ok(Json(quota::list_packages(&state.db, owner)?))
}

/// Oturumdaki hesabın (alt hesapsa bağlı olduğu hesabın) paketi ve kullanımı.
async fn own_usage(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Usage>> {
    let mut usage = quota::usage(&state.db, user.owner_id())?;
    usage.runtime = resource_limits::runtime(&state.db, user.owner_id()).await;
    Ok(Json(usage))
}

/// Kaynak sınırlarının (işlemci, bellek, süreç) bu sunucuda uygulanıp uygulanamayacağı. Paket formu uyarı için kullanır.
async fn limits_support(user: AuthUser) -> AppResult<Json<Value>> {
    require_manager(&user)?;
    let warning = resource_limits::enforcement_problem();
    Ok(Json(json!({ "enforced": warning.is_none(), "warning": warning })))
}

#[derive(Deserialize)]
struct PackageBody {
    name: String,
    #[serde(default)]
    limits: Limits,
}

async fn create(State(state): State<AppState>, user: AuthUser, Json(body): Json<PackageBody>) -> AppResult<Json<Value>> {
    require_manager(&user)?;
    let name = quota::validate_name(&body.name)?;
    let wanted = body.limits.normalized();
    wanted.validate()?;
    check_ceiling(&state, &user, &wanted)?;
    let limits = serde_json::to_string(&wanted).map_err(|err| AppError::internal(err.to_string()))?;
    let inserted = state.db.call(|conn| {
        let taken: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM packages WHERE owner_id = ?1 AND name = ?2)", params![user.id, name], |row| row.get(0))?;
        if taken {
            return Ok(None);
        }
        conn.execute("INSERT INTO packages (owner_id, name, limits, created_at) VALUES (?1, ?2, ?3, ?4)", params![user.id, name, limits, now()])?;
        Ok(Some(conn.last_insert_rowid()))
    })?;
    let id = inserted.ok_or_else(|| AppError::conflict(format!("“{name}” adında bir paketiniz zaten var")))?;
    auth::audit(&state.db, &user, "package.create", &name, "");
    Ok(Json(json!({ "id": id })))
}

async fn update(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<PackageBody>) -> AppResult<Json<Value>> {
    require_manager(&user)?;
    let package = load_manageable(&state, &user, id)?;
    let name = quota::validate_name(&body.name)?;
    let wanted = body.limits.normalized();
    wanted.validate()?;
    check_ceiling(&state, &user, &wanted)?;
    let limits = serde_json::to_string(&wanted).map_err(|err| AppError::internal(err.to_string()))?;
    let renamed = state.db.call(|conn| {
        let taken: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM packages WHERE owner_id = ?1 AND name = ?2 AND id != ?3)",
            params![package.owner_id, name, id],
            |row| row.get(0),
        )?;
        if taken {
            return Ok(false);
        }
        conn.execute("UPDATE packages SET name = ?1, limits = ?2 WHERE id = ?3", params![name, limits, id])?;
        Ok(true)
    })?;
    if !renamed {
        return Err(AppError::conflict(format!("“{name}” adında başka bir paket var")));
    }
    // Paketi kullanan bütün hesapların işlemci/bellek sınırları yeni değerlere getirilir.
    resource_limits::sync_package(&state, id).await;
    // Trafik sınırı ya da davranışı değişmiş olabilir: askıya alınmış siteler yeni sınıra göre hemen yeniden değerlendirilir.
    let enforce_state = state.clone();
    tokio::spawn(async move { quota::traffic::enforce_all(&enforce_state).await });
    auth::audit(&state.db, &user, "package.update", &name, "");
    Ok(Json(json!({ "ok": true })))
}

async fn remove(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Value>> {
    require_manager(&user)?;
    let package = load_manageable(&state, &user, id)?;
    if package.accounts > 0 {
        return Err(AppError::conflict(format!("Bu paketi {} hesap kullanıyor; önce o hesaplara başka paket atayın", package.accounts)));
    }
    state.db.call(|conn| conn.execute("DELETE FROM packages WHERE id = ?1", params![id]))?;
    auth::audit(&state.db, &user, "package.delete", &package.name, "");
    Ok(Json(json!({ "ok": true })))
}
