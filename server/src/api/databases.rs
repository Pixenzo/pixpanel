//! Veritabanı, veritabanı kullanıcısı ve yetki yönetimi uçları.

use std::collections::HashMap;
use std::process::Stdio;

use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

use crate::auth::{self, AuthUser};
use crate::db::now;
use crate::dbms::{self, busy, Access, Engine};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::sys::services;

const AREA: &str = "databases";
/// İçe aktarılabilecek en büyük döküm dosyası.
const MAX_IMPORT_BYTES: u64 = 4 * 1024 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(overview).post(create_database))
        .route("/{id}", delete(drop_database))
        .route("/{id}/export", get(export))
        .route("/{id}/open", post(open_tool))
        .route("/{id}/import", post(import).layer(axum::extract::DefaultBodyLimit::disable()))
        .route("/{id}/copy", post(copy_database))
        .route("/{id}/maintenance", post(maintenance))
        .route("/slow-log", get(slow_log).put(set_slow_log))
        .route("/{id}/grants/{db_user_id}", put(set_grant).delete(remove_grant))
        .route("/users", post(create_user))
        .route("/users/{id}", patch(update_user).delete(drop_user))
}

#[derive(Serialize)]
struct EngineInfo {
    id: &'static str,
    label: &'static str,
    installed: bool,
    running: bool,
}

#[derive(Serialize)]
struct GrantInfo {
    db_user_id: i64,
    name: String,
    host: String,
    access: String,
}

#[derive(Serialize)]
struct DatabaseInfo {
    id: i64,
    engine: String,
    name: String,
    owner: String,
    size: Option<u64>,
    /// Tablo sayısı (görünümler hariç).
    tables: Option<u64>,
    grants: Vec<GrantInfo>,
    created_at: i64,
}

#[derive(Serialize)]
struct DbUserInfo {
    id: i64,
    engine: String,
    name: String,
    host: String,
    owner: String,
    created_at: i64,
}

#[derive(Serialize)]
struct Overview {
    engines: Vec<EngineInfo>,
    databases: Vec<DatabaseInfo>,
    users: Vec<DbUserInfo>,
    /// Yönetici olmayan hesapların adlarının başlaması gereken ön ek.
    name_prefix: String,
    /// Kurulu web araçları (phpMyAdmin, Adminer).
    tools: crate::dbtools::Installed,
}

/// Hesaplar arası ad çakışmasını önlemek için yönetici dışındaki hesapların
/// veritabanı ve kullanıcı adları kendi kullanıcı adlarıyla başlar.
fn name_prefix(state: &AppState, user: &AuthUser) -> AppResult<String> {
    if user.is_admin() {
        return Ok(String::new());
    }
    let owner: String = state
        .db
        .call(|conn| conn.query_row("SELECT username FROM users WHERE id = ?1", params![user.owner_id()], |r| r.get(0)))?;
    Ok(format!("{}_", owner.replace('-', "_")))
}

fn check_prefix(prefix: &str, name: &str) -> AppResult<()> {
    if name.starts_with(prefix) && name.len() > prefix.len() {
        Ok(())
    } else {
        Err(AppError::bad(format!("Ad “{prefix}” ile başlamalı (örnek: {prefix}blog)")))
    }
}

