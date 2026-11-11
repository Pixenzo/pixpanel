//! Komut satırı: panelin işlemlerini betikten yapabilmek için.
//!
//! Komutlar işi kendileri yapmaz; çalışan panelin HTTP API'sini çağırır. Böylece arayüzden
//! yapılan her işlemle aynı doğrulama, aynı yetki denetimi ve aynı denetim günlüğü geçerlidir.
//! Kimlik için parola sorulmaz: komut root olarak çalışır, veritabanına kısa ömürlü ve
//! yalnızca yerel adresten geçerli bir API anahtarı yazar, işi bitince siler.

use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use clap::Subcommand;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use crate::auth;
use crate::config::Config;
use crate::db::{now, Db};

/// Geçici anahtarın ömrü; uzun süren görevleri beklerken de yeter.
const TOKEN_TTL_SECS: i64 = 2 * 3600;

#[derive(Subcommand)]
pub enum ApiCommand {
    /// Panelin API'sine doğrudan istek atar; yanıtı JSON olarak yazar
    ///
    /// Örnek: pixpanel api GET /sites
    ///        pixpanel api POST /sites --data '{"domain":"ornek.com","kind":"static"}'
    Api {
        /// GET, POST, PUT, PATCH ya da DELETE
        method: String,
        /// `/api/v1` sonrasındaki yol, ör. `/sites` ya da `/health`
        path: String,
        /// İstek gövdesi (JSON). `@dosya` dosyadan, `-` standart girdiden okur
        #[arg(long)]
        data: Option<String>,
        /// İsteği bu hesap adına yap (varsayılan: ilk yönetici)
        #[arg(long = "as")]
        user: Option<String>,
        /// Yanıt bir görev başlattıysa bitmesini bekle ve günlüğünü yaz
        #[arg(long)]
        wait: bool,
    },
    /// Siteleri listeler, ekler, kaldırır
    Site {
        #[command(subcommand)]
        action: SiteAction,
    },
    /// Hesapları listeler, ekler
    User {
        #[command(subcommand)]
        action: UserAction,
    },
    /// Sunucunun sağlık bulgularını yazar; kritik bulgu varsa 1 ile çıkar
    Health,
}

#[derive(Subcommand)]
pub enum SiteAction {
    /// Siteleri listeler
    List,
    /// Site ekler
    Add {
        domain: String,
        /// php, static, node ya da proxy
        #[arg(long, default_value = "static")]
        kind: String,
        /// PHP sürümü (php türünde; verilmezse kurulu en yeni sürüm)
        #[arg(long)]
        php: Option<String>,
        /// Node.js sürümü (node türünde; verilmezse kurulu en yeni sürüm)
        #[arg(long)]
        node: Option<String>,
        /// Hedef adres (proxy türünde), ör. http://127.0.0.1:3000
        #[arg(long)]
        target: Option<String>,
        /// Sitenin sahibi olacak hesap (varsayılan: isteği yapan yönetici)
        #[arg(long)]
        owner: Option<String>,
    },
    /// Siteyi kaldırır
    Remove {
        domain: String,
        /// Sitenin dosyalarını da sil
        #[arg(long)]
        delete_files: bool,
    },
}

#[derive(Subcommand)]
pub enum UserAction {
    /// Hesapları listeler
    List,
    /// Hesap ekler; parola verilmezse üretilir ve bir kez yazdırılır
    Add {
        username: String,
        /// admin, reseller ya da user
        #[arg(long, default_value = "user")]
        role: String,
        #[arg(long)]
        password: Option<String>,
        #[arg(long, default_value = "")]
        email: String,
    },
}

/// Çalışan panele bağlı, geçici anahtarı taşıyan istemci. Düşerken anahtar silinmez
/// (eşzamansız iş yapılamaz); `run` her çıkış yolunda `revoke` çağırır.
struct Client {
    http: reqwest::Client,
    base: String,
    token: String,
    token_id: i64,
    db: Db,
}

