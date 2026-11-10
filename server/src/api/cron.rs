//! Zamanlanmış görevler (cron).
//!
//! Görevler panel veritabanında tutulur ve her hesap için
//! `/etc/cron.d/capsule-<hesap>` dosyasına yazılır. Komutlar hesabın kendi
//! Linux kullanıcısıyla çalışır; yöneticiler root adına da görev tanımlayabilir.
//!
//! Buradaki komut metni bilerek kabuk tarafından yorumlanır: cron'un doğası
//! budur ve komut, onu yazan hesabın kendi yetkisiyle çalışır. Dosyanın
//! yapısını bozabilecek satır sonu gibi karakterler ise reddedilir; aksi
//! hâlde bir hesap dosyaya başka bir kullanıcı adına satır ekleyebilirdi.

use std::path::PathBuf;

use axum::extract::{Path, State};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{self, AuthUser};
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::hosting::{atomic_write, sysuser};
use crate::state::AppState;

const AREA: &str = "cron";
const MAX_JOBS_PER_ACCOUNT: i64 = 200;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", patch(update).delete(remove))
        .route("/{id}/run", post(run_now))
}

#[derive(Serialize)]
struct CronJob {
    id: i64,
    /// Görevin çalıştığı Linux kullanıcısı (`root` ya da hesap adı).
    run_as: String,
    schedule: String,
    command: String,
    comment: String,
    enabled: bool,
    created_at: i64,
}

const NAMES: &[&str] = &[
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec", "sun", "mon", "tue", "wed",
    "thu", "fri", "sat",
];
const SHORTCUTS: &[&str] = &["@reboot", "@hourly", "@daily", "@midnight", "@weekly", "@monthly", "@yearly", "@annually"];

/// Zamanlamayı doğrular: ya `@daily` gibi bir kısaltma ya da beş alanlı cron ifadesi.
fn validate_schedule(schedule: &str) -> AppResult<String> {
    let schedule = schedule.split_whitespace().collect::<Vec<_>>().join(" ");
    if SHORTCUTS.contains(&schedule.as_str()) {
        return Ok(schedule);
    }
    let fields: Vec<&str> = schedule.split(' ').collect();
    let field_ok = |field: &&str| {
        !field.is_empty()
            && field.len() <= 64
            && field.split([',', '-', '/']).all(|part| {
                part == "*" || (!part.is_empty() && part.len() <= 2 && part.bytes().all(|b| b.is_ascii_digit())) || NAMES.contains(&part.to_ascii_lowercase().as_str())
            })
    };
    if fields.len() != 5 || !fields.iter().all(field_ok) {
        return Err(AppError::bad("Geçersiz zamanlama. Beş alan bekleniyor: dakika saat gün ay haftanın-günü (örnek: */5 * * * *)"));
    }
    Ok(schedule)
}

fn validate_command(command: &str) -> AppResult<String> {
    let command = command.trim();
    if command.is_empty() || command.len() > 2000 {
        return Err(AppError::bad("Komut 1-2000 karakter olmalı"));
    }
    // Satır sonu, cron dosyasına yeni (ve başka kullanıcı adına) satır eklemek demektir.
    if command.chars().any(char::is_control) {
        return Err(AppError::bad("Komut tek satır olmalı; satır sonu ya da denetim karakteri içeremez"));
    }
    Ok(command.to_string())
}

fn validate_comment(comment: &str) -> AppResult<String> {
    let comment = comment.trim();
    if comment.chars().count() > 120 || comment.chars().any(char::is_control) {
        return Err(AppError::bad("Açıklama en fazla 120 karakter olmalı ve tek satır olmalı"));
    }
    Ok(comment.to_string())
}

/// Görevin hangi Linux kullanıcısıyla çalışacağını belirler.
fn resolve_run_as(state: &AppState, user: &AuthUser, requested: Option<&str>) -> AppResult<String> {
    let owner: String = state
        .db
        .call(|conn| conn.query_row("SELECT username FROM users WHERE id = ?1", params![user.owner_id()], |r| r.get(0)))?;
    match requested.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(owner),
        Some(name) if name == owner => Ok(owner),
        Some("root") if user.is_admin() => Ok("root".into()),
        Some(name) if user.is_admin() => {
            // Yönetici başka bir hosting hesabı adına da görev tanımlayabilir.
            let exists: bool = state.db.call(|conn| {
                conn.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE username = ?1 AND role != 'member')", params![name], |r| r.get(0))
            })?;
            if exists { Ok(name.to_string()) } else { Err(AppError::bad("Hesap bulunamadı")) }
        }
        Some(_) => Err(AppError::Forbidden),
    }
}

