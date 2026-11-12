//! WordPress araçları: tek tıkla kurulum, güncelleme, eklenti/tema yönetimi ve
//! parolasız yönetici girişi. Hepsi wp-cli üzerinden yürür.
//!
//! wp-cli sitenin PHP kodunu (wp-config.php, eklentiler) çalıştırır; bu yüzden her
//! komut sitenin Linux kullanıcısının kimliğiyle, sitenin PHP sürümüyle koşar. Panelin
//! yetkisiyle hiçbir site kodu çalışmaz.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::auth;
use crate::db::now;
use crate::dbms::{self, Access, Engine};
use crate::error::{AppError, AppResult};
use crate::hosting::sites::Site;
use crate::hosting::sysuser::{self, SysUser};
use crate::jobs::JobCtx;
use crate::state::AppState;
use crate::sys::exec::{self, CmdOutput};
use crate::sys::runas;

mod rollback;
pub mod staging;

/// Panelin kullandığı wp-cli sürümü. Dosya, gömülü özetle doğrulanmadan çalıştırılmaz.
pub const WP_CLI_VERSION: &str = "2.12.0";
const WP_CLI_SHA256: &str = "ce34ddd838f7351d6759068d09793f26755463b4a4610a5a5c0a97b68220d85c";
const WP_CLI_DIR: &str = "/usr/local/lib/pixpanel";
const WP_CLI_PATH: &str = "/usr/local/lib/pixpanel/wp-cli.phar";

const QUICK: Duration = Duration::from_secs(60);
/// Tek kullanımlık giriş bağlantısının geçerlilik süresi.
const LOGIN_TTL_SECS: i64 = 60;
const LOGIN_OPTION: &str = "pixpanel_login_token";
const LOGIN_PLUGIN: &str = include_str!("hosting/templates/wp-login.php");

/// wp-cli'yi (yoksa ya da özeti tutmuyorsa) indirir ve özetini doğrular.
pub async fn ensure_cli() -> AppResult<()> {
    let path = Path::new(WP_CLI_PATH);
    if let Ok(existing) = std::fs::read(path) {
        if hex::encode(Sha256::digest(&existing)) == WP_CLI_SHA256 {
            return Ok(());
        }
    }
    std::fs::create_dir_all(WP_CLI_DIR)?;
    let staging = format!("{WP_CLI_PATH}.new");
    let url = format!("https://github.com/wp-cli/wp-cli/releases/download/v{WP_CLI_VERSION}/wp-cli-{WP_CLI_VERSION}.phar");
    let download = exec::run_with_timeout("curl", &["-fsSL", "--max-time", "120", "-o", &staging, "--", &url], Duration::from_secs(150)).await?;
    if !download.success() {
        let _ = std::fs::remove_file(&staging);
        return Err(AppError::internal("wp-cli indirilemedi; sunucunun internete çıkabildiğini denetleyin"));
    }
    let actual = hex::encode(Sha256::digest(std::fs::read(&staging)?));
    if actual != WP_CLI_SHA256 {
        let _ = std::fs::remove_file(&staging);
        return Err(AppError::internal("İndirilen wp-cli dosyasının özeti uyuşmuyor; kurulum durduruldu"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o644))?;
    }
    std::fs::rename(&staging, path)?;
    Ok(())
}

/// Bir sitede wp-cli çalıştırmak için gerekenler.
pub struct Wp {
    user: SysUser,
    docroot: PathBuf,
    php: String,
}

impl Wp {
    pub fn for_site(site: &Site) -> AppResult<Self> {
        let version = site.php_version.as_deref().filter(|_| site.kind == "php").ok_or_else(|| AppError::bad("WordPress yalnızca PHP sitelerinde çalışır"))?;
        crate::hosting::php::validate_version(version)?;
        let php = format!("/usr/bin/php{version}");
        if !Path::new(&php).is_file() {
            return Err(AppError::bad(format!("PHP {version} kurulu değil")));
        }
        let user = sysuser::lookup(&site.owner).ok_or_else(|| AppError::internal("Hesabın Linux kullanıcısı bulunamadı"))?;
        Ok(Self { user, docroot: site.docroot(), php })
    }

    /// wp-cli komutunu sitenin kullanıcısı adına, site klasöründe çalışacak biçimde hazırlar.
    fn command(&self, args: &[&str]) -> tokio::process::Command {
        let path = format!("--path={}", self.docroot.display());
        let mut full: Vec<&str> = vec!["-d", "memory_limit=512M", WP_CLI_PATH, "--no-color", &path];
        full.extend_from_slice(args);
        let mut cmd = runas::command(&self.user, &self.docroot, &self.php, &full);
        // Önbellek ve yapılandırma kullanıcının kendi klasöründe kalır.
        cmd.env("WP_CLI_CACHE_DIR", self.user.home.join(".wp-cli/cache"));
        cmd.env("WP_CLI_DISABLE_AUTO_CHECK_UPDATE", "1");
        cmd
    }

    async fn run(&self, args: &[&str]) -> AppResult<CmdOutput> {
        runas::output(self.command(args), QUICK).await
    }

    /// Komutu çalıştırır; başarısızsa wp-cli'nin hata satırını kullanıcıya iletir.
    async fn run_ok(&self, args: &[&str]) -> AppResult<String> {
        let output = self.run(args).await?;
        if output.success() {
            return Ok(output.stdout.trim().to_string());
        }
        Err(AppError::bad(cli_error(&output)))
    }