impl Client {
    fn connect(cfg: &Config, db: &Db, username: Option<&str>) -> anyhow::Result<Self> {
        let user_id: Option<i64> = db
            .call(|conn| match username {
                Some(name) => conn.query_row("SELECT id FROM users WHERE username = ?1 AND status = 'active'", params![name], |row| row.get(0)).optional(),
                None => conn.query_row("SELECT id FROM users WHERE role = 'admin' AND status = 'active' ORDER BY id LIMIT 1", [], |row| row.get(0)).optional(),
            })
            .map_err(|err| anyhow!(err.to_string()))?;
        let user_id = user_id.ok_or_else(|| match username {
            Some(name) => anyhow!("etkin hesap bulunamadı: {name}"),
            None => anyhow!("etkin bir yönetici hesabı yok; önce `pixpanel create-admin` çalıştırın"),
        })?;
        // Panel bütün adresleri dinliyorsa yerel adresten, tek bir adrese bağlıysa o adresten çağrılır.
        let host = match cfg.listen_addr {
            addr if addr.is_unspecified() && addr.is_ipv6() => "::1".to_string(),
            addr if addr.is_unspecified() => "127.0.0.1".to_string(),
            addr => addr.to_string(),
        };
        let allowed = format!("127.0.0.1,::1,{host}");
        // Panel yerel adresten çağrılır; sertifikası bu ada düzenlenmediği için ad doğrulanmaz.
        let http = reqwest::Client::builder().danger_accept_invalid_certs(true).timeout(Duration::from_secs(120)).build()?;
        let token = format!("{}{}", auth::API_TOKEN_PREFIX, auth::random_token(24));
        let token_hash = auth::hash_token(&token);
        let prefix: String = token.chars().take(10).collect();
        let token_id = db
            .call(|conn| {
                // Önceki komutlardan kalmış, süresi dolmuş geçici anahtarlar temizlenir.
                conn.execute("DELETE FROM api_tokens WHERE name = 'pixpanel-cli' AND expires_at < ?1", params![now()])?;
                conn.execute(
                    "INSERT INTO api_tokens (user_id, name, token_hash, prefix, read_only, allowed_ips, expires_at, created_at)
                     VALUES (?1, 'pixpanel-cli', ?2, ?3, 0, ?4, ?5, ?6)",
                    params![user_id, token_hash, prefix, allowed, now() + TOKEN_TTL_SECS, now()],
                )?;
                Ok(conn.last_insert_rowid())
            })
            .map_err(|err| anyhow!(err.to_string()))?;
        let scheme = if cfg.tls { "https" } else { "http" };
        Ok(Self { http, base: format!("{scheme}://{}:{}/api/v1", if host.contains(':') { format!("[{host}]") } else { host }, cfg.listen_port), token, token_id, db: db.clone() })
    }

    fn revoke(&self) {
        let _ = self.db.call(|conn| conn.execute("DELETE FROM api_tokens WHERE id = ?1", params![self.token_id]));
    }

    /// İstek atar; (durum kodu, gövde) döner. Gövde JSON değilse metin olarak sarılır.
    async fn request(&self, method: &str, path: &str, body: Option<Value>) -> anyhow::Result<(u16, Value)> {
        let method = reqwest::Method::from_bytes(method.to_ascii_uppercase().as_bytes()).map_err(|_| anyhow!("geçersiz yöntem: {method}"))?;
        let path = if path.starts_with('/') { path.to_string() } else { format!("/{path}") };
        let mut request = self.http.request(method, format!("{}{path}", self.base)).bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.context("panele bağlanılamadı; servis çalışıyor mu? (systemctl status pixpanel)")?;
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        Ok((status, serde_json::from_str(&text).unwrap_or(Value::String(text))))
    }

