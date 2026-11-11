//! Git ile yayınlama: depodan çekme, yayın betiğini çalıştırma, geçmiş ve bildirim.
//!
//! Site klasörü deponun çalışma kopyasıdır. Her yayında uzak dal çekilir,
//! çalışma kopyası o sürüme getirilir (izlenmeyen dosyalar — yüklemeler, `.env` —
//! yerinde kalır) ve hesabın tanımladığı betik çalıştırılır. Bütün adımlar site
//! sahibinin Linux kullanıcısıyla yürür.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::auth;
use crate::db::{now, Db};
use crate::error::{AppError, AppResult};
use crate::hosting::sites::{self, Site};
use crate::hosting::sysuser::{self, SysUser};
use crate::jobs::JobCtx;
use crate::notify;
use crate::state::AppState;
use crate::sys::{exec, runas};

const DEPLOY_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const SHIM_DIR: &str = "/var/lib/pixpanel/shims";
const MAX_SCRIPT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct GitConfig {
    pub site_id: i64,
    pub repo_url: String,
    pub branch: String,
    pub auto_deploy: bool,
    pub script: String,
    pub webhook_token: String,
    pub public_key: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Deployment {
    pub id: i64,
    pub site_id: i64,
    pub source: String,
    pub commit_hash: String,
    pub commit_subject: String,
    pub commit_author: String,
    pub status: String,
    pub job_id: Option<i64>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

// ---- Doğrulama ----

/// Depo adresi: `https://sunucu/yol`, `ssh://kullanıcı@sunucu/yol` ya da
/// `kullanıcı@sunucu:yol`. Adrese gömülü parola, yerel yol ve git'in komut
/// çalıştırabilen taşıyıcıları (`ext::` vb.) kabul edilmez.
pub fn validate_repo_url(value: &str) -> AppResult<String> {
    let url = value.trim();
    let invalid = || AppError::bad("Geçersiz depo adresi (örnek: git@github.com:kullanici/depo.git ya da https://github.com/kullanici/depo.git)");
    let charset_ok = !url.is_empty()
        && url.len() <= 300
        && !url.starts_with('-')
        && url.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.:/@~-".contains(&b));
    if !charset_ok || url.contains("..") {
        return Err(invalid());
    }
    if url.contains("://") {
        let parsed = url::Url::parse(url).map_err(|_| invalid())?;
        let web = matches!(parsed.scheme(), "https" | "http") && parsed.username().is_empty();
        let ssh = parsed.scheme() == "ssh";
        let plain = parsed.host_str().is_some()
            && parsed.password().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none()
            && parsed.path().len() > 1;
        if (web || ssh) && plain { Ok(url.to_string()) } else { Err(invalid()) }
    } else {
        // scp biçimi: kullanıcı@sunucu:yol
        let shaped = url.split_once('@').is_some_and(|(user, rest)| {
            !user.is_empty()
                && user.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                && rest.split_once(':').is_some_and(|(host, path)| {
                    !host.is_empty() && !host.contains('/') && !host.contains('@') && !path.is_empty() && !path.contains(':') && !path.contains('@')
                })
        });
        if shaped { Ok(url.to_string()) } else { Err(invalid()) }
    }
}

pub fn validate_branch(value: &str) -> AppResult<String> {
    let branch = value.trim();
    let valid = !branch.is_empty()
        && branch.len() <= 100
        && !branch.starts_with(['-', '/', '.'])
        && !branch.ends_with(['/', '.'])
        && !branch.ends_with(".lock")
        && !branch.contains("..")
        && !branch.contains("//")
        && branch.bytes().all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b));
    if valid { Ok(branch.to_string()) } else { Err(AppError::bad("Geçersiz dal adı")) }
}