/// İsteği yapanın görebileceği görevlerin `run_as` süzgeci: yönetici hepsini görür.
fn visible_run_as(state: &AppState, user: &AuthUser) -> AppResult<Option<String>> {
    if user.is_admin() {
        return Ok(None);
    }
    resolve_run_as(state, user, None).map(Some)
}

async fn list(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Vec<CronJob>>> {
    user.require(AREA)?;
    let scope = visible_run_as(&state, &user)?;
    let jobs = state.db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, run_as, schedule, command, comment, enabled, created_at FROM cron_jobs
             WHERE ?1 IS NULL OR run_as = ?1 ORDER BY run_as, id",
        )?;
        let rows = stmt.query_map(params![scope], read_job)?;
        rows.collect()
    })?;
    Ok(Json(jobs))
}

fn read_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<CronJob> {
    Ok(CronJob {
        id: row.get(0)?,
        run_as: row.get(1)?,
        schedule: row.get(2)?,
        command: row.get(3)?,
        comment: row.get(4)?,
        enabled: row.get(5)?,
        created_at: row.get(6)?,
    })
}

/// Görevi okur ve isteği yapanın ona erişimini denetler.
fn load(state: &AppState, user: &AuthUser, id: i64) -> AppResult<CronJob> {
    let scope = visible_run_as(state, user)?;
    let job = state.db.call(|conn| {
        conn.query_row(
            "SELECT id, run_as, schedule, command, comment, enabled, created_at FROM cron_jobs WHERE id = ?1",
            params![id],
            read_job,
        )
        .optional()
    })?;
    match job {
        Some(job) if scope.as_deref().is_none_or(|run_as| run_as == job.run_as) => Ok(job),
        _ => Err(AppError::NotFound),
    }
}

fn cron_file(run_as: &str) -> PathBuf {
    PathBuf::from(format!("/etc/cron.d/capsule-{run_as}"))
}

/// Hesabın etkin görevlerini cron dosyasına yazar; etkin görev yoksa dosyayı kaldırır.
fn write_cron_file(state: &AppState, run_as: &str) -> AppResult<()> {
    let jobs: Vec<(i64, String, String, String)> = state.db.call(|conn| {
        let mut stmt = conn.prepare("SELECT id, schedule, command, comment FROM cron_jobs WHERE run_as = ?1 AND enabled = 1 ORDER BY id")?;
        let rows = stmt.query_map(params![run_as], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
        rows.collect()
    })?;
    let path = cron_file(run_as);
    if jobs.is_empty() {
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        return Ok(());
    }
    let mut content = String::from(
        "# Capsule tarafından üretildi. Elle yapılan değişiklikler kaybolur.\n\
         SHELL=/bin/sh\n\
         PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin\n\
         MAILTO=\"\"\n",
    );
    for (id, schedule, command, comment) in jobs {
        content.push_str(&format!("# görev {id}{}{comment}\n", if comment.is_empty() { "" } else { ": " }));
        content.push_str(&format!("{schedule} {run_as} {command}\n"));
    }
    // cron, grup ya da herkes tarafından yazılabilen dosyaları yok sayar.
    atomic_write(&path, content.as_bytes(), 0o644)
}

#[derive(Deserialize)]
struct CreateBody {
    schedule: String,
    command: String,
    #[serde(default)]
    comment: String,
    run_as: Option<String>,
}

async fn create(State(state): State<AppState>, user: AuthUser, Json(body): Json<CreateBody>) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let schedule = validate_schedule(&body.schedule)?;
    let command = validate_command(&body.command)?;
    let comment = validate_comment(&body.comment)?;
    let run_as = resolve_run_as(&state, &user, body.run_as.as_deref())?;

    let _guard = state.sys_lock.lock().await;
    if run_as != "root" {
        // Hesabın Linux kullanıcısı henüz yoksa oluşturulur.
        sysuser::ensure(&run_as).await?;
    }
    let count: i64 = state.db.call(|conn| conn.query_row("SELECT COUNT(*) FROM cron_jobs WHERE run_as = ?1", params![run_as], |r| r.get(0)))?;
    if count >= MAX_JOBS_PER_ACCOUNT {
        return Err(AppError::bad("Bu hesap için görev sınırına ulaşıldı"));
    }
    let id = state.db.call(|conn| {
        conn.execute(
            "INSERT INTO cron_jobs (run_as, schedule, command, comment, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![run_as, schedule, command, comment, now()],
        )?;
        Ok(conn.last_insert_rowid())
    })?;
    write_cron_file(&state, &run_as)?;
    auth::audit(&state.db, &user, "cron.create", &command, &run_as);
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
struct UpdateBody {
    schedule: Option<String>,
    command: Option<String>,
    comment: Option<String>,
    enabled: Option<bool>,
}

async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateBody>,
) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let _guard = state.sys_lock.lock().await;
    let job = load(&state, &user, id)?;
    let schedule = body.schedule.as_deref().map(validate_schedule).transpose()?.unwrap_or(job.schedule);
    let command = body.command.as_deref().map(validate_command).transpose()?.unwrap_or(job.command);
    let comment = body.comment.as_deref().map(validate_comment).transpose()?.unwrap_or(job.comment);
    let enabled = body.enabled.unwrap_or(job.enabled);
    state.db.call(|conn| {
        conn.execute(
            "UPDATE cron_jobs SET schedule = ?1, command = ?2, comment = ?3, enabled = ?4 WHERE id = ?5",
            params![schedule, command, comment, enabled, id],
        )
    })?;
    write_cron_file(&state, &job.run_as)?;
    auth::audit(&state.db, &user, "cron.update", &command, &job.run_as);
    Ok(Json(json!({ "ok": true })))
}