async fn overview(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Overview>> {
    user.require(AREA)?;
    let mut engines = Vec::new();
    let mut sizes: HashMap<Engine, dbms::stats::Stats> = HashMap::new();
    for engine in Engine::ALL {
        let installed = engine.is_installed();
        let running = installed && services::is_active(engine.service()).await;
        if running {
            // Boyutlar kısa süreli önbellekten gelir; sayfa her açıldığında sunucuya sorgu atılmaz.
            sizes.insert(engine, dbms::stats::get(engine).await);
        }
        engines.push(EngineInfo { id: engine.as_str(), label: engine.label(), installed, running });
    }

    let (all, owner) = (user.is_admin(), user.owner_id());
    let (mut databases, users) = state.db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT d.id, d.engine, d.name, u.username, d.created_at FROM databases d
             JOIN users u ON u.id = d.user_id WHERE ?1 OR d.user_id = ?2 ORDER BY d.name",
        )?;
        let mut databases: Vec<DatabaseInfo> = stmt
            .query_map(params![all, owner], |row| {
                Ok(DatabaseInfo {
                    id: row.get(0)?,
                    engine: row.get(1)?,
                    name: row.get(2)?,
                    owner: row.get(3)?,
                    size: None,
                    tables: None,
                    grants: Vec::new(),
                    created_at: row.get(4)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        let mut grants = conn.prepare(
            "SELECT g.db_user_id, du.name, du.host, g.access FROM database_grants g
             JOIN database_users du ON du.id = g.db_user_id WHERE g.database_id = ?1 ORDER BY du.name",
        )?;
        for database in &mut databases {
            database.grants = grants
                .query_map(params![database.id], |row| {
                    Ok(GrantInfo { db_user_id: row.get(0)?, name: row.get(1)?, host: row.get(2)?, access: row.get(3)? })
                })?
                .collect::<Result<_, _>>()?;
        }
        let mut stmt = conn.prepare(
            "SELECT du.id, du.engine, du.name, du.host, u.username, du.created_at FROM database_users du
             JOIN users u ON u.id = du.user_id WHERE ?1 OR du.user_id = ?2 ORDER BY du.name",
        )?;
        let users: Vec<DbUserInfo> = stmt
            .query_map(params![all, owner], |row| {
                Ok(DbUserInfo {
                    id: row.get(0)?,
                    engine: row.get(1)?,
                    name: row.get(2)?,
                    host: row.get(3)?,
                    owner: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok((databases, users))
    })?;
    for database in &mut databases {
        let engine = Engine::parse(&database.engine)?;
        let stat = sizes.get(&engine).and_then(|map| map.get(&database.name));
        database.size = stat.map(|stat| stat.size);
        database.tables = stat.map(|stat| stat.tables);
    }
    Ok(Json(Overview { engines, databases, users, name_prefix: name_prefix(&state, &user)?, tools: crate::dbtools::installed() }))
}

struct DbRow {
    id: i64,
    user_id: i64,
    engine: Engine,
    name: String,
}

/// Veritabanı kaydını okur ve isteği yapanın sahibi olduğunu doğrular.
fn load_database(state: &AppState, user: &AuthUser, id: i64) -> AppResult<DbRow> {
    let found = state.db.call(|conn| {
        conn.query_row("SELECT id, user_id, engine, name FROM databases WHERE id = ?1", params![id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?))
        })
        .optional()
    })?;
    let (id, user_id, engine, name) = found.ok_or(AppError::NotFound)?;
    if !user.is_admin() && user_id != user.owner_id() {
        return Err(AppError::NotFound);
    }
    Ok(DbRow { id, user_id, engine: Engine::parse(&engine)?, name })
}

/// Veritabanını web aracında (phpMyAdmin ya da Adminer) açmak için tek kullanımlık adres üretir.
async fn open_tool(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let database = load_database(&state, &user, id)?;
    database.engine.require_installed()?;
    let _guard = state.sys_lock.lock().await;
    let url = crate::dbtools::open(&state.db, &state.cfg, database.engine, &database.name).await?;
    auth::audit(&state.db, &user, "database.open_tool", &database.name, database.engine.as_str());
    Ok(Json(json!({ "url": url })))
}

struct DbUserRow {
    id: i64,
    user_id: i64,
    engine: Engine,
    name: String,
    host: String,
}

fn load_db_user(state: &AppState, user: &AuthUser, id: i64) -> AppResult<DbUserRow> {
    let found = state.db.call(|conn| {
        conn.query_row("SELECT id, user_id, engine, name, host FROM database_users WHERE id = ?1", params![id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .optional()
    })?;
    let (id, user_id, engine, name, host) = found.ok_or(AppError::NotFound)?;
    if !user.is_admin() && user_id != user.owner_id() {
        return Err(AppError::NotFound);
    }
    Ok(DbUserRow { id, user_id, engine: Engine::parse(&engine)?, name, host })
}

#[derive(Deserialize)]
struct NewDbUser {
    name: String,
    password: String,
    #[serde(default)]
    host: Option<String>,
}

#[derive(Deserialize)]
struct CreateDatabaseBody {
    engine: String,
    name: String,
    /// Verilirse veritabanıyla birlikte tam yetkili bir kullanıcı da oluşturulur.
    user: Option<NewDbUser>,
}

fn normalize_host(engine: Engine, host: Option<&str>) -> AppResult<String> {
    match engine {
        // PostgreSQL'de bağlantı adresi kullanıcıya değil sunucu ayarına bağlıdır.
        Engine::Postgresql => Ok(String::new()),
        Engine::Mariadb => {
            let host = host.map(str::trim).filter(|h| !h.is_empty()).unwrap_or("localhost");
            dbms::validate_host(host)?;
            Ok(host.to_string())
        }
    }
}

/// Kullanıcıyı sunucuda oluşturur ve panel kaydını ekler; kaydın kimliğini döner.
async fn provision_user(
    state: &AppState,
    owner_id: i64,
    engine: Engine,
    name: &str,
    host: &str,
    password: &str,
) -> AppResult<i64> {
    let taken: bool = state.db.call(|conn| {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM database_users WHERE engine = ?1 AND name = ?2 AND host = ?3)",
            params![engine.as_str(), name, host],
            |row| row.get(0),
        )
    })?;
    if taken {
        return Err(AppError::conflict(format!("“{name}” adında bir veritabanı kullanıcısı zaten var")));
    }
    dbms::create_user(engine, name, host, password).await?;
    let inserted = state.db.call(|conn| {
        conn.execute(
            "INSERT INTO database_users (user_id, engine, name, host, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![owner_id, engine.as_str(), name, host, now()],
        )?;
        Ok(conn.last_insert_rowid())
    });
    if inserted.is_err() {
        let _ = dbms::drop_user(engine, name, host).await;
    }
    inserted
}

async fn create_database(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateDatabaseBody>,
) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let engine = Engine::parse(&body.engine)?;
    engine.require_installed()?;
    let prefix = name_prefix(&state, &user)?;
    let name = body.name.trim().to_lowercase();
    dbms::validate_name(&name, "Veritabanı adı")?;
    check_prefix(&prefix, &name)?;

    let new_user = match &body.user {
        Some(new_user) => {
            let user_name = new_user.name.trim().to_lowercase();
            dbms::validate_name(&user_name, "Kullanıcı adı")?;
            check_prefix(&prefix, &user_name)?;
            dbms::validate_password(&new_user.password)?;
            Some((user_name, normalize_host(engine, new_user.host.as_deref())?, new_user.password.clone()))
        }
        None => None,
    };

    let owner_id = user.owner_id();
    crate::quota::check(&state.db, owner_id, crate::quota::Resource::Database)?;
    let _guard = state.sys_lock.lock().await;
    let taken: bool = state.db.call(|conn| {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM databases WHERE engine = ?1 AND name = ?2)",
            params![engine.as_str(), name],
            |row| row.get(0),
        )
    })?;
    if taken {
        return Err(AppError::conflict(format!("“{name}” adında bir veritabanı zaten var")));
    }

    dbms::create_database(engine, &name).await?;
    let database_id = match state.db.call(|conn| {
        conn.execute(
            "INSERT INTO databases (user_id, engine, name, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![owner_id, engine.as_str(), name, now()],
        )?;
        Ok(conn.last_insert_rowid())
    }) {
        Ok(id) => id,
        Err(err) => {
            let _ = dbms::drop_database(engine, &name).await;
            return Err(err);
        }
    };

    if let Some((user_name, host, password)) = new_user {
        // Kullanıcı oluşturulamazsa veritabanı yine de kalır; hata iletisi nedenini söyler.
        let db_user_id = provision_user(&state, owner_id, engine, &user_name, &host, &password).await?;
        dbms::grant(engine, &name, &user_name, &host, Access::Full).await?;
        state.db.call(|conn| {
            conn.execute(
                "INSERT INTO database_grants (database_id, db_user_id, access) VALUES (?1, ?2, 'full')",
                params![database_id, db_user_id],
            )
        })?;
    }
    auth::audit(&state.db, &user, "database.create", &name, engine.as_str());
    Ok(Json(json!({ "id": database_id })))
}

async fn drop_database(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let _guard = state.sys_lock.lock().await;
    let database = load_database(&state, &user, id)?;
    if busy::is_busy(database.engine, &database.name) {
        return Err(busy_error());
    }
    // PostgreSQL'de yetkiler rol üzerinde tutulduğundan veritabanı silinmeden önce alınır.
    let grantees: Vec<(String, String)> = state.db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT du.name, du.host FROM database_grants g JOIN database_users du ON du.id = g.db_user_id
             WHERE g.database_id = ?1",
        )?;
        let rows = stmt.query_map(params![id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    })?;
    for (name, host) in &grantees {
        dbms::revoke(database.engine, &database.name, name, host).await?;
    }
    dbms::drop_database(database.engine, &database.name).await?;
    state.db.call(|conn| conn.execute("DELETE FROM databases WHERE id = ?1", params![database.id]))?;
    auth::audit(&state.db, &user, "database.delete", &database.name, database.engine.as_str());
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct CreateUserBody {
    engine: String,
    name: String,
    password: String,
    host: Option<String>,
}

async fn create_user(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateUserBody>,
) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let engine = Engine::parse(&body.engine)?;
    engine.require_installed()?;
    let name = body.name.trim().to_lowercase();
    dbms::validate_name(&name, "Kullanıcı adı")?;
    check_prefix(&name_prefix(&state, &user)?, &name)?;
    dbms::validate_password(&body.password)?;
    let host = normalize_host(engine, body.host.as_deref())?;

    let _guard = state.sys_lock.lock().await;
    let id = provision_user(&state, user.owner_id(), engine, &name, &host, &body.password).await?;
    auth::audit(&state.db, &user, "database.user_create", &name, engine.as_str());
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
struct UpdateUserBody {
    password: String,
}

async fn update_user(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateUserBody>,
) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    dbms::validate_password(&body.password)?;
    let db_user = load_db_user(&state, &user, id)?;
    dbms::set_password(db_user.engine, &db_user.name, &db_user.host, &body.password).await?;
    auth::audit(&state.db, &user, "database.user_password", &db_user.name, db_user.engine.as_str());
    Ok(Json(json!({ "ok": true })))
}

