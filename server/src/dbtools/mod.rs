//! Panelin yanında çalışan hazır web uygulamaları: MariaDB için phpMyAdmin,
//! PostgreSQL için Adminer ve web postası için Roundcube.
//!
//! Araçlar yalnızca sunucunun içinde dinleyen bir nginx sunucusunda çalışır ve
//! dışarıya panelin `/tools/` yolu üzerinden, panel oturumu denetlenerek açılır.
//! Kullanıcı bir veritabanını açtığında panel o veritabanına özel, birkaç saat
//! sonra silinen geçici bir hesap oluşturur; kimlik bilgileri araca tek
//! kullanımlık bir anahtar dosyasıyla geçer. Kalıcı parolalar hiçbir yerde saklanmaz.

use std::path::{Path, PathBuf};

use rusqlite::params;
use serde::Serialize;

use crate::auth;
use crate::config::Config;
use crate::db::{now, Db};
use crate::dbms::{self, Access, Engine};
use crate::error::{AppError, AppResult};
use crate::hosting::php::installed_versions;
use crate::hosting::{atomic_write, sysuser};
use crate::sys::exec;

/// Araçların dinlediği yerel port.
pub const PORT: u16 = 8446;
const WORK_DIR: &str = "/var/lib/pixpanel/tools";
const SOCKET: &str = "/run/php/pixpanel-tools.sock";
const NGINX_CONF: &str = "/etc/nginx/conf.d/pixpanel-tools.conf";
const PMA_DIR: &str = "/usr/share/phpmyadmin";
const ADMINER_SCRIPT: &str = "/usr/share/adminer/adminer.php";
const ROUNDCUBE_ROOT: &str = "/var/lib/roundcube/public_html";
const ROUNDCUBE_CONFIG: &str = "/etc/roundcube/config.inc.php";
/// Geçici hesapların ömrü.
const SESSION_SECS: i64 = 4 * 3600;
const TEMP_PREFIX: &str = "pxt_";

const NGINX_TEMPLATE: &str = include_str!("nginx.conf");
const POOL_TEMPLATE: &str = include_str!("pool.conf");
const SIGNON_TEMPLATE: &str = include_str!("signon.php");
const PMA_CONFIG_TEMPLATE: &str = include_str!("pma-config.php");
const ADMINER_TEMPLATE: &str = include_str!("adminer.php");
const ROUNDCUBE_TEMPLATE: &str = include_str!("roundcube-config.php");

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Installed {
    pub phpmyadmin: bool,
    pub adminer: bool,
    pub roundcube: bool,
}

pub fn installed() -> Installed {
    Installed {
        phpmyadmin: Path::new(PMA_DIR).join("index.php").is_file(),
        adminer: Path::new(ADMINER_SCRIPT).is_file(),
        roundcube: Path::new(ROUNDCUBE_ROOT).join("index.php").is_file(),
    }
}

fn work(sub: &str) -> PathBuf {
    Path::new(WORK_DIR).join(sub)
}

