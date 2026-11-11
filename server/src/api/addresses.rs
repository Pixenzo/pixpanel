//! Sunucunun IP adresleri: listeleme, ekleme, hesaba ayırma, kaldırma. Yalnızca yönetici.

use axum::extract::{Path, State};
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::addresses::{self, Address};
use crate::auth::{self, AuthUser};
use crate::error::AppResult;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(list).post(create)).route("/{address}", put(describe).delete(remove))
}

async fn list(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let items: Vec<Address> = addresses::list(&state.db).await?;
    // Yeni adres eklerken seçilebilecek arayüzler: üzerinde adres bulunanlar.
    let mut interfaces: Vec<String> = items.iter().filter(|item| item.present).map(|item| item.interface.clone()).collect();
    interfaces.sort();
    interfaces.dedup();
    Ok(Json(json!({ "addresses": items, "interfaces": interfaces, "dns_address": crate::dns::server_ip(&state.db) })))
}

#[derive(Deserialize)]
struct CreateBody {
    address: String,
    prefix: u8,
    interface: Option<String>,
    #[serde(default)]
    label: String,
}

async fn create(State(state): State<AppState>, user: AuthUser, Json(body): Json<CreateBody>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let _guard = state.sys_lock.lock().await;
    addresses::add(&state.db, &body.address, body.prefix, body.interface.as_deref(), &body.label).await?;
    auth::audit(&state.db, &user, "ip.add", body.address.trim(), &format!("/{}", body.prefix));
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct DescribeBody {
    #[serde(default)]
    label: String,
    #[serde(default)]
    public_address: String,
    user_id: Option<i64>,
}

async fn describe(State(state): State<AppState>, user: AuthUser, Path(address): Path<String>, Json(body): Json<DescribeBody>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let _guard = state.sys_lock.lock().await;
    // Dış adres değişirse bu adrese bağlı sitelerin DNS kayıtları da yeni adrese çevrilir.
    let key = addresses::normalize(&address)?.to_string();
    let old = crate::dns::site_address_record(&state.db, "", &key);
    addresses::describe(&state.db, &address, &body.label, &body.public_address, body.user_id).await?;
    if crate::dns::site_address_record(&state.db, "", &key).content != old.content {
        let bound: Vec<(i64, i64, String)> = state.db.call(|conn| {
            let mut stmt = conn.prepare("SELECT id, user_id, domain FROM sites WHERE ip = ?1")?;
            let rows = stmt.query_map(rusqlite::params![key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            rows.collect()
        })?;
        for (site_id, owner_id, domain) in bound {
            let mut names = vec![domain.clone()];
            names.extend(state.db.call(|conn| {
                let mut stmt = conn.prepare("SELECT name FROM site_aliases WHERE site_id = ?1")?;
                let rows = stmt.query_map(rusqlite::params![site_id], |row| row.get::<_, String>(0))?;
                rows.collect::<rusqlite::Result<Vec<String>>>()
            })?);
            for line in crate::dns::move_site_records(&state, owner_id, &names, &old).await {
                tracing::info!(domain = %domain, "DNS: {line}");
            }
        }
    }
    // Adres bir hesaba ayrıldıysa yalnızca onundur: başka hesapların o adrese bağlı siteleri
    // bütün adreslere döndürülür (iki müşteri aynı "özel" adresi paylaşmasın).
    if let Some(owner) = body.user_id {
        let others: Vec<i64> = state.db.call(|conn| {
            let mut stmt = conn.prepare("SELECT id FROM sites WHERE ip = ?1 AND user_id != ?2")?;
            let rows = stmt.query_map(rusqlite::params![key, owner], |row| row.get(0))?;
            rows.collect()
        })?;
        for site_id in others {
            let Some(mut site) = crate::hosting::sites::load(&state.db, site_id)? else { continue };
            let old = crate::dns::site_address_record(&state.db, &site.domain, &site.ip);
            site.ip = String::new();
            crate::hosting::sites::save(&state.db, &site)?;
            if let Err(err) = crate::hosting::sites::provision(&site).await {
                tracing::warn!(domain = %site.domain, error = %err, "adresi alınan site yeniden yapılandırılamadı");
            }
            let mut names = vec![site.domain.clone()];
            names.extend(site.aliases.iter().cloned());
            for line in crate::dns::move_site_records(&state, site.user_id, &names, &old).await {
                tracing::info!(domain = %site.domain, "DNS: {line}");
            }
        }
    }
    let detail = body.user_id.map(|id| format!("hesap {id}")).unwrap_or_default();
    auth::audit(&state.db, &user, "ip.update", address.trim(), &detail);
    Ok(Json(json!({ "ok": true })))
}

async fn remove(State(state): State<AppState>, user: AuthUser, Path(address): Path<String>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let _guard = state.sys_lock.lock().await;
    addresses::remove(&state.db, &address).await?;
    auth::audit(&state.db, &user, "ip.remove", address.trim(), "");
    Ok(Json(json!({ "ok": true })))
}