pub fn validate_script(value: &str) -> AppResult<String> {
    // Windows satır sonları bash'te komutları bozar.
    let script = value.replace("\r\n", "\n");
    if script.len() > MAX_SCRIPT_BYTES {
        return Err(AppError::bad("Yayın betiği 16 KB'ı aşamaz"));
    }
    if script.contains('\0') {
        return Err(AppError::bad("Yayın betiği geçersiz karakter içeriyor"));
    }
    Ok(script.trim().to_string())
}

pub fn validate_commit(value: &str) -> AppResult<String> {
    let valid = (7..=64).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_hexdigit());
    if valid { Ok(value.to_ascii_lowercase()) } else { Err(AppError::bad("Geçersiz sürüm kimliği")) }
}

// ---- Kayıtlar ----

fn read_config(row: &rusqlite::Row<'_>) -> rusqlite::Result<GitConfig> {
    Ok(GitConfig {
        site_id: row.get(0)?,
        repo_url: row.get(1)?,
        branch: row.get(2)?,
        auto_deploy: row.get(3)?,
        script: row.get(4)?,
        webhook_token: row.get(5)?,
        public_key: row.get(6)?,
        created_at: row.get(7)?,
    })
}

const CONFIG_COLUMNS: &str = "site_id, repo_url, branch, auto_deploy, script, webhook_token, public_key, created_at";

pub fn load_config(db: &Db, site_id: i64) -> AppResult<Option<GitConfig>> {
    db.call(|conn| {
        conn.query_row(&format!("SELECT {CONFIG_COLUMNS} FROM site_git WHERE site_id = ?1"), params![site_id], read_config)
            .optional()
    })
}

pub fn config_by_token(db: &Db, token: &str) -> AppResult<Option<GitConfig>> {
    db.call(|conn| {
        conn.query_row(&format!("SELECT {CONFIG_COLUMNS} FROM site_git WHERE webhook_token = ?1"), params![token], read_config)
            .optional()
    })
}

pub fn save_config(db: &Db, config: &GitConfig) -> AppResult<()> {
    db.call(|conn| {
        conn.execute(
            "INSERT INTO site_git (site_id, repo_url, branch, auto_deploy, script, webhook_token, public_key, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(site_id) DO UPDATE SET repo_url = excluded.repo_url, branch = excluded.branch,
                 auto_deploy = excluded.auto_deploy, script = excluded.script,
                 webhook_token = excluded.webhook_token, public_key = excluded.public_key",
            params![
                config.site_id, config.repo_url, config.branch, config.auto_deploy, config.script,
                config.webhook_token, config.public_key, config.created_at
            ],
        )
    })?;
    Ok(())
}

pub fn delete_config(db: &Db, site_id: i64) -> AppResult<()> {
    db.call(|conn| conn.execute("DELETE FROM site_git WHERE site_id = ?1", params![site_id]))?;
    Ok(())
}

fn read_deployment(row: &rusqlite::Row<'_>) -> rusqlite::Result<Deployment> {
    Ok(Deployment {
        id: row.get(0)?,
        site_id: row.get(1)?,
        source: row.get(2)?,
        commit_hash: row.get(3)?,
        commit_subject: row.get(4)?,
        commit_author: row.get(5)?,
        status: row.get(6)?,
        job_id: row.get(7)?,
        started_at: row.get(8)?,
        finished_at: row.get(9)?,
    })
}

const DEPLOYMENT_COLUMNS: &str =
    "id, site_id, source, commit_hash, commit_subject, commit_author, status, job_id, started_at, finished_at";

pub fn list_deployments(db: &Db, site_id: i64, limit: i64) -> AppResult<Vec<Deployment>> {
    db.call(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {DEPLOYMENT_COLUMNS} FROM deployments WHERE site_id = ?1 ORDER BY id DESC LIMIT ?2"
        ))?;
        stmt.query_map(params![site_id, limit], read_deployment)?.collect()
    })
}

