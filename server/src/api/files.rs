//! Dosya yöneticisi uçları. İşlemlerin kendisi `fsop` yardımcı sürecinde,
//! ilgili hesabın kimliğiyle ve kendi klasörüne hapsedilmiş olarak yürür.

use std::process::Stdio;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Query, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio_util::io::ReaderStream;

use crate::auth::{self, AuthUser, Role};
use crate::error::{AppError, AppResult};
use crate::fsop::{Envelope, Request};
use crate::hosting::sysuser;
use crate::state::AppState;

const AREA: &str = "files";
const OP_TIMEOUT: Duration = Duration::from_secs(600);
/// Yardımcıdan okunacak en büyük JSON yanıtı (büyük klasör listeleri için geniş tutulur).
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/roots", get(roots))
        .route("/op", post(op))
        .route("/download", get(download))
        .route("/upload", post(upload).layer(DefaultBodyLimit::disable()))
}

#[derive(Serialize)]
struct RootInfo {
    /// `user:<kimlik>` ya da `root`.
    id: String,
    label: String,
    /// Bu kapsamın sunucudaki gerçek yolu (yalnızca bilgi için).
    path: String,
}

async fn roots(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Vec<RootInfo>>> {
    user.require(AREA)?;
    let (all, include_children, owner) = (user.is_admin(), user.role == Role::Reseller, user.owner_id());
    let accounts: Vec<(i64, String)> = state.db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, username FROM users
             WHERE role != 'member' AND (?1 OR id = ?2 OR (?3 AND parent_id = ?2))
             ORDER BY (id = ?2) DESC, username",
        )?;
        let rows = stmt.query_map(params![all, owner, include_children], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    })?;
    let mut list: Vec<RootInfo> = accounts
        .into_iter()
        // Kendi hesabı her zaman listelenir; başkalarınınki yalnızca dosya alanı açılmışsa.
        .filter(|(id, username)| *id == owner || sysuser::lookup(username).is_some())
        .map(|(id, username)| RootInfo { id: format!("user:{id}"), path: format!("/home/{username}"), label: username })
        .collect();
    if user.is_admin() {
        list.push(RootInfo { id: "root".into(), label: "Sunucu (root)".into(), path: "/".into() });
    }
    Ok(Json(list))
}

struct Scope {
    root: String,
    uid: u32,
    gid: u32,
    label: String,
    /// Dosya alanının sahibi hesap (`root` kapsamında yok); disk kotası bu hesaba göre okunur.
    account_id: Option<i64>,
    /// Arşiv açma için hesabın kalan disk kotası (yalnızca açma işleminde doldurulur).
    disk_budget: Option<u64>,
}

/// İstenen kapsamı çözer ve isteği yapanın ona erişimini denetler.
async fn resolve_scope(state: &AppState, user: &AuthUser, scope: &str) -> AppResult<Scope> {
    if scope == "root" {
        user.require_admin()?;
        return Ok(Scope { root: "/".into(), uid: 0, gid: 0, label: "root".into(), account_id: None, disk_budget: None });
    }
    let target: i64 =
        scope.strip_prefix("user:").and_then(|id| id.parse().ok()).ok_or_else(|| AppError::bad("Geçersiz dosya alanı"))?;
    let found = state.db.call(|conn| {
        conn.query_row("SELECT username, role, parent_id FROM users WHERE id = ?1", params![target], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<i64>>(2)?))
        })
        .optional()
    })?;
    let (username, role, parent_id) = found.ok_or(AppError::NotFound)?;
    let allowed = role != "member"
        && (user.is_admin() || target == user.owner_id() || (user.role == Role::Reseller && parent_id == Some(user.id)));
    if !allowed {
        return Err(AppError::NotFound);
    }
    let account = {
        let _guard = state.sys_lock.lock().await;
        sysuser::ensure(&username).await?
    };
    Ok(Scope { root: account.home.to_string_lossy().into_owned(), uid: account.uid, gid: account.gid, label: username, account_id: Some(target), disk_budget: None })
}