    /// Komutu, standart girdisine `input` yazarak çalıştırır (parolalar argüman olarak
    /// verilmez; süreç listesinde görünmesinler diye girdiden okutulur).
    async fn run_with_input(&self, args: &[&str], input: &str) -> AppResult<String> {
        use tokio::io::AsyncWriteExt;
        let mut cmd = self.command(args);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|err| AppError::internal(format!("wp-cli başlatılamadı: {err}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(input.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }
        let output = tokio::time::timeout(QUICK, child.wait_with_output())
            .await
            .map_err(|_| AppError::internal("komut zaman aşımına uğradı"))?
            .map_err(|err| AppError::internal(err.to_string()))?;
        let output = CmdOutput {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        };
        if output.success() { Ok(output.stdout.trim().to_string()) } else { Err(AppError::bad(cli_error(&output))) }
    }

    /// Parola alan komutu çalıştırır. wp-cli girdiden okuduğu değeri çıktısında yinelediği
    /// için bu komutların çıktısı görev günlüğüne aktarılmaz; yalnızca komutun adı yazılır.
    async fn run_secret(&self, ctx: &JobCtx, args: &[&str], secret: &str) -> AppResult<()> {
        ctx.log(format!("$ wp {} (parola girdiden verildi, çıktı gizlendi)", args.join(" ")));
        self.run_with_input(args, &format!("{secret}\n")).await.map(|_| ()).map_err(|err| {
            // Hata iletisinde parola geçiyorsa o da gizlenir.
            AppError::bad(err.to_string().replace(secret, "••••"))
        })
    }

    /// Komutu görev günlüğüne canlı aktararak çalıştırır.
    async fn stream(&self, ctx: &JobCtx, args: &[&str], input: Option<&str>) -> AppResult<()> {
        ctx.log(format!("$ wp {}", args.join(" ")));
        ctx.run_streaming(&mut self.command(args), "wp", input.map(|text| text.as_bytes().to_vec())).await
    }

    /// Site klasöründe WordPress dosyaları var mı?
    async fn has_files(&self) -> bool {
        self.run(&["core", "version"]).await.map(|out| out.success()).unwrap_or(false)
    }

    /// Yayın klasörü boş mu (panelin karşılama sayfası sayılmaz)?
    async fn docroot_is_empty(&self) -> bool {
        let cmd = runas::command(&self.user, &self.docroot, "find", &[".", "-mindepth", "1", "-maxdepth", "1", "-not", "-name", "index.html", "-print", "-quit"]);
        runas::output(cmd, QUICK).await.map(|out| out.success() && out.stdout.trim().is_empty()).unwrap_or(false)
    }
}

/// wp-cli'nin hata çıktısından kullanıcıya gösterilecek satırı seçer.
fn cli_error(output: &CmdOutput) -> String {
    let text = if output.stderr.trim().is_empty() { &output.stdout } else { &output.stderr };
    let line = text.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("wp-cli başarısız oldu");
    line.trim().trim_start_matches("Error:").trim().chars().take(300).collect()
}

// ---- Durum ----

#[derive(Debug, Serialize, PartialEq)]
pub struct Extension {
    pub name: String,
    pub title: String,
    pub status: String,
    /// `available` ise güncelleme var.
    pub update: String,
    pub version: String,
    pub update_version: Option<String>,
    /// `on` ise kendiliğinden güncellenir.
    pub auto_update: String,
}

/// Panelin sitelere bıraktığı giriş eklentisi; kullanıcının yönettiği eklentiler arasında gösterilmez.
const OWN_PLUGIN: &str = "pixpanel-login";

/// wp-cli'nin eklenti/tema listesini okur. Alanların türü sabit değildir (olmazsa olmaz
/// eklentilerde sürüm boş, güncelleme alanı `false` gelebilir); tek bir beklenmedik değer
/// bütün listeyi düşürmesin diye her alan ayrı ayrı metne çevrilir.
fn parse_extensions(json: &str) -> Vec<Extension> {
    let text = |value: &serde_json::Value| match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    };
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(json) else { return Vec::new() };
    items
        .iter()
        .filter_map(|item| {
            let name = text(item.get("name")?);
            if name.is_empty() || name == OWN_PLUGIN {
                return None;
            }
            let field = |key: &str| item.get(key).map(&text).unwrap_or_default();
            Some(Extension {
                name,
                title: field("title"),
                status: field("status"),
                update: field("update"),
                version: field("version"),
                update_version: Some(field("update_version")).filter(|version| !version.is_empty() && version != "false"),
                auto_update: field("auto_update"),
            })
        })
        .collect()
}

#[derive(Debug, Serialize)]
pub struct Status {
    /// Site klasöründe WordPress var.
    pub installed: bool,
    /// Kurulu değilse: klasör boş olduğu için tek tıkla kurulabilir.
    pub can_install: bool,
    /// MariaDB kurulu mu (kurulum için gerekir)?
    pub database_ready: bool,
    pub version: String,
    /// Yeni çekirdek sürümü varsa numarası.
    pub core_update: Option<String>,
    pub url: String,
    pub title: String,
    pub maintenance: bool,
    /// `minor`, `all` ya da `off`.
    pub auto_core: String,
    pub plugins: Vec<Extension>,
    pub themes: Vec<Extension>,
    /// WordPress dosyaları var ama veritabanına bağlanamıyor ya da kurulum tamamlanmamış.
    pub problem: Option<String>,
}

#[derive(Deserialize)]
struct CoreUpdate {
    version: String,
}

const EXTENSION_FIELDS: &str = "--fields=name,title,status,update,version,update_version,auto_update";

pub async fn status(site: &Site) -> AppResult<Status> {
    ensure_cli().await?;
    let wp = Wp::for_site(site)?;
    let database_ready = Engine::Mariadb.is_installed();
    let mut status = Status {
        installed: false,
        can_install: false,
        database_ready,
        version: String::new(),
        core_update: None,
        url: String::new(),
        title: String::new(),
        maintenance: false,
        auto_core: "minor".into(),
        plugins: Vec::new(),
        themes: Vec::new(),
        problem: None,
    };
    let version = wp.run(&["core", "version"]).await?;
    if !version.success() {
        status.can_install = wp.docroot_is_empty().await;
        return Ok(status);
    }
    status.installed = true;
    status.version = version.stdout.trim().to_string();

    // Eklentiler yüklenmeden sorulur: bozuk bir eklenti durum sayfasını da bozmasın.
    let ready = wp.run(&["core", "is-installed", "--skip-plugins", "--skip-themes"]).await?;
    if !ready.success() {
        status.problem = Some(if ready.stderr.trim().is_empty() {
            "WordPress dosyaları var ama kurulum tamamlanmamış.".into()
        } else {
            cli_error(&ready)
        });
        return Ok(status);
    }

    let (url, title, updates, plugins, themes, maintenance, auto_core) = tokio::join!(
        wp.run(&["option", "get", "siteurl", "--skip-plugins", "--skip-themes"]),
        wp.run(&["option", "get", "blogname", "--skip-plugins", "--skip-themes"]),
        wp.run(&["core", "check-update", "--format=json", "--skip-plugins", "--skip-themes"]),
        wp.run(&["plugin", "list", "--format=json", EXTENSION_FIELDS, "--skip-plugins", "--skip-themes"]),
        wp.run(&["theme", "list", "--format=json", EXTENSION_FIELDS, "--skip-plugins", "--skip-themes"]),
        wp.run(&["maintenance-mode", "is-active", "--skip-plugins", "--skip-themes"]),
        wp.run(&["config", "get", "WP_AUTO_UPDATE_CORE", "--format=json", "--skip-plugins", "--skip-themes"]),
    );
    let text = |result: AppResult<CmdOutput>| result.ok().filter(CmdOutput::success).map(|out| out.stdout.trim().to_string()).unwrap_or_default();
    status.maintenance = maintenance.as_ref().map(CmdOutput::success).unwrap_or(false);
    status.url = text(url);
    status.title = text(title);
    status.core_update = serde_json::from_str::<Vec<CoreUpdate>>(&text(updates)).ok().and_then(|list| list.into_iter().next_back()).map(|update| update.version);
    status.plugins = parse_extensions(&text(plugins));
    status.themes = parse_extensions(&text(themes));
    status.auto_core = auto_core_level(&text(auto_core)).into();
    Ok(status)
}

/// `wp config get WP_AUTO_UPDATE_CORE --format=json` çıktısını seçeneğe çevirir.
/// Sabit tanımlı değilse WordPress yalnızca küçük sürümleri kendiliğinden kurar.
fn auto_core_level(json: &str) -> &'static str {
    match json.trim() {
        "true" => "all",
        "false" => "off",
        _ => "minor",
    }
}

// ---- Kurulum ----

