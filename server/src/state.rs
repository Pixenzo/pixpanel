//! Bütün istek işleyicilerinin paylaştığı uygulama durumu.

use std::sync::Arc;

use crate::auth::LoginThrottle;
use crate::config::Config;
use crate::db::Db;
use crate::jobs::JobManager;
use crate::sys::metrics::Metrics;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub db: Db,
    pub jobs: JobManager,
    pub metrics: Metrics,
    pub throttle: Arc<LoginThrottle>,
    /// Sistem yapılandırmasına (nginx, PHP havuzları...) dokunan işlemleri sıraya sokar.
    pub sys_lock: Arc<tokio::sync::Mutex<()>>,
    /// Aynı anda yalnızca bir paket kurulumu yürüsün diye.
    pub apt_lock: Arc<tokio::sync::Mutex<()>>,
}
