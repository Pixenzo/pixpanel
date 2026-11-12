//! WordPress sıkılaştırma denetimi: sitenin ayarlarını bilinen güvenlik kontrol listesine göre
//! okur ve düzeltilebilenleri tek tıkla düzeltir.
//!
//! Okuma ve yazma işlemlerinin hepsi sitenin Linux kullanıcısıyla (`Wp`) yürür; panelin yetkisiyle
//! site dosyası okunmaz ya da yazılmaz. `wp-config.php` metni ayrıştırılarak okunur (wp-cli ile
//! her sabit için ayrı süreç açmak yerine tek `cat`), yazma ise `wp config set` ile yapılır.

use std::time::Duration;

use regex::Regex;
use serde::Serialize;

use super::{cli_error, read_define, ensure_cli, Wp, QUICK};
use crate::error::{AppError, AppResult};
use crate::hosting::sites::{self, Site};
use crate::state::AppState;
use crate::sys::exec::CmdOutput;
use crate::sys::runas;

/// Sağlama toplamı denetimi bütün çekirdek dosyalarını okur; sıradan komutlardan uzun sürebilir.
const SLOW: Duration = Duration::from_secs(120);

/// WordPress'in kurulumda ürettiği örnek anahtar metni; değiştirilmemişse anahtar yok demektir.
const DEFAULT_SALT: &str = "put your unique phrase here";
const SALT_KEYS: [&str; 8] = ["AUTH_KEY", "SECURE_AUTH_KEY", "LOGGED_IN_KEY", "NONCE_KEY", "AUTH_SALT", "SECURE_AUTH_SALT", "LOGGED_IN_SALT", "NONCE_SALT"];
/// Bundan kısa anahtar, rastgele üretilmiş sayılmaz.
const MIN_SALT_LEN: usize = 16;

/// Denetimdeki bütün maddeler (arayüzde gösterim sırası).
pub const ALL_IDS: [&str; 9] = ["file_edit", "debug_display", "config_permissions", "salts", "template", "auto_minor", "core_checksums", "admin_user", "unused_extensions"];
/// Panelin kendisinin düzeltebildiği maddeler.
const FIXABLE_IDS: [&str; 6] = ["file_edit", "debug_display", "config_permissions", "salts", "template", "auto_minor"];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SecurityItem {
    pub id: &'static str,
    pub title: &'static str,
    pub ok: bool,
    pub detail: String,
    /// Tamam değil ve panel düzeltebiliyor.
    pub fixable: bool,
    /// Bilgi maddesi: özetteki sayıma girmez.
    pub info: bool,
}

#[derive(Debug, Serialize)]
pub struct Audit {
    pub items: Vec<SecurityItem>,
    /// Tamam olan (bilgi olmayan) madde sayısı.
    pub passed: usize,
    /// Bilgi olmayan madde sayısı.
    pub total: usize,
}

impl Audit {
    fn new(items: Vec<SecurityItem>) -> Self {
        let counted = items.iter().filter(|item| !item.info);
        let total = counted.clone().count();
        let passed = counted.filter(|item| item.ok).count();
        Self { items, passed, total }
    }
}

#[derive(Debug, Serialize)]
pub struct FixFailure {
    pub id: &'static str,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct FixResult {
    pub fixed: Vec<&'static str>,
    /// Zaten tamam olduğu için dokunulmayanlar.
    pub skipped: Vec<&'static str>,
    pub failed: Vec<FixFailure>,
    /// Düzeltmelerden sonraki denetim.
    pub audit: Audit,
}

/// İstekteki madde kimliklerini doğrular: bilinmeyen ya da otomatik düzeltilemeyen kimlik reddedilir.
pub fn validate_ids(ids: &[String]) -> AppResult<Vec<&'static str>> {
    let mut valid: Vec<&'static str> = Vec::new();
    for id in ids {
        let known: &'static str = ALL_IDS.iter().copied().find(|known| *known == id.as_str()).ok_or_else(|| AppError::bad("Geçersiz denetim maddesi"))?;
        if !FIXABLE_IDS.contains(&known) {
            return Err(AppError::bad(format!("“{}” maddesi otomatik düzeltilemez; elle düzeltilmeli", title_of(known))));
        }
        if !valid.contains(&known) {
            valid.push(known);
        }
    }
    if valid.is_empty() {
        return Err(AppError::bad("Düzeltilecek en az bir madde seçin"));
    }
    Ok(valid)
}