pub fn get_deployment(db: &Db, site_id: i64, id: i64) -> AppResult<Deployment> {
    db.call(|conn| {
        conn.query_row(
            &format!("SELECT {DEPLOYMENT_COLUMNS} FROM deployments WHERE id = ?1 AND site_id = ?2"),
            params![id, site_id],
            read_deployment,
        )
        .optional()
    })?
    .ok_or(AppError::NotFound)
}

/// Panel yeniden başladıysa yarıda kalan yayınlar artık çalışmıyordur.
pub fn mark_interrupted(db: &Db) -> AppResult<()> {
    db.call(|conn| {
        conn.execute("UPDATE deployments SET status = 'failed', finished_at = ?1 WHERE status = 'running'", params![now()])
    })?;
    Ok(())
}

// ---- Yayın anahtarı ----

fn key_path(user: &SysUser, site_id: i64) -> PathBuf {
    user.home.join(".ssh").join(format!("pixpanel-deploy-{site_id}"))
}

/// Sitenin özel depolara erişmek için kullandığı SSH anahtarını (yoksa üretip)
/// açık anahtarını döner. Anahtar hesabın `.ssh` klasöründe durur ve hesabın
/// kendi kimliğiyle üretilir.
pub async fn ensure_deploy_key(site: &Site) -> AppResult<String> {
    let user = sysuser::ensure(&site.owner).await?;
    let key = key_path(&user, site.id);
    let key_str = key.to_string_lossy().into_owned();
    let read_public = || runas::output(runas::command(&user, &user.home, "ssh-keygen", &["-y", "-f", &key_str]), Duration::from_secs(20));

    let existing = read_public().await?;
    if existing.success() {
        return Ok(existing.stdout.trim().to_string());
    }
    let comment = format!("pixpanel-{}", site.domain);
    let generated = runas::output(
        runas::command(&user, &user.home, "ssh-keygen", &["-q", "-t", "ed25519", "-N", "", "-C", &comment, "-f", &key_str]),
        Duration::from_secs(20),
    )
    .await?;
    if !generated.success() {
        return Err(AppError::internal(format!("Yayın anahtarı üretilemedi: {}", generated.stderr.trim())));
    }
    let public = read_public().await?;
    if !public.success() {
        return Err(AppError::internal("Yayın anahtarı okunamadı"));
    }
    Ok(format!("{} {comment}", public.stdout.trim()))
}

pub async fn remove_deploy_key(site: &Site) {
    if let Some(user) = sysuser::lookup(&site.owner) {
        let key = key_path(&user, site.id).to_string_lossy().into_owned();
        let public = format!("{key}.pub");
        let _ = runas::output(runas::command(&user, &user.home, "rm", &["-f", "--", &key, &public]), Duration::from_secs(10)).await;
    }
}

// ---- Yayın ----

/// Site başına yayın durumu: aynı anda tek yayın yürür; o sırada gelen
/// istekler tek bir "sıradaki" yayına indirgenir.
static RUNNING: LazyLock<Mutex<HashMap<i64, bool>>> = LazyLock::new(Mutex::default);

fn running() -> std::sync::MutexGuard<'static, HashMap<i64, bool>> {
    RUNNING.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn is_running(site_id: i64) -> bool {
    running().contains_key(&site_id)
}

pub enum Started {
    /// Yayın başladı; kimliği.
    Now(i64),
    /// Süren bir yayın var; bittiğinde yenisi kendiliğinden başlayacak.
    Queued,
}

