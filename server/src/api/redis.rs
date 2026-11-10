//! Redis yönetimi: durum, ayarlar, anahtar tarayıcı. Yalnızca yöneticiler.
//!
//! Redis sunucudaki bütün uygulamalarca paylaşılır ve hesaplar arası yalıtımı
//! yoktur; bu yüzden içeriğine yalnızca yönetici bakabilir.

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json_};

use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::sys::redis::{Client, Value};
use crate::sys::{exec, services};

const POLICIES: &[&str] =
    &["noeviction", "allkeys-lru", "allkeys-lfu", "allkeys-random", "volatile-lru", "volatile-lfu", "volatile-random", "volatile-ttl"];
const SCAN_COUNT: &str = "300";
const MAX_KEYS_PER_PAGE: usize = 200;
const PREVIEW_ITEMS: &str = "199";
const PREVIEW_BYTES: usize = 64 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(overview))
        .route("/config", post(save_config))
        .route("/keys", get(list_keys))
        .route("/key", get(show_key).delete(delete_key))
        .route("/flush", post(flush))
}

fn parse_info(raw: &str) -> HashMap<String, String> {
    raw.lines().filter(|line| !line.starts_with('#')).filter_map(|line| line.split_once(':')).map(|(k, v)| (k.to_string(), v.trim().to_string())).collect()
}

async fn config_get(client: &mut Client, key: &str) -> AppResult<String> {
    Ok(client.run(&["CONFIG", "GET", key]).await?.items().get(1).map(Value::text).unwrap_or_default())
}

#[derive(Serialize)]
struct KeyspaceEntry {
    db: i64,
    keys: i64,
    expires: i64,
}

async fn overview(user: AuthUser) -> AppResult<Json<Json_>> {
    user.require_admin()?;
    if !exec::binary_exists("redis-server") {
        return Ok(Json(json!({ "installed": false, "running": false })));
    }
    if !services::is_active("redis-server.service").await {
        return Ok(Json(json!({ "installed": true, "running": false })));
    }
    let mut client = Client::connect().await?;
    let info = parse_info(&client.run(&["INFO"]).await?.text());
    let number = |key: &str| info.get(key).and_then(|value| value.parse::<f64>().ok()).unwrap_or(0.0);
    let text = |key: &str| info.get(key).cloned().unwrap_or_default();
    // Anahtar alanı satırları `db0:keys=12,expires=3,avg_ttl=0` biçimindedir.
    let mut keyspace: Vec<KeyspaceEntry> = info
        .iter()
        .filter_map(|(key, value)| {
            let db = key.strip_prefix("db")?.parse().ok()?;
            let field = |name: &str| value.split(',').find_map(|part| part.strip_prefix(name)?.strip_prefix('=')?.parse().ok()).unwrap_or(0);
            Some(KeyspaceEntry { db, keys: field("keys"), expires: field("expires") })
        })
        .collect();
    keyspace.sort_by_key(|entry| entry.db);
    let (hits, misses) = (number("keyspace_hits"), number("keyspace_misses"));
    let maxmemory: f64 = config_get(&mut client, "maxmemory").await?.parse().unwrap_or(0.0);

    Ok(Json(json!({
        "installed": true,
        "running": true,
        "version": text("redis_version"),
        "uptime_secs": number("uptime_in_seconds"),
        "used_memory": number("used_memory"),
        "used_memory_peak": number("used_memory_peak"),
        "used_memory_rss": number("used_memory_rss"),
        "connected_clients": number("connected_clients"),
        "ops_per_sec": number("instantaneous_ops_per_sec"),
        "total_commands": number("total_commands_processed"),
        "hit_rate": if hits + misses > 0.0 { Some(hits / (hits + misses) * 100.0) } else { None },
        "evicted_keys": number("evicted_keys"),
        "expired_keys": number("expired_keys"),
        "last_save_at": number("rdb_last_save_time"),
        "keyspace": keyspace,
        "databases": config_get(&mut client, "databases").await?.parse::<i64>().unwrap_or(16),
        "config": {
            "maxmemory_mb": (maxmemory / 1024.0 / 1024.0).round(),
            "policy": config_get(&mut client, "maxmemory-policy").await?,
            "password_set": !config_get(&mut client, "requirepass").await?.is_empty(),
            "appendonly": config_get(&mut client, "appendonly").await? == "yes",
            "snapshots": !config_get(&mut client, "save").await?.is_empty(),
        },
        "policies": POLICIES,
    })))
}