async fn drop_user(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let _guard = state.sys_lock.lock().await;
    let db_user = load_db_user(&state, &user, id)?;
    let granted: Vec<String> = state.db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT d.name FROM database_grants g JOIN databases d ON d.id = g.database_id WHERE g.db_user_id = ?1",
        )?;
        let rows = stmt.query_map(params![id], |row| row.get(0))?;
        rows.collect()
    })?;
    for database in &granted {
        dbms::revoke(db_user.engine, database, &db_user.name, &db_user.host).await?;
    }
    dbms::drop_user(db_user.engine, &db_user.name, &db_user.host).await?;
    state.db.call(|conn| conn.execute("DELETE FROM database_users WHERE id = ?1", params![db_user.id]))?;
    auth::audit(&state.db, &user, "database.user_delete", &db_user.name, db_user.engine.as_str());
    Ok(Json(json!({ "ok": true })))
}

/// Veritabanı ile kullanıcının aynı motora ve aynı hesaba ait olduğunu doğrular.
fn load_pair(state: &AppState, user: &AuthUser, id: i64, db_user_id: i64) -> AppResult<(DbRow, DbUserRow)> {
    let database = load_database(state, user, id)?;
    let db_user = load_db_user(state, user, db_user_id)?;
    if database.engine != db_user.engine {
        return Err(AppError::bad("Kullanıcı ile veritabanı aynı türde olmalı"));
    }
    if database.user_id != db_user.user_id {
        return Err(AppError::bad("Kullanıcı ile veritabanı aynı hesaba ait olmalı"));
    }
    Ok((database, db_user))
}

