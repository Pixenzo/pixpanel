//! Hesabın kendi yedekleri: müşteri kendi sitelerinin, veritabanlarının ve postasının
//! yedeklerini görür, elle yedek alır, yerine geri yükler ve kendi aldığı yedeği siler.
//!
//! Yedek yerlerini ve planları yönetici tanımlar; müşteri onların ayrıntısını
//! (adres, anahtar, parola) görmez. Her işlemde hem yedeğin sahibi hem de kaynağın
//! bugünkü sahibi denetlenir: el değiştirmiş bir sitenin eski yedeğine eski sahibi ulaşamaz.

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::{self, AuthUser};
use crate::backup::restic::{self, Entry, Snapshot};
use crate::backup::run::{self, RestoreOptions};
use crate::backup::{self, Scope, Target};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

const AREA: &str = "backups";
/// Hesap başına saklanabilen elle alınmış yedek sayısı. Plan yedeklerini saklama ilkesi
/// temizler; elle alınanlar birikip yöneticinin deposunu doldurmasın diye sınırlanır.
const MANUAL_LIMIT: usize = 15;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(overview))
        .route("/run", post(run_backup))
        .route("/restore", post(restore))
        .route("/snapshots/{target}/{snapshot}", delete(delete_snapshot))
        .route("/snapshots/{target}/{snapshot}/files", get(list_files))
}

/// İsteği yapan hesabın (alt hesapsa bağlı olduğu hesabın) kimliği ve adı.
fn account(state: &AppState, user: &AuthUser) -> AppResult<(i64, String)> {
    user.require(AREA)?;
    let id = user.owner_id();
    let name: String = state.db.call(|conn| conn.query_row("SELECT username FROM users WHERE id = ?1", params![id], |row| row.get(0)))?;
    Ok((id, name))
}

fn ensure_ready() -> AppResult<()> {
    if backup::is_installed() { Ok(()) } else { Err(AppError::bad("Bu sunucuda yedekleme kurulu değil; yöneticinize başvurun")) }
}

/// Yedeğin kaynağı bugün de bu hesaba mı ait? Kaynak silinmişse ya da el değiştirmişse `false`.
fn source_owned(state: &AppState, account_id: i64, snapshot: &Snapshot) -> AppResult<bool> {
    let sql = match snapshot.kind.as_str() {
        "site" => "SELECT user_id FROM sites WHERE domain = ?1",
        "db" => "SELECT user_id FROM databases WHERE name = ?1 AND engine = ?2",
        "mail" => "SELECT user_id FROM mail_domains WHERE domain = ?1",
        _ => return Ok(false),
    };
    let owner: Option<i64> = state.db.call(|conn| {
        if snapshot.kind == "db" {
            conn.query_row(sql, params![snapshot.name, snapshot.engine], |row| row.get(0)).optional()
        } else {
            conn.query_row(sql, params![snapshot.name], |row| row.get(0)).optional()
        }
    })?;
    Ok(owner == Some(account_id))
}

/// Yedeği okur ve bu hesabın ona erişebildiğini denetler. Erişemiyorsa yedeğin varlığı da sızdırılmaz.
async fn own_snapshot(state: &AppState, account: &(i64, String), target_id: i64, snapshot_id: &str) -> AppResult<(Target, Snapshot)> {
    ensure_ready()?;
    let target = backup::load_target(&state.db, target_id)?;
    let snapshot = restic::find_snapshot(&target, snapshot_id).await.map_err(|_| AppError::NotFound)?;
    if snapshot.owner != account.1 || !source_owned(state, account.0, &snapshot)? {
        return Err(AppError::NotFound);
    }
    Ok((target, snapshot))
}

#[derive(Serialize)]
struct OwnSnapshot {
    target_id: i64,
    #[serde(flatten)]
    snapshot: Snapshot,
    /// Elle alınmış yedek (hesap kendisi silebilir); plan yedeklerini saklama ilkesi yönetir.
    manual: bool,
}

#[derive(Serialize)]
struct Source {
    id: i64,
    name: String,
}

