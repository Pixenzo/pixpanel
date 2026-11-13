//! PixPanel — sunucu ve hosting yönetim paneli.

mod addresses;
mod api;
mod auth;
mod backup;
mod backup_import;
mod blacklist;
mod cli;
mod clone;
mod components;
mod config;
mod db;
mod dbms;
mod dbtools;
mod dbtrash;
mod deploy;
mod dns;
mod error;
mod i18n;
mod events;
mod fail2ban;
mod firewall;
mod fsop;
mod health;
mod hosting;
mod jobs;
mod mail;
mod migrate;
mod notify;
mod panelcert;
mod quota;
mod state;
mod stats;
mod svchist;
mod sys;
mod tasks;
mod totp;
mod update;
mod uptime;
mod web;
mod webhooks;
mod weekly;
mod wordpress;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use rusqlite::params;

use crate::config::Config;
use crate::db::Db;
use crate::state::AppState;

#[derive(Parser)]
#[command(name = "pixpanel", version = update::VERSION, about = "Sunucu ve hosting yönetim paneli")]
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
        /// Hesabın panel dili (`tr`, `en`). Verilmezse ilk girişte tarayıcının dilinden belirlenir.
        #[arg(long)]
        language: Option<String>,
    },
    /// Bir kullanıcının parolasını sıfırlar ve yeni parolayı yazdırır
    ResetPassword { username: String },
    /// Bir kullanıcının iki adımlı doğrulamasını kapatır (telefonunu kaybedenler için)
    DisableTwoFactor { username: String },
    /// Güncelleme sonrası paneli yeniden başlatıp gerekirse eski sürüme dönen bekçi
    #[command(hide = true)]
    UpdateWatchdog,
    /// Dosya yöneticisinin yardımcı süreci; panel tarafından başlatılır
    #[command(hide = true)]
    FsHelper,
    /// Port bağlama kısıtının çalıştığını deneyen yardımcı; panel tarafından başlatılır
    #[command(hide = true)]
    ProbeBind { port: u16 },
    /// Panelin eklediği IP adreslerini kurar; açılışta web sunucusundan önce çalışır
    #[command(hide = true)]
    ApplyAddresses,
    /// Komut satırının dilini ayarlar (tr ya da en); panelin arayüz diline dokunmaz
    #[command(name = "setlang")]
    SetLang {
        /// tr ya da en
        language: String,
    },
    #[command(flatten)]
    Api(cli::ApiCommand),
}

fn main() {
    // `ssh` parolayı sormak için bu ikili dosyayı çağırır (taşıma aracı).
    if let Some(code) = migrate::askpass() {
        std::process::exit(code);
    }
    // Yardım metinleri komut satırının dilinde gösterilir.
    let matches = localize_command(Cli::command()).get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|err| err.exit());
    if let Err(err) = run(cli) {
        // Hata iletileri Türkçe yazılır; ekrana komut satırının dilinde çıkar.
        eprintln!("{} {}", i18n::cli("Hata:"), i18n::cli(&format!("{err:#}")));
        std::process::exit(1);
    }
}

/// Komut ağacındaki açıklama ve yardım metinlerini (Türkçe kaynak) komut satırının diline çevirir.
fn localize_command(mut command: clap::Command) -> clap::Command {
    let translate = |text: Option<&clap::builder::StyledStr>| text.map(|text| i18n::cli(&text.to_string()));
    if let Some(about) = translate(command.get_about()) {
        command = command.about(about);
    }
    if let Some(long_about) = translate(command.get_long_about()) {
        command = command.long_about(long_about);
    }
    let arguments: Vec<String> = command.get_arguments().map(|argument| argument.get_id().to_string()).collect();
    for id in arguments {
        command = command.mut_arg(id, |mut argument| {
            if let Some(help) = translate(argument.get_help()) {
                argument = argument.help(help);
            }
            if let Some(long_help) = translate(argument.get_long_help()) {
                argument = argument.long_help(long_help);
            }
            argument
        });
    }
    let subcommands: Vec<String> = command.get_subcommands().map(|sub| sub.get_name().to_string()).collect();
    for name in subcommands {
        command = command.mut_subcommand(name, localize_command);
    }
    command
}