    /// Başarılı yanıtın gövdesini döner; hata yanıtında panelin iletisiyle başarısız olur.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> anyhow::Result<Value> {
        let (status, value) = self.request(method, path, body).await?;
        if status >= 400 {
            let message = value.pointer("/error/message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| value.to_string());
            bail!("{message} (HTTP {status})");
        }
        Ok(value)
    }

    /// Görev bitene kadar bekler; günlüğünü yazar ve başarısızsa hata döner.
    async fn wait_for_job(&self, job_id: i64) -> anyhow::Result<()> {
        loop {
            let detail = self.call("GET", &format!("/jobs/{job_id}"), None).await?;
            let status = detail.pointer("/job/status").and_then(Value::as_str).unwrap_or("");
            if status != "running" {
                if let Some(log) = detail.get("log").and_then(Value::as_str) {
                    print!("{log}");
                    if !log.ends_with('\n') {
                        println!();
                    }
                }
                if status != "success" {
                    bail!("görev başarısız oldu");
                }
                return Ok(());
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

fn read_body(data: Option<String>) -> anyhow::Result<Option<Value>> {
    let Some(data) = data else { return Ok(None) };
    let text = if data == "-" {
        std::io::read_to_string(std::io::stdin())?
    } else if let Some(path) = data.strip_prefix('@') {
        std::fs::read_to_string(path).with_context(|| format!("dosya okunamadı: {path}"))?
    } else {
        data
    };
    Ok(Some(serde_json::from_str(&text).context("gövde geçerli JSON değil")?))
}

/// Satırları sütun genişliklerine göre hizalayarak yazar.
fn print_table(header: &[&str], rows: &[Vec<String>]) {
    let width = |column: usize| rows.iter().map(|row| row[column].chars().count()).chain([header[column].chars().count()]).max().unwrap_or(0);
    let widths: Vec<usize> = (0..header.len()).map(width).collect();
    let line = |cells: Vec<String>| {
        let padded: Vec<String> = cells.iter().enumerate().map(|(index, cell)| format!("{cell:<width$}", width = widths[index])).collect();
        println!("{}", padded.join("  ").trim_end());
    };
    line(header.iter().map(|cell| cell.to_string()).collect());
    for row in rows {
        line(row.clone());
    }
}

fn text(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

async fn site_id(client: &Client, domain: &str) -> anyhow::Result<i64> {
    let sites = client.call("GET", "/sites", None).await?;
    sites.as_array().and_then(|list| list.iter().find(|site| site["domain"] == domain)).and_then(|site| site["id"].as_i64()).ok_or_else(|| anyhow!("site bulunamadı: {domain}"))
}

async fn run_command(client: &Client, command: ApiCommand) -> anyhow::Result<()> {
    match command {
        ApiCommand::Api { method, path, data, wait, .. } => {
            let (status, value) = client.request(&method, &path, read_body(data)?).await?;
            println!("{}", serde_json::to_string_pretty(&value)?);
            if status >= 400 {
                bail!("HTTP {status}");
            }
            if let (true, Some(job_id)) = (wait, value.get("job_id").and_then(Value::as_i64)) {
                client.wait_for_job(job_id).await?;
            }
        }
        ApiCommand::Site { action: SiteAction::List } => {
            let sites = client.call("GET", "/sites", None).await?;
            let rows: Vec<Vec<String>> = sites
                .as_array()
                .map(|list| list.iter().map(|site| vec![text(site, "domain"), text(site, "kind"), text(site, "owner"), text(site, "ssl_mode"), text(site, "status")]).collect())
                .unwrap_or_default();
            print_table(&["ALAN ADI", "TÜR", "SAHİBİ", "SSL", "DURUM"], &rows);
        }
        ApiCommand::Site { action: SiteAction::Add { domain, kind, php, node, target, owner } } => {
            let options = client.call("GET", "/sites/options", None).await?;
            let first = |key: &str| options.get(key).and_then(Value::as_array).and_then(|list| list.first()).and_then(Value::as_str).map(str::to_string);
            let mut body = json!({ "domain": domain, "kind": kind });
            match kind.as_str() {
                "php" => body["php_version"] = json!(php.or_else(|| first("php_versions")).ok_or_else(|| anyhow!("kurulu PHP sürümü yok"))?),
                "node" => body["node_version"] = json!(node.or_else(|| first("node_versions")).ok_or_else(|| anyhow!("kurulu Node.js sürümü yok"))?),
                "proxy" => body["proxy_target"] = json!(target.ok_or_else(|| anyhow!("proxy türünde --target gerekir"))?),
                _ => {}
            }
            if let Some(owner) = owner {
                let users = client.call("GET", "/users", None).await?;
                let id = users.as_array().and_then(|list| list.iter().find(|user| user["username"] == owner.as_str())).and_then(|user| user["id"].as_i64());
                body["user_id"] = json!(id.ok_or_else(|| anyhow!("hesap bulunamadı: {owner}"))?);
            }
            let site = client.call("POST", "/sites", Some(body)).await?;
            println!("{} eklendi (no {}).", text(&site, "domain"), text(&site, "id"));
        }
        ApiCommand::Site { action: SiteAction::Remove { domain, delete_files } } => {
            let id = site_id(client, &domain).await?;
            client.call("DELETE", &format!("/sites/{id}?delete_files={delete_files}"), None).await?;
            println!("{domain} kaldırıldı{}.", if delete_files { " (dosyalarıyla birlikte)" } else { " (dosyaları duruyor)" });
        }
        ApiCommand::User { action: UserAction::List } => {
            let users = client.call("GET", "/users", None).await?;
            let rows: Vec<Vec<String>> = users
                .as_array()
                .map(|list| list.iter().map(|user| vec![text(user, "username"), text(user, "role"), text(user, "package_name"), text(user, "status"), text(user, "email")]).collect())
                .unwrap_or_default();
            print_table(&["HESAP", "TÜR", "PAKET", "DURUM", "E-POSTA"], &rows);
        }
        ApiCommand::User { action: UserAction::Add { username, role, password, email } } => {
            let generated = password.is_none();
            let password = password.unwrap_or_else(|| auth::random_password(20));
            client.call("POST", "/users", Some(json!({ "username": username, "role": role, "password": password, "email": email }))).await?;
            println!("{username} oluşturuldu.");
            if generated {
                println!("Parola: {password}");
            }
        }
        ApiCommand::Health => {
            let view = client.call("POST", "/health/check", None).await?;
            let findings = view.pointer("/report/findings").and_then(Value::as_array).cloned().unwrap_or_default();
            if findings.is_empty() {
                println!("Her şey yolunda.");
            }
            let label = |severity: &str| match severity {
                "critical" => "KRİTİK",
                "warning" => "UYARI",
                _ => "ÖNERİ",
            };
            for finding in &findings {
                println!("{:<7} {}", label(finding["severity"].as_str().unwrap_or("")), text(finding, "title"));
            }
            if findings.iter().any(|finding| finding["severity"] == "critical") {
                bail!("kritik bulgu var");
            }
        }
    }
    Ok(())
}

/// Komutu çalıştırır. Geçici anahtar her çıkış yolunda silinir.
pub fn run(cfg: &Config, db: &Db, command: ApiCommand) -> anyhow::Result<()> {
    let username = match &command {
        ApiCommand::Api { user, .. } => user.clone(),
        _ => None,
    };
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = Client::connect(cfg, db, username.as_deref())?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    // Çıktı `head` gibi erken kapanan bir komuta aktarılırsa yazma hatası panik olarak gelir.
    // Panik burada yakalanır ki geçici anahtar yine silinsin; ileti basılmaz, sessizce çıkılır.
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.block_on(run_command(&client, command))));
    client.revoke();
    match outcome {
        Ok(result) => result,
        Err(_) => std::process::exit(141),
    }
}