#[derive(Deserialize)]
struct GrantBody {
    access: String,
}

async fn set_grant(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, db_user_id)): Path<(i64, i64)>,
    Json(body): Json<GrantBody>,
) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let access = Access::parse(&body.access)?;
    let _guard = state.sys_lock.lock().await;
    let (database, db_user) = load_pair(&state, &user, id, db_user_id)?;
    dbms::grant(database.engine, &database.name, &db_user.name, &db_user.host, access).await?;
    state.db.call(|conn| {
        conn.execute(
            "INSERT INTO database_grants (database_id, db_user_id, access) VALUES (?1, ?2, ?3)
             ON CONFLICT(database_id, db_user_id) DO UPDATE SET access = excluded.access",
            params![database.id, db_user.id, access.as_str()],
        )
    })?;
    auth::audit(&state.db, &user, "database.grant", &database.name, &format!("{} → {}", db_user.name, access.as_str()));
    Ok(Json(json!({ "ok": true })))
}

async fn remove_grant(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, db_user_id)): Path<(i64, i64)>,
) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let _guard = state.sys_lock.lock().await;
    let (database, db_user) = load_pair(&state, &user, id, db_user_id)?;
    dbms::revoke(database.engine, &database.name, &db_user.name, &db_user.host).await?;
    state.db.call(|conn| {
        conn.execute(
            "DELETE FROM database_grants WHERE database_id = ?1 AND db_user_id = ?2",
            params![database.id, db_user.id],
        )
    })?;
    auth::audit(&state.db, &user, "database.revoke", &database.name, &db_user.name);
    Ok(Json(json!({ "ok": true })))
}