fn title_of(id: &str) -> &'static str {
    match id {
        "file_edit" => "Yönetim panelinden dosya düzenleme kapalı",
        "debug_display" => "Hata iletileri ziyaretçilere gösterilmiyor",
        "config_permissions" => "wp-config.php dosya izni güvenli",
        "salts" => "Güvenlik anahtarları benzersiz",
        "template" => "Site WordPress koruma şablonunu kullanıyor",
        "auto_minor" => "Güvenlik sürümleri kendiliğinden kuruluyor",
        "core_checksums" => "Çekirdek dosyaları değiştirilmemiş",
        "admin_user" => "“admin” adlı yönetici hesabı yok",
        _ => "Kullanılmayan eklenti ve temalar",
    }
}

// ---- wp-config.php sabitleri ----

/// `wp-config.php` içindeki bir `define()` değeri.
#[derive(Debug, PartialEq)]
enum Constant {
    Bool(bool),
    Number(i64),
    Text(String),
    /// Değer bir ifade (`getenv(...)`, başka sabit…); statik olarak okunamaz.
    Other,
}

impl Constant {
    /// PHP'deki doğruluk değeri; ifade ise bilinmez.
    fn truthy(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            Self::Number(value) => Some(*value != 0),
            Self::Text(text) => Some(!(text.is_empty() || text == "0")),
            Self::Other => None,
        }
    }
}