/// Yardımcı süreci başlatır ve istek zarfını standart girdisine yazar.
async fn spawn_helper(scope: &Scope, request: Request) -> AppResult<Child> {
    let exe = std::env::current_exe()?;
    // Ağır işlemler (kopyalama, sıkıştırma, arşiv açma) hesabın kaynak diliminde çalışır; dilim
    // kullanılamıyorsa ya da hesabın sınırı yoksa yardımcı eskisi gibi doğrudan başlar.
    let heavy = matches!(request, Request::Copy { .. } | Request::Compress { .. } | Request::Extract { .. });
    let (program, prefix_args) = if heavy { crate::sys::runas::wrap_for_account(scope.uid, exe) } else { (exe.into_os_string(), Vec::new()) };
    let mut child = Command::new(program)
        .args(prefix_args)
        .arg("fs-helper")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| AppError::internal(format!("dosya yardımcısı başlatılamadı: {err}")))?;
    let envelope = Envelope { root: scope.root.clone(), uid: scope.uid, gid: scope.gid, disk_budget: scope.disk_budget, request };
    let mut header = serde_json::to_vec(&envelope).map_err(|err| AppError::internal(err.to_string()))?;
    header.push(b'\n');
    child.stdin.as_mut().expect("stdin borusu").write_all(&header).await?;
    Ok(child)
}

/// Yardımcının tek satırlık JSON yanıtını çözer.
fn parse_response(line: &str) -> AppResult<Value> {
    let mut value: Value =
        serde_json::from_str(line.trim()).map_err(|_| AppError::internal("dosya yardımcısı beklenmedik biçimde sonlandı"))?;
    if value["ok"].as_bool() == Some(true) {
        Ok(value["data"].take())
    } else {
        Err(AppError::bad(value["error"].as_str().unwrap_or("Dosya işlemi başarısız oldu").to_string()))
    }
}

async fn finish(mut child: Child) -> AppResult<Value> {
    drop(child.stdin.take());
    let stdout = child.stdout.take().expect("stdout borusu");
    let mut output = String::new();
    let read = async {
        stdout.take(MAX_RESPONSE_BYTES).read_to_string(&mut output).await?;
        child.wait().await
    };
    tokio::time::timeout(OP_TIMEOUT, read).await.map_err(|_| AppError::bad("Dosya işlemi zaman aşımına uğradı"))??;
    parse_response(&output)
}

#[derive(Deserialize)]
struct OpBody {
    scope: String,
    request: Request,
}

/// Denetim günlüğü için işlemin adı ve etkilediği yol; okuma işlemleri kaydedilmez.
fn audit_entry(request: &Request) -> Option<(&'static str, String)> {
    Some(match request {
        Request::List { .. } | Request::Read { .. } | Request::Download { .. } => return None,
        Request::Write { path, .. } => ("file.write", path.clone()),
        Request::Mkdir { path } => ("file.mkdir", path.clone()),
        Request::Rename { from, to } => ("file.rename", format!("{from} → {to}")),
        Request::Move { paths, dest } => ("file.move", format!("{} → {dest}", paths.join(", "))),
        Request::Copy { paths, dest } => ("file.copy", format!("{} → {dest}", paths.join(", "))),
        Request::Delete { paths } => ("file.delete", paths.join(", ")),
        Request::Chmod { paths, mode, .. } => ("file.chmod", format!("{} ({mode:o})", paths.join(", "))),
        Request::Compress { dest, .. } => ("file.compress", dest.clone()),
        Request::Extract { path, dest } => ("file.extract", format!("{path} → {dest}")),
        Request::Upload { dir, name, .. } => ("file.upload", format!("{dir}/{name}")),
    })
}

