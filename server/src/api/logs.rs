//! Günlük tarayıcı: site erişim/hata günlükleri ve (yöneticiler için) sistem günlüğü.

use std::io::{Read, Seek, SeekFrom};

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::hosting::{nginx, sites};
use crate::state::AppState;
use crate::sys::{exec, services};

const AREA: &str = "logs";
/// Dosyanın sonundan en fazla bu kadar bayt okunur.
const TAIL_BYTES: u64 = 1024 * 1024;
/// Süzme yapılırken sistem günlüğünden geriye doğru taranan satır sayısı.
const JOURNAL_SCAN_LINES: usize = 5000;

pub fn router() -> Router<AppState> {
    Router::new().route("/sources", get(sources)).route("/read", get(read))
}

#[derive(Serialize)]
struct Source {
    id: String,
    label: String,
    /// Arayüzde gruplanması için: `site` ya da `system`.
    group: &'static str,
}

/// Yöneticiye hazır sunulan servis günlükleri; kurulu olmayanlar listelenmez.
const SYSTEM_UNITS: &[(&str, &str)] = &[
    ("pixpanel.service", "Panel"),
    ("nginx.service", "nginx"),
    ("mariadb.service", "MariaDB"),
    ("postgresql.service", "PostgreSQL"),
    ("redis-server.service", "Redis"),
    ("docker.service", "Docker"),
    ("ssh.service", "SSH"),
    ("cron.service", "Cron"),
    ("fail2ban.service", "Fail2ban"),
];

async fn sources(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Vec<Source>>> {
    user.require(AREA)?;
    let mut list = Vec::new();
    for site in sites::list(&state.db, &user)? {
        list.push(Source { id: format!("site:{}:access", site.id), label: format!("{} erişim", site.domain), group: "site" });
        list.push(Source { id: format!("site:{}:error", site.id), label: format!("{} hata", site.domain), group: "site" });
        if site.kind == "node" {
            list.push(Source { id: format!("site:{}:app", site.id), label: format!("{} uygulama", site.domain), group: "site" });
        }
    }
    if user.is_admin() {
        list.push(Source { id: "journal".into(), label: "Bütün sistem günlüğü".into(), group: "system" });
        let installed = services::list().await.unwrap_or_default();
        for (unit, label) in SYSTEM_UNITS {
            if installed.iter().any(|service| service.name == *unit) {
                list.push(Source { id: format!("unit:{unit}"), label: (*label).to_string(), group: "system" });
            }
        }
        for service in installed.iter().filter(|service| service.name.starts_with("php") && service.name.ends_with("-fpm.service")) {
            let label = service.name.trim_end_matches("-fpm.service").replace("php", "PHP ");
            list.push(Source { id: format!("unit:{}", service.name), label, group: "system" });
        }
    }
    Ok(Json(list))
}

#[derive(Deserialize)]
struct ReadQuery {
    source: String,
    lines: Option<usize>,
    /// Yalnızca bu metni içeren satırlar (büyük/küçük harf ayırmaz).
    q: Option<String>,
}

#[derive(Serialize)]
struct LogView {
    lines: Vec<String>,
    /// Günlük dosyası henüz yoksa (ör. siteye hiç istek gelmediyse) `false`.
    exists: bool,
}

/// Dosyanın son kısmını satırlara ayırır.
fn tail_file(path: &str) -> AppResult<Option<Vec<String>>> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let size = file.metadata()?.len();
    let start = size.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    // Ortadan başlandıysa ilk satır yarımdır.
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    Ok(Some(lines))
}

async fn journal(unit: Option<&str>, scan: usize) -> AppResult<Vec<String>> {
    let count = scan.to_string();
    let mut args = vec!["--no-pager", "--output=short-iso", "--lines", &count];
    if let Some(unit) = unit {
        services::validate_unit_name(unit)?;
        args.extend(["--unit", unit]);
    }
    let output = exec::run_ok("journalctl", &args).await?;
    Ok(output.lines().map(str::to_string).collect())
}

async fn read(State(state): State<AppState>, user: AuthUser, Query(query): Query<ReadQuery>) -> AppResult<Json<LogView>> {
    user.require(AREA)?;
    let wanted = query.lines.unwrap_or(300).clamp(10, 2000);
    let needle = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty()).map(str::to_lowercase);
    let parts: Vec<&str> = query.source.split(':').collect();

    let lines = match parts.as_slice() {
        ["site", id, kind] => {
            let id: i64 = id.parse().map_err(|_| AppError::bad("Geçersiz günlük kaynağı"))?;
            // `sites::get` erişim denetimini yapar; yol alan adından üretilir, istekten alınmaz.
            let site = sites::get(&state.db, &user, id)?;
            if *kind == "app" {
                // Node.js uygulamasının çıktısı systemd günlüğündedir; birim adı kimlikten üretilir.
                let unit = crate::hosting::node::unit_name(site.id);
                let lines = journal(Some(&unit), if needle.is_some() { JOURNAL_SCAN_LINES } else { wanted }).await?;
                let mut lines = lines;
                if let Some(needle) = &needle {
                    lines.retain(|line| line.to_lowercase().contains(needle));
                }
                let skip = lines.len().saturating_sub(wanted);
                return Ok(Json(LogView { lines: lines.split_off(skip), exists: true }));
            }
            let path = match *kind {
                "access" => nginx::access_log_path(&site.domain),
                "error" => nginx::error_log_path(&site.domain),
                _ => return Err(AppError::bad("Geçersiz günlük kaynağı")),
            };
            tail_file(&path)?
        }
        ["journal"] => {
            user.require_admin()?;
            Some(journal(None, if needle.is_some() { JOURNAL_SCAN_LINES } else { wanted }).await?)
        }
        ["unit", unit] => {
            user.require_admin()?;
            Some(journal(Some(unit), if needle.is_some() { JOURNAL_SCAN_LINES } else { wanted }).await?)
        }
        _ => return Err(AppError::bad("Geçersiz günlük kaynağı")),
    };

    let exists = lines.is_some();
    let mut lines = lines.unwrap_or_default();
    if let Some(needle) = &needle {
        lines.retain(|line| line.to_lowercase().contains(needle));
    }
    let skip = lines.len().saturating_sub(wanted);
    Ok(Json(LogView { lines: lines.split_off(skip), exists }))
}
