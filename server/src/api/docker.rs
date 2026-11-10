//! Docker yönetimi: konteynerler, imajlar, birimler, ağlar ve compose yığınları.
//!
//! Docker'a erişim sunucuda root yetkisi demektir; bu yüzden bütün uçlar
//! yalnızca yöneticilere açıktır. Docker ile `docker` komutu üzerinden,
//! argümanlar ayrı ayrı verilerek konuşulur.

use std::path::{Path as FsPath, PathBuf};
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::hosting::atomic_write;
use crate::state::AppState;
use crate::sys::{exec, services};

const STACKS_DIR: &str = "/opt/pixpanel/stacks";
const MAX_COMPOSE_BYTES: usize = 256 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(status))
        .route("/containers", get(list_containers).post(create_container))
        .route("/containers/stats", get(container_stats))
        .route("/containers/{id}", get(inspect_container).delete(remove_container))
        .route("/containers/{id}/logs", get(container_logs))
        .route("/containers/{id}/{action}", post(container_action))
        .route("/images", get(list_images))
        .route("/images/pull", post(pull_image))
        .route("/images/prune", post(prune_images))
        .route("/images/{id}", delete(remove_image))
        .route("/volumes", get(list_volumes).post(create_volume))
        .route("/volumes/{name}", delete(remove_volume))
        .route("/networks", get(list_networks).post(create_network))
        .route("/networks/{name}", delete(remove_network))
        .route("/stacks", get(list_stacks).post(create_stack))
        .route("/stacks/{name}", get(show_stack).put(save_stack).delete(remove_stack))
        .route("/stacks/{name}/{action}", post(stack_action))
}

// ---- Yardımcılar ----

fn ensure_installed() -> AppResult<()> {
    if exec::binary_exists("docker") {
        Ok(())
    } else {
        Err(AppError::bad("Docker kurulu değil; önce Bileşenler sayfasından kurun"))
    }
}

/// Docker'ın döndürdüğü hata iletisini kullanıcıya olduğu gibi aktarır.
async fn docker(args: &[&str]) -> AppResult<String> {
    ensure_installed()?;
    let output = exec::run_with_timeout("docker", args, Duration::from_secs(60)).await?;
    if output.success() {
        return Ok(output.stdout);
    }
    let detail = output.stderr.trim().trim_start_matches("Error response from daemon: ");
    Err(AppError::bad(format!("Docker: {detail}")))
}

/// `--format '{{json .}}'` çıktısını (satır başına bir JSON nesnesi) ayrıştırır.
fn json_lines(raw: &str) -> Vec<Value> {
    raw.lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
}

/// Konteyner, birim, ağ adları ve kimlikleri.
fn validate_name(value: &str) -> AppResult<&str> {
    let mut chars = value.bytes();
    let valid = value.len() <= 128
        && chars.next().is_some_and(|b| b.is_ascii_alphanumeric())
        && chars.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
    if valid { Ok(value) } else { Err(AppError::bad("Geçersiz ad ya da kimlik")) }
}

fn validate_image(value: &str) -> AppResult<&str> {
    let value = value.trim();
    let valid = !value.is_empty()
        && value.len() <= 255
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"._/:@-".contains(&b));
    if valid { Ok(value) } else { Err(AppError::bad("Geçersiz imaj adı (örnek: nginx:1.27 ya da ghcr.io/kullanici/uygulama:latest)")) }
}

fn validate_stack_name(value: &str) -> AppResult<&str> {
    let mut chars = value.bytes();
    let valid = value.len() <= 40
        && chars.next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && chars.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'));
    if valid { Ok(value) } else { Err(AppError::bad("Yığın adı küçük harf, rakam, tire ve alt çizgiden oluşmalı")) }
}

// ---- Durum ----

async fn status(user: AuthUser) -> AppResult<Json<Value>> {
    user.require_admin()?;
    if !exec::binary_exists("docker") {
        return Ok(Json(json!({ "installed": false, "running": false })));
    }
    let running = services::is_active("docker.service").await;
    if !running {
        return Ok(Json(json!({ "installed": true, "running": false })));
    }
    let info: Value = serde_json::from_str(&docker(&["info", "--format", "{{json .}}"]).await?).unwrap_or_default();
    let compose = exec::run("docker", &["compose", "version", "--short"]).await.ok().filter(|o| o.success()).map(|o| o.stdout.trim().to_string());
    Ok(Json(json!({
        "installed": true,
        "running": true,
        "version": info["ServerVersion"],
        "compose_version": compose,
        "containers": info["Containers"],
        "containers_running": info["ContainersRunning"],
        "images": info["Images"],
        "storage_driver": info["Driver"],
        "root_dir": info["DockerRootDir"],
    })))
}