async fn remove(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let _guard = state.sys_lock.lock().await;
    let job = load(&state, &user, id)?;
    state.db.call(|conn| conn.execute("DELETE FROM cron_jobs WHERE id = ?1", params![id]))?;
    write_cron_file(&state, &job.run_as)?;
    auth::audit(&state.db, &user, "cron.delete", &job.command, &job.run_as);
    Ok(Json(json!({ "ok": true })))
}

/// Görevi zamanını beklemeden bir kez çalıştırır; çıktısı görev günlüğünde izlenir.
async fn run_now(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let job = load(&state, &user, id)?;
    let account = if job.run_as == "root" {
        None
    } else {
        Some(sysuser::lookup(&job.run_as).ok_or_else(|| AppError::bad("Hesabın Linux kullanıcısı bulunamadı"))?)
    };
    let title = format!("Zamanlanmış görev: {}", job.command.chars().take(60).collect::<String>());
    let command = job.command.clone();
    let job_id = state.jobs.start("cron.run", &title, Some(user.id), move |ctx| async move {
        match &account {
            None => ctx.exec_in("/root", "/bin/sh", &["-c", &command]).await,
            Some(account) => {
                let home = account.home.to_string_lossy().into_owned();
                ctx.exec_in(
                    &home,
                    "setpriv",
                    &[
                        &format!("--reuid={}", account.uid),
                        &format!("--regid={}", account.gid),
                        "--init-groups",
                        "--",
                        "env",
                        &format!("HOME={home}"),
                        &format!("USER={}", account.name),
                        "/bin/sh",
                        "-c",
                        &command,
                    ],
                )
                .await
            }
        }
    })?;
    auth::audit(&state.db, &user, "cron.run", &job.command, &job.run_as);
    Ok(Json(json!({ "job_id": job_id })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_validation() {
        for good in ["* * * * *", "*/5 * * * *", "0 3 * * 1-5", "15,45 */2 1 jan mon", "@daily", "  0   0 * * *  "] {
            assert!(validate_schedule(good).is_ok(), "{good}");
        }
        for bad in ["", "* * * *", "* * * * * *", "*/5 * * * * root rm -rf /", "a b c d e", "@yanlis", "0 0 * * *\n* * * * * root id", "100 * * * *"] {
            assert!(validate_schedule(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn command_rejects_newlines() {
        assert!(validate_command("php /home/u/sites/x/artisan schedule:run").is_ok());
        assert!(validate_command("echo a\n* * * * * root id").is_err());
        assert!(validate_command("echo a\r").is_err());
        assert!(validate_command("   ").is_err());
    }
}