/// Veritabanının SQL dökümünü akış olarak indirir; döküm bellekte biriktirilmez.
async fn export(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Response> {
    user.require(AREA)?;
    let database = load_database(&state, &user, id)?;
    let mut child = dbms::dump_command(database.engine, &database.name)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| AppError::internal(format!("döküm başlatılamadı: {err}")))?;
    let stdout = child.stdout.take().expect("stdout borusu");
    // Süreç, yanıt akışı bitene (ya da istemci bağlantıyı kesene) kadar yaşar.
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    auth::audit(&state.db, &user, "database.export", &database.name, database.engine.as_str());
    let filename = format!("{}-{}.sql", database.name, now());
    Ok((
        [
            (CONTENT_TYPE, "application/sql".to_string()),
            (CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\"")),
        ],
        Body::from_stream(ReaderStream::new(stdout)),
    )
        .into_response())
}

/// Yüklenen `.sql` ya da `.sql.gz` dosyasını veritabanına aktarır.
/// Dosya önce diske yazılır, aktarım arka plan görevi olarak yürür.
async fn import(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    mut multipart: Multipart,
) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let database = load_database(&state, &user, id)?;
    // Kopyalama ya da bakım sürerken içe aktarma başlamasın (kilit görev bitene kadar tutulur).
    let busy_guard = busy::try_lock(database.engine, &database.name).ok_or_else(busy_error)?;

    let upload_dir = state.cfg.data_dir.join("uploads");
    tokio::fs::create_dir_all(&upload_dir).await?;
    let path = upload_dir.join(format!("import-{}.sql", auth::random_token(12)));

    let mut gzipped = false;
    let mut received: u64 = 0;
    let mut found = false;
    while let Some(mut field) =
        multipart.next_field().await.map_err(|err| AppError::bad(format!("Yükleme okunamadı: {err}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        found = true;
        gzipped = field.file_name().is_some_and(|name| name.to_lowercase().ends_with(".gz"));
        let mut file = tokio::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).await?;
        while let Some(chunk) =
            field.chunk().await.map_err(|err| AppError::bad(format!("Yükleme yarıda kesildi: {err}")))?
        {
            received += chunk.len() as u64;
            if received > MAX_IMPORT_BYTES {
                drop(file);
                let _ = tokio::fs::remove_file(&path).await;
                return Err(AppError::bad("Dosya 4 GB sınırını aşıyor"));
            }
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        break;
    }
    if !found || received == 0 {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(AppError::bad("Dosya seçilmedi ya da boş"));
    }

    let (engine, name) = (database.engine, database.name.clone());
    let title = format!("{name} veritabanına içe aktarma");
    let job_id = state.jobs.start("database.import", &title, Some(user.id), move |ctx| async move {
        let _busy_guard = busy_guard;
        let outcome = run_import(&ctx, engine, &name, &path, gzipped).await;
        let _ = tokio::fs::remove_file(&path).await;
        dbms::stats::invalidate(engine);
        outcome
    })?;
    auth::audit(&state.db, &user, "database.import", &database.name, database.engine.as_str());
    Ok(Json(json!({ "job_id": job_id })))
}

async fn run_import(
    ctx: &crate::jobs::JobCtx,
    engine: Engine,
    database: &str,
    path: &std::path::Path,
    gzipped: bool,
) -> AppResult<()> {
    ctx.log(format!("{database} veritabanına aktarılıyor"));
    // Döküm güvenilmeyen girdidir: sınırlı hesapla ve yetkisiz kullanıcıyla yüklenir (bkz. `dbms::import`).
    if gzipped {
        // Sıkıştırılmış dosyada gzip'in çıktısı doğrudan yükleme hattına bağlanır.
        let mut gzip = crate::sys::exec::base_command("gzip")
            .arg("-dc")
            .arg("--")
            .arg(path)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| AppError::internal(format!("gzip başlatılamadı: {err}")))?;
        let stdout: Stdio = gzip.stdout.take().expect("stdout borusu").try_into()?;
        let loaded = dbms::import::run(ctx, engine, database, stdout).await;
        if !gzip.wait().await?.success() {
            return Err(AppError::bad("Dosya açılamadı; geçerli bir .gz dosyası değil"));
        }
        loaded?;
    } else {
        dbms::import::run(ctx, engine, database, Stdio::from(std::fs::File::open(path)?)).await?;
    }
    ctx.log("İçe aktarma tamamlandı");
    Ok(())
}

