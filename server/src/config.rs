//! Panel yapılandırması: `/etc/pixpanel/config.toml` dosyasından okunur.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

pub const DEFAULT_CONFIG_PATH: &str = "/etc/pixpanel/config.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Panelin dinlediği adres.
    pub listen_addr: IpAddr,
    /// Panelin dinlediği port.
    pub listen_port: u16,
    /// Veritabanı, sertifikalar ve çalışma dosyalarının tutulduğu klasör.
    pub data_dir: PathBuf,
    /// Kapatılırsa panel düz HTTP ile çalışır (yalnızca ters vekil arkasında önerilir).
    pub tls: bool,
    /// Oturumun boşta kalma süresi (saniye).
    pub session_ttl_secs: i64,
    /// Ücretsiz sertifikaların alındığı ACME sağlayıcısının dizin adresi.
    pub acme_directory: String,
    /// ACME sağlayıcısı özel bir kök sertifika kullanıyorsa (ör. sınama sunucusu) PEM dosyasının yolu.
    pub acme_root_ca: Option<PathBuf>,
    /// Panelin kendi adresi dışında WebSocket bağlantısına izin verilen kaynaklar
    /// (ör. geliştirme sunucusu: `http://localhost:5183`).
    pub allowed_origins: Vec<String>,
    /// Telegram bildirimlerinin gönderildiği API adresi (sınama için değiştirilebilir).
    pub telegram_api: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0".parse().expect("geçerli adres"),
            listen_port: 8443,
            data_dir: PathBuf::from("/var/lib/pixpanel"),
            tls: true,
            session_ttl_secs: 12 * 3600,
            acme_directory: "https://acme-v02.api.letsencrypt.org/directory".into(),
            acme_root_ca: None,
            allowed_origins: Vec::new(),
            telegram_api: "https://api.telegram.org".into(),
        }
    }
}

impl Config {
    /// Dosya yoksa varsayılan değerlerle döner.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("yapılandırma okunamadı: {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("yapılandırma hatalı: {}", path.display()))
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("pixpanel.db")
    }

    pub fn tls_dir(&self) -> PathBuf {
        self.data_dir.join("tls")
    }
}