fn run(cli: Cli) -> anyhow::Result<()> {

    // Yardımcı süreç yetki düşürüp chroot yapacağından olabildiğince yalın
    // başlar: ne eşzamansız çalışma zamanı kurulur ne de veritabanı açılır.
    if matches!(cli.command, Some(Command::FsHelper)) {
        std::process::exit(fsop::run_helper());
    }
    if let Some(Command::ProbeBind { port }) = cli.command {
        std::process::exit(hosting::portguard::run_probe(port));
    }
    // Dil ayarı panelin veritabanına ve yapılandırmasına dokunmaz.
    if let Some(Command::SetLang { language }) = &cli.command {
        return set_cli_language(language);
    }

    tracing_subscriber::fmt()
        // Günlükler systemd'ye gider; renk kodları satırları kirletir ve fail2ban süzgecini bozar.
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pixpanel=info,tower_http=warn".into()),
        )
        .init();

    let cfg = Config::load(&cli.config)?;
    // Bekçi, panelin veritabanına dokunmadan çalışır (gerekirse onu geri koyacaktır).
    if matches!(cli.command, Some(Command::UpdateWatchdog)) {
        return update::run_watchdog(&cfg);
    }
    let db = Db::open(&cfg.db_path())?;

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(serve(cfg, db, cli.config)),
        Command::CreateAdmin { username, password, language } => create_admin(&db, &username, password, language),
        Command::ResetPassword { username } => reset_password(&db, &username),
        Command::DisableTwoFactor { username } => {
            let changed = db
                .call(|conn| conn.execute("UPDATE users SET totp_enabled = 0, totp_secret = NULL, recovery_codes = '[]' WHERE username = ?1", params![username]))
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            anyhow::ensure!(changed > 0, "kullanıcı bulunamadı: {username}");
            println!("{}", i18n::cli(&format!("{username} için iki adımlı doğrulama kapatıldı.")));
            Ok(())
        }
        Command::Api(command) => cli::run(&cfg, &db, command),
        Command::ApplyAddresses => {
            let added = tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(addresses::apply_managed(&db)).map_err(|err| anyhow::anyhow!(err.to_string()))?;
            println!("{}", i18n::cli(&format!("{added} adres kuruldu.")));
            Ok(())
        }
        Command::FsHelper | Command::UpdateWatchdog | Command::ProbeBind { .. } | Command::SetLang { .. } => unreachable!("yardımcı süreçler yukarıda ele alındı"),
    }
}