// ---- Konteynerler ----

#[derive(Serialize)]
struct PortBinding {
    container_port: String,
    protocol: String,
    host_ip: String,
    host_port: String,
}

#[derive(Serialize)]
struct Container {
    id: String,
    name: String,
    image: String,
    state: String,
    status_text: String,
    created: String,
    started_at: String,
    restart_policy: String,
    /// Compose ile oluşturulduysa ait olduğu yığın.
    stack: Option<String>,
    ports: Vec<PortBinding>,
    mounts: Vec<String>,
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

fn container_from_inspect(item: &Value) -> Container {
    let mut ports = Vec::new();
    if let Some(map) = item["NetworkSettings"]["Ports"].as_object() {
        for (key, bindings) in map {
            let (container_port, protocol) = key.split_once('/').unwrap_or((key, "tcp"));
            for binding in bindings.as_array().into_iter().flatten() {
                let host_ip = text(&binding["HostIp"]);
                // Aynı port IPv4 ve IPv6 için iki kez listelenir; biri yeterli.
                if host_ip == "::" || host_ip == "::1" {
                    continue;
                }
                ports.push(PortBinding {
                    container_port: container_port.to_string(),
                    protocol: protocol.to_string(),
                    host_ip,
                    host_port: text(&binding["HostPort"]),
                });
            }
        }
    }
    ports.sort_by(|a, b| a.host_port.cmp(&b.host_port));
    let mounts = item["Mounts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|mount| {
            let source = if mount["Type"] == "volume" { text(&mount["Name"]) } else { text(&mount["Source"]) };
            format!("{source}:{}", text(&mount["Destination"]))
        })
        .collect();
    let state = &item["State"];
    Container {
        id: text(&item["Id"]),
        name: text(&item["Name"]).trim_start_matches('/').to_string(),
        image: text(&item["Config"]["Image"]),
        state: text(&state["Status"]),
        status_text: match state["Health"]["Status"].as_str() {
            Some(health) => format!("{} ({health})", text(&state["Status"])),
            None => text(&state["Status"]),
        },
        created: text(&item["Created"]),
        started_at: text(&state["StartedAt"]),
        restart_policy: text(&item["HostConfig"]["RestartPolicy"]["Name"]),
        stack: item["Config"]["Labels"]["com.docker.compose.project"].as_str().map(str::to_string),
        ports,
        mounts,
    }
}

async fn list_containers(user: AuthUser) -> AppResult<Json<Vec<Container>>> {
    user.require_admin()?;
    let ids = docker(&["ps", "-aq", "--no-trunc"]).await?;
    let ids: Vec<&str> = ids.lines().filter(|id| validate_name(id).is_ok()).collect();
    if ids.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let mut args = vec!["inspect", "--"];
    args.extend(&ids);
    let inspected: Value = serde_json::from_str(&docker(&args).await?).unwrap_or_default();
    let mut containers: Vec<Container> = inspected.as_array().into_iter().flatten().map(container_from_inspect).collect();
    containers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(containers))
}

async fn container_stats(user: AuthUser) -> AppResult<Json<Vec<Value>>> {
    user.require_admin()?;
    let raw = docker(&["stats", "--no-stream", "--no-trunc", "--format", "{{json .}}"]).await?;
    let stats = json_lines(&raw)
        .into_iter()
        .map(|row| {
            json!({
                "id": row["ID"], "cpu": row["CPUPerc"], "memory": row["MemUsage"],
                "memory_percent": row["MemPerc"], "network": row["NetIO"], "block": row["BlockIO"],
            })
        })
        .collect();
    Ok(Json(stats))
}

async fn inspect_container(user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let raw = docker(&["inspect", "--", validate_name(&id)?]).await?;
    let inspected: Value = serde_json::from_str(&raw).unwrap_or_default();
    let item = inspected.get(0).cloned().ok_or(AppError::NotFound)?;
    Ok(Json(json!({
        "container": container_from_inspect(&item),
        "command": item["Config"]["Cmd"],
        "entrypoint": item["Config"]["Entrypoint"],
        // Ortam değişkenleri parola içerebilir; yalnızca yöneticiler gördüğü için gösterilir.
        "env": item["Config"]["Env"],
        "networks": item["NetworkSettings"]["Networks"].as_object().map(|n| n.keys().cloned().collect::<Vec<_>>()),
        "exit_code": item["State"]["ExitCode"],
        "error": item["State"]["Error"],
    })))
}

