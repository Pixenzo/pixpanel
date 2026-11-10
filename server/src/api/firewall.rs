//! Güvenlik duvarı uçları. Yalnızca yöneticiler.

use std::net::IpAddr;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::firewall::{self, Preset, Settings};
use crate::state::AppState;
use crate::sys::exec;

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(show).put(apply)).route("/confirm", post(confirm)).route("/revert", post(revert))
}

/// Sunucuda dinleyen bir port ve güvenlik duvarının ona bakışı.
#[derive(Serialize)]
struct Listener {
    protocol: String,
    port: u16,
    /// Dinlediği adres; `127.0.0.1` gibi yerel adresler dışarıdan zaten erişilemez.
    address: String,
    process: String,
    local_only: bool,
    /// `open` (her adrese açık), `restricted` (belirli adreslere açık) ya da `closed`.
    verdict: &'static str,
}

#[derive(Serialize)]
struct PendingView {
    settings: Settings,
    seconds_left: u64,
}

#[derive(Serialize)]
struct View {
    /// nftables kurulu mu?
    available: bool,
    /// Onaylanmış (kalıcı) ayarlar.
    settings: Settings,
    presets: Vec<Preset>,
    pending: Option<PendingView>,
    confirm_seconds: u64,
    your_ip: String,
    listeners: Vec<Listener>,
}

fn port_matches(spec: &str, port: u16) -> bool {
    spec.split(',').any(|part| {
        let (from, to) = part.trim().split_once('-').unwrap_or((part.trim(), part.trim()));
        matches!((from.parse::<u16>(), to.parse::<u16>()), (Ok(from), Ok(to)) if (from..=to).contains(&port))
    })
}

/// Portun, herhangi bir adresten gelen bağlantı için durumu.
fn verdict(settings: &Settings, presets: &[Preset], protocol: &str, port: u16) -> &'static str {
    if !settings.enabled {
        return "open";
    }
    let mut restricted = false;
    for rule in &settings.rules {
        if (rule.protocol == protocol || rule.protocol == "both") && port_matches(&rule.ports, port) {
            if rule.source.is_empty() {
                return match (rule.action.as_str(), restricted) {
                    ("accept", _) => "open",
                    (_, true) => "restricted",
                    _ => "closed",
                };
            }
            restricted |= rule.action == "accept";
        }
    }
    for preset in presets {
        let Some(state) = settings.presets.iter().find(|state| state.key == preset.key && state.enabled) else { continue };
        let ports = if protocol == "tcp" { &preset.tcp } else { &preset.udp };
        if ports.iter().any(|spec| port_matches(spec, port)) {
            if state.source.is_empty() {
                return "open";
            }
            restricted = true;
        }
    }
    if restricted { "restricted" } else { "closed" }
}

async fn listeners(settings: &Settings, presets: &[Preset]) -> Vec<Listener> {
    let Ok(output) = exec::run("ss", &["-H", "-l", "-t", "-u", "-n", "-p"]).await else { return Vec::new() };
    let mut list: Vec<Listener> = Vec::new();
    for line in output.stdout.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5 {
            continue;
        }
        let protocol = fields[0].to_string();
        let Some((address, port)) = fields[4].rsplit_once(':') else { continue };
        let Ok(port) = port.parse::<u16>() else { continue };
        let address = address.trim_matches(['[', ']']).split('%').next().unwrap_or_default().to_string();
        let local_only = address.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
        // `users:(("nginx",pid=123,fd=5))` → nginx
        let process = line.split("((\"").nth(1).and_then(|rest| rest.split('"').next()).unwrap_or_default().to_string();
        // Aynı port IPv4 ve IPv6 için ayrı satırlarda gelir; dışarıya açık olan satır tutulur.
        if let Some(existing) = list.iter_mut().find(|other| other.protocol == protocol && other.port == port) {
            if existing.local_only && !local_only {
                existing.local_only = false;
                existing.address = address;
            }
            continue;
        }
        list.push(Listener { verdict: verdict(settings, presets, &protocol, port), protocol, port, address, process, local_only });
    }
    list.sort_by_key(|listener| (listener.local_only, listener.port, listener.protocol.clone()));
    list
}

async fn view(state: &AppState, user: &AuthUser) -> View {
    let presets = firewall::presets(&state.cfg);
    let settings = firewall::load(&state.db, &state.cfg);
    let pending = firewall::pending_state().map(|(settings, seconds_left)| PendingView { settings, seconds_left });
    // Bekleyen değişiklik varsa port durumları ona göre gösterilir (şu an yürürlükte olan odur).
    let effective = pending.as_ref().map(|waiting| &waiting.settings).unwrap_or(&settings);
    View {
        available: firewall::is_available(),
        listeners: listeners(effective, &presets).await,
        settings,
        presets,
        pending,
        confirm_seconds: firewall::CONFIRM_SECS,
        your_ip: user.ip.clone(),
    }
}

async fn show(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<View>> {
    user.require_admin()?;
    Ok(Json(view(&state, &user).await))
}

#[derive(Deserialize)]
struct ApplyBody {
    settings: Settings,
}

async fn apply(State(state): State<AppState>, user: AuthUser, Json(body): Json<ApplyBody>) -> AppResult<Json<View>> {
    user.require_admin()?;
    if !firewall::is_available() {
        return Err(AppError::bad("nftables kurulu değil (nft komutu bulunamadı)"));
    }
    let presets = firewall::presets(&state.cfg);
    let settings = body.settings.normalized(&presets)?;
    // Bu kurallar isteği yapanın panele erişimini kesiyorsa hiç uygulanmaz.
    if let Ok(ip) = user.ip.parse::<IpAddr>() {
        if !firewall::allows(&settings, &presets, ip, state.cfg.listen_port) {
            return Err(AppError::bad(format!(
                "Bu kurallar sizin adresinizden ({ip}) panele erişimi kesiyor; uygulanmadı. Panel portunu kendi adresinize açık bırakın."
            )));
        }
    }
    let _guard = state.sys_lock.lock().await;
    firewall::apply_pending(&state, settings).await?;
    auth::audit(&state.db, &user, "firewall.apply", "", "onay bekliyor");
    Ok(Json(view(&state, &user).await))
}

async fn confirm(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<View>> {
    user.require_admin()?;
    let _guard = state.sys_lock.lock().await;
    firewall::confirm(&state).await?;
    auth::audit(&state.db, &user, "firewall.confirm", "", "");
    Ok(Json(view(&state, &user).await))
}

async fn revert(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<View>> {
    user.require_admin()?;
    let _guard = state.sys_lock.lock().await;
    firewall::revert(&state).await?;
    auth::audit(&state.db, &user, "firewall.revert", "", "");
    Ok(Json(view(&state, &user).await))
}
