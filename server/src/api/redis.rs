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
use crate::hosting::redis_account::{self, Instance};
use crate::state::AppState;
use crate::sys::redis::{Client, Value};
use crate::sys::{exec, services};

const POLICIES: &[&str] =
    &["noeviction", "allkeys-lru", "allkeys-lfu", "allkeys-random", "volatile-lru", "volatile-lfu", "volatile-random", "volatile-ttl"];
const KEY_TYPES: &[&str] = &["string", "hash", "list", "set", "zset", "stream"];
const SCAN_COUNT: &str = "500";
/// Bir istekte dönen anahtar sayısı için hedef ve SCAN yineleme üst sınırı.
const KEYS_PER_PAGE: usize = 200;
const MAX_SCAN_ROUNDS: usize = 40;
/// Koleksiyonlarda (list, zset) sayfa başına öğe.
const PAGE_ITEMS: i64 = 200;
const STRING_PREVIEW_BYTES: usize = 1024 * 1024;
const MAX_VALUE_BYTES: usize = 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(overview))
        .route("/config", post(save_config))
        .route("/keys", get(list_keys))
        .route("/key", get(show_key).post(edit_key).delete(delete_key))
        .route("/command", post(run_command))
        .route("/flush", post(flush))
        .route("/mine", get(mine))
        .route("/mine/enable", post(mine_enable))
        .route("/mine/disable", post(mine_disable))
        .route("/mine/password", post(mine_password))
        .route("/accounts", get(accounts))
        .route("/accounts/{user_id}", axum::routing::put(account_memory).delete(account_close))
}

fn parse_info(raw: &str) -> HashMap<String, String> {
    raw.lines().filter(|line| !line.starts_with('#')).filter_map(|line| line.split_once(':')).map(|(k, v)| (k.to_string(), v.trim().to_string())).collect()
}

/// Alt hesapların Redis'e erişimi veritabanı yetkisine bağlıdır.
const ACCOUNT_AREA: &str = "databases";

/// Anahtar tarayıcısının ve konsolun bağlanacağı sunucu: yönetici ortak Redis'e,
/// diğer hesaplar yalnızca kendi örneklerine bağlanır.
async fn client_for(state: &AppState, user: &AuthUser) -> AppResult<Client> {
    if user.is_admin() {
        return Client::connect().await;
    }
    user.require(ACCOUNT_AREA)?;
    let instance = redis_account::get(&state.db, user.owner_id())?.ok_or_else(|| AppError::bad("Bu hesap için Redis açık değil"))?;
    redis_account::connect(&instance).await
}

// ---- Hesabın kendi örneği ----

/// Örneğin canlı durumu; bağlanılamazsa yalnızca kayıtlı bilgiler döner.
async fn instance_json(instance: &Instance, with_secret: bool) -> Json_ {
    let running = redis_account::is_running(instance).await;
    let mut view = json!({
        "user_id": instance.user_id,
        "username": instance.username,
        "running": running,
        "host": "127.0.0.1",
        "port": instance.port,
        "socket": instance.socket(),
        "memory_mb": instance.memory_mb,
        "created_at": instance.created_at,
        "used_memory": Json_::Null,
        "keys": Json_::Null,
        "version": "",
    });
    if with_secret {
        view["password"] = json!(instance.password);
    }
    if running {
        if let Ok(mut client) = redis_account::connect(instance).await {
            if let Ok(reply) = client.run(&["INFO"]).await {
                let info = parse_info(&reply.text());
                let keys: i64 = info
                    .iter()
                    .filter(|(key, _)| key.strip_prefix("db").is_some_and(|rest| rest.bytes().all(|byte| byte.is_ascii_digit()) && !rest.is_empty()))
                    .filter_map(|(_, value)| value.split(',').find_map(|part| part.strip_prefix("keys=")?.parse::<i64>().ok()))
                    .sum();
                view["used_memory"] = json!(info.get("used_memory").and_then(|value| value.parse::<u64>().ok()));
                view["keys"] = json!(keys);
                view["version"] = json!(info.get("redis_version").cloned().unwrap_or_default());
            }
        }
    }
    view
}

