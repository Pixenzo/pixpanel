//! systemd servislerinin listelenmesi ve yönetimi.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::sys::exec;

#[derive(Debug, Serialize)]
pub struct Service {
    pub name: String,
    pub description: String,
    /// loaded / not-found / masked
    pub load: String,
    /// active / inactive / failed
    pub active: String,
    /// running / exited / dead ...
    pub sub: String,
    /// enabled / disabled / static ...
    pub enabled: String,
}

#[derive(Deserialize)]
struct UnitRow {
    unit: String,
    #[serde(default)]
    load: String,
    #[serde(default)]
    active: String,
    #[serde(default)]
    sub: String,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct UnitFileRow {
    unit_file: String,
    #[serde(default)]
    state: String,
}

pub async fn list() -> AppResult<Vec<Service>> {
    let units = exec::run_ok(
        "systemctl",
        &["list-units", "--type=service", "--all", "--no-pager", "--output=json"],
    )
    .await?;
    let files = exec::run_ok(
        "systemctl",
        &["list-unit-files", "--type=service", "--no-pager", "--output=json"],
    )
    .await?;

    let units: Vec<UnitRow> = serde_json::from_str(&units)
        .map_err(|err| AppError::internal(format!("servis listesi çözülemedi: {err}")))?;
    let files: Vec<UnitFileRow> = serde_json::from_str(&files).unwrap_or_default();
    let enabled: HashMap<String, String> =
        files.into_iter().map(|row| (row.unit_file, row.state)).collect();

    let mut services: Vec<Service> = units
        .into_iter()
        .filter(|row| row.load != "not-found")
        .map(|row| Service {
            enabled: enabled.get(&row.unit).cloned().unwrap_or_default(),
            name: row.unit,
            description: row.description,
            load: row.load,
            active: row.active,
            sub: row.sub,
        })
        .collect();
    services.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(services)
}

pub const ACTIONS: &[&str] = &["start", "stop", "restart", "reload", "enable", "disable"];

/// Birim adı yalnızca systemd'nin izin verdiği karakterlerden oluşabilir.
pub fn validate_unit_name(name: &str) -> AppResult<()> {
    let valid = !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('-')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@' | ':' | '\\'));
    if valid { Ok(()) } else { Err(AppError::bad("Geçersiz servis adı")) }
}

pub async fn action(name: &str, action: &str) -> AppResult<()> {
    validate_unit_name(name)?;
    if !ACTIONS.contains(&action) {
        return Err(AppError::bad("Geçersiz servis işlemi"));
    }
    exec::run_ok("systemctl", &[action, "--", name]).await?;
    Ok(())
}

pub async fn is_active(name: &str) -> bool {
    exec::run("systemctl", &["is-active", "--quiet", "--", name])
        .await
        .map(|out| out.success())
        .unwrap_or(false)
}