#[derive(Deserialize)]
struct LogsQuery {
    #[serde(default = "default_tail")]
    tail: u32,
}

fn default_tail() -> u32 {
    300
}

async fn container_logs(user: AuthUser, Path(id): Path<String>, Query(query): Query<LogsQuery>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    ensure_installed()?;
    let tail = query.tail.clamp(10, 5000).to_string();
    let output = exec::run("docker", &["logs", "--timestamps", "--tail", &tail, "--", validate_name(&id)?]).await?;
    if !output.success() {
        return Err(AppError::bad(format!("Docker: {}", output.stderr.trim())));
    }
    // Uygulamalar günlüğü iki akışa da yazar; zaman damgasına göre birleştirilir.
    let mut lines: Vec<&str> = output.stdout.lines().chain(output.stderr.lines()).collect();
    lines.sort_by_key(|line| line.split(' ').next().unwrap_or_default().to_string());
    Ok(Json(json!({ "lines": lines })))
}

async fn container_action(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, action)): Path<(String, String)>,
) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let id = validate_name(&id)?;
    if !matches!(action.as_str(), "start" | "stop" | "restart" | "pause" | "unpause" | "kill") {
        return Err(AppError::bad("Geçersiz işlem"));
    }
    docker(&[&action, "--", id]).await?;
    auth::audit(&state.db, &user, &format!("docker.container.{action}"), id, "");
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct RemoveContainerQuery {
    #[serde(default)]
    volumes: bool,
}

async fn remove_container(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(query): Query<RemoveContainerQuery>,
) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let id = validate_name(&id)?;
    let mut args = vec!["rm", "-f"];
    if query.volumes {
        args.push("-v");
    }
    args.extend(["--", id]);
    docker(&args).await?;
    auth::audit(&state.db, &user, "docker.container.remove", id, "");
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct PortSpec {
    host_port: u16,
    container_port: u16,
    #[serde(default)]
    udp: bool,
    /// Kapalıysa port yalnızca sunucunun içinden (ör. ters vekil üzerinden) erişilir.
    #[serde(default)]
    public: bool,
}

#[derive(Deserialize)]
struct EnvSpec {
    key: String,
    value: String,
}

#[derive(Deserialize)]
struct VolumeSpec {
    /// Birim adı ya da sunucudaki mutlak yol.
    source: String,
    target: String,
    #[serde(default)]
    read_only: bool,
}

#[derive(Deserialize)]
struct CreateContainer {
    image: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    ports: Vec<PortSpec>,
    #[serde(default)]
    env: Vec<EnvSpec>,
    #[serde(default)]
    volumes: Vec<VolumeSpec>,
    #[serde(default)]
    restart: String,
    /// Bellek sınırı (MB); 0 sınırsız.
    #[serde(default)]
    memory_mb: u32,
    #[serde(default)]
    command: Vec<String>,
}

fn mount_path_ok(path: &str) -> bool {
    path.starts_with('/') && path.len() <= 255 && !path.contains(':') && !path.contains("..") && !path.chars().any(char::is_control)
}