#[derive(Debug, Deserialize)]
pub struct InstallRequest {
    pub title: String,
    pub admin_user: String,
    pub admin_email: String,
    /// Arayüzde üretilip kullanıcıya gösterilen parola; günlüğe yazılmaz.
    pub admin_password: String,
    #[serde(default)]
    pub locale: String,
}

impl InstallRequest {
    pub fn validated(mut self) -> AppResult<Self> {
        self.title = self.title.trim().to_string();
        self.admin_user = self.admin_user.trim().to_string();
        self.admin_email = self.admin_email.trim().to_string();
        if self.locale.trim().is_empty() {
            self.locale = "tr_TR".into();
        }
        if self.title.is_empty() || self.title.chars().count() > 100 || self.title.chars().any(char::is_control) {
            return Err(AppError::bad("Site başlığı 1-100 karakter olmalı"));
        }
        let user_ok = (3..=60).contains(&self.admin_user.len())
            && self.admin_user.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            && self.admin_user.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
        if !user_ok {
            return Err(AppError::bad("Yönetici adı 3-60 karakter olmalı; harf, rakam, nokta, _ ve - içerebilir"));
        }
        if !valid_email(&self.admin_email) {
            return Err(AppError::bad("Geçerli bir e-posta adresi girin"));
        }
        if !(10..=128).contains(&self.admin_password.chars().count()) || self.admin_password.chars().any(char::is_control) {
            return Err(AppError::bad("Yönetici parolası 10-128 karakter olmalı"));
        }
        if !valid_locale(&self.locale) {
            return Err(AppError::bad("Geçersiz dil kodu"));
        }
        Ok(self)
    }
}

fn valid_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else { return false };
    value.len() <= 254
        && !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '@'))
}

