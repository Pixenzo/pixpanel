//! Veritabanı sunucularının (MariaDB, PostgreSQL) yönetimi.
//!
//! İki motor da kendi komut satırı istemcisiyle, yerel soket üzerinden ve
//! yönetici kimliğiyle sürülür. SQL metni istemciye standart girdiden verilir;
//! böylece parolalar süreç listesinde görünmez.

pub mod mariadb;
pub mod postgres;

use std::collections::HashMap;
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Mariadb,
    Postgresql,
}

impl Engine {
    pub const ALL: [Engine; 2] = [Engine::Mariadb, Engine::Postgresql];

    pub fn parse(value: &str) -> AppResult<Self> {
        match value {
            "mariadb" => Ok(Self::Mariadb),
            "postgresql" => Ok(Self::Postgresql),
            _ => Err(AppError::bad("Geçersiz veritabanı türü")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mariadb => "mariadb",
            Self::Postgresql => "postgresql",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mariadb => "MariaDB",
            Self::Postgresql => "PostgreSQL",
        }
    }

    pub fn service(self) -> &'static str {
        match self {
            Self::Mariadb => "mariadb.service",
            Self::Postgresql => "postgresql.service",
        }
    }

    pub fn is_installed(self) -> bool {
        crate::sys::exec::binary_exists(match self {
            Self::Mariadb => "mariadb",
            Self::Postgresql => "psql",
        })
    }

    pub fn require_installed(self) -> AppResult<()> {
        if self.is_installed() {
            Ok(())
        } else {
            Err(AppError::bad(format!("{} kurulu değil; önce Bileşenler sayfasından kurun", self.label())))
        }
    }
}

/// Bir kullanıcının bir veritabanı üzerindeki erişim düzeyi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    /// Tabloları oluşturma ve silme dahil her şey.
    Full,
    /// Veri okuma ve yazma; yapıyı değiştiremez.
    Readwrite,
    /// Yalnızca okuma.
    Readonly,
}

impl Access {
    pub fn parse(value: &str) -> AppResult<Self> {
        match value {
            "full" => Ok(Self::Full),
            "readwrite" => Ok(Self::Readwrite),
            "readonly" => Ok(Self::Readonly),
            _ => Err(AppError::bad("Geçersiz erişim düzeyi")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Readwrite => "readwrite",
            Self::Readonly => "readonly",
        }
    }
}

/// Sunucunun kendi kullandığı, panelden oluşturulamayan adlar.
const RESERVED: &[&str] = &[
    "mysql", "information_schema", "performance_schema", "sys", "test", "postgres", "template0", "template1",
    "root", "mariadb", "public", "admin", "pg_monitor", "replication",
];

/// Veritabanı ve kullanıcı adları SQL metnine gömüldüğünden yalnızca küçük
/// harf, rakam ve alt çizgiden oluşabilir.
pub fn validate_name(name: &str, what: &str) -> AppResult<()> {
    let mut chars = name.chars();
    let valid = name.len() <= 48
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !valid {
        return Err(AppError::bad(format!(
            "{what} en fazla 48 karakter olmalı, küçük harfle başlamalı; yalnızca küçük harf, rakam ve _ içerebilir"
        )));
    }
    if RESERVED.contains(&name) || name.starts_with("pg_") || name.starts_with("pixpanel_") {
        return Err(AppError::bad(format!("“{name}” adı sunucu tarafından kullanılıyor; başka bir ad seçin")));
    }
    Ok(())
}

pub fn validate_password(password: &str) -> AppResult<()> {
    if !(10..=128).contains(&password.chars().count()) {
        return Err(AppError::bad("Veritabanı parolası 10-128 karakter olmalı"));
    }
    if password.chars().any(char::is_control) {
        return Err(AppError::bad("Parola satır sonu gibi denetim karakterleri içeremez"));
    }
    Ok(())
}

/// MariaDB kullanıcısının bağlanabileceği adres: `localhost`, `%` (her yer)
/// ya da bir IP adresi / `192.168.1.%` gibi bir kalıp.
pub fn validate_host(host: &str) -> AppResult<()> {
    let valid = host == "localhost"
        || host == "%"
        || (!host.is_empty()
            && host.len() <= 60
            && host.bytes().all(|b| b.is_ascii_hexdigit() || matches!(b, b'.' | b':' | b'%')));
    if valid { Ok(()) } else { Err(AppError::bad("Geçersiz bağlantı adresi (örnek: localhost, % ya da 203.0.113.10)")) }
}

/// SQL betiğini istemcinin standart girdisine yazar ve çıktısını döner.
async fn run_client(mut command: Command, script: &str, label: &str) -> AppResult<String> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| AppError::internal(format!("{label} istemcisi başlatılamadı: {err}")))?;
    let mut stdin = child.stdin.take().expect("stdin borusu");
    stdin.write_all(script.as_bytes()).await?;
    drop(stdin);
    let output = tokio::time::timeout(std::time::Duration::from_secs(120), child.wait_with_output())
        .await
        .map_err(|_| AppError::internal(format!("{label} yanıt vermedi")))??;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let detail = String::from_utf8_lossy(&output.stderr);
        Err(AppError::bad(format!("{label} hatası: {}", detail.trim())))
    }
}