/// `docker run` argümanlarını üretir; her girdi önce doğrulanır.
fn run_arguments(body: &CreateContainer) -> AppResult<Vec<String>> {
    let mut args: Vec<String> = vec!["run".into(), "-d".into()];
    if !body.name.is_empty() {
        args.extend(["--name".into(), validate_name(&body.name)?.to_string()]);
    }
    let restart = if body.restart.is_empty() { "unless-stopped" } else { body.restart.as_str() };
    if !matches!(restart, "no" | "always" | "unless-stopped" | "on-failure") {
        return Err(AppError::bad("Geçersiz yeniden başlatma kuralı"));
    }
    args.extend(["--restart".into(), restart.into()]);
    if body.memory_mb > 0 {
        if body.memory_mb < 8 {
            return Err(AppError::bad("Bellek sınırı en az 8 MB olmalı"));
        }
        args.extend(["--memory".into(), format!("{}m", body.memory_mb)]);
    }
    for port in &body.ports {
        if port.host_port == 0 || port.container_port == 0 {
            return Err(AppError::bad("Port 1-65535 arasında olmalı"));
        }
        let bind = if port.public { "0.0.0.0" } else { "127.0.0.1" };
        let protocol = if port.udp { "udp" } else { "tcp" };
        args.extend(["-p".into(), format!("{bind}:{}:{}/{protocol}", port.host_port, port.container_port)]);
    }
    for variable in &body.env {
        let mut chars = variable.key.bytes();
        let key_ok = variable.key.len() <= 128
            && chars.next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
            && chars.all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if !key_ok {
            return Err(AppError::bad(format!("Geçersiz ortam değişkeni adı: {}", variable.key)));
        }
        if variable.value.len() > 8192 || variable.value.chars().any(|c| c == '\0' || c == '\n' || c == '\r') {
            return Err(AppError::bad(format!("{} için geçersiz değer", variable.key)));
        }
        args.extend(["-e".into(), format!("{}={}", variable.key, variable.value)]);
    }
    for volume in &body.volumes {
        let source_ok = if volume.source.starts_with('/') { mount_path_ok(&volume.source) } else { validate_name(&volume.source).is_ok() };
        if !source_ok || !mount_path_ok(&volume.target) {
            return Err(AppError::bad("Geçersiz birim tanımı (kaynak: birim adı ya da /mutlak/yol, hedef: /mutlak/yol)"));
        }
        let mode = if volume.read_only { ":ro" } else { "" };
        args.extend(["-v".into(), format!("{}:{}{mode}", volume.source, volume.target)]);
    }
    args.extend(["--".into(), validate_image(&body.image)?.to_string()]);
    for part in &body.command {
        if part.len() > 4096 || part.contains('\0') {
            return Err(AppError::bad("Geçersiz komut"));
        }
        args.push(part.clone());
    }
    Ok(args)
}

async fn create_container(State(state): State<AppState>, user: AuthUser, Json(body): Json<CreateContainer>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    ensure_installed()?;
    let args = run_arguments(&body)?;
    let image = body.image.trim().to_string();
    let title = format!("Konteyner oluşturma: {image}");
    // İmajın indirilmesi dakikalar sürebildiğinden arka plan görevi olarak yürür.
    let job_id = state.jobs.start("docker.run", &title, Some(user.id), move |ctx| async move {
        ctx.log(format!("{image} imajı hazırlanıyor"));
        ctx.exec("docker", &["pull", "--", &image]).await?;
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        // Ortam değişkenleri parola içerebildiğinden komut satırı günlüğe yazılmaz.
        ctx.log("Konteyner başlatılıyor");
        ctx.exec_in("/", "docker", &args).await
    })?;
    auth::audit(&state.db, &user, "docker.container.create", body.image.trim(), &body.name);
    Ok(Json(json!({ "job_id": job_id })))
}

// ---- İmajlar ----

async fn list_images(user: AuthUser) -> AppResult<Json<Vec<Value>>> {
    user.require_admin()?;
    let raw = docker(&["images", "--no-trunc", "--format", "{{json .}}"]).await?;
    let images = json_lines(&raw)
        .into_iter()
        .map(|row| {
            json!({
                "id": row["ID"], "repository": row["Repository"], "tag": row["Tag"],
                "size": row["Size"], "created": row["CreatedSince"],
            })
        })
        .collect();
    Ok(Json(images))
}

#[derive(Deserialize)]
struct PullBody {
    image: String,
}

async fn pull_image(State(state): State<AppState>, user: AuthUser, Json(body): Json<PullBody>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    ensure_installed()?;
    let image = validate_image(&body.image)?.to_string();
    let title = format!("İmaj indirme: {image}");
    let job_image = image.clone();
    let job_id = state.jobs.start("docker.pull", &title, Some(user.id), move |ctx| async move {
        ctx.exec("docker", &["pull", "--", &job_image]).await
    })?;
    auth::audit(&state.db, &user, "docker.image.pull", &image, "");
    Ok(Json(json!({ "job_id": job_id })))
}

async fn remove_image(State(state): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    // Kimlik `sha256:...` biçiminde gelir.
    let reference = validate_image(&id)?;
    docker(&["rmi", "--", reference]).await?;
    auth::audit(&state.db, &user, "docker.image.remove", reference, "");
    Ok(Json(json!({ "ok": true })))
}