/// Yayını arka plan görevi olarak başlatır.
///
/// `commit` verilirse dalın ucu yerine o sürüm yayınlanır (geri dönüş).
/// `actor` görevi başlatan panel kullanıcısıdır; web kancasında site sahibidir.
pub fn start(state: &AppState, site: &Site, source: &'static str, commit: Option<String>, actor: i64) -> AppResult<Started> {
    let config = load_config(&state.db, site.id)?.ok_or_else(|| AppError::bad("Bu site için Git ile yayınlama tanımlı değil"))?;
    {
        let mut running = running();
        if let Some(pending) = running.get_mut(&site.id) {
            // Elle seçilen bir sürüm sıraya alınmaz; kullanıcı bittiğinde yeniden dener.
            if commit.is_some() {
                return Err(AppError::conflict("Süren bir yayın var; bitmesini bekleyin"));
            }
            *pending = true;
            return Ok(Started::Queued);
        }
        running.insert(site.id, false);
    }

    let started = start_job(state, site, &config, source, commit, actor);
    if started.is_err() {
        running().remove(&site.id);
    }
    started.map(Started::Now)
}

fn start_job(
    state: &AppState,
    site: &Site,
    config: &GitConfig,
    source: &'static str,
    commit: Option<String>,
    actor: i64,
) -> AppResult<i64> {
    let deployment_id = state.db.call(|conn| {
        conn.execute(
            "INSERT INTO deployments (site_id, source, status, started_at) VALUES (?1, ?2, 'running', ?3)",
            params![site.id, source, now()],
        )?;
        Ok(conn.last_insert_rowid())
    })?;

    let title = format!("Yayınlama: {}", site.domain);
    let (job_state, job_site, job_config) = (state.clone(), site.clone(), config.clone());
    let job_id = state.jobs.start("deploy", &title, Some(actor), move |ctx| async move {
        let outcome = match tokio::time::timeout(DEPLOY_TIMEOUT, run(&ctx, &job_state, &job_site, &job_config, deployment_id, commit)).await {
            Ok(outcome) => outcome,
            Err(_) => Err(AppError::internal("Yayınlama 30 dakikada bitmedi ve durduruldu")),
        };
        finish(&ctx, &job_state, &job_site, &job_config, deployment_id, &outcome, actor).await;
        outcome
    });
    match job_id {
        Ok(job_id) => {
            state.db.call(|conn| conn.execute("UPDATE deployments SET job_id = ?1 WHERE id = ?2", params![job_id, deployment_id]))?;
            Ok(deployment_id)
        }
        Err(err) => {
            state.db.call(|conn| {
                conn.execute("UPDATE deployments SET status = 'failed', finished_at = ?1 WHERE id = ?2", params![now(), deployment_id])
            })?;
            Err(err)
        }
    }
}

/// Yayın bittiğinde kaydı kapatır, bildirimi gönderir ve sırada bekleyen varsa başlatır.
async fn finish(
    ctx: &JobCtx,
    state: &AppState,
    site: &Site,
    config: &GitConfig,
    deployment_id: i64,
    outcome: &AppResult<()>,
    actor: i64,
) {
    let status = if outcome.is_ok() { "success" } else { "failed" };
    let saved = state.db.call(|conn| {
        conn.execute("UPDATE deployments SET status = ?1, finished_at = ?2 WHERE id = ?3", params![status, now(), deployment_id])
    });
    if let Err(err) = saved {
        tracing::error!(error = %err, "yayın sonucu kaydedilemedi");
    }

    let deployment = get_deployment(&state.db, site.id, deployment_id).ok();
    let commit = deployment
        .as_ref()
        .filter(|d| !d.commit_hash.is_empty())
        .map(|d| format!("{} {}", &d.commit_hash[..d.commit_hash.len().min(8)], d.commit_subject))
        .unwrap_or_else(|| "(sürüm alınamadı)".into());
    match outcome {
        Ok(()) => {
            let body = format!("Site: {}\nDal: {}\nSürüm: {commit}", site.domain, config.branch);
            notify::send_event(state, site.user_id, "deploy_success", &format!("{} yayınlandı", site.domain), &body).await;
        }
        Err(err) => {
            let body = format!(
                "Site: {}\nDal: {}\nSürüm: {commit}\nHata: {err}\n\nGünlüğün son satırları:\n{}",
                site.domain,
                config.branch,
                ctx.tail(20)
            );
            notify::send_event(state, site.user_id, "deploy_failed", &format!("{} yayınlanamadı", site.domain), &body).await;
        }
    }

    let pending = running().remove(&site.id).unwrap_or(false);
    if pending {
        // Yayın sürerken yeni gönderim geldi; ayarlar değişmiş olabileceğinden yeniden okunur.
        match sites::load(&state.db, site.id) {
            Ok(Some(current)) => {
                if let Err(err) = start(state, &current, "webhook", None, actor) {
                    tracing::warn!(error = %err, domain = %site.domain, "sıradaki yayın başlatılamadı");
                }
            }
            _ => tracing::warn!(domain = %site.domain, "sıradaki yayın için site bulunamadı"),
        }
    }
}