async fn mine_view(state: &AppState, user: &AuthUser) -> AppResult<Json_> {
    let instance = redis_account::get(&state.db, user.owner_id())?;
    Ok(json!({
        "available": redis_account::is_available(),
        "instance": match &instance {
            Some(instance) => instance_json(instance, true).await,
            None => Json_::Null,
        },
    }))
}

async fn mine(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Json_>> {
    user.require(ACCOUNT_AREA)?;
    Ok(Json(mine_view(&state, &user).await?))
}

async fn mine_enable(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Json_>> {
    user.require(ACCOUNT_AREA)?;
    let _guard = state.sys_lock.lock().await;
    let created = redis_account::get(&state.db, user.owner_id())?.is_none();
    let instance = redis_account::enable(&state.db, user.owner_id()).await?;
    if created {
        auth::audit(&state.db, &user, "redis.account.enable", &instance.username, &format!("port {}", instance.port));
    }
    Ok(Json(mine_view(&state, &user).await?))
}

async fn mine_disable(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Json_>> {
    user.require(ACCOUNT_AREA)?;
    let _guard = state.sys_lock.lock().await;
    if let Some(instance) = redis_account::get(&state.db, user.owner_id())? {
        redis_account::disable(&state.db, user.owner_id()).await?;
        auth::audit(&state.db, &user, "redis.account.disable", &instance.username, "");
    }
    Ok(Json(mine_view(&state, &user).await?))
}

async fn mine_password(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Json_>> {
    user.require(ACCOUNT_AREA)?;
    let _guard = state.sys_lock.lock().await;
    let instance = redis_account::rotate_password(&state.db, user.owner_id()).await?;
    auth::audit(&state.db, &user, "redis.account.password", &instance.username, "");
    Ok(Json(mine_view(&state, &user).await?))
}

// ---- Hesap örnekleri (yönetici) ----

async fn accounts(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Json_>> {
    user.require_admin()?;
    let mut list = Vec::new();
    for instance in redis_account::list(&state.db)? {
        // Parola yöneticinin listesinde gösterilmez; hesabın kendi sayfasındadır.
        list.push(instance_json(&instance, false).await);
    }
    Ok(Json(json!({ "accounts": list, "max_memory_mb": redis_account::MAX_MEMORY_MB })))
}

#[derive(Deserialize)]
struct MemoryBody {
    memory_mb: u32,
}

async fn account_memory(State(state): State<AppState>, user: AuthUser, axum::extract::Path(user_id): axum::extract::Path<i64>, Json(body): Json<MemoryBody>) -> AppResult<Json<Json_>> {
    user.require_admin()?;
    let _guard = state.sys_lock.lock().await;
    let instance = redis_account::set_memory(&state.db, user_id, body.memory_mb).await?;
    auth::audit(&state.db, &user, "redis.account.memory", &instance.username, &format!("{} MB", instance.memory_mb));
    Ok(Json(instance_json(&instance, false).await))
}

async fn account_close(State(state): State<AppState>, user: AuthUser, axum::extract::Path(user_id): axum::extract::Path<i64>) -> AppResult<Json<Json_>> {
    user.require_admin()?;
    let _guard = state.sys_lock.lock().await;
    let instance = redis_account::get(&state.db, user_id)?.ok_or(AppError::NotFound)?;
    redis_account::disable(&state.db, user_id).await?;
    auth::audit(&state.db, &user, "redis.account.disable", &instance.username, "yönetici");
    Ok(Json(json!({ "ok": true })))
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
    /// Yalnızca bu türdeki anahtarlar (`string`, `hash`, `list`, `set`, `zset`, `stream`).
    #[serde(default, rename = "type")]
    kind: String,
}

fn validate_db(db: u32) -> AppResult<String> {
    if db < 1024 { Ok(db.to_string()) } else { Err(AppError::bad("Geçersiz veritabanı numarası")) }
}

fn validate_kind(kind: &str) -> AppResult<()> {
    if kind.is_empty() || KEY_TYPES.contains(&kind) { Ok(()) } else { Err(AppError::bad("Geçersiz anahtar türü")) }
}

/// Anahtarları tarar. Redis'in SCAN komutu tek adımda az (hatta hiç) sonuç
/// dönebildiğinden, yeterince anahtar birikene ya da tarama bitene kadar yinelenir.
async fn list_keys(State(state): State<AppState>, user: AuthUser, Query(query): Query<KeysQuery>) -> AppResult<Json<Json_>> {
    let mut cursor = if query.cursor.is_empty() { "0".to_string() } else { query.cursor };
    if !cursor.bytes().all(|b| b.is_ascii_digit()) || cursor.len() > 20 {
        return Err(AppError::bad("Geçersiz imleç"));
    }
    if query.pattern.len() > 200 {
        return Err(AppError::bad("Arama kalıbı çok uzun"));
    }
    validate_kind(&query.kind)?;
    let pattern = if query.pattern.is_empty() { "*".to_string() } else { query.pattern };
    let mut client = client_for(&state, &user).await?;
    client.run(&["SELECT", &validate_db(query.db)?]).await?;

    let mut names: Vec<Value> = Vec::new();
    for _ in 0..MAX_SCAN_ROUNDS {
        let mut args = vec!["SCAN", cursor.as_str(), "MATCH", pattern.as_str(), "COUNT", SCAN_COUNT];
        if !query.kind.is_empty() {
            args.extend(["TYPE", query.kind.as_str()]);
        }
        let reply = client.run(&args).await?.items();
        cursor = reply.first().map(Value::text).unwrap_or_else(|| "0".into());
        names.extend(reply.get(1).map(Value::items).unwrap_or_default());
        if cursor == "0" || names.len() >= KEYS_PER_PAGE {
            break;
        }
    }
    let mut keys = Vec::new();
    for key in names {
        let Value::Bulk(Some(raw)) = &key else { continue };
        let kind = client.command(&[b"TYPE", raw]).await?.text();
        let ttl = client.command(&[b"TTL", raw]).await?.int();
        let size = client.command(&[b"MEMORY", b"USAGE", raw]).await.map(|value| value.int()).unwrap_or(0);
        keys.push(json!({ "key": key.text(), "type": kind, "ttl": ttl, "size": size }));
    }
    Ok(Json(json!({ "cursor": cursor, "keys": keys })))
}

#[derive(Deserialize)]
struct KeyQuery {
    #[serde(default)]
    db: u32,
    key: String,
    /// Koleksiyonlarda kaçıncı öğeden başlanacağı (list, zset) ya da tarama imleci (hash, set).
    #[serde(default)]
    cursor: String,
    /// Hash ve set içinde alan/üye araması.
    #[serde(default)]
    filter: String,
}

/// Uzun değerler panelde kırpılarak gösterilir.
fn preview(value: &Value) -> Json_ {
    match value {
        Value::Bulk(Some(bytes)) => {
            let cut = bytes.len().min(STRING_PREVIEW_BYTES);
            json!({
                "text": String::from_utf8_lossy(&bytes[..cut]),
                "truncated": bytes.len() > cut,
                "bytes": bytes.len(),
                // Metin olarak gösterilemeyen (ikili) değerler düzenlenemez.
                "binary": std::str::from_utf8(&bytes[..cut]).is_err() && bytes.len() == cut,
            })
        }
        other => json!({ "text": other.text(), "truncated": false, "bytes": other.text().len(), "binary": false }),
    }
}

/// Anahtarın ayrıntısı. Koleksiyonlar sayfa sayfa okunur; yanıt bir sonraki
/// sayfanın imlecini (`next`; bittiyse boş) içerir.
async fn show_key(State(state): State<AppState>, user: AuthUser, Query(query): Query<KeyQuery>) -> AppResult<Json<Json_>> {
    let mut client = client_for(&state, &user).await?;
    client.run(&["SELECT", &validate_db(query.db)?]).await?;
    let key = query.key.as_str();
    let cursor = if query.cursor.is_empty() { "0".to_string() } else { query.cursor.clone() };
    if !cursor.bytes().all(|b| b.is_ascii_digit()) || cursor.len() > 20 {
        return Err(AppError::bad("Geçersiz imleç"));
    }
    if query.filter.len() > 200 {
        return Err(AppError::bad("Arama metni çok uzun"));
    }
    let kind = client.run(&["TYPE", key]).await?.text();
    if kind == "none" {
        return Err(AppError::NotFound);
    }
    let ttl = client.run(&["TTL", key]).await?.int();
    let size = client.run(&["MEMORY", "USAGE", key]).await.map(|value| value.int()).unwrap_or(0);
    let encoding = client.run(&["OBJECT", "ENCODING", key]).await.map(|value| value.text()).unwrap_or_default();
    let texts = |value: Value| value.items().iter().map(Value::text).collect::<Vec<_>>();
    let pattern = if query.filter.is_empty() { "*".to_string() } else { format!("*{}*", query.filter) };
    // Sıralı türlerde imleç, öğenin sırasıdır.
    let offset: i64 = cursor.parse().unwrap_or(0);
    let last = (offset + PAGE_ITEMS - 1).to_string();
    let next_offset = |count: usize, total: i64| if offset + (count as i64) < total { (offset + count as i64).to_string() } else { String::new() };
    let scan_next = |reply: &[Value]| reply.first().map(Value::text).filter(|next| next != "0").unwrap_or_default();

    let (length, content, next) = match kind.as_str() {
        "string" => (client.run(&["STRLEN", key]).await?.int(), json!({ "value": preview(&client.run(&["GET", key]).await?) }), String::new()),
        "list" => {
            let total = client.run(&["LLEN", key]).await?.int();
            let items = texts(client.run(&["LRANGE", key, &cursor, &last]).await?);
            let next = next_offset(items.len(), total);
            (total, json!({ "items": items, "offset": offset }), next)
        }
        "set" => {
            let reply = client.run(&["SSCAN", key, &cursor, "MATCH", &pattern, "COUNT", "200"]).await?.items();
            let next = scan_next(&reply);
            (client.run(&["SCARD", key]).await?.int(), json!({ "items": reply.get(1).cloned().map(texts).unwrap_or_default() }), next)
        }
        "zset" => {
            let total = client.run(&["ZCARD", key]).await?.int();
            let flat = texts(client.run(&["ZRANGE", key, &cursor, &last, "WITHSCORES"]).await?);
            let pairs: Vec<Json_> = flat.chunks(2).map(|pair| json!({ "member": pair[0], "score": pair.get(1) })).collect();
            let next = next_offset(pairs.len(), total);
            (total, json!({ "pairs": pairs, "offset": offset }), next)
        }
        "hash" => {
            let reply = client.run(&["HSCAN", key, &cursor, "MATCH", &pattern, "COUNT", "200"]).await?.items();
            let flat = reply.get(1).cloned().map(texts).unwrap_or_default();
            let fields: Vec<Json_> = flat.chunks(2).map(|pair| json!({ "field": pair[0], "value": pair.get(1) })).collect();
            let next = scan_next(&reply);
            (client.run(&["HLEN", key]).await?.int(), json!({ "fields": fields }), next)
        }
        "stream" => {
            // En yeni kayıtlar önce; imleç bir önceki sayfanın son kimliğidir.
            let entries = client.run(&["XREVRANGE", key, "+", "-", "COUNT", "100"]).await?.items();
            let rows: Vec<Json_> = entries
                .iter()
                .map(|entry| {
                    let parts = entry.items();
                    let flat = parts.get(1).cloned().map(texts).unwrap_or_default();
                    let fields: Vec<Json_> = flat.chunks(2).map(|pair| json!({ "field": pair[0], "value": pair.get(1) })).collect();
                    json!({ "id": parts.first().map(Value::text).unwrap_or_default(), "fields": fields })
                })
                .collect();
            (client.run(&["XLEN", key]).await?.int(), json!({ "entries": rows }), String::new())
        }
        _ => (0, json!({}), String::new()),
    };
    Ok(Json(json!({ "key": key, "type": kind, "ttl": ttl, "size": size, "encoding": encoding, "length": length, "content": content, "next": next })))
}

/// Anahtar üzerinde yapılabilen değişiklikler. Her biri tek bir Redis komutuna karşılık gelir.
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Edit {
    /// Metin değeri yazar (anahtar yoksa oluşturur).
    SetString { value: String },
    /// Süreyi ayarlar; `null` süreyi kaldırır.
    SetTtl { seconds: Option<i64> },
    Rename { new_key: String },
    HashSet { field: String, value: String },
    HashDelete { field: String },
    /// Listenin sonuna ekler.
    ListPush { value: String },
    ListSet { index: i64, value: String },
    /// Değerin ilk geçtiği öğeyi siler.
    ListRemove { value: String },
    SetAdd { member: String },
    SetRemove { member: String },
    ZsetAdd { member: String, score: f64 },
    ZsetRemove { member: String },
}

#[derive(Deserialize)]
struct EditBody {
    #[serde(default)]
    db: u32,
    key: String,
    #[serde(flatten)]
    edit: Edit,
}

async fn edit_key(State(state): State<AppState>, user: AuthUser, Json(body): Json<EditBody>) -> AppResult<Json<Json_>> {
    if body.key.is_empty() || body.key.len() > 1024 {
        return Err(AppError::bad("Anahtar adı boş olamaz ve 1024 baytı aşamaz"));
    }
    let mut client = client_for(&state, &user).await?;
    client.run(&["SELECT", &validate_db(body.db)?]).await?;
    let key = body.key.as_str();
    let too_big = |value: &str| value.len() > MAX_VALUE_BYTES;
    let (label, reply) = match &body.edit {
        Edit::SetString { value } => {
            if too_big(value) {
                return Err(AppError::bad("Değer panelden yazılamayacak kadar büyük (en çok 1 MB)"));
            }
            // Var olan süre korunur.
            ("set", client.run(&["SET", key, value, "KEEPTTL"]).await?)
        }
        Edit::SetTtl { seconds: Some(seconds) } => {
            if !(1..=3_153_600_000).contains(seconds) {
                return Err(AppError::bad("Süre 1 saniyeden kısa olamaz"));
            }
            ("expire", client.run(&["EXPIRE", key, &seconds.to_string()]).await?)
        }
        Edit::SetTtl { seconds: None } => ("persist", client.run(&["PERSIST", key]).await?),
        Edit::Rename { new_key } => {
            if new_key.is_empty() || new_key.len() > 1024 {
                return Err(AppError::bad("Yeni ad boş olamaz ve 1024 baytı aşamaz"));
            }
            let renamed = client.run(&["RENAMENX", key, new_key]).await?;
            if renamed.int() == 0 {
                return Err(AppError::conflict("Bu adda bir anahtar zaten var"));
            }
            ("rename", renamed)
        }
        Edit::HashSet { field, value } if !too_big(value) => ("hset", client.run(&["HSET", key, field, value]).await?),
        Edit::HashDelete { field } => ("hdel", client.run(&["HDEL", key, field]).await?),
        Edit::ListPush { value } if !too_big(value) => ("rpush", client.run(&["RPUSH", key, value]).await?),
        Edit::ListSet { index, value } if !too_big(value) => ("lset", client.run(&["LSET", key, &index.to_string(), value]).await?),
        Edit::ListRemove { value } => ("lrem", client.run(&["LREM", key, "1", value]).await?),
        Edit::SetAdd { member } if !too_big(member) => ("sadd", client.run(&["SADD", key, member]).await?),
        Edit::SetRemove { member } => ("srem", client.run(&["SREM", key, member]).await?),
        Edit::ZsetAdd { member, score } if !too_big(member) && score.is_finite() => ("zadd", client.run(&["ZADD", key, &score.to_string(), member]).await?),
        Edit::ZsetRemove { member } => ("zrem", client.run(&["ZREM", key, member]).await?),
        _ => return Err(AppError::bad("Değer geçersiz ya da çok büyük (en çok 1 MB)")),
    };
    auth::audit(&state.db, &user, &format!("redis.key.{label}"), &key.chars().take(120).collect::<String>(), "");
    Ok(Json(json!({ "result": reply.text() })))
}

/// Konsoldan çalıştırılması engellenen komutlar: sunucuyu durduran, yapılandırmayı
/// ya da kimlik doğrulamayı değiştiren, bağlantıyı süresiz meşgul eden ya da
/// sunucuda kod çalıştıran komutlar. (Yapılandırma ve boşaltma için ayrı, onaylı uçlar var.)
const BLOCKED_COMMANDS: &[&str] = &[
    "SHUTDOWN", "DEBUG", "CONFIG", "ACL", "AUTH", "HELLO", "MODULE", "REPLICAOF", "SLAVEOF", "CLUSTER", "FAILOVER", "MIGRATE",
    "MONITOR", "SUBSCRIBE", "PSUBSCRIBE", "SSUBSCRIBE", "SYNC", "PSYNC", "WAIT", "WAITAOF", "EVAL", "EVALSHA", "EVAL_RO",
    "EVALSHA_RO", "FUNCTION", "FCALL", "FCALL_RO", "SCRIPT", "FLUSHALL", "FLUSHDB", "SWAPDB", "BGREWRITEAOF", "BGSAVE", "SAVE",
    "RESTORE", "CLIENT", "SELECT", "MULTI", "EXEC", "WATCH", "RESET", "QUIT", "BLPOP", "BRPOP", "BLMOVE", "BRPOPLPUSH", "BZPOPMIN",
    "BZPOPMAX", "BLMPOP", "BZMPOP", "XREAD", "XREADGROUP", "KEYS",
];

/// Komut satırını argümanlara ayırır; çift ve tek tırnak içindeki boşluklar korunur.
fn split_command(line: &str) -> AppResult<Vec<String>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => current.push(chars.next().ok_or_else(|| AppError::bad("Komut eksik bitiyor"))?),
            (Some(_), c) => current.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, c) => current.push(c),
        }
    }
    if quote.is_some() {
        return Err(AppError::bad("Kapanmamış tırnak"));
    }
    if started || !current.is_empty() {
        args.push(current);
    }
    Ok(args)
}

/// Yanıtı arayüzün gösterebileceği iç içe yapıya çevirir.
fn reply_json(value: &Value, depth: usize) -> Json_ {
    match value {
        Value::Simple(text) => json!({ "kind": "status", "value": text }),
        Value::Int(number) => json!({ "kind": "integer", "value": number }),
        Value::Bulk(None) | Value::Array(None) => json!({ "kind": "nil" }),
        Value::Bulk(Some(bytes)) => {
            let cut = bytes.len().min(16 * 1024);
            json!({ "kind": "string", "value": String::from_utf8_lossy(&bytes[..cut]), "truncated": bytes.len() > cut })
        }
        Value::Array(Some(items)) => {
            let shown: Vec<Json_> = if depth >= 6 { Vec::new() } else { items.iter().take(500).map(|item| reply_json(item, depth + 1)).collect() };
            json!({ "kind": "array", "items": shown, "total": items.len() })
        }
    }
}

#[derive(Deserialize)]
struct CommandBody {
    #[serde(default)]
    db: u32,
    command: String,
}

/// Konsol: tek bir Redis komutu çalıştırır. Tehlikeli komutlar reddedilir, her komut denetim günlüğüne yazılır.
async fn run_command(State(state): State<AppState>, user: AuthUser, Json(body): Json<CommandBody>) -> AppResult<Json<Json_>> {
    if body.command.len() > 64 * 1024 {
        return Err(AppError::bad("Komut çok uzun"));
    }
    let args = split_command(body.command.trim())?;
    let name = args.first().map(|name| name.to_ascii_uppercase()).ok_or_else(|| AppError::bad("Bir komut yazın"))?;
    if BLOCKED_COMMANDS.contains(&name.as_str()) {
        return Err(AppError::bad(format!("{name} komutu konsoldan çalıştırılamaz")));
    }
    let mut client = client_for(&state, &user).await?;
    client.run(&["SELECT", &validate_db(body.db)?]).await?;
    let raw: Vec<&[u8]> = args.iter().map(|arg| arg.as_bytes()).collect();
    // Redis'in hata yanıtı da bir sonuçtur; konsolda olduğu gibi gösterilir.
    let reply = match client.command(&raw).await {
        Ok(value) => reply_json(&value, 0),
        Err(AppError::BadRequest(message)) => json!({ "kind": "error", "value": message.trim_start_matches("Redis: ") }),
        Err(err) => return Err(err),
    };
    auth::audit(&state.db, &user, "redis.command", &name, &format!("db{}", body.db));
    Ok(Json(json!({ "reply": reply })))
}

async fn delete_key(State(state): State<AppState>, user: AuthUser, Query(query): Query<KeyQuery>) -> AppResult<Json<Json_>> {
    let mut client = client_for(&state, &user).await?;
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
    let mut client = client_for(&state, &user).await?;
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