async fn prune_images(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let output = docker(&["image", "prune", "-f"]).await?;
    auth::audit(&state.db, &user, "docker.image.prune", "", "");
    let summary = output.lines().last().unwrap_or_default().to_string();
    Ok(Json(json!({ "ok": true, "summary": summary })))
}

// ---- Birimler ve ağlar ----

#[derive(Deserialize)]
struct NameBody {
    name: String,
}

async fn list_volumes(user: AuthUser) -> AppResult<Json<Vec<Value>>> {
    user.require_admin()?;
    let raw = docker(&["volume", "ls", "--format", "{{json .}}"]).await?;
    Ok(Json(json_lines(&raw).into_iter().map(|row| json!({ "name": row["Name"], "driver": row["Driver"], "mountpoint": row["Mountpoint"] })).collect()))
}

async fn create_volume(State(state): State<AppState>, user: AuthUser, Json(body): Json<NameBody>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let name = validate_name(&body.name)?;
    docker(&["volume", "create", "--", name]).await?;
    auth::audit(&state.db, &user, "docker.volume.create", name, "");
    Ok(Json(json!({ "ok": true })))
}

async fn remove_volume(State(state): State<AppState>, user: AuthUser, Path(name): Path<String>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let name = validate_name(&name)?;
    docker(&["volume", "rm", "--", name]).await?;
    auth::audit(&state.db, &user, "docker.volume.remove", name, "");
    Ok(Json(json!({ "ok": true })))
}

async fn list_networks(user: AuthUser) -> AppResult<Json<Vec<Value>>> {
    user.require_admin()?;
    let raw = docker(&["network", "ls", "--no-trunc", "--format", "{{json .}}"]).await?;
    Ok(Json(json_lines(&raw).into_iter().map(|row| json!({ "id": row["ID"], "name": row["Name"], "driver": row["Driver"], "scope": row["Scope"] })).collect()))
}

async fn create_network(State(state): State<AppState>, user: AuthUser, Json(body): Json<NameBody>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let name = validate_name(&body.name)?;
    docker(&["network", "create", "--", name]).await?;
    auth::audit(&state.db, &user, "docker.network.create", name, "");
    Ok(Json(json!({ "ok": true })))
}

async fn remove_network(State(state): State<AppState>, user: AuthUser, Path(name): Path<String>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let name = validate_name(&name)?;
    docker(&["network", "rm", "--", name]).await?;
    auth::audit(&state.db, &user, "docker.network.remove", name, "");
    Ok(Json(json!({ "ok": true })))
}

// ---- Compose yığınları ----

fn stack_dir(name: &str) -> PathBuf {
    FsPath::new(STACKS_DIR).join(name)
}

fn compose_file(name: &str) -> PathBuf {
    stack_dir(name).join("compose.yaml")
}

#[derive(Serialize)]
struct Stack {
    name: String,
    /// Docker'ın bildirdiği durum (ör. `running(2)`); hiç başlatılmadıysa boş.
    status: String,
}

async fn list_stacks(user: AuthUser) -> AppResult<Json<Vec<Stack>>> {
    user.require_admin()?;
    let mut names: Vec<String> = std::fs::read_dir(STACKS_DIR)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().join("compose.yaml").is_file())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| validate_stack_name(name).is_ok())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let running: Value = if names.is_empty() || !exec::binary_exists("docker") {
        Value::Null
    } else {
        let output = exec::run("docker", &["compose", "ls", "-a", "--format", "json"]).await?;
        serde_json::from_str(&output.stdout).unwrap_or_default()
    };
    let stacks = names
        .into_iter()
        .map(|name| {
            let status = running
                .as_array()
                .into_iter()
                .flatten()
                .find(|row| row["Name"] == name.as_str())
                .map(|row| text(&row["Status"]))
                .unwrap_or_default();
            Stack { name, status }
        })
        .collect();
    Ok(Json(stacks))
}

#[derive(Deserialize)]
struct StackBody {
    #[serde(default)]
    name: String,
    content: String,
}

