//! PixPanel — sunucu ve hosting yönetim paneli.

mod api;
mod auth;
mod components;
mod config;
mod db;
mod dbms;
mod deploy;
mod error;
mod fsop;
mod hosting;
mod jobs;
mod notify;
mod state;
mod sys;
mod tasks;
mod web;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, Subcommand};
use rusqlite::params;

use crate::config::Config;
use crate::db::Db;
use crate::state::AppState;

#[derive(Parser)]
#[command(name = "pixpanel", version, about = "Sunucu ve hosting yönetim paneli")]
struct Cli {
    /// Yapılandırma dosyasının yolu
    #[arg(long, global = true, default_value = config::DEFAULT_CONFIG_PATH)]
    config: PathBuf,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Paneli çalıştırır (varsayılan)
    Serve,
    /// Yönetici hesabı oluşturur; parola verilmezse rastgele üretilir
    CreateAdmin {
        #[arg(long, default_value = "admin")]
        username: String,
        #[arg(long)]
        password: Option<String>,
    },
    /// Bir kullanıcının parolasını sıfırlar ve yeni parolayı yazdırır
    ResetPassword { username: String },
    /// Dosya yöneticisinin yardımcı süreci; panel tarafından başlatılır
    #[command(hide = true)]
    FsHelper,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Yardımcı süreç yetki düşürüp chroot yapacağından olabildiğince yalın
    // başlar: ne eşzamansız çalışma zamanı kurulur ne de veritabanı açılır.
    if matches!(cli.command, Some(Command::FsHelper)) {
        std::process::exit(fsop::run_helper());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pixpanel=info,tower_http=warn".into()),
        )
        .init();

    let cfg = Config::load(&cli.config)?;
    let db = Db::open(&cfg.db_path())?;

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(serve(cfg, db)),
        Command::CreateAdmin { username, password } => create_admin(&db, &username, password),
        Command::ResetPassword { username } => reset_password(&db, &username),
        Command::FsHelper => unreachable!("yardımcı süreç yukarıda ele alındı"),
    }
}

async fn serve(cfg: Config, db: Db) -> anyhow::Result<()> {
    let metrics = sys::metrics::Metrics::default();
    metrics.spawn_sampler();

    let state = AppState {
        jobs: jobs::JobManager::new(db.clone()).map_err(|err| anyhow::anyhow!(err.to_string()))?,
        cfg: Arc::new(cfg.clone()),
        db,
        metrics,
        throttle: Arc::default(),
        sys_lock: Arc::default(),
        apt_lock: Arc::default(),
    };

    // TLS kullanan her şey (panel, ACME, bildirimler) aynı şifreleme sağlayıcısını kullanır.
    let _ = rustls::crypto::ring::default_provider().install_default();
    deploy::mark_interrupted(&state.db).map_err(|err| anyhow::anyhow!(err.to_string()))?;
    tasks::spawn_certificate_renewal(state.clone());

    let app = web::app(state).into_make_service_with_connect_info::<SocketAddr>();
    let addr = SocketAddr::new(cfg.listen_addr, cfg.listen_port);

    if cfg.tls {
        let (cert, key) = ensure_panel_certificate(&cfg.tls_dir())?;
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key)
            .await
            .context("panel sertifikası yüklenemedi")?;
        tracing::info!("panel https://{addr} adresinde dinliyor");
        axum_server::bind_rustls(addr, tls).serve(app).await?;
    } else {
        tracing::info!("panel http://{addr} adresinde dinliyor");
        axum_server::bind(addr).serve(app).await?;
    }
    Ok(())
}

/// Panelin kendi sertifikası yoksa kendinden imzalı bir tane üretir.
fn ensure_panel_certificate(dir: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let cert_path = dir.join("panel.crt");
    let key_path = dir.join("panel.key");
    if cert_path.exists() && key_path.exists() {
        return Ok((cert_path, key_path));
    }
    std::fs::create_dir_all(dir)?;
    let hostname = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|h| h.trim().to_string())
        .unwrap_or_else(|_| "localhost".into());
    let names = vec![hostname, "localhost".to_string()];
    let generated = rcgen::generate_simple_self_signed(names).context("sertifika üretilemedi")?;
    std::fs::write(&cert_path, generated.cert.pem())?;
    std::fs::write(&key_path, generated.signing_key.serialize_pem())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
    }
    tracing::info!("kendinden imzalı panel sertifikası üretildi: {}", cert_path.display());
    Ok((cert_path, key_path))
}

fn create_admin(db: &Db, username: &str, password: Option<String>) -> anyhow::Result<()> {
    let to_anyhow = |err: error::AppError| anyhow::anyhow!(err.to_string());
    let generated = password.is_none();
    let password = password.unwrap_or_else(|| auth::random_password(20));
    auth::validate_password(&password).map_err(to_anyhow)?;
    let hash = auth::hash_password(&password).map_err(to_anyhow)?;
    db.call(|conn| {
        conn.execute(
            "INSERT INTO users (username, password_hash, role, created_at) VALUES (?1, ?2, 'admin', ?3)",
            params![username, hash, db::now()],
        )
    })
    .map_err(to_anyhow)?;
    println!("Yönetici hesabı oluşturuldu.");
    println!("  Kullanıcı adı: {username}");
    if generated {
        println!("  Parola       : {password}");
    }
    Ok(())
}

fn reset_password(db: &Db, username: &str) -> anyhow::Result<()> {
    let to_anyhow = |err: error::AppError| anyhow::anyhow!(err.to_string());
    let password = auth::random_password(20);
    let hash = auth::hash_password(&password).map_err(to_anyhow)?;
    let updated = db
        .call(|conn| {
            let tx = conn.transaction()?;
            let updated = tx.execute(
                "UPDATE users SET password_hash = ?1, status = 'active' WHERE username = ?2",
                params![hash, username],
            )?;
            tx.execute(
                "DELETE FROM sessions WHERE user_id IN (SELECT id FROM users WHERE username = ?1)",
                params![username],
            )?;
            tx.commit()?;
            Ok(updated)
        })
        .map_err(to_anyhow)?;
    anyhow::ensure!(updated > 0, "kullanıcı bulunamadı: {username}");
    println!("Yeni parola: {password}");
    Ok(())
}