fn record(state: &AppState, user: &AuthUser, scope: &Scope, entry: Option<(&'static str, String)>) {
    if let Some((action, target)) = entry {
        // Denetim kaydı çok uzun yol listeleriyle şişmesin.
        let target: String = target.chars().take(500).collect();
        auth::audit(&state.db, user, action, &target, &scope.label);
    }
}

async fn op(State(state): State<AppState>, user: AuthUser, Json(body): Json<OpBody>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    if matches!(body.request, Request::Download { .. } | Request::Upload { .. }) {
        return Err(AppError::bad("Bu işlem için indirme ya da yükleme ucunu kullanın"));
    }
    let mut scope = resolve_scope(&state, &user, &body.scope).await?;
    // Arşiv açma, hesabın kalan disk kotasıyla sınırlanır; değeri istek değil panel verir.
    if let (Request::Extract { .. }, Some(account)) = (&body.request, scope.account_id) {
        scope.disk_budget = crate::quota::remaining_disk(&state.db, account)?;
    }
    let entry = audit_entry(&body.request);
    let data = finish(spawn_helper(&scope, body.request).await?).await?;
    record(&state, &user, &scope, entry);
    Ok(Json(data))
}

#[derive(Deserialize)]
struct DownloadQuery {
    scope: String,
    path: String,
}

/// ASCII dışı adları da doğru taşıyan `Content-Disposition` değeri (RFC 5987).
fn content_disposition(name: &str) -> String {
    let fallback: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || "._- ".contains(c) { c } else { '_' }).collect();
    let mut encoded = String::new();
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || b"._-".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}

async fn download(State(state): State<AppState>, user: AuthUser, Query(query): Query<DownloadQuery>) -> AppResult<Response> {
    user.require(AREA)?;
    let scope = resolve_scope(&state, &user, &query.scope).await?;
    let mut child = spawn_helper(&scope, Request::Download { path: query.path }).await?;
    drop(child.stdin.take());
    let mut reader = BufReader::new(child.stdout.take().expect("stdout borusu"));
    let mut header = String::new();
    reader.read_line(&mut header).await?;
    let info = parse_response(&header)?;

    // Süreç, akış bitene (ya da istemci bağlantıyı kesene) kadar yaşar.
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    let name = info["name"].as_str().unwrap_or("indir");
    let mut response = (
        [(CONTENT_TYPE, "application/octet-stream".to_string()), (CONTENT_DISPOSITION, content_disposition(name))],
        Body::from_stream(ReaderStream::new(reader)),
    )
        .into_response();
    if let Some(size) = info["size"].as_u64() {
        response.headers_mut().insert(CONTENT_LENGTH, size.into());
    }
    Ok(response)
}

#[derive(Deserialize)]
struct UploadQuery {
    scope: String,
    /// Dosyanın yükleneceği klasör.
    dir: String,
    #[serde(default)]
    overwrite: bool,
}

/// Dosyayı belleğe almadan, geldiği gibi yardımcı sürece akıtır.
async fn upload(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<UploadQuery>,
    mut multipart: Multipart,
) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let scope = resolve_scope(&state, &user, &query.scope).await?;
    let read_error = |err: axum::extract::multipart::MultipartError| AppError::bad(format!("Yükleme okunamadı: {err}"));

    while let Some(mut field) = multipart.next_field().await.map_err(read_error)? {
        if field.name() != Some("file") {
            continue;
        }
        // Tarayıcının gönderdiği addan yalnızca son parça alınır.
        let name = field
            .file_name()
            .and_then(|name| name.rsplit(['/', '\\']).next())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| AppError::bad("Dosya adı eksik"))?
            .to_string();
        let request = Request::Upload { dir: query.dir.clone(), name: name.clone(), overwrite: query.overwrite };
        let entry = audit_entry(&request);
        let mut child = spawn_helper(&scope, request).await?;
        let mut stdin = child.stdin.take().expect("stdin borusu");
        while let Some(chunk) = field.chunk().await.map_err(read_error)? {
            // Yardımcı erken çıktıysa (ör. dosya zaten var) yazma hata verir; asıl neden yanıtında yazar.
            if stdin.write_all(&chunk).await.is_err() {
                break;
            }
        }
        drop(stdin);
        let data = finish(child).await?;
        record(&state, &user, &scope, entry);
        return Ok(Json(data));
    }
    Err(AppError::bad("Dosya seçilmedi"))
}