/// Veritabanı adı → bayt cinsinden boyut.
pub async fn sizes(engine: Engine) -> HashMap<String, u64> {
    let result = match engine {
        Engine::Mariadb => mariadb::sizes().await,
        Engine::Postgresql => postgres::sizes().await,
    };
    result.unwrap_or_default()
}

pub async fn create_database(engine: Engine, name: &str) -> AppResult<()> {
    match engine {
        Engine::Mariadb => mariadb::create_database(name).await,
        Engine::Postgresql => postgres::create_database(name).await,
    }
}

pub async fn drop_database(engine: Engine, name: &str) -> AppResult<()> {
    match engine {
        Engine::Mariadb => mariadb::drop_database(name).await,
        Engine::Postgresql => postgres::drop_database(name).await,
    }
}

pub async fn create_user(engine: Engine, name: &str, host: &str, password: &str) -> AppResult<()> {
    match engine {
        Engine::Mariadb => mariadb::create_user(name, host, password).await,
        Engine::Postgresql => postgres::create_user(name, password).await,
    }
}

pub async fn set_password(engine: Engine, name: &str, host: &str, password: &str) -> AppResult<()> {
    match engine {
        Engine::Mariadb => mariadb::set_password(name, host, password).await,
        Engine::Postgresql => postgres::set_password(name, password).await,
    }
}

pub async fn drop_user(engine: Engine, name: &str, host: &str) -> AppResult<()> {
    match engine {
        Engine::Mariadb => mariadb::drop_user(name, host).await,
        Engine::Postgresql => postgres::drop_user(name).await,
    }
}

pub async fn grant(engine: Engine, database: &str, user: &str, host: &str, access: Access) -> AppResult<()> {
    match engine {
        Engine::Mariadb => mariadb::grant(database, user, host, access).await,
        Engine::Postgresql => postgres::grant(database, user, access).await,
    }
}

pub async fn revoke(engine: Engine, database: &str, user: &str, host: &str) -> AppResult<()> {
    match engine {
        Engine::Mariadb => mariadb::revoke(database, user, host).await,
        Engine::Postgresql => postgres::revoke(database, user).await,
    }
}

/// Veritabanının SQL dökümünü standart çıktısına yazan süreci hazırlar.
pub fn dump_command(engine: Engine, database: &str) -> Command {
    match engine {
        Engine::Mariadb => mariadb::dump_command(database),
        Engine::Postgresql => postgres::dump_command(database),
    }
}

/// SQL dosyasını standart girdisinden okuyup veritabanına yükleyen süreci hazırlar.
pub fn import_command(engine: Engine, database: &str) -> Command {
    match engine {
        Engine::Mariadb => mariadb::import_command(database),
        Engine::Postgresql => postgres::import_command(database),
    }
}