/// Betiklerde `php` komutunun sitenin PHP sürümünü çalıştırması için `PATH`
/// başına eklenen klasörü hazırlar.
fn php_shim(version: &str) -> Option<String> {
    let binary = PathBuf::from(format!("/usr/bin/php{version}"));
    if !binary.is_file() {
        return None;
    }
    let dir = Path::new(SHIM_DIR).join(format!("php{version}"));
    let link = dir.join("php");
    if !link.exists() {
        std::fs::create_dir_all(&dir).ok()?;
        std::os::unix::fs::symlink(&binary, &link).ok()?;
    }
    Some(dir.to_string_lossy().into_owned())
}

struct Runner<'a> {
    ctx: &'a JobCtx,
    user: SysUser,
    root: PathBuf,
    env: Vec<(String, String)>,
}

impl Runner<'_> {
    fn command(&self, program: &str, args: &[&str]) -> tokio::process::Command {
        let mut cmd = runas::command(&self.user, &self.root, program, args);
        cmd.envs(self.env.iter().map(|(key, value)| (key.as_str(), value.as_str())));
        cmd
    }

    /// git komutunu çalıştırıp çıktısını görev günlüğüne aktarır.
    async fn git(&self, args: &[&str]) -> AppResult<()> {
        self.ctx.log(format!("$ git {}", args.join(" ")));
        self.ctx.run_streaming(&mut self.command("git", args), "git", None).await
    }

    /// git komutunun çıktısını döner; başarısızsa `None`.
    async fn git_output(&self, args: &[&str]) -> AppResult<Option<String>> {
        let output = runas::output(self.command("git", args), Duration::from_secs(60)).await?;
        Ok(output.success().then(|| output.stdout.trim().to_string()))
    }
}