#[derive(Serialize)]
struct Overview {
    /// Yedekleme kurulu ve en az bir yedek yeri tanımlı.
    available: bool,
    snapshots: Vec<OwnSnapshot>,
    sites: Vec<Source>,
    databases: Vec<Source>,
    mail_domains: Vec<Source>,
    manual_count: usize,
    manual_limit: usize,
}

fn sources(state: &AppState, account_id: i64, sql: &str) -> AppResult<Vec<Source>> {
    state.db.call(|conn| {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![account_id], |row| Ok(Source { id: row.get(0)?, name: row.get(1)? }))?;
        rows.collect()
    })
}

/// Hesabın, kaynağı hâlâ kendisine ait olan bütün yedekleri (yeniden eskiye).
async fn own_snapshots(state: &AppState, account: &(i64, String)) -> AppResult<Vec<OwnSnapshot>> {
    let mut list = Vec::new();
    for target in backup::list_targets(&state.db)? {
        // Erişilemeyen bir yedek yeri, ötekilerdeki yedeklerin görünmesini engellemesin.
        let Ok(snapshots) = restic::snapshots(&target).await else { continue };
        for snapshot in snapshots {
            if snapshot.owner == account.1 && source_owned(state, account.0, &snapshot)? {
                list.push(OwnSnapshot { target_id: target.id, manual: snapshot.plan_id.is_none(), snapshot });
            }
        }
    }
    list.sort_by(|a, b| b.snapshot.time.cmp(&a.snapshot.time));
    Ok(list)
}