async fn serve(cfg: Config, db: Db, config_path: PathBuf) -> anyhow::Result<()> {
    let metrics = sys::metrics::Metrics::default();
    metrics.spawn_sampler();
    // Panelin eklediği adresler yeniden başlatmada kaybolmuş olabilir (açılış birimi çalışmadıysa).
    if let Err(err) = addresses::apply_managed(&db).await {
        tracing::warn!(error = %err, "ek IP adresleri kurulamadı");
    }

    // TLS kullanan her şey (panel, ACME, bildirimler) aynı şifreleme sağlayıcısını kullanır.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let tls = if cfg.tls {
        let (cert, key) = ensure_panel_certificate(&cfg.tls_dir())?;
        Some(axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key).await.context("panel sertifikası yüklenemedi")?)
    } else {
        None
    };

    let state = AppState {
        jobs: jobs::JobManager::new(db.clone()).map_err(|err| anyhow::anyhow!(err.to_string()))?,
        cfg: Arc::new(cfg.clone()),
        config_path: Arc::new(config_path),
        db,
        metrics,
        throttle: Arc::default(),
        sys_lock: Arc::default(),
        apt_lock: Arc::default(),
        health: health::Monitor::default(),
        tls: tls.clone(),
    };

    deploy::mark_interrupted(&state.db).map_err(|err| anyhow::anyhow!(err.to_string()))?;
    tasks::spawn_certificate_renewal(state.clone());
    tasks::spawn_db_tool_cleanup(state.clone());
    tasks::spawn_backup_scheduler(state.clone());
    tasks::spawn_update_check(state.clone());
    tasks::spawn_health_monitor(state.clone());
    tasks::spawn_wordpress_scan(state.clone());
    tasks::spawn_disk_measurement(state.clone());
    tasks::spawn_dkim_owner_sync(state.clone());
    tasks::spawn_stats_collector(state.clone());
    tasks::spawn_metrics_archive(state.clone());
    tasks::spawn_dns_cluster_sync(state.clone());
    tasks::spawn_ops_history(state.clone());
    webhooks::spawn(state.clone());
    // Hesap diliminde komut çalıştırma (systemd-run --scope) bir kez sınanır; sonuç önbelleğe alınır.
    sys::runas::probe_slice_support().await;
    // Uygulama portlarının korunması arka planda kurulur; panelin açılışını bekletmez.
    // Sıra önemli: uygulama servislerinin hesap dilimine alınması port korumasının sonucuna bağlıdır.
    tokio::spawn({
        let state = state.clone();
        async move {
            hosting::portguard::apply().await;
            hosting::resource_limits::sync(&state, None).await;
            // MariaDB'nin uzaktan erişim sertifikası kopyası ve uzak kullanıcıların TLS şartı güncel tutulur.
            let _guard = state.sys_lock.lock().await;
            dbms::remote::refresh_mariadb_tls(&state.db, &state.cfg).await;
        }
    });
    // Kayıtlı güvenlik duvarı kuralları panel açılırken yeniden yüklenir.
    if let Err(err) = firewall::reapply(&state).await {
        tracing::error!(error = %err, "güvenlik duvarı kuralları yüklenemedi");
    }

    let app = web::app(state).into_make_service_with_connect_info::<SocketAddr>();
    let addr = SocketAddr::new(cfg.listen_addr, cfg.listen_port);

    if let Some(tls) = tls {
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

fn create_admin(db: &Db, username: &str, password: Option<String>, language: Option<String>) -> anyhow::Result<()> {
    let to_anyhow = |err: error::AppError| anyhow::anyhow!(err.to_string());
    let language = match language.as_deref() {
        None => String::new(),
        Some(value @ ("tr" | "en")) => value.to_string(),
        Some(other) => anyhow::bail!("geçersiz dil: {other} (tr ya da en)"),
    };
    let generated = password.is_none();
    let password = password.unwrap_or_else(|| auth::random_password(20));
    auth::validate_password(&password).map_err(to_anyhow)?;
    let hash = auth::hash_password(&password).map_err(to_anyhow)?;
    db.call(|conn| {
        let tx = conn.transaction()?;
        let id = db::next_user_id(&tx)?;
        tx.execute(
            // Dil verilmediyse boş bırakılır: ilk girişte tarayıcının diline göre belirlenir.
            "INSERT INTO users (id, username, password_hash, role, created_at, language) VALUES (?1, ?2, ?3, 'admin', ?4, ?5)",
            params![id, username, hash, db::now(), language],
        )?;
        tx.commit()
    })
    .map_err(to_anyhow)?;
    println!("{}", i18n::cli("Yönetici hesabı oluşturuldu."));
    println!("{}", i18n::cli(&format!("  Kullanıcı adı: {username}")));
    if generated {
        println!("{}", i18n::cli(&format!("  Parola       : {password}")));
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
    println!("{}", i18n::cli(&format!("Yeni parola: {password}")));
    Ok(())
}

/// `pixpanel setlang`: komut satırının dilini kaydeder.
fn set_cli_language(language: &str) -> anyhow::Result<()> {
    let language = language.trim().to_ascii_lowercase();
    anyhow::ensure!(matches!(language.as_str(), "tr" | "en"), "geçersiz dil: {language} (tr ya da en)");
    anyhow::ensure!(nix::unistd::geteuid().is_root(), "bu komut root olarak çalıştırılmalıdır (sudo pixpanel …)");
    let path = Path::new(i18n::CLI_LANGUAGE_FILE);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, format!("{language}\n")).with_context(|| format!("{} yazılamadı", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))?;
    }
    // İleti yeni seçilen dilde yazılır.
    println!("{}", if language == "en" { "The command line language is now English." } else { "Komut satırının dili Türkçe olarak ayarlandı." });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Her yardım metninin İngilizcesi sözlükte olmalı; yoksa İngilizce yardımda Türkçe satır kalır.
    #[test]
    fn every_help_text_has_an_english_translation() {
        fn walk(command: &clap::Command, missing: &mut Vec<String>) {
            let mut texts: Vec<String> = Vec::new();
            texts.extend(command.get_about().map(ToString::to_string));
            texts.extend(command.get_long_about().map(ToString::to_string));
            for argument in command.get_arguments() {
                texts.extend(argument.get_help().map(ToString::to_string));
                texts.extend(argument.get_long_help().map(ToString::to_string));
            }
            for text in texts {
                if i18n::translate("en", &text) == text && text.chars().any(|c| "çğıöşüÇĞİÖŞÜ".contains(c)) {
                    missing.push(text);
                }
            }
            for sub in command.get_subcommands() {
                walk(sub, missing);
            }
        }
        let mut missing = Vec::new();
        walk(&Cli::command(), &mut missing);
        for text in &missing {
            println!("çevirisi olmayan yardım metni: {text:?}");
        }
        assert!(missing.is_empty(), "{} yardım metninin çevirisi yok", missing.len());
    }

    #[test]
    fn localized_command_still_parses() {
        let matches = localize_command(Cli::command()).try_get_matches_from(["pixpanel", "setlang", "tr"]).unwrap();
        assert!(matches!(Cli::from_arg_matches(&matches).unwrap().command, Some(Command::SetLang { .. })));
    }
}