#[derive(Deserialize)]
struct ConfigBody {
    /// 0 sınırsız demektir.
    maxmemory_mb: u64,
    policy: String,
    appendonly: bool,
    /// Verilmezse parola değişmez; boş dize parolayı kaldırır.
    password: Option<String>,
}

async fn save_config(State(state): State<AppState>, user: AuthUser, Json(body): Json<ConfigBody>) -> AppResult<Json<Json_>> {
    user.require_admin()?;
    if !POLICIES.contains(&body.policy.as_str()) {
        return Err(AppError::bad("Geçersiz bellek dolunca davranışı"));
    }
    if body.maxmemory_mb > 1_048_576 {
        return Err(AppError::bad("Bellek sınırı çok büyük"));
    }
    if let Some(password) = body.password.as_deref().filter(|password| !password.is_empty()) {
        // Parola yapılandırma dosyasına yazıldığından boşluk ve tırnak içeremez.
        let valid = (16..=128).contains(&password.len()) && password.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-+=.@:!".contains(&b));
        if !valid {
            return Err(AppError::bad("Redis parolası en az 16 karakter olmalı; harf, rakam ve _-+=.@:! kullanılabilir"));
        }
    }
    let mut client = Client::connect().await?;
    let bytes = (body.maxmemory_mb * 1024 * 1024).to_string();
    client.run(&["CONFIG", "SET", "maxmemory", &bytes]).await?;
    client.run(&["CONFIG", "SET", "maxmemory-policy", &body.policy]).await?;
    client.run(&["CONFIG", "SET", "appendonly", if body.appendonly { "yes" } else { "no" }]).await?;
    if let Some(password) = &body.password {
        client.run(&["CONFIG", "SET", "requirepass", password]).await?;
    }
    // Değişiklikler yapılandırma dosyasına da yazılır; yeniden başlatmada kaybolmaz.
    client.run(&["CONFIG", "REWRITE"]).await?;
    auth::audit(&state.db, &user, "redis.config", "", if body.password.is_some() { "parola değişti" } else { "" });
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct KeysQuery {
    #[serde(default)]
    db: u32,
    #[serde(default)]
    pattern: String,
    #[serde(default)]
    cursor: String,
}

fn validate_db(db: u32) -> AppResult<String> {
    if db < 1024 { Ok(db.to_string()) } else { Err(AppError::bad("Geçersiz veritabanı numarası")) }
}

async fn list_keys(user: AuthUser, Query(query): Query<KeysQuery>) -> AppResult<Json<Json_>> {
    user.require_admin()?;
    let cursor = if query.cursor.is_empty() { "0".to_string() } else { query.cursor };
    if !cursor.bytes().all(|b| b.is_ascii_digit()) || cursor.len() > 20 {
        return Err(AppError::bad("Geçersiz imleç"));
    }
    if query.pattern.len() > 200 {
        return Err(AppError::bad("Arama kalıbı çok uzun"));
    }
    let pattern = if query.pattern.is_empty() { "*".to_string() } else { query.pattern };
    let mut client = Client::connect().await?;
    client.run(&["SELECT", &validate_db(query.db)?]).await?;
    let reply = client.run(&["SCAN", &cursor, "MATCH", &pattern, "COUNT", SCAN_COUNT]).await?.items();
    let next = reply.first().map(Value::text).unwrap_or_else(|| "0".into());
    let mut keys = Vec::new();
    for key in reply.get(1).map(Value::items).unwrap_or_default().into_iter().take(MAX_KEYS_PER_PAGE) {
        let Value::Bulk(Some(raw)) = &key else { continue };
        let kind = client.command(&[b"TYPE", raw]).await?.text();
        let ttl = client.command(&[b"TTL", raw]).await?.int();
        let size = client.command(&[b"MEMORY", b"USAGE", raw]).await.map(|value| value.int()).unwrap_or(0);
        keys.push(json!({ "key": key.text(), "type": kind, "ttl": ttl, "size": size }));
    }
    Ok(Json(json!({ "cursor": next, "keys": keys })))
}

#[derive(Deserialize)]
struct KeyQuery {
    #[serde(default)]
    db: u32,
    key: String,
}

/// Uzun değerler panelde kırpılarak gösterilir.
fn preview(value: &Value) -> Json_ {
    match value {
        Value::Bulk(Some(bytes)) => {
            let cut = bytes.len().min(PREVIEW_BYTES);
            json!({ "text": String::from_utf8_lossy(&bytes[..cut]), "truncated": bytes.len() > cut, "bytes": bytes.len() })
        }
        other => json!({ "text": other.text(), "truncated": false, "bytes": other.text().len() }),
    }
}

async fn show_key(user: AuthUser, Query(query): Query<KeyQuery>) -> AppResult<Json<Json_>> {
    user.require_admin()?;
    let mut client = Client::connect().await?;
    client.run(&["SELECT", &validate_db(query.db)?]).await?;
    let key = query.key.as_str();
    let kind = client.run(&["TYPE", key]).await?.text();
    let ttl = client.run(&["TTL", key]).await?.int();
    let texts = |value: Value| value.items().iter().map(Value::text).collect::<Vec<_>>();
    let (length, content) = match kind.as_str() {
        "none" => return Err(AppError::NotFound),
        "string" => (client.run(&["STRLEN", key]).await?.int(), json!({ "value": preview(&client.run(&["GET", key]).await?) })),
        "list" => (client.run(&["LLEN", key]).await?.int(), json!({ "items": texts(client.run(&["LRANGE", key, "0", PREVIEW_ITEMS]).await?) })),
        "set" => {
            let scan = client.run(&["SSCAN", key, "0", "COUNT", "200"]).await?.items();
            (client.run(&["SCARD", key]).await?.int(), json!({ "items": scan.get(1).cloned().map(texts).unwrap_or_default() }))
        }
        "zset" => {
            let flat = texts(client.run(&["ZRANGE", key, "0", PREVIEW_ITEMS, "WITHSCORES"]).await?);
            let pairs: Vec<Json_> = flat.chunks(2).map(|pair| json!({ "member": pair[0], "score": pair.get(1) })).collect();
            (client.run(&["ZCARD", key]).await?.int(), json!({ "pairs": pairs }))
        }
        "hash" => {
            let scan = client.run(&["HSCAN", key, "0", "COUNT", "200"]).await?.items();
            let flat = scan.get(1).cloned().map(texts).unwrap_or_default();
            let fields: Vec<Json_> = flat.chunks(2).map(|pair| json!({ "field": pair[0], "value": pair.get(1) })).collect();
            (client.run(&["HLEN", key]).await?.int(), json!({ "fields": fields }))
        }
        "stream" => (client.run(&["XLEN", key]).await?.int(), json!({})),
        _ => (0, json!({})),
    };
    Ok(Json(json!({ "key": key, "type": kind, "ttl": ttl, "length": length, "content": content })))
}

async fn delete_key(State(state): State<AppState>, user: AuthUser, Query(query): Query<KeyQuery>) -> AppResult<Json<Json_>> {
    user.require_admin()?;
    let mut client = Client::connect().await?;
    client.run(&["SELECT", &validate_db(query.db)?]).await?;
    let removed = client.run(&["DEL", &query.key]).await?.int();
    auth::audit(&state.db, &user, "redis.key.delete", &query.key.chars().take(120).collect::<String>(), "");
    Ok(Json(json!({ "removed": removed })))
}

#[derive(Deserialize)]
struct FlushBody {
    /// Verilirse yalnızca o veritabanı, verilmezse hepsi boşaltılır.
    db: Option<u32>,
}

async fn flush(State(state): State<AppState>, user: AuthUser, Json(body): Json<FlushBody>) -> AppResult<Json<Json_>> {
    user.require_admin()?;
    let mut client = Client::connect().await?;
    match body.db {
        Some(db) => {
            client.run(&["SELECT", &validate_db(db)?]).await?;
            client.run(&["FLUSHDB"]).await?;
        }
        None => {
            client.run(&["FLUSHALL"]).await?;
        }
    }
    auth::audit(&state.db, &user, "redis.flush", &body.db.map(|db| format!("db{db}")).unwrap_or_else(|| "tümü".into()), "");
    Ok(Json(json!({ "ok": true })))
}