fn set_owner_mode(path: &Path, uid: u32, gid: u32, mode: u32) -> AppResult<()> {
    use std::os::unix::fs::PermissionsExt;
    std::os::unix::fs::chown(path, Some(uid), Some(gid))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

/// Dosyayı yalnızca içeriği değiştiyse yazar; değiştiyse `true` döner.
fn write_if_changed(path: &Path, content: &str, mode: u32) -> AppResult<bool> {
    if std::fs::read(path).ok().as_deref() == Some(content.as_bytes()) {
        return Ok(false);
    }
    atomic_write(path, content.as_bytes(), mode)?;
    Ok(true)
}

/// Araçların çalışacağı PHP sürümü: gereken veritabanı eklentisi kurulu olan en yeni sürüm.
fn php_version(tools: Installed) -> AppResult<String> {
    let has = |version: &str, module: &str| Path::new(&format!("/etc/php/{version}/mods-available/{module}.ini")).is_file();
    installed_versions()
        .into_iter()
        .find(|version| {
            (!tools.phpmyadmin || has(version, "mysqli"))
                && (!tools.adminer || has(version, "pgsql"))
                && (!tools.roundcube || (has(version, "pdo_sqlite") && has(version, "mbstring") && has(version, "intl")))
        })
        .ok_or_else(|| AppError::bad("Bu araçlar için gereken eklentilerin (mysqli, pgsql, pdo_sqlite, mbstring, intl) kurulu olduğu bir PHP sürümü yok"))
}

/// Araçların çalışma ortamını (nginx sunucusu, PHP havuzu, giriş betikleri)
/// kurar ya da günceller. Tekrar tekrar çağrılabilir.
pub async fn ensure_runtime(cfg: &Config) -> AppResult<()> {
    let tools = installed();
    if !tools.phpmyadmin && !tools.adminer && !tools.roundcube {
        return Ok(());
    }
    let web_gid = sysuser::group_id(sysuser::WEB_GROUP)?;
    let web_uid = sysuser::lookup("www-data").map(|user| user.uid).ok_or_else(|| AppError::internal("www-data kullanıcısı bulunamadı"))?;
    let secure = if cfg.tls { "true" } else { "false" };

    // ---- Klasörler ----
    let (tokens, sessions, web_root) = (work("tokens"), work("sessions"), work("root"));
    for dir in [Path::new(WORK_DIR).to_path_buf(), web_root.join("tools"), work("adminer")] {
        std::fs::create_dir_all(&dir)?;
    }
    for (dir, mode) in [(&tokens, 0o730), (&sessions, 0o700)] {
        std::fs::create_dir_all(dir)?;
        // Anahtar klasörü: havuz dosyayı okuyup silebilir ama içeriği listeleyemez.
        let owner = if mode == 0o700 { web_uid } else { 0 };
        set_owner_mode(dir, owner, web_gid, mode)?;
    }

    // ---- Giriş betikleri ----
    let render = |template: &str| {
        template
            .replace("{{token_dir}}", &tokens.to_string_lossy())
            .replace("{{secure}}", secure)
            .replace("{{adminer_script}}", ADMINER_SCRIPT)
            .replace("{{temp_dir}}", "/var/lib/phpmyadmin/tmp")
    };
    let link = |name: &str, target: &Path| -> AppResult<()> {
        let path = web_root.join("tools").join(name);
        if std::fs::read_link(&path).ok().as_deref() != Some(target) {
            let _ = std::fs::remove_file(&path);
            std::os::unix::fs::symlink(target, &path)?;
        }
        Ok(())
    };
    if tools.phpmyadmin {
        write_if_changed(&Path::new(PMA_DIR).join("pixpanel-signon.php"), &render(SIGNON_TEMPLATE), 0o644)?;
        write_if_changed(Path::new("/etc/phpmyadmin/conf.d/pixpanel.php"), &render(PMA_CONFIG_TEMPLATE), 0o644)?;
        link("phpmyadmin", Path::new(PMA_DIR))?;
    }
    if tools.adminer {
        write_if_changed(&work("adminer").join("index.php"), &render(ADMINER_TEMPLATE), 0o644)?;
        link("adminer", &work("adminer"))?;
    }
    if tools.roundcube {
        let data = work("roundcube");
        let temp = data.join("temp");
        std::fs::create_dir_all(&temp)?;
        for dir in [&data, &temp] {
            set_owner_mode(dir, web_uid, web_gid, 0o700)?;
        }
        // Oturum çerezlerini şifreleyen anahtar bir kez üretilir ve korunur.
        let key_path = data.join("des_key");
        let des_key = match std::fs::read_to_string(&key_path) {
            Ok(key) if key.trim().len() == 24 => key.trim().to_string(),
            _ => {
                let key = auth::random_token(12);
                atomic_write(&key_path, key.as_bytes(), 0o600)?;
                key
            }
        };
        let config = ROUNDCUBE_TEMPLATE
            .replace("{{db_file}}", &data.join("roundcube.db").to_string_lossy())
            .replace("{{des_key}}", &des_key)
            .replace("{{secure}}", secure)
            .replace("{{temp_dir}}", &temp.to_string_lossy());
        // Dosya şifreleme anahtarını içerdiğinden yalnızca havuzun grubu okuyabilir.
        if write_if_changed(Path::new(ROUNDCUBE_CONFIG), &config, 0o640)? {
            std::os::unix::fs::chown(ROUNDCUBE_CONFIG, Some(0), Some(web_gid))?;
        }
        // Web postası `/tools/` altında değil, kendi yolunda yayınlanır.
        let path = web_root.join("webmail");
        if std::fs::read_link(&path).ok().as_deref() != Some(Path::new(ROUNDCUBE_ROOT)) {
            let _ = std::fs::remove_file(&path);
            std::os::unix::fs::symlink(ROUNDCUBE_ROOT, &path)?;
        }
    }

    // ---- PHP havuzu ----
    let version = php_version(tools)?;
    let pool = POOL_TEMPLATE.replace("{{socket}}", SOCKET).replace("{{session_dir}}", &sessions.to_string_lossy());
    let pool_path = |version: &str| PathBuf::from(format!("/etc/php/{version}/fpm/pool.d/pixpanel-tools.conf"));
    let mut reload = Vec::new();
    for other in installed_versions().into_iter().filter(|other| *other != version) {
        if pool_path(&other).exists() {
            std::fs::remove_file(pool_path(&other))?;
            reload.push(other);
        }
    }
    if write_if_changed(&pool_path(&version), &pool, 0o644)? || !Path::new(SOCKET).exists() {
        reload.push(version.clone());
    }
    for version in reload {
        exec::run_ok("systemctl", &["reload-or-restart", "--", &format!("php{version}-fpm.service")]).await?;
    }

    // ---- Roundcube'un temizlik görevi ----
    // Dağıtımın zamanlı temizlik görevi sistemin varsayılan PHP'siyle çalışır. O sürüm web
    // postasının çalıştığı sürümden yeniyse (Ubuntu 26.04'te 8.5) betik hata verir ve birim
    // "başarısız" görünür; görev, havuzla aynı PHP sürümüne bağlanır.
    let gc_unit = Path::new("/usr/lib/systemd/system/roundcube-gc.service");
    if tools.roundcube && gc_unit.exists() {
        let drop_in = Path::new("/etc/systemd/system/roundcube-gc.service.d/pixpanel.conf");
        let content = format!("# PixPanel tarafından üretildi.
[Service]
ExecStart=
ExecStart=/usr/bin/php{version} /usr/share/roundcube/bin/gc.sh
");
        if let Some(parent) = drop_in.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if write_if_changed(drop_in, &content, 0o644)? {
            let _ = exec::run("systemctl", &["daemon-reload"]).await;
            let _ = exec::run("systemctl", &["reset-failed", "roundcube-gc.service"]).await;
        }
    }

    // ---- nginx ----
    let conf = NGINX_TEMPLATE
        .replace("{{port}}", &PORT.to_string())
        .replace("{{web_root}}", &web_root.to_string_lossy())
        .replace("{{https}}", if cfg.tls { "on" } else { "off" })
        .replace("{{socket}}", SOCKET);
    let previous = std::fs::read(NGINX_CONF).ok();
    if write_if_changed(Path::new(NGINX_CONF), &conf, 0o644)? {
        let test = exec::run("nginx", &["-t"]).await?;
        if !test.success() {
            match previous {
                Some(content) => atomic_write(Path::new(NGINX_CONF), &content, 0o644)?,
                None => std::fs::remove_file(NGINX_CONF)?,
            }
            return Err(AppError::internal(format!("Veritabanı araçlarının nginx yapılandırması geçersiz: {}", test.stderr.trim())));
        }
        exec::run_ok("systemctl", &["reload", "nginx.service"]).await?;
    }
    Ok(())
}

/// Bir araç (phpMyAdmin, Adminer, Roundcube) paketi kaldırıldıktan sonra çalışma ortamını
/// temizler: kaldırılan aracın bağlantısı silinir; hiç araç kalmadıysa nginx sunucusu ve PHP
/// havuzu da kaldırılır, aksi hâlde ortam kalan araçlara göre yeniden kurulur.
/// Araçların yapılandırma ve veri dosyaları (ör. Roundcube'un kendi veritabanı) silinmez.
pub async fn after_removal(cfg: &Config) -> AppResult<()> {
    let tools = installed();
    let web_root = work("root");
    let drop_link = |path: PathBuf| match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(AppError::from(err)),
    };
    if !tools.phpmyadmin {
        drop_link(web_root.join("tools").join("phpmyadmin"))?;
    }
    if !tools.adminer {
        drop_link(web_root.join("tools").join("adminer"))?;
    }
    if !tools.roundcube {
        drop_link(web_root.join("webmail"))?;
    }
    if tools.phpmyadmin || tools.adminer || tools.roundcube {
        return ensure_runtime(cfg).await;
    }
    // Hiç araç kalmadı: sunucuyu ve havuzları kaldır, ilgili servisleri yeniden yükle.
    if Path::new(NGINX_CONF).exists() {
        std::fs::remove_file(NGINX_CONF)?;
        exec::run_ok("nginx", &["-t"]).await?;
        exec::run_ok("systemctl", &["reload", "nginx.service"]).await?;
    }
    for version in installed_versions() {
        let pool = PathBuf::from(format!("/etc/php/{version}/fpm/pool.d/pixpanel-tools.conf"));
        if pool.exists() {
            std::fs::remove_file(&pool)?;
            exec::run_ok("systemctl", &["reload-or-restart", "--", &format!("php{version}-fpm.service")]).await?;
        }
    }
    Ok(())
}

/// Veritabanı için geçici bir hesap açar ve aracı o hesapla başlatan adresi döner.
pub async fn open(db: &Db, cfg: &Config, engine: Engine, database: &str) -> AppResult<String> {
    let tools = installed();
    let (tool, available) = match engine {
        Engine::Mariadb => ("phpmyadmin", tools.phpmyadmin),
        Engine::Postgresql => ("adminer", tools.adminer),
    };
    if !available {
        return Err(AppError::bad(match engine {
            Engine::Mariadb => "phpMyAdmin kurulu değil; yöneticinin Bileşenler sayfasından kurması gerekiyor",
            Engine::Postgresql => "Adminer kurulu değil; yöneticinin Bileşenler sayfasından kurması gerekiyor",
        }));
    }
    ensure_runtime(cfg).await?;

    // Geçici hesap yalnızca bu veritabanında yetkilidir.
    let user = format!("{TEMP_PREFIX}{}", auth::random_token(6));
    let password = auth::random_token(20);
    let host = if engine == Engine::Mariadb { "localhost" } else { "" };
    dbms::create_user(engine, &user, host, &password).await?;
    db.call(|conn| {
        conn.execute(
            "INSERT INTO db_tool_sessions (engine, name, host, database, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![engine.as_str(), user, host, database, now() + SESSION_SECS],
        )
    })?;
    dbms::grant(engine, database, &user, host, Access::Full).await?;

    let token = auth::random_token(24);
    let payload = serde_json::json!({ "tool": tool, "user": user, "password": password, "database": database });
    let path = work("tokens").join(format!("{token}.json"));
    atomic_write(&path, payload.to_string().as_bytes(), 0o640)?;
    std::os::unix::fs::chown(&path, Some(0), Some(sysuser::group_id(sysuser::WEB_GROUP)?))?;

    Ok(match engine {
        Engine::Mariadb => format!("/tools/phpmyadmin/pixpanel-signon.php?token={token}"),
        Engine::Postgresql => format!("/tools/adminer/?token={token}"),
    })
}

/// Süresi dolan geçici hesapları ve kullanılmamış anahtar dosyalarını siler.
pub async fn cleanup(db: &Db) -> AppResult<()> {
    let expired: Vec<(i64, String, String, String, String)> = db.call(|conn| {
        let mut stmt = conn.prepare("SELECT id, engine, name, host, database FROM db_tool_sessions WHERE expires_at < ?1")?;
        stmt.query_map(params![now()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))?.collect()
    })?;
    for (id, engine, name, host, database) in expired {
        let Ok(engine) = Engine::parse(&engine) else { continue };
        // Yalnızca panelin açtığı geçici hesaplar silinir.
        if !name.starts_with(TEMP_PREFIX) || !engine.is_installed() {
            continue;
        }
        // Veritabanı bu arada silinmiş olabilir; yetki geri alınamasa da hesap silinmeye çalışılır.
        let _ = dbms::revoke(engine, &database, &name, &host).await;
        match dbms::drop_user(engine, &name, &host).await {
            Ok(()) => {
                db.call(|conn| conn.execute("DELETE FROM db_tool_sessions WHERE id = ?1", params![id]))?;
            }
            Err(err) => tracing::warn!(error = %err, user = %name, "geçici veritabanı hesabı silinemedi"),
        }
    }
    if let Ok(entries) = std::fs::read_dir(work("tokens")) {
        for entry in entries.flatten() {
            let stale = entry.metadata().and_then(|meta| meta.modified()).ok().and_then(|at| at.elapsed().ok()).is_some_and(|age| age.as_secs() > 300);
            if stale {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    Ok(())
}