fn busy_error() -> AppError {
    AppError::conflict("Bu veritabanında başka bir işlem sürüyor; bitmesini bekleyin")
}

/// Veritabanının sahibi olan hesabın ad öneki (yönetici hesabında yok). Yönetici başkasının
/// veritabanını kopyalarken yeni ad da o hesabın kuralına uyar.
fn owner_prefix(state: &AppState, owner_id: i64) -> AppResult<String> {
    let (username, role): (String, String) = state
        .db
        .call(|conn| conn.query_row("SELECT username, role FROM users WHERE id = ?1", params![owner_id], |row| Ok((row.get(0)?, row.get(1)?))))?;
    if role == "admin" {
        return Ok(String::new());
    }
    Ok(format!("{}_", username.replace('-', "_")))
}

#[derive(Deserialize)]
struct CopyBody {
    name: String,
}

/// Veritabanını aynı hesapta yeni bir adla kopyalar. Döküm diske yazılmadan akıtılır; yükleme
/// tarafı, yüklenen dökümlerle aynı sınırlı yetkili yoldan yürür (bkz. `dbms::import`).
async fn copy_database(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<CopyBody>,
) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let source = load_database(&state, &user, id)?;
    let engine = source.engine;
    engine.require_installed()?;

    // Yeni veritabanı kaynağın sahibi olan hesaba açılır; ad kuralları oluşturma ucuyla aynıdır.
    let owner_id = source.user_id;
    let prefix = owner_prefix(&state, owner_id)?;
    let name = body.name.trim().to_lowercase();
    dbms::validate_name(&name, "Veritabanı adı")?;
    check_prefix(&prefix, &name)?;

    crate::quota::check(&state.db, owner_id, crate::quota::Resource::Database)?;
    let source_size = dbms::stats::get(engine).await.get(&source.name).map_or(0, |stat| stat.size);
    crate::quota::check_extra_disk(&state.db, owner_id, source_size)?;

    let source_guard = busy::try_lock(engine, &source.name).ok_or_else(busy_error)?;
    let target_guard = busy::try_lock(engine, &name).ok_or_else(busy_error)?;
    let _guard = state.sys_lock.lock().await;
    let taken: bool = state.db.call(|conn| {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM databases WHERE engine = ?1 AND name = ?2)",
            params![engine.as_str(), name],
            |row| row.get(0),
        )
    })?;
    if taken {
        return Err(AppError::conflict(format!("“{name}” adında bir veritabanı zaten var")));
    }

    dbms::create_database(engine, &name).await?;
    let target_id = match state.db.call(|conn| {
        conn.execute(
            "INSERT INTO databases (user_id, engine, name, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![owner_id, engine.as_str(), name, now()],
        )?;
        Ok(conn.last_insert_rowid())
    }) {
        Ok(id) => id,
        Err(err) => {
            let _ = dbms::drop_database(engine, &name).await;
            return Err(err);
        }
    };

    let (source_name, target_name) = (source.name.clone(), name.clone());
    let (db, sys_lock) = (state.db.clone(), state.sys_lock.clone());
    let title = format!("{source_name} veritabanı {target_name} olarak kopyalanıyor");
    let started = state.jobs.start("database.copy", &title, Some(user.id), move |ctx| async move {
        let _guards = (source_guard, target_guard);
        let outcome = run_copy(&ctx, engine, &source_name, &target_name).await;
        if outcome.is_err() {
            // Yarım kalmış kopya bırakılmaz: yeni veritabanı ve kaydı geri alınır.
            ctx.log("Kopyalama başarısız; yeni veritabanı geri alınıyor");
            let _lock = sys_lock.lock().await;
            match dbms::drop_database(engine, &target_name).await {
                Ok(()) => {
                    let _ = db.call(|conn| conn.execute("DELETE FROM databases WHERE id = ?1", params![target_id]));
                    ctx.log("Yeni veritabanı silindi");
                }
                Err(err) => ctx.log(format!("Yeni veritabanı silinemedi, elle silmeniz gerekebilir: {err}")),
            }
        }
        dbms::stats::invalidate(engine);
        outcome
    });
    let job_id = match started {
        Ok(job_id) => job_id,
        Err(err) => {
            let _ = dbms::drop_database(engine, &name).await;
            let _ = state.db.call(|conn| conn.execute("DELETE FROM databases WHERE id = ?1", params![target_id]));
            return Err(err);
        }
    };
    auth::audit(&state.db, &user, "database.copy", &source.name, &format!("→ {name}"));
    Ok(Json(json!({ "job_id": job_id, "id": target_id })))
}

