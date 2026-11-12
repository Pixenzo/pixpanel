//! Arka plan görevlerinin listelenmesi ve canlı izlenmesi.

use std::convert::Infallible;

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::stream::{self, Stream, StreamExt};
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::jobs::{JobEvent, JobInfo};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list))
        .route("/{id}", get(detail))
        .route("/{id}/stream", get(stream_job))
}

/// Görev türünün ait olduğu yetki alanı. Alan kapsamlı API anahtarları yalnızca kapsamındaki
/// alanların görevlerini görür; eşlenmeyen (sistem düzeyi) görevler kapsamlı anahtara kapalıdır.
fn job_area(kind: &str) -> Option<&'static str> {
    match kind.split('.').next().unwrap_or("") {
        "cron" => Some("cron"),
        "database" => Some("databases"),
        "docker" => Some("docker"),
        "mail" => Some("mail"),
        "ssl" => Some("ssl"),
        "site" | "wordpress" | "python" | "node" | "deploy" => Some("sites"),
        "backup" | "restore" => Some("backups"),
        _ => None,
    }
}

/// Yönetici bütün görevleri, diğerleri yalnızca kendi başlattıklarını görür.
/// Kapsamlı API anahtarı ayrıca yalnızca kapsamındaki alanların görevlerini görür.
fn ensure_visible(user: &AuthUser, job: &JobInfo) -> AppResult<()> {
    if user.token_scope.is_some() && !job_area(&job.kind).is_some_and(|area| user.scope_allows(area)) {
        return Err(AppError::NotFound);
    }
    if user.is_admin() || job.user_id == Some(user.id) { Ok(()) } else { Err(AppError::NotFound) }
}

async fn list(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Vec<JobInfo>>> {
    let jobs = state.jobs.list(200)?;
    Ok(Json(jobs.into_iter().filter(|job| ensure_visible(&user, job).is_ok()).collect()))
}

async fn detail(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    let (info, log) = state.jobs.get(id)?;
    ensure_visible(&user, &info)?;
    Ok(Json(json!({ "job": info, "log": log })))
}

fn to_sse(event: &JobEvent) -> Result<Event, Infallible> {
    Ok(Event::default().data(serde_json::to_string(event).unwrap_or_default()))
}

async fn stream_job(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let (info, _) = state.jobs.get(id)?;
    ensure_visible(&user, &info)?;

    let (log, receiver) = state.jobs.subscribe(id)?;
    // Önce o ana kadar birikmiş günlük gönderilir.
    let backlog: Vec<_> =
        log.lines().map(|line| to_sse(&JobEvent::Log { line: line.to_string() })).collect();

    let tail = match receiver {
        Some(rx) => stream::unfold((rx, false), |(mut rx, finished)| async move {
            if finished {
                return None;
            }
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let done = matches!(event, JobEvent::Done { .. });
                        return Some((to_sse(&event), (rx, done)));
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return None,
                }
            }
        })
        .boxed(),
        None => {
            // Görev çoktan bitmiş; son durum veritabanından okunur.
            let status = state.jobs.get(id)?.0.status;
            stream::once(async move { to_sse(&JobEvent::Done { status }) }).boxed()
        }
    };

    Ok(Sse::new(stream::iter(backlog).chain(tail)).keep_alive(KeepAlive::default()))
}