/// Sabitin ilk tanımını okurken yorum satırları sayılmaz. Tanım yoksa `None` döner.
fn read_constant(config: &str, key: &str) -> Option<Constant> {
    let defined = Regex::new(&format!(r#"(?m)^[ \t]*define\(\s*['"]{key}['"]\s*,"#)).ok()?;
    if !defined.is_match(config) {
        return None;
    }
    if let Some(text) = read_define(config, key) {
        return Some(Constant::Text(text));
    }
    let literal = Regex::new(&format!(r#"(?m)^[ \t]*define\(\s*['"]{key}['"]\s*,\s*([A-Za-z_]+|-?[0-9]+)\s*\)"#)).ok()?;
    let found = literal.captures(config).and_then(|caps| caps.get(1)).map(|m| m.as_str().to_string());
    Some(match found.as_deref() {
        Some(word) if word.eq_ignore_ascii_case("true") => Constant::Bool(true),
        Some(word) if word.eq_ignore_ascii_case("false") => Constant::Bool(false),
        Some(number) => number.parse().map(Constant::Number).unwrap_or(Constant::Other),
        None => Constant::Other,
    })
}

// ---- Çekirdek sağlaması ----

#[derive(Debug, PartialEq)]
enum Checksums {
    Clean,
    Modified { files: Vec<String> },
    /// Denetlenemedi (ağ yok, sürüm için sağlama listesi yok…).
    Unknown(String),
}

/// `wp core verify-checksums` çıktısını yorumlar. Başlıca iki kaynaktan uyarı gelir ve ikisi
/// de gürültüdür: `wp-content` (eklenti ve temalar kendiliğinden güncellenir) ve kök klasördeki
/// dosyaların eksikliği (yöneticiler `readme.html`, `license.txt` gibi dosyaları bilerek siler).
/// Sayılanlar: değişmiş her çekirdek dosyası ile `wp-admin`/`wp-includes` içinde eksik ya da fazladan dosya.
fn parse_checksums(output: &CmdOutput) -> Checksums {
    let mut files: Vec<String> = Vec::new();
    let mut saw_warning = false;
    for line in output.stderr.lines().chain(output.stdout.lines()) {
        let line = line.trim().trim_start_matches("Warning:").trim();
        let (kind, path) = if let Some(path) = line.strip_prefix("File doesn't verify against checksum:") {
            ("modified", path)
        } else if let Some(path) = line.strip_prefix("File doesn't exist:") {
            ("missing", path)
        } else if let Some(path) = line.strip_prefix("File should not exist:") {
            ("extra", path)
        } else {
            continue;
        };
        saw_warning = true;
        let path = path.trim();
        if path.is_empty() || path.starts_with("wp-content/") {
            continue;
        }
        let in_core_dir = path.starts_with("wp-admin/") || path.starts_with("wp-includes/");
        if kind == "modified" || in_core_dir {
            files.push(path.to_string());
        }
    }
    if !files.is_empty() {
        return Checksums::Modified { files };
    }
    if output.success() || saw_warning {
        return Checksums::Clean;
    }
    Checksums::Unknown(cli_error(output))
}

// ---- Denetim ----

/// Denetimin girdileri; hepsi okunduktan sonra `evaluate` saf işlevle yorumlar.
struct Inputs {
    /// `wp-config.php` metni; okunamadıysa `None`.
    config: Option<String>,
    /// `wp-config.php` izni (sekizlik, ör. 0o640).
    mode: Option<u32>,
    /// Yönetici rolündeki kullanıcıların giriş adları.
    admins: Option<Vec<String>>,
    checksums: Checksums,
    template: String,
    inactive_plugins: Option<usize>,
    inactive_themes: Option<usize>,
}

const NO_CONFIG: &str = "wp-config.php okunamadı; dosya yayın klasörünün içinde olmalı";

fn item(id: &'static str, ok: bool, detail: impl Into<String>, fixable: bool) -> SecurityItem {
    SecurityItem { id, title: title_of(id), ok, detail: detail.into(), fixable: fixable && !ok, info: false }
}

/// Bir `wp-config.php` sabitine bakan madde; dosya okunamadıysa düzeltilemez.
fn config_item(id: &'static str, config: Option<&str>, judge: impl FnOnce(&str) -> (bool, String)) -> SecurityItem {
    match config {
        Some(text) => {
            let (ok, detail) = judge(text);
            item(id, ok, detail, true)
        }
        None => item(id, false, NO_CONFIG, false),
    }
}

fn evaluate(inputs: &Inputs) -> Vec<SecurityItem> {
    let config = inputs.config.as_deref();
    let mut items = Vec::new();

    items.push(config_item("file_edit", config, |text| {
        if read_constant(text, "DISALLOW_FILE_EDIT").and_then(|value| value.truthy()) == Some(true) {
            (true, "Yönetim panelindeki eklenti ve tema düzenleyicisi kapalı.".into())
        } else {
            (false, "Ele geçirilen bir yönetici hesabı, panelden eklenti dosyasını düzenleyerek sunucuda kod çalıştırabilir.".into())
        }
    }));

    items.push(config_item("debug_display", config, |text| {
        // Tanımsız WP_DEBUG kapalı demektir; okunamayan bir ifade ise açık varsayılır.
        let debug_on = read_constant(text, "WP_DEBUG").is_some_and(|value| value.truthy().unwrap_or(true));
        let display_off = read_constant(text, "WP_DEBUG_DISPLAY").and_then(|value| value.truthy()) == Some(false);
        if !debug_on {
            (true, "Hata ayıklama kapalı.".into())
        } else if display_off {
            (true, "Hata ayıklama açık ama hata iletileri sayfada gösterilmiyor.".into())
        } else {
            (false, "Hata ayıklama açık ve hata iletileri ziyaretçilere gösteriliyor; dosya yolları ve ayarlar sızabilir.".into())
        }
    }));

    items.push(match inputs.mode {
        Some(mode) => {
            let shown = format!("{mode:o}");
            if mode & 0o007 != 0 {
                item("config_permissions", false, format!("İzin {shown}: sunucudaki diğer kullanıcılar veritabanı parolasını okuyabilir."), true)
            } else if mode & 0o020 != 0 {
                item("config_permissions", false, format!("İzin {shown}: grup dosyayı değiştirebilir."), true)
            } else {
                item("config_permissions", true, format!("İzin {shown}: yalnızca site kullanıcısı (ve en çok okuma için grup) erişebilir."), true)
            }
        }
        None => item("config_permissions", false, NO_CONFIG, false),
    });

    items.push(config_item("salts", config, |text| {
        let weak: Vec<&str> = SALT_KEYS
            .iter()
            .copied()
            .filter(|key| match read_constant(text, key) {
                None => true,
                Some(Constant::Text(value)) => value.trim().len() < MIN_SALT_LEN || value.contains(DEFAULT_SALT),
                Some(_) => false,
            })
            .collect();
        if weak.is_empty() {
            (true, "Sekiz güvenlik anahtarının hepsi tanımlı ve rastgele.".into())
        } else {
            (false, format!("Tanımsız, boş ya da varsayılan: {}. Oturum çerezleri tahmin edilebilir olabilir.", weak.join(", ")))
        }
    }));

    items.push(if inputs.template == "wordpress" {
        item("template", true, "Yüklenen dosyalar arasında PHP çalışmıyor, xmlrpc.php kapalı.", true)
    } else {
        item("template", false, "Yüklenen dosyalar arasına bırakılan PHP çalışabilir ve xmlrpc.php açık; site şablonu “WordPress” olmalı.", true)
    });

    items.push(config_item("auto_minor", config, |text| {
        let core = read_constant(text, "WP_AUTO_UPDATE_CORE").and_then(|value| value.truthy());
        let disabled = read_constant(text, "AUTOMATIC_UPDATER_DISABLED").and_then(|value| value.truthy());
        if disabled == Some(true) {
            (false, "Kendiliğinden güncelleme tümüyle kapalı; güvenlik yamaları elle kurulmak zorunda.".into())
        } else if core == Some(false) {
            (false, "Çekirdeğin kendiliğinden güncellenmesi kapalı; güvenlik yamaları elle kurulmak zorunda.".into())
        } else {
            (true, "Güvenlik ve bakım sürümleri kendiliğinden kuruluyor.".into())
        }
    }));

    items.push(match &inputs.checksums {
        Checksums::Clean => item("core_checksums", true, "Çekirdek dosyaları WordPress'in resmî sağlama toplamlarıyla uyuşuyor.", false),
        Checksums::Modified { files } => {
            let shown: Vec<&str> = files.iter().take(5).map(String::as_str).collect();
            let more = if files.len() > shown.len() { format!(" (+{} dosya daha)", files.len() - shown.len()) } else { String::new() };
            item(
                "core_checksums",
                false,
                format!("{} çekirdek dosyası resmî sürümle uyuşmuyor: {}{more}. Site ele geçirilmiş olabilir; çekirdeği yeniden kurun ya da yedekten dönün.", files.len(), shown.join(", ")),
                false,
            )
        }
        Checksums::Unknown(reason) => item("core_checksums", false, format!("Denetlenemedi: {reason}"), false),
    });

    items.push(match &inputs.admins {
        Some(admins) if admins.iter().any(|login| login.eq_ignore_ascii_case("admin")) => {
            item("admin_user", false, "“admin” adlı bir yönetici hesabı var; parola deneme saldırıları ilk bu adı dener. Başka adla yeni bir yönetici açıp eskisini silin.", false)
        }
        Some(_) => item("admin_user", true, "Tahmin edilmesi kolay “admin” adında yönetici yok.", false),
        None => item("admin_user", false, "Yönetici hesapları okunamadı.", false),
    });

    let mut unused = item("unused_extensions", true, String::new(), false);
    unused.info = true;
    unused.detail = match (inputs.inactive_plugins, inputs.inactive_themes) {
        (Some(0), Some(0)) => "Etkin olmayan eklenti ya da tema yok.".into(),
        (Some(plugins), Some(themes)) => format!("{plugins} etkin olmayan eklenti, {themes} etkin olmayan tema var. Kullanmıyorsanız silin; güncellenmeyen eklenti ve temalar sık görülen saldırı yoludur."),
        _ => "Eklenti ve tema listesi okunamadı.".into(),
    };
    items.push(unused);
    items
}

/// `wp-config.php` metnini sitenin kullanıcısıyla okur; yoksa ya da okunamazsa `None`.
async fn read_config(wp: &Wp) -> Option<String> {
    let read = runas::output(runas::command(&wp.user, &wp.docroot, "cat", &["--", "wp-config.php"]), QUICK).await.ok()?;
    read.success().then_some(read.stdout)
}

/// `stat -c %a` çıktısını sekizlik izne çevirir.
fn parse_mode(output: &str) -> Option<u32> {
    u32::from_str_radix(output.trim(), 8).ok().filter(|mode| *mode <= 0o7777)
}

fn count(output: AppResult<CmdOutput>) -> Option<usize> {
    output.ok().filter(CmdOutput::success).and_then(|out| out.stdout.trim().parse().ok())
}

/// Sitenin sıkılaştırma denetimini yapar. Sitede WordPress yoksa hata döner.
pub async fn audit(site: &Site) -> AppResult<Audit> {
    ensure_cli().await?;
    let wp = Wp::for_site(site)?;
    if !wp.has_files().await {
        return Err(AppError::bad("Bu sitede WordPress kurulu değil"));
    }
    let stat = runas::command(&wp.user, &wp.docroot, "stat", &["-c", "%a", "--", "wp-config.php"]);
    let (config, mode, admins, checksums, plugins, themes) = tokio::join!(
        read_config(&wp),
        runas::output(stat, QUICK),
        wp.run(&["user", "list", "--role=administrator", "--field=user_login", "--skip-plugins", "--skip-themes"]),
        runas::output(wp.command(&["core", "verify-checksums", "--skip-plugins", "--skip-themes"]), SLOW),
        wp.run(&["plugin", "list", "--status=inactive", "--format=count", "--skip-plugins", "--skip-themes"]),
        wp.run(&["theme", "list", "--status=inactive", "--format=count", "--skip-plugins", "--skip-themes"]),
    );
    let inputs = Inputs {
        config,
        mode: mode.ok().filter(CmdOutput::success).and_then(|out| parse_mode(&out.stdout)),
        admins: admins.ok().filter(CmdOutput::success).map(|out| out.stdout.lines().map(|line| line.trim().to_string()).filter(|line| !line.is_empty()).collect()),
        checksums: match checksums {
            Ok(output) => parse_checksums(&output),
            Err(err) => Checksums::Unknown(err.to_string()),
        },
        template: site.template.clone(),
        inactive_plugins: count(plugins),
        inactive_themes: count(themes),
    };
    Ok(Audit::new(evaluate(&inputs)))
}

// ---- Düzeltme ----

/// Seçilen maddeleri düzeltir. Çağıran, sitenin `SiteLock`unu tutar. Zaten tamam olan maddelere
/// dokunulmaz; bir maddenin düzeltilememesi ötekileri durdurmaz.
pub async fn fix(state: &AppState, site: &Site, ids: &[&'static str]) -> AppResult<FixResult> {
    let wp = Wp::for_site(site)?;
    let before = audit(site).await?;
    let (mut fixed, mut skipped, mut failed) = (Vec::new(), Vec::new(), Vec::new());
    // Dosya izni en sona kalır: ayar dosyasını yeniden yazan öteki düzeltmeler izni bozmasın.
    let mut ordered = ids.to_vec();
    ordered.sort_by_key(|id| *id == "config_permissions");
    for id in ordered {
        let Some(current) = before.items.iter().find(|item| item.id == id) else { continue };
        if current.ok {
            skipped.push(id);
            continue;
        }
        if !current.fixable {
            failed.push(FixFailure { id, error: current.detail.clone() });
            continue;
        }
        let outcome = match id {
            "file_edit" => wp.run_ok(&["config", "set", "DISALLOW_FILE_EDIT", "true", "--raw"]).await.map(|_| ()),
            "debug_display" => wp.run_ok(&["config", "set", "WP_DEBUG_DISPLAY", "false", "--raw"]).await.map(|_| ()),
            "config_permissions" => fix_permissions(&wp).await,
            "salts" => wp.run_ok(&["config", "shuffle-salts"]).await.map(|_| ()),
            "template" => fix_template(state, site).await,
            "auto_minor" => fix_auto_minor(&wp).await,
            _ => Err(AppError::bad("Bu madde otomatik düzeltilemez")),
        };
        match outcome {
            Ok(()) => fixed.push(id),
            Err(err) => failed.push(FixFailure { id, error: err.to_string() }),
        }
    }
    // Şablon düzeltmesi sitenin kaydını değiştirdiği için denetim güncel kayıtla yinelenir.
    let fresh = sites::load(&state.db, site.id)?.unwrap_or_else(|| site.clone());
    Ok(FixResult { fixed, skipped, failed, audit: audit(&fresh).await? })
}

/// Veritabanı parolasını içeren dosyayı yalnızca site kullanıcısına açar (PHP-FPM havuzu o kullanıcıyla çalışır).
async fn fix_permissions(wp: &Wp) -> AppResult<()> {
    let output = runas::output(runas::command(&wp.user, &wp.docroot, "chmod", &["600", "--", "wp-config.php"]), QUICK).await?;
    if output.success() { Ok(()) } else { Err(AppError::bad("wp-config.php izni değiştirilemedi")) }
}

/// Çekirdeğin kendiliğinden güncellenmesini küçük sürümler için geri açar.
async fn fix_auto_minor(wp: &Wp) -> AppResult<()> {
    let config = read_config(wp).await.ok_or_else(|| AppError::bad(NO_CONFIG))?;
    if read_constant(&config, "AUTOMATIC_UPDATER_DISABLED").and_then(|value| value.truthy()) == Some(true) {
        wp.run_ok(&["config", "set", "AUTOMATIC_UPDATER_DISABLED", "false", "--raw"]).await?;
    }
    if read_constant(&config, "WP_AUTO_UPDATE_CORE").and_then(|value| value.truthy()) == Some(false) {
        wp.run_ok(&["config", "set", "WP_AUTO_UPDATE_CORE", "'minor'", "--raw"]).await?;
    }
    Ok(())
}

/// Siteyi WordPress şablonuyla yeniden yapılandırır. Site güncelleme yolundaki gibi: nginx
/// yapılandırması üretilip sınanır, uygulanamazsa kayıt ve sistem eski hâline döner.
async fn fix_template(state: &AppState, site: &Site) -> AppResult<()> {
    let _guard = state.sys_lock.lock().await;
    let previous = sites::load(&state.db, site.id)?.ok_or(AppError::NotFound)?;
    if !crate::hosting::nginx::template_allowed(&previous.kind, "wordpress") {
        return Err(AppError::bad("WordPress şablonu bu site türünde kullanılamaz"));
    }
    let mut updated = previous.clone();
    updated.template = "wordpress".into();
    sites::save(&state.db, &updated)?;
    if let Err(err) = sites::provision(&updated).await {
        sites::save(&state.db, &previous)?;
        if let Err(restore) = sites::provision(&previous).await {
            tracing::error!(error = %restore, domain = %previous.domain, "site eski ayarlarına döndürülemedi");
        }
        return Err(err);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "<?php
define( 'DB_NAME', 'wp' );
define( 'DISALLOW_FILE_EDIT', true );
define( 'WP_DEBUG', false );
// define( 'WP_DEBUG_DISPLAY', true );
define( 'WP_AUTO_UPDATE_CORE', 'minor' );
";

    fn salts(value: &str) -> String {
        SALT_KEYS.iter().map(|key| format!("define( '{key}', '{value}' );\n")).collect()
    }

    fn inputs(config: &str) -> Inputs {
        Inputs {
            config: Some(format!("{config}{}", salts("kO8v!pQ3z-Lm2xRt9wYb6nHc4sJd1fGa7"))),
            mode: Some(0o640),
            admins: Some(vec!["yonetici".into()]),
            checksums: Checksums::Clean,
            template: "wordpress".into(),
            inactive_plugins: Some(0),
            inactive_themes: Some(0),
        }
    }

    fn state_of(items: &[SecurityItem], id: &str) -> bool {
        items.iter().find(|item| item.id == id).unwrap().ok
    }

    #[test]
    fn constants_are_read_statically() {
        assert_eq!(read_constant(CONFIG, "DISALLOW_FILE_EDIT"), Some(Constant::Bool(true)));
        assert_eq!(read_constant(CONFIG, "WP_DEBUG"), Some(Constant::Bool(false)));
        assert_eq!(read_constant(CONFIG, "WP_AUTO_UPDATE_CORE"), Some(Constant::Text("minor".into())));
        // Yorum satırındaki tanım sayılmaz.
        assert_eq!(read_constant(CONFIG, "WP_DEBUG_DISPLAY"), None);
        assert_eq!(read_constant("define('X', 1);", "X"), Some(Constant::Number(1)));
        assert_eq!(read_constant("define('X', getenv('Y'));", "X"), Some(Constant::Other));
        assert_eq!(read_constant("define('X', FALSE);", "X").and_then(|value| value.truthy()), Some(false));
        assert_eq!(read_constant("define('X', getenv('Y'));", "X").and_then(|value| value.truthy()), None);
    }

    #[test]
    fn healthy_site_passes_everything() {
        let items = evaluate(&inputs(CONFIG));
        assert_eq!(items.len(), ALL_IDS.len());
        assert!(items.iter().all(|item| item.ok), "{items:?}");
        assert!(items.iter().all(|item| !item.fixable));
        assert_eq!(Audit::new(items).total, 8, "bilgi maddesi sayıma girmez");
    }

    #[test]
    fn debug_display_follows_wordpress_defaults() {
        let judge = |extra: &str| state_of(&evaluate(&inputs(&format!("<?php\n{extra}\n"))), "debug_display");
        assert!(judge(""), "WP_DEBUG tanımsızsa kapalıdır");
        assert!(judge("define('WP_DEBUG', false);"));
        assert!(!judge("define('WP_DEBUG', true);"), "açık ve gösterim varsayılan: sızar");
        assert!(judge("define('WP_DEBUG', true);\ndefine('WP_DEBUG_DISPLAY', false);"));
        assert!(!judge("define('WP_DEBUG', true);\ndefine('WP_DEBUG_DISPLAY', true);"));
        assert!(!judge("define('WP_DEBUG', getenv('DEBUG'));"), "okunamayan değer güvenli sayılmaz");
    }

    #[test]
    fn insecure_settings_are_reported_and_fixable() {
        let mut broken = inputs("<?php\ndefine('WP_DEBUG', true);\ndefine('WP_AUTO_UPDATE_CORE', false);\n");
        broken.mode = Some(0o644);
        broken.template = "default".into();
        let items = evaluate(&broken);
        for id in ["file_edit", "debug_display", "config_permissions", "template", "auto_minor"] {
            let found = items.iter().find(|item| item.id == id).unwrap();
            assert!(!found.ok && found.fixable, "{id}");
        }
        assert!(state_of(&items, "salts"));
        // Kendiliğinden güncelleme tümüyle kapatılmışsa da uyarır.
        let disabled = evaluate(&inputs("<?php\ndefine('AUTOMATIC_UPDATER_DISABLED', true);\n"));
        assert!(!state_of(&disabled, "auto_minor"));
    }

    #[test]
    fn config_permissions_rules() {
        let judge = |mode: u32| {
            let mut input = inputs(CONFIG);
            input.mode = Some(mode);
            state_of(&evaluate(&input), "config_permissions")
        };
        assert!(judge(0o600) && judge(0o640) && judge(0o400));
        assert!(!judge(0o644) && !judge(0o660) && !judge(0o777) && !judge(0o604));
        assert_eq!(parse_mode("640\n"), Some(0o640));
        assert_eq!(parse_mode("zz"), None);
    }

    #[test]
    fn default_or_missing_salts_are_reported() {
        let mut input = inputs(CONFIG);
        input.config = Some(format!("{CONFIG}{}", salts(DEFAULT_SALT)));
        assert!(!state_of(&evaluate(&input), "salts"));
        input.config = Some(CONFIG.to_string());
        let items = evaluate(&input);
        let entry = items.iter().find(|item| item.id == "salts").unwrap();
        assert!(!entry.ok && entry.detail.contains("AUTH_KEY"), "{entry:?}");
        input.config = Some(format!("{CONFIG}{}", salts_with_one_short()));
        assert!(!state_of(&evaluate(&input), "salts"));
    }

    fn salts_with_one_short() -> String {
        let mut text = salts("kO8v!pQ3z-Lm2xRt9wYb6nHc4sJd1fGa7");
        text = text.replacen("kO8v!pQ3z-Lm2xRt9wYb6nHc4sJd1fGa7", "kisa", 1);
        text
    }

    #[test]
    fn missing_config_blocks_config_based_items_without_offering_a_fix() {
        let mut input = inputs(CONFIG);
        input.config = None;
        input.mode = None;
        let items = evaluate(&input);
        for id in ["file_edit", "debug_display", "config_permissions", "salts", "auto_minor"] {
            let found = items.iter().find(|item| item.id == id).unwrap();
            assert!(!found.ok && !found.fixable, "{id}");
        }
    }

    #[test]
    fn admin_user_is_flagged_case_insensitively_without_fix() {
        let mut input = inputs(CONFIG);
        input.admins = Some(vec!["Editor".into(), "ADMIN".into()]);
        let items = evaluate(&input);
        let found = items.iter().find(|item| item.id == "admin_user").unwrap();
        assert!(!found.ok && !found.fixable);
    }

    #[test]
    fn checksum_output_is_interpreted() {
        let output = |code: i32, stderr: &str| CmdOutput { code, stdout: String::new(), stderr: stderr.into() };
        assert_eq!(parse_checksums(&output(0, "")), Checksums::Clean);
        let modified = output(1, "Warning: File doesn't verify against checksum: wp-includes/version.php\nError: WordPress installation doesn't verify against checksums.\n");
        assert_eq!(parse_checksums(&modified), Checksums::Modified { files: vec!["wp-includes/version.php".into()] });
        // Silinmiş readme.html, kök klasördeki fazladan dosya ve wp-content gürültüsü sayılmaz.
        let noise = output(
            1,
            "Warning: File doesn't exist: readme.html\nWarning: File should not exist: error_log\nWarning: File doesn't exist: wp-content/themes/twentytwentyfour/style.css\nError: WordPress installation doesn't verify against checksums.\n",
        );
        assert_eq!(parse_checksums(&noise), Checksums::Clean);
        // wp-includes içine bırakılmış fazladan dosya sayılır.
        let extra = output(1, "Warning: File should not exist: wp-includes/kabuk.php\n");
        assert_eq!(parse_checksums(&extra), Checksums::Modified { files: vec!["wp-includes/kabuk.php".into()] });
        let offline = output(1, "Error: Couldn't fetch checksums.\n");
        assert!(matches!(parse_checksums(&offline), Checksums::Unknown(_)));
    }

    #[test]
    fn fix_ids_are_whitelisted() {
        let ids = |list: &[&str]| validate_ids(&list.iter().map(|id| id.to_string()).collect::<Vec<_>>());
        assert_eq!(ids(&["salts", "file_edit", "salts"]).unwrap(), vec!["salts", "file_edit"]);
        for bad in [&["--all"][..], &["salts; id"], &["../x"], &[""], &["SALTS"]] {
            assert!(ids(bad).is_err(), "{bad:?}");
        }
        // Bilinen ama otomatik düzeltilemeyen maddeler de reddedilir.
        for unfixable in ["admin_user", "core_checksums", "unused_extensions"] {
            assert!(ids(&[unfixable]).is_err(), "{unfixable}");
        }
        assert!(ids(&[]).is_err());
    }
}