/// Compose dosyasını yazar; Docker dosyayı geçersiz bulursa eski hâli geri konur.
async fn write_compose(name: &str, content: &str) -> AppResult<()> {
    if content.len() > MAX_COMPOSE_BYTES || content.contains('\0') {
        return Err(AppError::bad("Compose dosyası çok büyük ya da geçersiz"));
    }
    ensure_installed()?;
    let path = compose_file(name);
    let previous = std::fs::read(&path).ok();
    // Dosya parola içerebildiğinden yalnızca root okuyabilir.
    atomic_write(&path, content.as_bytes(), 0o600)?;
    let path_str = path.to_string_lossy().into_owned();
    let check = exec::run("docker", &["compose", "-f", &path_str, "-p", name, "config", "-q"]).await?;
    if check.success() {
        return Ok(());
    }
    match previous {
        Some(previous) => atomic_write(&path, &previous, 0o600)?,
        None => {
            let _ = std::fs::remove_dir_all(stack_dir(name));
        }
    }
    Err(AppError::bad(format!("Compose dosyası geçersiz: {}", check.stderr.trim())))
}

async fn create_stack(State(state): State<AppState>, user: AuthUser, Json(body): Json<StackBody>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let name = validate_stack_name(&body.name)?;
    if stack_dir(name).exists() {
        return Err(AppError::conflict("Bu adda bir yığın zaten var"));
    }
    write_compose(name, &body.content).await?;
    auth::audit(&state.db, &user, "docker.stack.create", name, "");
    Ok(Json(json!({ "ok": true })))
}

async fn show_stack(user: AuthUser, Path(name): Path<String>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let name = validate_stack_name(&name)?;
    let content = std::fs::read_to_string(compose_file(name)).map_err(|_| AppError::NotFound)?;
    Ok(Json(json!({ "name": name, "content": content })))
}

async fn save_stack(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
    Json(body): Json<StackBody>,
) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let name = validate_stack_name(&name)?;
    if !compose_file(name).is_file() {
        return Err(AppError::NotFound);
    }
    write_compose(name, &body.content).await?;
    auth::audit(&state.db, &user, "docker.stack.update", name, "");
    Ok(Json(json!({ "ok": true })))
}

async fn stack_action(
    State(state): State<AppState>,
    user: AuthUser,
    Path((name, action)): Path<(String, String)>,
) -> AppResult<Json<Value>> {
    user.require_admin()?;
    ensure_installed()?;
    let name = validate_stack_name(&name)?.to_string();
    let file = compose_file(&name);
    if !file.is_file() {
        return Err(AppError::NotFound);
    }
    let (label, steps): (&str, Vec<Vec<&str>>) = match action.as_str() {
        "up" => ("başlatma", vec![vec!["up", "-d", "--remove-orphans"]]),
        "down" => ("durdurma", vec![vec!["down"]]),
        "restart" => ("yeniden başlatma", vec![vec!["restart"]]),
        "update" => ("güncelleme", vec![vec!["pull"], vec!["up", "-d", "--remove-orphans"]]),
        _ => return Err(AppError::bad("Geçersiz işlem")),
    };
    let title = format!("Yığın {label}: {name}");
    let file = file.to_string_lossy().into_owned();
    let job_name = name.clone();
    let steps: Vec<Vec<String>> = steps.into_iter().map(|step| step.into_iter().map(str::to_string).collect()).collect();
    let job_id = state.jobs.start("docker.stack", &title, Some(user.id), move |ctx| async move {
        for step in steps {
            let mut args = vec!["compose", "-f", file.as_str(), "-p", job_name.as_str(), "--progress", "plain"];
            args.extend(step.iter().map(String::as_str));
            ctx.exec("docker", &args).await?;
        }
        Ok(())
    })?;
    auth::audit(&state.db, &user, &format!("docker.stack.{action}"), &name, "");
    Ok(Json(json!({ "job_id": job_id })))
}

async fn remove_stack(State(state): State<AppState>, user: AuthUser, Path(name): Path<String>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let name = validate_stack_name(&name)?;
    let file = compose_file(name);
    if !file.is_file() {
        return Err(AppError::NotFound);
    }
    // Konteynerler durdurulup kaldırılır; birimlerdeki veri korunur.
    let file_str = file.to_string_lossy().into_owned();
    let down = exec::run_with_timeout("docker", &["compose", "-f", &file_str, "-p", name, "down"], Duration::from_secs(180)).await?;
    if !down.success() {
        return Err(AppError::bad(format!("Yığın durdurulamadı: {}", down.stderr.trim())));
    }
    std::fs::remove_dir_all(stack_dir(name))?;
    auth::audit(&state.db, &user, "docker.stack.remove", name, "");
    Ok(Json(json!({ "ok": true })))
}