async fn run(
    ctx: &JobCtx,
    state: &AppState,
    site: &Site,
    config: &GitConfig,
    deployment_id: i64,
    commit: Option<String>,
) -> AppResult<()> {
    if !exec::binary_exists("git") {
        ctx.log("git kurulu değil, kuruluyor");
        let _guard = state.apt_lock.lock().await;
        crate::components::apt_install(ctx, &["git".into(), "openssh-client".into()]).await?;
    }
    let user = sysuser::lookup(&site.owner).ok_or_else(|| AppError::internal("Hesabın Linux kullanıcısı bulunamadı"))?;
    let root = site.root();
    if !root.is_dir() {
        return Err(AppError::internal("Site klasörü bulunamadı"));
    }

    let key = key_path(&user, site.id).to_string_lossy().into_owned();
    let mut path = runas::USER_PATH.to_string();
    if let Some(shim) = site.php_version.as_deref().and_then(php_shim) {
        path = format!("{shim}:{path}");
    }
    if site.kind == "node" {
        // Betikteki `node` ve `npm` sitenin Node.js sürümünü kullanır.
        path = format!("{}:{path}", crate::hosting::node::bin_dir(&site.node.version).display());
    }
    if site.kind == "python" {
        // Betikteki `python` ve `pip` sitenin sanal ortamını kullanır.
        path = format!("{}:{path}", site.root().join(crate::hosting::node::VENV_DIR).join("bin").display());
    }
    let runner = Runner {
        ctx,
        user,
        root: root.clone(),
        env: vec![
            ("PATH".into(), path),
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            (
                "GIT_SSH_COMMAND".into(),
                format!("ssh -i {key} -o IdentitiesOnly=yes -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=20"),
            ),
            ("PIXPANEL_DOMAIN".into(), site.domain.clone()),
            ("PIXPANEL_BRANCH".into(), config.branch.clone()),
        ],
    };

    ctx.log(format!("{} deposundan {} dalı yayınlanıyor", config.repo_url, config.branch));
    if !root.join(".git").exists() {
        runner.git(&["init", "-q"]).await?;
    }
    if runner.git_output(&["remote", "get-url", "origin"]).await?.is_some() {
        runner.git(&["remote", "set-url", "origin", &config.repo_url]).await?;
    } else {
        runner.git(&["remote", "add", "origin", &config.repo_url]).await?;
    }
    let previous = runner.git_output(&["rev-parse", "--verify", "-q", "HEAD"]).await?.unwrap_or_default();

    let remote_ref = format!("refs/remotes/origin/{}", config.branch);
    let refspec = format!("+refs/heads/{}:{remote_ref}", config.branch);
    runner.git(&["fetch", "--prune", "origin", &refspec]).await?;

    let target = commit.unwrap_or(remote_ref);
    // `-f`: depoda izlenen dosyalar depodaki hâline döner; izlenmeyenlere dokunulmaz.
    runner.git(&["checkout", "-q", "-f", "-B", &config.branch, &target]).await?;
    if root.join(".gitmodules").exists() {
        runner.git(&["submodule", "update", "--init", "--recursive"]).await?;
    }

    let info = runner.git_output(&["log", "-1", "--format=%H%x1f%s%x1f%an"]).await?.unwrap_or_default();
    let mut fields = info.split('\x1f');
    let (hash, subject, author) = (
        fields.next().unwrap_or_default().to_string(),
        fields.next().unwrap_or_default().chars().take(200).collect::<String>(),
        fields.next().unwrap_or_default().chars().take(100).collect::<String>(),
    );
    state.db.call(|conn| {
        conn.execute(
            "UPDATE deployments SET commit_hash = ?1, commit_subject = ?2, commit_author = ?3 WHERE id = ?4",
            params![hash, subject, author, deployment_id],
        )
    })?;
    ctx.log(format!("Sürüm: {} {subject}", &hash[..hash.len().min(8)]));

    if config.script.is_empty() {
        ctx.log("Yayın betiği tanımlı değil; dosyalar güncellendi");
        return restart_node(ctx, site).await;
    }
    ctx.log("Yayın betiği çalıştırılıyor");
    // Betik diske yazılmaz, bash'e standart girdiden verilir. `-e`: bir komut
    // başarısız olursa yayın orada durur ve başarısız sayılır.
    let mut script = runner.command("bash", &["-eo", "pipefail", "-s"]);
    script.env("PIXPANEL_COMMIT", &hash).env("PIXPANEL_PREVIOUS_COMMIT", &previous);
    ctx.run_streaming(&mut script, "yayın betiği", Some(config.script.clone().into_bytes())).await?;
    restart_node(ctx, site).await?;
    ctx.log("Yayınlama tamamlandı");
    Ok(())
}

/// Node.js sitelerinde yeni kodun devreye girmesi için uygulama yeniden başlatılır.
async fn restart_node(ctx: &JobCtx, site: &Site) -> AppResult<()> {
    if crate::hosting::node::is_app(&site.kind) && site.status == "active" {
        ctx.log("Uygulama yeniden başlatılıyor");
        crate::hosting::node::action(site.id, "restart").await?;
    }
    Ok(())
}

/// Yeni bir web kancası anahtarı üretir.
pub fn new_webhook_token() -> String {
    auth::random_token(24)
}
