//! Posta kutusunu başka sunucudan IMAP ile içe aktarma.
//!
//! İşi `imap-import.py` betiği yapar: `vmail` kullanıcısıyla çalışır, karşı sunucuya bağlanır ve
//! iletileri kutunun Maildir klasörüne yazar. Panel burada girdiyi doğrular, karşı sunucunun adresini
//! çözüp denetler (yerel ve özel ağ adreslerine bağlanılmaz) ve betiği doğrulanmış adresle başlatır.
//! Giriş bilgileri betiğe standart girdiden verilir; komut satırına, diske ve görev günlüğüne yazılmaz.

use std::net::IpAddr;
use std::path::Path;

use serde::Deserialize;
use serde_json::json;

use super::{mailbox_home, vmail, Mailbox};
use crate::error::{AppError, AppResult};
use crate::jobs::JobCtx;
use crate::sys::{exec, runas};

const SCRIPT: &str = include_str!("imap-import.py");
const SCRIPT_PATH: &str = "/usr/local/lib/pixpanel/imap-import.py";

/// Karşı sunucunun bağlantı bilgileri.
#[derive(Deserialize)]
pub struct ImapSource {
    pub host: String,
    pub port: Option<u16>,
    /// `ssl` (993) ya da `starttls` (143). Şifresiz bağlantı desteklenmez.
    #[serde(default)]
    pub security: String,
    pub username: String,
    pub password: String,
    /// Kendinden imzalı sertifikalı sunucular için doğrulama kapatılabilir.
    #[serde(default = "yes")]
    pub verify_tls: bool,
}

fn yes() -> bool {
    true
}

impl ImapSource {
    pub fn validate(&mut self) -> AppResult<()> {
        self.host = self.host.trim().trim_end_matches('.').to_lowercase();
        self.username = self.username.trim().to_string();
        if self.security.is_empty() {
            self.security = "ssl".into();
        }
        let host_ok = !self.host.is_empty()
            && self.host.len() <= 253
            && !self.host.starts_with('-')
            && self.host.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':'));
        if !host_ok {
            return Err(AppError::bad("Sunucu adresi geçersiz (alan adı ya da IP adresi yazın)"));
        }
        if !matches!(self.security.as_str(), "ssl" | "starttls") {
            return Err(AppError::bad("Güvenlik türü SSL/TLS ya da STARTTLS olmalı"));
        }
        if self.port == Some(0) {
            return Err(AppError::bad("Bağlantı noktası geçersiz"));
        }
        let plain = |value: &str, max: usize| !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control);
        if !plain(&self.username, 255) {
            return Err(AppError::bad("Kullanıcı adı gerekli"));
        }
        if !plain(&self.password, 512) {
            return Err(AppError::bad("Parola gerekli"));
        }
        Ok(())
    }

    pub fn port(&self) -> u16 {
        self.port.unwrap_or(if self.security == "starttls" { 143 } else { 993 })
    }
}

/// Sunucu adını çözer ve adresi denetler. Yöneticiler özel ağdaki sunucudan da aktarabilir;
/// loopback ve bağlantı-yerel adresler kimseye açık değildir.
pub async fn resolve(source: &ImapSource, allow_private: bool) -> AppResult<IpAddr> {
    let addresses: Vec<IpAddr> = match source.host.parse::<IpAddr>() {
        Ok(address) => vec![address],
        Err(_) => tokio::time::timeout(std::time::Duration::from_secs(10), tokio::net::lookup_host((source.host.as_str(), source.port())))
            .await
            .map_err(|_| AppError::bad("Sunucu adı çözülemedi (zaman aşımı)"))?
            .map_err(|_| AppError::bad(format!("{} adresi çözülemedi", source.host)))?
            .map(|address| address.ip())
            .collect(),
    };
    if addresses.is_empty() {
        return Err(AppError::bad(format!("{} adresi çözülemedi", source.host)));
    }
    if addresses.iter().any(|address| crate::webhooks::is_blocked(*address, allow_private)) {
        return Err(AppError::bad("Bu adrese bağlanılamaz: yerel ve özel ağ adresleri kullanılamaz"));
    }
    Ok(addresses[0])
}

/// İçe aktarma görevinin gövdesi.
pub async fn run(ctx: &JobCtx, mailbox: &Mailbox, source: &ImapSource, address: IpAddr) -> AppResult<()> {
    let account = vmail()?;
    if !Path::new("/usr/bin/python3").is_file() {
        ctx.log("python3 kuruluyor");
        crate::components::apt_install(ctx, &["python3".into()]).await?;
    }
    crate::hosting::atomic_write(Path::new(SCRIPT_PATH), SCRIPT.as_bytes(), 0o644)?;

    let (local, domain) = mailbox.address.split_once('@').ok_or_else(|| AppError::internal("kutu adresi okunamadı"))?;
    let maildir = mailbox_home(domain, local).join("Maildir");
    let config = json!({
        "host": source.host,
        "ip": address.to_string(),
        "port": source.port(),
        "security": source.security,
        "verify_tls": source.verify_tls,
        "username": source.username,
        "password": source.password,
        "maildir": maildir,
    });
    ctx.log(format!("{}:{} sunucusundan {} kutusuna aktarılıyor", source.host, source.port(), mailbox.address));
    // `-I`: yalıtılmış kip; betik çalışma klasöründen ya da ortamdan modül yüklemez.
    let mut cmd = runas::command(&account, Path::new(super::MAIL_ROOT), "/usr/bin/python3", &["-I", SCRIPT_PATH]);
    ctx.run_streaming(&mut cmd, "içe aktarma", Some(config.to_string().into_bytes())).await?;

    // Kota sayacı diske doğrudan yazılan iletileri bilmez; yeniden hesaplatılır.
    let _ = exec::run("doveadm", &["quota", "recalc", "-u", &mailbox.address]).await;
    ctx.log("İçe aktarma tamamlandı. Eski sunucudaki iletilere dokunulmadı.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(host: &str) -> ImapSource {
        ImapSource { host: host.into(), port: None, security: String::new(), username: "ali@ornek.com".into(), password: "gizli".into(), verify_tls: true }
    }

    #[test]
    fn source_is_validated() {
        let mut ok = source(" IMAP.Ornek.com. ");
        ok.validate().unwrap();
        assert_eq!((ok.host.as_str(), ok.security.as_str(), ok.port()), ("imap.ornek.com", "ssl", 993));
        let mut tls = source("imap.ornek.com");
        tls.security = "starttls".into();
        tls.validate().unwrap();
        assert_eq!(tls.port(), 143);
        for host in ["", "-oX", "a b", "ornek.com/x", "a;id"] {
            assert!(source(host).validate().is_err(), "{host}");
        }
        let mut plain = source("imap.ornek.com");
        plain.security = "none".into();
        assert!(plain.validate().is_err());
        let mut empty = source("imap.ornek.com");
        empty.password.clear();
        assert!(empty.validate().is_err());
        let mut newline = source("imap.ornek.com");
        newline.username = "a\nb".into();
        assert!(newline.validate().is_err());
    }
}