/// Kaynağı dökerken çıktıyı doğrudan sınırlı yetkili yükleme hattına bağlar.
async fn run_copy(ctx: &crate::jobs::JobCtx, engine: Engine, source: &str, target: &str) -> AppResult<()> {
    ctx.log(format!("{source} veritabanı {target} adıyla kopyalanıyor (döküm diske yazılmaz)"));
    let mut dump = dbms::copy_dump_command(engine, source)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| AppError::internal(format!("döküm başlatılamadı: {err}")))?;
    let stdout: Stdio = dump.stdout.take().expect("stdout borusu").try_into().map_err(|err: std::io::Error| AppError::internal(err.to_string()))?;
    let stderr = dump.stderr.take().expect("stderr borusu");
    let err_ctx = ctx.clone();
    let err_task = tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut lines = BufReader::new(stderr).lines();
        let mut count = 0;
        while let Ok(Some(line)) = lines.next_line().await {
            // Bozuk bir kaynak binlerce uyarı üretebilir; günlük şişmesin.
            if count < 200 {
                err_ctx.log(line);
            }
            count += 1;
        }
    });
    let loaded = dbms::import::run_strict(ctx, engine, target, stdout).await;
    let status = dump.wait().await?;
    let _ = err_task.await;
    loaded?;
    if !status.success() {
        return Err(AppError::bad("Kaynak veritabanı dökülemedi (ayrıntı yukarıda)"));
    }
    ctx.log("Kopyalama tamamlandı");
    Ok(())
}