/// `tr_TR`, `en_US`, `de_DE_formal` gibi WordPress dil kodları.
fn valid_locale(value: &str) -> bool {
    (2..=20).contains(&value.len())
        && value.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Alan adından veritabanı adı türetir: `blog.ornek.com` → `wp_blog_ornek_com`.
/// Müşteri hesaplarında ad, öteki veritabanları gibi hesap adıyla başlar.
fn database_name(prefix: &str, label: &str, domain: &str, taken: &[String]) -> String {
    let slug: String = domain.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    let base: String = format!("{prefix}{label}_{slug}").chars().take(40).collect();
    let base = base.trim_end_matches('_').to_string();
    if !taken.contains(&base) {
        return base;
    }
    (2..).map(|n| format!("{base}_{n}")).find(|name| !taken.contains(name)).unwrap_or(base)
}

/// Kurulum için veritabanı ve tam yetkili kullanıcı açar; (ad, kullanıcı, parola) döner.
async fn create_database(state: &AppState, site: &Site) -> AppResult<(String, String, String)> {
    create_site_database(state, site, "wp").await
}

/// Site için adı alan adından türetilen bir MariaDB veritabanı ve tam yetkili kullanıcı açar;
/// (ad, kullanıcı, parola) döner. `label` adın önekidir (`wp`, `db`).
pub async fn create_site_database(state: &AppState, site: &Site, label: &str) -> AppResult<(String, String, String)> {
    let engine = Engine::Mariadb;
    engine.require_installed()?;
    let _guard = state.sys_lock.lock().await;
    let (owner_role, owner_name): (String, String) =
        state.db.call(|conn| conn.query_row("SELECT role, username FROM users WHERE id = ?1", params![site.user_id], |row| Ok((row.get(0)?, row.get(1)?))))?;
    let prefix = if owner_role == "admin" { String::new() } else { format!("{}_", owner_name.replace('-', "_")) };
    let taken: Vec<String> = state.db.call(|conn| {
        let mut stmt = conn.prepare("SELECT name FROM databases WHERE engine = 'mariadb' UNION SELECT name FROM database_users WHERE engine = 'mariadb'")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    })?;
    let name = database_name(&prefix, label, &site.domain, &taken);
    dbms::validate_name(&name, "Veritabanı adı")?;
    let password = auth::random_password(24);

    dbms::create_database(engine, &name).await?;
    // Sunucuda panelin bilmediği aynı adlı bir kullanıcı varsa oluşturma başarısız olur;
    // geri alma o yabancı kullanıcıyı silmemeli. Bu yüzden neyin oluşturulduğu izlenir.
    let mut user_created = false;
    let created = async {
        let database_id = state.db.call(|conn| {
            conn.execute("INSERT INTO databases (user_id, engine, name, created_at) VALUES (?1, 'mariadb', ?2, ?3)", params![site.user_id, name, now()])?;
            Ok(conn.last_insert_rowid())
        })?;
        dbms::create_user(engine, &name, "localhost", &password).await?;
        user_created = true;
        let user_id = state.db.call(|conn| {
            conn.execute(
                "INSERT INTO database_users (user_id, engine, name, host, created_at) VALUES (?1, 'mariadb', ?2, 'localhost', ?3)",
                params![site.user_id, name, now()],
            )?;
            Ok(conn.last_insert_rowid())
        })?;
        dbms::grant(engine, &name, &name, "localhost", Access::Full).await?;
        state.db.call(|conn| conn.execute("INSERT INTO database_grants (database_id, db_user_id, access) VALUES (?1, ?2, 'full')", params![database_id, user_id]))?;
        AppResult::Ok(())
    }
    .await;
    if let Err(err) = created {
        drop_database(state, &name, user_created).await;
        return Err(err);
    }
    Ok((name.clone(), name, password))
}

/// Yarım kalan kurulumun veritabanını ve (bu kurulum oluşturduysa) kullanıcısını geri alır.
pub async fn drop_database(state: &AppState, name: &str, drop_user: bool) {
    if drop_user {
        let _ = dbms::drop_user(Engine::Mariadb, name, "localhost").await;
    }
    let _ = dbms::drop_database(Engine::Mariadb, name).await;
    let _ = state.db.call(|conn| {
        conn.execute("DELETE FROM database_users WHERE engine = 'mariadb' AND name = ?1", params![name])?;
        conn.execute("DELETE FROM databases WHERE engine = 'mariadb' AND name = ?1", params![name])
    });
}

// ---- Kopyalama ----

/// Site klasöründe çalışan bir WordPress kurulumu var mı?
pub async fn is_wordpress(site: &Site) -> bool {
    match Wp::for_site(site) {
        Ok(wp) => wp.has_files().await,
        Err(_) => false,
    }
}

/// `wp-config.php` içindeki bir sabitin değerini değiştirir. Sabit bulunamazsa `None` döner.
/// Değer yalnızca harf, rakam ve alt çizgiden oluşmalıdır (çağıran doğrular).
fn replace_define(config: &str, key: &str, value: &str) -> Option<String> {
    let pattern = regex::Regex::new(&format!(r#"(?m)^[ \t]*define\(\s*['"]{key}['"]\s*,.*$"#)).ok()?;
    if !pattern.is_match(config) {
        return None;
    }
    let line = format!("define( '{key}', '{value}' );");
    Some(pattern.replace(config, regex::NoExpand(&line)).into_owned())
}

/// Kaynak sitedeki WordPress'in veritabanını hedef siteye kopyalar: hedef için yeni bir
/// veritabanı açılır, kaynak dökülüp oraya yüklenir, hedefin ayar dosyası yeni veritabanını
/// gösterecek şekilde düzeltilir ve içerikteki adresler yeni alan adına çevrilir.
/// Dosyaların hedefe önceden kopyalanmış olması gerekir.
pub async fn clone_database(ctx: &JobCtx, state: &AppState, source: &Site, target: &Site) -> AppResult<()> {
    let from = Wp::for_site(source)?;
    let to = Wp::for_site(target)?;
    let old_url = from.run_ok(&["option", "get", "siteurl"]).await?;
    let old_host = old_url.split("://").nth(1).unwrap_or(&source.domain).trim_end_matches('/').to_string();
    if old_host.is_empty() || old_host.chars().any(|ch| ch.is_whitespace() || ch == '\'') {
        return Err(AppError::bad("Kaynak sitenin adresi okunamadı"));
    }

    ctx.log("Veritabanı oluşturuluyor");
    let (database, db_user, db_password) = create_database(state, target).await?;
    let dump = format!("pixpanel-kopya-{}.sql", auth::random_token(8));
    let dump_path = from.user.tmp_dir().join(&dump);
    let dump_text = dump_path.to_string_lossy().into_owned();

    let copied = async {
        std::fs::create_dir_all(from.user.tmp_dir()).ok();
        let _ = std::os::unix::fs::chown(from.user.tmp_dir(), Some(from.user.uid), Some(from.user.gid));
        ctx.log("Kaynak veritabanı dökülüyor");
        from.stream(ctx, &["db", "export", &dump_text, "--add-drop-table"], None).await?;

        // Kopyalanan ayar dosyası hâlâ kaynağın veritabanını gösteriyor; yenisine çevrilir.
        let config_path = to.docroot.join("wp-config.php");
        let read = runas::output(runas::command(&to.user, &to.docroot, "cat", &["--", "wp-config.php"]), QUICK).await?;
        if !read.success() {
            return Err(AppError::bad("Kopyalanan sitede wp-config.php bulunamadı (yayın klasörünün içinde olmalı)"));
        }
        let mut config = read.stdout;
        for (key, value) in [("DB_NAME", &database), ("DB_USER", &db_user), ("DB_PASSWORD", &db_password), ("DB_HOST", &"localhost".to_string())] {
            config = replace_define(&config, key, value)
                .ok_or_else(|| AppError::bad(format!("wp-config.php içinde {key} tanımı bulunamadı; veritabanı ayarı elle düzenlenmiş olabilir")))?;
        }
        write_as_user(&to, "wp-config.php", &config).await.map_err(|_| AppError::internal(format!("{} yazılamadı", config_path.display())))?;

        ctx.log("Veritabanı yeni siteye yükleniyor");
        to.stream(ctx, &["db", "import", &dump_text], None).await?;

        let new_host = target.domain.as_str();
        ctx.log(format!("Adresler çevriliyor: {old_host} → {new_host}"));
        // Yeni sitenin henüz sertifikası yok; adresler http olarak yazılır (SSL açılınca WordPress ayarından değişir).
        for scheme in ["https", "http"] {
            to.stream(ctx, &["search-replace", &format!("{scheme}://{old_host}"), &format!("http://{new_host}"), "--all-tables", "--skip-columns=guid", "--report-changed-only"], None)
                .await?;
        }
        let _ = to.run(&["cache", "flush"]).await;
        AppResult::Ok(())
    }
    .await;
    let _ = std::fs::remove_file(&dump_path);
    if let Err(err) = copied {
        drop_database(state, &database, true).await;
        return Err(err);
    }
    ctx.log(format!("WordPress veritabanı kopyalandı: {database}"));
    Ok(())
}

// ---- Taşıma ----

/// `wp-config.php` içinde yazılı veritabanı bağlantısı.
pub struct ConfigDatabase {
    pub name: String,
    pub user: String,
    pub password: String,
    pub host: String,
}

/// `wp-config.php` metnindeki bir sabitin değerini okur (yorum satırları sayılmaz).
fn read_define(config: &str, key: &str) -> Option<String> {
    let pattern = regex::Regex::new(&format!(r#"(?m)^[ \t]*define\(\s*['"]{key}['"]\s*,\s*(?:'((?:[^'\\]|\\.)*)'|"((?:[^"\\]|\\.)*)")\s*\)"#)).ok()?;
    let found = pattern.captures(config)?;
    let (raw, single) = match (found.get(1), found.get(2)) {
        (Some(value), _) => (value.as_str(), true),
        (_, Some(value)) => (value.as_str(), false),
        _ => return None,
    };
    // PHP tek tırnakta yalnızca \' ve \\ kaçışlarını, çift tırnakta ayrıca \" ve \$ kaçışlarını çözer.
    let mut value = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        match (ch, chars.peek().copied()) {
            ('\\', Some(next)) if next == '\\' || (single && next == '\'') || (!single && (next == '"' || next == '$')) => {
                value.push(next);
                chars.next();
            }
            _ => value.push(ch),
        }
    }
    Some(value)
}

/// Ayar dosyası metninden veritabanı bağlantısını çıkarır.
pub fn parse_config_database(config: &str) -> Option<ConfigDatabase> {
    Some(ConfigDatabase {
        name: read_define(config, "DB_NAME")?,
        user: read_define(config, "DB_USER")?,
        password: read_define(config, "DB_PASSWORD")?,
        host: read_define(config, "DB_HOST").unwrap_or_else(|| "localhost".to_string()),
    })
}

/// Sitenin yayın klasöründeki `wp-config.php`den veritabanı bağlantısını okur; dosya yoksa `None`.
pub async fn config_database(site: &Site) -> AppResult<Option<ConfigDatabase>> {
    let wp = Wp::for_site(site)?;
    let read = runas::output(runas::command(&wp.user, &wp.docroot, "cat", &["--", "wp-config.php"]), QUICK).await?;
    if !read.success() {
        return Ok(None);
    }
    parse_config_database(&read.stdout).map(Some).ok_or_else(|| AppError::bad("wp-config.php içindeki veritabanı bilgileri okunamadı; veritabanını elle taşıyın"))
}

/// Başka yerden getirilen bir WordPress'i bu sunucudaki yeni bir veritabanına bağlar: veritabanı
/// açılır, `load` onu doldurur, ayar dosyası yeni veritabanını gösterecek şekilde düzeltilir ve
/// içerikteki adresler sitenin alan adından farklıysa çevrilir. Başarısız olursa veritabanı geri alınır.
pub async fn adopt_database<F, Fut>(ctx: &JobCtx, state: &AppState, site: &Site, load: F) -> AppResult<()>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = AppResult<()>>,
{
    let wp = Wp::for_site(site)?;
    ensure_cli().await?;
    ctx.log("Veritabanı oluşturuluyor");
    let (database, db_user, db_password) = create_database(state, site).await?;
    let adopted = async {
        load(database.clone()).await?;
        let read = runas::output(runas::command(&wp.user, &wp.docroot, "cat", &["--", "wp-config.php"]), QUICK).await?;
        if !read.success() {
            return Err(AppError::bad("wp-config.php okunamadı"));
        }
        let mut config = read.stdout;
        for (key, value) in [("DB_NAME", &database), ("DB_USER", &db_user), ("DB_PASSWORD", &db_password), ("DB_HOST", &"localhost".to_string())] {
            config = replace_define(&config, key, value)
                .ok_or_else(|| AppError::bad(format!("wp-config.php içinde {key} tanımı bulunamadı; veritabanı ayarı elle düzenlenmiş olabilir")))?;
        }
        write_as_user(&wp, "wp-config.php", &config).await.map_err(|_| AppError::internal("wp-config.php yazılamadı"))?;
        ctx.log(format!("wp-config.php yeni veritabanına bağlandı: {database}"));

        // Site başka bir adla taşındıysa içerikteki adresler yeni ada çevrilir; aynı adla
        // taşındıysa (olağan durum) dokunulmaz.
        let old_url = wp.run_ok(&["option", "get", "siteurl"]).await?;
        let old_host = old_url.split("://").nth(1).unwrap_or("").trim_end_matches('/').to_string();
        let known = old_host == site.domain || site.aliases.iter().any(|alias| *alias == old_host);
        if !known && !old_host.is_empty() && !old_host.chars().any(|ch| ch.is_whitespace() || ch == '\'') {
            let new_host = site.domain.as_str();
            ctx.log(format!("Adresler çevriliyor: {old_host} → {new_host}"));
            for scheme in ["https", "http"] {
                wp.stream(ctx, &["search-replace", &format!("{scheme}://{old_host}"), &format!("http://{new_host}"), "--all-tables", "--skip-columns=guid", "--report-changed-only"], None)
                    .await?;
            }
        }
        let _ = wp.run(&["cache", "flush"]).await;
        AppResult::Ok(())
    }
    .await;
    if let Err(err) = adopted {
        drop_database(state, &database, true).await;
        return Err(err);
    }
    Ok(())
}

/// Kurulabilirliği görev başlamadan denetler (hata hemen kullanıcıya dönsün diye).
pub async fn check_installable(site: &Site) -> AppResult<()> {
    ensure_cli().await?;
    let wp = Wp::for_site(site)?;
    Engine::Mariadb.require_installed()?;
    if wp.has_files().await {
        return Err(AppError::conflict("Bu sitede WordPress zaten kurulu"));
    }
    if !wp.docroot_is_empty().await {
        return Err(AppError::bad("Site klasörü boş değil; WordPress yalnızca boş siteye kurulabilir"));
    }
    Ok(())
}

/// Aynı sitede aynı anda tek bir kurulum ya da güncelleme yürür: iki wp-cli süreci aynı
/// dosyaları değiştirirse site bozulur, her biri ayrıca tam yedek alır.
static BUSY_SITES: std::sync::Mutex<Vec<i64>> = std::sync::Mutex::new(Vec::new());

/// Sitenin meşgul işaretini tutar; düşünce işaret kalkar (görev paniklese bile).
pub struct SiteLock(i64);

impl SiteLock {
    pub fn acquire(site: &Site) -> AppResult<Self> {
        let mut busy = BUSY_SITES.lock().unwrap_or_else(|p| p.into_inner());
        if busy.contains(&site.id) {
            return Err(AppError::conflict("Bu sitede süren bir WordPress işlemi var; bitmesini bekleyin"));
        }
        busy.push(site.id);
        Ok(Self(site.id))
    }
}

impl Drop for SiteLock {
    fn drop(&mut self) {
        BUSY_SITES.lock().unwrap_or_else(|p| p.into_inner()).retain(|id| *id != self.0);
    }
}

pub async fn install(ctx: &JobCtx, state: &AppState, site: &Site, request: &InstallRequest) -> AppResult<()> {
    let wp = Wp::for_site(site)?;
    ctx.log("Veritabanı oluşturuluyor");
    let (database, db_user, db_password) = create_database(state, site).await?;
    ctx.log(format!("Veritabanı: {database}"));

    let outcome = install_files(ctx, site, &wp, request, &database, &db_user, &db_password).await;
    if outcome.is_ok() {
        // Site WordPress'e özgü korumalarla sunulur (yüklenen dosyalarda PHP çalışmaz, xmlrpc kapalıdır).
        let mut updated = site.clone();
        updated.template = "wordpress".into();
        let _guard = state.sys_lock.lock().await;
        let applied = async {
            crate::hosting::sites::save(&state.db, &updated)?;
            crate::hosting::sites::provision(&updated).await
        }
        .await;
        match applied {
            Ok(()) => ctx.log("WordPress şablonu uygulandı"),
            // Şablon uygulanamasa da kurulum geçerlidir; elle seçilebilir.
            Err(err) => ctx.log(format!("UYARI: WordPress şablonu uygulanamadı: {err}")),
        }
    }
    if outcome.is_err() {
        // Yarım kurulum, sonraki denemenin "zaten kurulu" ya da "klasör boş değil" diye
        // reddedilmesine yol açmasın. Klasör kurulumdan önce boştu (karşılama sayfası dışında);
        // bu yüzden içindeki her şey bu kurulumun bıraktığıdır.
        ctx.log("Kurulum tamamlanamadı; indirilen dosyalar ve veritabanı geri alınıyor");
        let clean = runas::command(&wp.user, &wp.docroot, "find", &[".", "-mindepth", "1", "-not", "-name", "index.html", "-delete"]);
        let _ = runas::output(clean, Duration::from_secs(120)).await;
        drop_database(state, &database, true).await;
    }
    outcome
}

async fn install_files(ctx: &JobCtx, site: &Site, wp: &Wp, request: &InstallRequest, database: &str, db_user: &str, db_password: &str) -> AppResult<()> {
    let locale = format!("--locale={}", request.locale);
    wp.stream(ctx, &["core", "download", &locale], None).await?;

    // Veritabanı parolası argüman olarak değil, girdiden verilir.
    let dbname = format!("--dbname={database}");
    let dbuser = format!("--dbuser={db_user}");
    wp.run_secret(ctx, &["config", "create", &dbname, &dbuser, "--dbhost=localhost", &locale, "--prompt=dbpass"], db_password).await?;
    // Yönetim panelinden dosya düzenlemek, ele geçirilen bir yönetici hesabını kod çalıştırmaya çevirir.
    wp.stream(ctx, &["config", "set", "DISALLOW_FILE_EDIT", "true", "--raw"], None).await?;
    let chmod = runas::command(&wp.user, &wp.docroot, "chmod", &["640", "wp-config.php"]);
    let _ = runas::output(chmod, QUICK).await;

    let scheme = if site.ssl_mode == "none" { "http" } else { "https" };
    let url = format!("--url={scheme}://{}", site.domain);
    let title = format!("--title={}", request.title);
    let admin_user = format!("--admin_user={}", request.admin_user);
    let admin_email = format!("--admin_email={}", request.admin_email);
    wp.run_secret(ctx, &["core", "install", &url, &title, &admin_user, &admin_email, "--skip-email", "--prompt=admin_password"], &request.admin_password).await?;

    wp.stream(ctx, &["rewrite", "structure", "/%postname%/"], None).await?;
    if request.locale == "tr_TR" {
        wp.stream(ctx, &["option", "update", "timezone_string", "Europe/Istanbul"], None).await?;
    }
    // Panelin karşılama sayfası WordPress'in önüne geçmesin.
    let remove = runas::command(&wp.user, &wp.docroot, "rm", &["-f", "--", "index.html"]);
    let _ = runas::output(remove, QUICK).await;
    ctx.log(format!("WordPress kuruldu: {scheme}://{}", site.domain));
    Ok(())
}

// ---- Güncelleme ve ayarlar ----

/// Güncelleme görevini başlatır: önce (yedek yeri varsa) sitenin ve veritabanının yedeği
/// alınır, sonra güncelleme yapılır, en son sitenin hâlâ yanıt verdiği denetlenir.
pub fn start_update_job(state: &AppState, user_id: Option<i64>, site: Site, what: String) -> AppResult<i64> {
    let title = format!("WordPress güncellemesi: {}", site.domain);
    let job_state = state.clone();
    let lock = SiteLock::acquire(&site)?;
    state.jobs.start("wordpress.update", &title, user_id, move |ctx| async move {
        let _lock = lock;
        backup_before_update(&ctx, &job_state, &site).await?;
        // Site güncellemeden önce de bozuksa geri dönmenin anlamı yoktur; sorun güncellemeden gelmemiştir.
        let healthy_before = !matches!(probe_status(&site).await, Some(status) if status >= 500);
        let snapshot = if healthy_before { rollback::take(&ctx, &site).await } else { None };
        let updated = update(&ctx, &site, &what).await;
        let mut outcome = match updated {
            Ok(()) => verify_after_update(&ctx, &site).await,
            Err(err) => Err(err),
        };
        // Güncelleme siteyi bozduysa (sunucu hatası dönüyorsa) güncellemeden önceki hâl geri konur.
        let broken = healthy_before && matches!(probe_status(&site).await, Some(status) if status >= 500);
        if let (true, Some(snapshot)) = (broken, &snapshot) {
            outcome = match rollback::restore(&ctx, &site, snapshot).await {
                Ok(()) => Err(AppError::internal(
                    "Güncelleme siteyi bozdu (sunucu hatası veriyordu); site güncellemeden önceki hâline döndürüldü. Güncellemeleri tek tek kurarak soruna yol açanı bulabilirsiniz.",
                )),
                Err(err) => Err(AppError::internal(format!("Güncelleme siteyi bozdu ve eski hâline döndürülemedi: {err}. Yedekler sayfasından güncelleme öncesi yedeğe dönün."))),
            };
        }
        if let Some(snapshot) = snapshot {
            rollback::discard(&site, snapshot).await;
        }
        // Genel bakıştaki "güncelleme bekliyor" bulgusu bir sonraki taramayı beklemeden kalksın.
        crate::health::rescan_wordpress_site(&job_state, &site).await;
        outcome
    })
}

/// Güncellemeden önce sitenin dosyalarını ve WordPress veritabanını yedekler. Yedek
/// yeri yoksa uyarıp sürer; yedek alınamazsa güncelleme yapılmaz.
async fn backup_before_update(ctx: &JobCtx, state: &AppState, site: &Site) -> AppResult<()> {
    let target = if crate::backup::is_installed() { crate::backup::list_targets(&state.db)?.into_iter().next() } else { None };
    let Some(target) = target else {
        ctx.log("UYARI: Yedek yeri tanımlı olmadığı için güncellemeden önce yedek alınamadı. Yedekler sayfasından bir yedek yeri ekleyin.");
        return Ok(());
    };
    let wp = Wp::for_site(site)?;
    let database = wp.run_ok(&["config", "get", "DB_NAME", "--skip-plugins", "--skip-themes"]).await.unwrap_or_default();
    let database_id: Option<i64> = state.db.call(|conn| {
        use rusqlite::OptionalExtension;
        conn.query_row("SELECT id FROM databases WHERE engine = 'mariadb' AND name = ?1 AND user_id = ?2", params![database, site.user_id], |row| row.get(0)).optional()
    })?;
    let scope = crate::backup::Scope { site_ids: vec![site.id], database_ids: database_id.into_iter().collect(), ..Default::default() };
    ctx.log(format!("== Güncellemeden önce yedek alınıyor ({})", target.name));
    crate::backup::run::run_inline(state, ctx, &target, &scope)
        .await
        .map_err(|err| AppError::internal(format!("Yedek alınamadığı için güncelleme yapılmadı: {err}")))?;
    ctx.log(String::new());
    Ok(())
}

/// Sitenin sunucunun içinden yoklanınca döndürdüğü durum kodu; yoklanamıyorsa `None`.
async fn probe_status(site: &Site) -> Option<u16> {
    crate::uptime::probe(&site.domain, &site.ip, site.force_https && site.ssl_mode != "none", Duration::from_secs(20)).await.ok()
}

/// Güncellemeden sonra siteyi sunucunun içinden yoklar; sunucu hatası dönüyorsa görev
/// başarısız sayılır ki yönetici hemen fark edip az önce alınan yedeğe dönebilsin.
async fn verify_after_update(ctx: &JobCtx, site: &Site) -> AppResult<()> {
    match crate::uptime::probe(&site.domain, &site.ip, site.force_https && site.ssl_mode != "none", Duration::from_secs(20)).await {
        Ok(status) if status >= 500 => Err(AppError::internal(format!(
            "Site güncellemeden sonra hata veriyor (HTTP {status}). Yedekler sayfasından az önce alınan yedeğe dönebilirsiniz."
        ))),
        Ok(status) => {
            ctx.log(format!("Site yanıt veriyor (HTTP {status})."));
            Ok(())
        }
        Err(_) => {
            ctx.log("UYARI: Site yoklanamadı; elle denetleyin.");
            Ok(())
        }
    }
}

/// Bekleyen güncellemeler; sitede WordPress yoksa `None`.
#[derive(Debug, Clone, Default)]
pub struct Pending {
    pub core: Option<String>,
    pub plugins: usize,
    pub themes: usize,
}

impl Pending {
    pub fn is_empty(&self) -> bool {
        self.core.is_none() && self.plugins == 0 && self.themes == 0
    }
}

pub async fn pending_updates(site: &Site) -> Option<Pending> {
    let wp = Wp::for_site(site).ok()?;
    if !wp.has_files().await {
        return None;
    }
    let count = |output: AppResult<CmdOutput>| output.ok().filter(CmdOutput::success).and_then(|out| out.stdout.trim().parse::<usize>().ok()).unwrap_or(0);
    let (core, plugins, themes) = tokio::join!(
        wp.run(&["core", "check-update", "--format=json", "--skip-plugins", "--skip-themes"]),
        wp.run(&["plugin", "list", "--update=available", "--format=count", "--skip-plugins", "--skip-themes"]),
        wp.run(&["theme", "list", "--update=available", "--format=count", "--skip-plugins", "--skip-themes"]),
    );
    let core = core
        .ok()
        .filter(CmdOutput::success)
        .and_then(|out| serde_json::from_str::<Vec<CoreUpdate>>(out.stdout.trim()).ok())
        .and_then(|list| list.into_iter().next_back())
        .map(|update| update.version);
    Some(Pending { core, plugins: count(plugins), themes: count(themes) })
}

pub async fn update(ctx: &JobCtx, site: &Site, what: &str) -> AppResult<()> {
    let wp = Wp::for_site(site)?;
    if matches!(what, "all" | "core") {
        wp.stream(ctx, &["core", "update"], None).await?;
        wp.stream(ctx, &["core", "update-db"], None).await?;
    }
    if matches!(what, "all" | "plugins") {
        wp.stream(ctx, &["plugin", "update", "--all"], None).await?;
    }
    if matches!(what, "all" | "themes") {
        wp.stream(ctx, &["theme", "update", "--all"], None).await?;
    }
    if what == "all" {
        // Çeviri güncellemesi başarısız olursa asıl güncelleme başarısız sayılmaz.
        let _ = wp.stream(ctx, &["language", "core", "update"], None).await;
    }
    ctx.log("Güncelleme tamamlandı");
    Ok(())
}

/// Eklenti ve tema adları wp-cli'ye argüman olarak gider; yalnızca dizin adı biçimi kabul edilir.
pub fn validate_slug(value: &str) -> AppResult<()> {
    let valid = (1..=100).contains(&value.len())
        && value.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if valid { Ok(()) } else { Err(AppError::bad("Geçersiz eklenti ya da tema adı")) }
}

/// Tek bir eklenti ya da tema üzerinde işlem yapar. `kind`: `plugin` ya da `theme`.
pub async fn extension_action(site: &Site, kind: &str, slug: &str, action: &str) -> AppResult<String> {
    validate_slug(slug)?;
    let wp = Wp::for_site(site)?;
    let args: Vec<&str> = match (kind, action) {
        ("plugin", "activate" | "deactivate" | "update" | "delete") | ("theme", "activate" | "update" | "delete") => vec![kind, action, slug],
        (_, "auto-on") => vec![kind, "auto-updates", "enable", slug],
        (_, "auto-off") => vec![kind, "auto-updates", "disable", slug],
        _ => return Err(AppError::bad("Geçersiz işlem")),
    };
    wp.run_ok(&args).await
}

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub maintenance: Option<bool>,
    /// `minor`, `all` ya da `off`.
    pub auto_core: Option<String>,
    /// Bütün eklentilerin kendiliğinden güncellenmesi.
    pub auto_plugins: Option<bool>,
    pub auto_themes: Option<bool>,
}

pub async fn apply_settings(site: &Site, settings: &Settings) -> AppResult<()> {
    let wp = Wp::for_site(site)?;
    if let Some(maintenance) = settings.maintenance {
        let active = wp.run(&["maintenance-mode", "is-active"]).await?.success();
        if active != maintenance {
            wp.run_ok(&["maintenance-mode", if maintenance { "activate" } else { "deactivate" }]).await?;
        }
    }
    if let Some(level) = settings.auto_core.as_deref() {
        let value = match level {
            "minor" => "'minor'",
            "all" => "true",
            "off" => "false",
            _ => return Err(AppError::bad("Geçersiz çekirdek güncelleme seçeneği")),
        };
        wp.run_ok(&["config", "set", "WP_AUTO_UPDATE_CORE", value, "--raw"]).await?;
    }
    for (kind, wanted) in [("plugin", settings.auto_plugins), ("theme", settings.auto_themes)] {
        if let Some(enabled) = wanted {
            // Hiç eklenti yoksa wp-cli hata döner; bu bir sorun değildir.
            let _ = wp.run(&[kind, "auto-updates", if enabled { "enable" } else { "disable" }, "--all"]).await?;
        }
    }
    Ok(())
}

// ---- Parolasız giriş ----

/// İlk yöneticinin oturumunu açan tek kullanımlık, kısa ömürlü bir adres üretir.
///
/// Siteye küçük bir "olmazsa olmaz eklenti" bırakılır; eklenti adresteki anahtarın
/// özetini veritabanındaki kayıtla karşılaştırır ve kaydı ilk kullanımda siler.
pub async fn login_url(site: &Site) -> AppResult<String> {
    let wp = Wp::for_site(site)?;
    let admin = wp.run_ok(&["user", "list", "--role=administrator", "--field=ID", "--number=1", "--orderby=ID", "--order=ASC", "--skip-plugins", "--skip-themes"]).await?;
    let admin_id: i64 = admin.lines().next().and_then(|line| line.trim().parse().ok()).ok_or_else(|| AppError::bad("Bu sitede yönetici hesabı bulunamadı"))?;

    let mkdir = runas::command(&wp.user, &wp.docroot, "mkdir", &["-p", "--", "wp-content/mu-plugins"]);
    runas::output(mkdir, QUICK).await?;
    write_as_user(&wp, "wp-content/mu-plugins/pixpanel-login.php", LOGIN_PLUGIN).await?;

    let token = auth::random_token(32);
    let record = serde_json::json!({
        "hash": hex::encode(Sha256::digest(token.as_bytes())),
        "expires": now() + LOGIN_TTL_SECS,
        "user": admin_id,
    });
    wp.run_with_input(&["option", "update", LOGIN_OPTION, "--format=json", "--autoload=no", "--skip-plugins", "--skip-themes"], &record.to_string()).await?;
    let stored = wp.run_ok(&["option", "get", "siteurl", "--skip-plugins", "--skip-themes"]).await.unwrap_or_default();
    Ok(format!("{}/?pixpanel_login={token}", login_base(&site.domain, &site.aliases, site.ssl_mode != "none", &stored)))
}

/// Giriş adresinin kökü. `siteurl` sitenin veritabanından gelir ve site sahibi onu
/// istediği gibi değiştirebilir; adres panelde açıldığı için yalnızca sitenin kendi alan
/// adlarından birini gösteren http(s) adresi kabul edilir (yoksa `javascript:` gibi bir
/// değer, düğmeye basan yöneticinin oturumunda betik çalıştırırdı). Uymuyorsa adres
/// panelin bildiği alan adından kurulur.
fn login_base(domain: &str, aliases: &[String], secure: bool, stored: &str) -> String {
    let stored = stored.trim().trim_end_matches('/');
    let host_and_path = stored.strip_prefix("https://").or_else(|| stored.strip_prefix("http://"));
    let trusted = host_and_path.is_some_and(|rest| {
        let host = rest.split(['/', '?', '#']).next().unwrap_or("");
        let plain = stored.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '/' | '.' | '-' | '_' | '~'));
        plain && (host == domain || aliases.iter().any(|alias| alias == host))
    });
    if trusted {
        return stored.to_string();
    }
    format!("{}://{domain}", if secure { "https" } else { "http" })
}

/// Dosyayı sitenin kullanıcısı olarak yazar (içerik `tee`nin girdisinden verilir).
async fn write_as_user(wp: &Wp, relative: &str, content: &str) -> AppResult<()> {
    use tokio::io::AsyncWriteExt;
    let mut cmd = runas::command(&wp.user, &wp.docroot, "tee", &["--", relative]);
    cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|err| AppError::internal(format!("dosya yazılamadı: {err}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(content.as_bytes()).await.map_err(|err| AppError::internal(err.to_string()))?;
        let _ = stdin.shutdown().await;
    }
    let status = tokio::time::timeout(QUICK, child.wait()).await.map_err(|_| AppError::internal("dosya yazma zaman aşımına uğradı"))??;
    if status.success() { Ok(()) } else { Err(AppError::internal("Giriş eklentisi site klasörüne yazılamadı")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_database_is_read() {
        let config = r#"<?php
// define('DB_NAME', 'yorum');
define( 'DB_NAME', 'wp_site' );
define("DB_USER", "kullanici");
  define('DB_PASSWORD', 'a\'b\\c"d$e');
define( 'DB_HOST', 'localhost:3307' );
"#;
        let database = parse_config_database(config).unwrap();
        assert_eq!(database.name, "wp_site");
        assert_eq!(database.user, "kullanici");
        assert_eq!(database.password, r#"a'b\c"d$e"#);
        assert_eq!(database.host, "localhost:3307");
        assert!(parse_config_database("<?php define('DB_NAME', getenv('X'));").is_none());
    }

    #[test]
    fn define_replacement() {
        let config = "<?php\ndefine( 'DB_NAME', 'eski' );\n  define(\"DB_PASSWORD\", 'a$1b');\n// define('DB_USER', 'yorum');\n$table_prefix = 'wp_';\n";
        let out = replace_define(config, "DB_NAME", "yeni_ad").unwrap();
        assert!(out.contains("define( 'DB_NAME', 'yeni_ad' );"));
        assert!(!out.contains("'eski'"));
        // Değerdeki `$` yer tutucu sayılmaz; satırın gerisi olduğu gibi kalır.
        let out = replace_define(config, "DB_PASSWORD", "Yeni1Parola").unwrap();
        assert!(out.contains("define( 'DB_PASSWORD', 'Yeni1Parola' );"));
        assert!(out.contains("$table_prefix = 'wp_';"));
        // Yorum satırındaki tanım sayılmaz; gerçek tanım yoksa değişiklik yapılmaz.
        assert!(replace_define(config, "DB_USER", "x").is_none());
        assert!(replace_define(config, "DB_HOST", "x").is_none());
    }

    fn request() -> InstallRequest {
        InstallRequest {
            title: "Örnek Site".into(),
            admin_user: "yonetici".into(),
            admin_email: "ali@ornek.com".into(),
            admin_password: "Guclu-Parola-123".into(),
            locale: String::new(),
        }
    }

    #[test]
    fn install_request_defaults_to_turkish() {
        assert_eq!(request().validated().unwrap().locale, "tr_TR");
    }

    #[test]
    fn install_request_rejects_option_like_and_unsafe_values() {
        for user in ["-u", "a b", "ab", "kök", "x;id"] {
            assert!(InstallRequest { admin_user: user.into(), ..request() }.validated().is_err(), "{user}");
        }
        for email in ["ali", "ali@", "@ornek.com", "ali@ornek", "ali@ornek.com --allow-root", "a\nb@c.com"] {
            assert!(InstallRequest { admin_email: email.into(), ..request() }.validated().is_err(), "{email}");
        }
        for locale in ["../x", "TR", "t", "tr-TR", "tr_TR --exec"] {
            assert!(InstallRequest { locale: locale.into(), ..request() }.validated().is_err(), "{locale}");
        }
        assert!(InstallRequest { title: "a\nb".into(), ..request() }.validated().is_err());
        assert!(InstallRequest { admin_password: "kisa".into(), ..request() }.validated().is_err());
    }

    #[test]
    fn database_name_is_derived_from_domain() {
        assert_eq!(database_name("", "wp", "blog.ornek.com", &[]), "wp_blog_ornek_com");
        assert_eq!(database_name("musteri_", "wp", "Ornek.com", &[]), "musteri_wp_ornek_com");
        assert_eq!(database_name("", "wp", "ornek.com", &["wp_ornek_com".into()]), "wp_ornek_com_2");
        let long = database_name("", "wp", "cok-uzun-bir-alan-adi-ornegi.sirket-adi.com.tr", &[]);
        assert!(long.len() <= 40 && dbms::validate_name(&long, "ad").is_ok(), "{long}");
    }

    #[test]
    fn slugs_cannot_be_options_or_paths() {
        assert!(validate_slug("akismet").is_ok());
        assert!(validate_slug("woo-commerce_2.1").is_ok());
        for slug in ["", "--all", "-x", "../hello", "a/b", "a b"] {
            assert!(validate_slug(slug).is_err(), "{slug}");
        }
    }

    #[test]
    fn extension_list_tolerates_odd_field_types() {
        let json = r#"[
            {"name":"akismet","title":"Akismet","status":"active","update":"available","version":"5.3","update_version":"5.4","auto_update":"on"},
            {"name":"pixpanel-login","title":"","status":"must-use","update":false,"version":"","update_version":"","auto_update":"off"},
            {"name":"ozel","title":null,"status":"must-use","update":false,"version":1,"update_version":false,"auto_update":"off"}
        ]"#;
        let list = parse_extensions(json);
        assert_eq!(list.len(), 2, "panelin kendi eklentisi listelenmez");
        assert_eq!(list[0].update_version.as_deref(), Some("5.4"));
        assert_eq!((list[1].name.as_str(), list[1].title.as_str(), list[1].version.as_str(), &list[1].update_version), ("ozel", "", "1", &None));
        assert!(parse_extensions("bozuk").is_empty());
    }

    #[test]
    fn auto_core_setting_is_read_from_json() {
        assert_eq!(auto_core_level("true"), "all");
        assert_eq!(auto_core_level("false\n"), "off");
        assert_eq!(auto_core_level("\"minor\""), "minor");
        assert_eq!(auto_core_level(""), "minor");
    }

    #[test]
    fn login_address_never_trusts_foreign_or_script_urls() {
        let aliases = vec!["www.ornek.com".to_string()];
        let base = |stored: &str| login_base("ornek.com", &aliases, true, stored);
        assert_eq!(base("https://ornek.com"), "https://ornek.com");
        assert_eq!(base("https://www.ornek.com/"), "https://www.ornek.com");
        assert_eq!(base("http://ornek.com/blog"), "http://ornek.com/blog", "alt klasöre kurulu site");
        // Site sahibinin veritabanına yazabileceği zararlı değerler panelin bildiği adrese düşer.
        for stored in ["javascript:alert(1)//", "https://kotu.example", "https://ornek.com.kotu.example", "data:text/html,x", "https://ornek.com/\"><script>", "https://ornek.com@kotu.example", ""] {
            assert_eq!(base(stored), "https://ornek.com", "{stored}");
        }
        assert_eq!(login_base("ornek.com", &[], false, "ftp://ornek.com"), "http://ornek.com");
    }

    #[test]
    fn cli_error_uses_last_meaningful_line() {
        let output = CmdOutput { code: 1, stdout: String::new(), stderr: "Warning: x\nError: Veritabanına bağlanılamadı.\n\n".into() };
        assert_eq!(cli_error(&output), "Veritabanına bağlanılamadı.");
    }
}