async fn overview(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Overview>> {
    let account = account(&state, &user)?;
    let available = backup::is_installed() && !backup::list_targets(&state.db)?.is_empty();
    let snapshots = if available { own_snapshots(&state, &account).await? } else { Vec::new() };
    Ok(Json(Overview {
        available,
        manual_count: snapshots.iter().filter(|item| item.manual).count(),
        manual_limit: MANUAL_LIMIT,
        snapshots,
        sites: sources(&state, account.0, "SELECT id, domain FROM sites WHERE user_id = ?1 ORDER BY domain")?,
        databases: sources(&state, account.0, "SELECT id, name || ' (' || engine || ')' FROM databases WHERE user_id = ?1 ORDER BY name")?,
        mail_domains: sources(&state, account.0, "SELECT id, domain FROM mail_domains WHERE user_id = ?1 ORDER BY domain")?,
    }))
}

#[derive(Deserialize)]
struct RunBody {
    #[serde(default)]
    site_ids: Vec<i64>,
    #[serde(default)]
    database_ids: Vec<i64>,
    #[serde(default)]
    mail_domain_ids: Vec<i64>,
}

/// Verilen kimliklerin hepsi bu hesaba mı ait?
fn all_owned(state: &AppState, account_id: i64, table: &str, ids: &[i64]) -> AppResult<bool> {
    // Tablo adı çağıran koddaki sabitlerden gelir; kullanıcı girdisi değildir.
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE id = ?1 AND user_id = ?2");
    state.db.call(|conn| {
        for id in ids {
            let owned: i64 = conn.query_row(&sql, params![id, account_id], |row| row.get(0))?;
            if owned == 0 {
                return Ok(false);
            }
        }
        Ok(true)
    })
}

async fn run_backup(State(state): State<AppState>, user: AuthUser, Json(body): Json<RunBody>) -> AppResult<Json<Value>> {
    let account = account(&state, &user)?;
    ensure_ready()?;
    restic::require_supported().await?;
    if body.site_ids.len() + body.database_ids.len() + body.mail_domain_ids.len() == 0 {
        return Err(AppError::bad("Yedeklenecek en az bir şey seçin"));
    }
    let owned = all_owned(&state, account.0, "sites", &body.site_ids)?
        && all_owned(&state, account.0, "databases", &body.database_ids)?
        && all_owned(&state, account.0, "mail_domains", &body.mail_domain_ids)?;
    if !owned {
        return Err(AppError::NotFound);
    }
    // Müşteri yedekleri yöneticinin ilk tanımladığı yedek yerine gider.
    let target = backup::list_targets(&state.db)?.into_iter().next().ok_or_else(|| AppError::bad("Yedek yeri tanımlı değil; yöneticinize başvurun"))?;
    let manual = own_snapshots(&state, &account).await?.into_iter().filter(|item| item.manual).count();
    let adding = body.site_ids.len() + body.database_ids.len() + body.mail_domain_ids.len();
    if manual + adding > MANUAL_LIMIT {
        return Err(AppError::bad(format!(
            "En çok {MANUAL_LIMIT} elle alınmış yedek saklayabilirsiniz ({manual} tane var). Yenisini almak için eskilerden silin."
        )));
    }
    let scope = Scope { site_ids: body.site_ids, database_ids: body.database_ids, mail_domain_ids: body.mail_domain_ids, ..Scope::default() };
    auth::audit(&state.db, &user, "backup.run", &account.1, "hesap");
    let job_id = run::start_backup(&state, Some(user.id), target, scope, None)?;
    Ok(Json(json!({ "job_id": job_id })))
}

#[derive(Deserialize)]
struct FilesQuery {
    #[serde(default)]
    path: String,
}

async fn list_files(
    State(state): State<AppState>,
    user: AuthUser,
    Path((target_id, snapshot_id)): Path<(i64, String)>,
    Query(query): Query<FilesQuery>,
) -> AppResult<Json<Value>> {
    let account = account(&state, &user)?;
    let (target, snapshot) = own_snapshot(&state, &account, target_id, &snapshot_id).await?;
    let root = super::backup::browse_root(&snapshot)?;
    let path = if query.path.is_empty() { root.clone() } else { query.path.trim_end_matches('/').to_string() };
    let inside = std::path::Path::new(&path).starts_with(&root) && !path.split('/').any(|part| part == "..") && !path.chars().any(char::is_control) && path.len() < 4096;
    if !inside {
        return Err(AppError::bad("Bu yol yedeğin kapsamı dışında"));
    }
    let entries: Vec<Entry> = restic::list_dir(&target, &snapshot.id, &path).await?;
    Ok(Json(json!({ "root": root, "path": path, "entries": entries })))
}

#[derive(Deserialize)]
struct RestoreBody {
    target_id: i64,
    snapshot: String,
    /// Yalnızca bu dosya ve klasörler geri yüklenir; boşsa hepsi.
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    delete_extra: bool,
}

/// Yedeği alındığı yere geri yükler. Başka bir siteye ya da veritabanına yükleme ve silinmiş
/// kaynağı yeniden kurma yöneticiye bırakılmıştır (paket sınırlarını dolanmanın yolu olmasın).
async fn restore(State(state): State<AppState>, user: AuthUser, Json(body): Json<RestoreBody>) -> AppResult<Json<Value>> {
    let account = account(&state, &user)?;
    restic::require_supported().await?;
    let (target, snapshot) = own_snapshot(&state, &account, body.target_id, &body.snapshot).await?;
    let options = RestoreOptions { paths: body.paths, delete_extra: body.delete_extra, destination_site_id: None, destination_database_id: None };
    auth::audit(&state.db, &user, "backup.restore", &format!("{} {}", snapshot.kind, snapshot.name), &snapshot.short_id);
    let job_id = run::start_restore(&state, user.id, target, snapshot, options)?;
    Ok(Json(json!({ "job_id": job_id })))
}

async fn delete_snapshot(State(state): State<AppState>, user: AuthUser, Path((target_id, snapshot_id)): Path<(i64, String)>) -> AppResult<Json<Value>> {
    let account = account(&state, &user)?;
    let (target, snapshot) = own_snapshot(&state, &account, target_id, &snapshot_id).await?;
    if snapshot.plan_id.is_some() {
        return Err(AppError::bad("Bu yedek bir plana ait; plan yedeklerini saklama süresi dolunca panel siler"));
    }
    auth::audit(&state.db, &user, "backup.snapshot.delete", &format!("{} {}", snapshot.kind, snapshot.name), &snapshot.short_id);
    let job_id = run::start_forget(&state, user.id, target, snapshot)?;
    Ok(Json(json!({ "job_id": job_id })))
}