#[derive(Deserialize)]
struct MaintenanceBody {
    action: String,
}

/// Veritabanı tablolarını denetler, onarır ya da iyileştirir; çıktı görev günlüğüne akar.
async fn maintenance(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<MaintenanceBody>,
) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let action = dbms::Maintenance::parse(&body.action)?;
    let database = load_database(&state, &user, id)?;
    database.engine.require_installed()?;
    // Ad komut satırına girdiğinden, panelin açabileceği türden olduğu bir kez daha doğrulanır.
    dbms::validate_name(&database.name, "Veritabanı adı")?;
    let (mut command, program) = dbms::maintenance_command(database.engine, &database.name, action)?;
    // Onarım ve iyileştirme tabloları kilitleyebilir; aynı veritabanında iki işlem birden yürümez.
    let guard = busy::try_lock(database.engine, &database.name).ok_or_else(busy_error)?;

    let name = database.name.clone();
    let title = format!("{name} veritabanı {}", action.label());
    let job_id = state.jobs.start("database.maintenance", &title, Some(user.id), move |ctx| async move {
        let _guard = guard;
        ctx.log(format!("{name} veritabanı {} ({program})", action.label()));
        ctx.run_streaming(&mut command, program, None).await?;
        ctx.log("İşlem tamamlandı");
        Ok(())
    })?;
    auth::audit(&state.db, &user, "database.maintenance", &database.name, action.as_str());
    Ok(Json(json!({ "job_id": job_id })))
}

#[derive(Deserialize)]
struct SlowLogQuery {
    limit: Option<usize>,
}

/// Yavaş sorgu günlüğünün durumu ve son kayıtları. Günlük bütün hesapların sorgularını
/// içerdiğinden yalnızca yöneticiye açıktır.
async fn slow_log(user: AuthUser, Query(query): Query<SlowLogQuery>) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    Engine::Mariadb.require_installed()?;
    let settings = dbms::slowlog::settings().await?;
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let (file_size, entries) =
        if settings.file_output { dbms::slowlog::recent(&settings.file, limit).await? } else { (0, Vec::new()) };
    Ok(Json(json!({
        "enabled": settings.enabled,
        "threshold": settings.threshold,
        "thresholds": dbms::slowlog::THRESHOLDS,
        "file": settings.file,
        "file_output": settings.file_output,
        "file_size": file_size,
        "persistent": settings.persistent,
        "entries": entries,
    })))
}

#[derive(Deserialize)]
struct SlowLogBody {
    enabled: bool,
    threshold: Option<u32>,
}

async fn set_slow_log(State(state): State<AppState>, user: AuthUser, Json(body): Json<SlowLogBody>) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    Engine::Mariadb.require_installed()?;
    let threshold = body.threshold.unwrap_or(2);
    if !dbms::slowlog::THRESHOLDS.contains(&threshold) {
        return Err(AppError::bad("Eşik 1, 2, 5 ya da 10 saniye olmalı"));
    }
    let _guard = state.sys_lock.lock().await;
    dbms::slowlog::apply(body.enabled, threshold).await?;
    let detail = if body.enabled { format!("açık, eşik {threshold} sn") } else { "kapalı".to_string() };
    auth::audit(&state.db, &user, "database.slow_log", Engine::Mariadb.as_str(), &detail);
    Ok(Json(json!({ "ok": true })))
}
