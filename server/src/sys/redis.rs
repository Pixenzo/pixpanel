//! Yerel Redis sunucusuyla konuşan küçük istemci (RESP2).
//!
//! Komutlar argüman dizisi olarak, uzunluk önekli biçimde gönderilir; kullanıcı
//! girdisi komut metnine karışamaz ve ikili veriler bozulmaz.

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::error::{AppError, AppResult};

pub const CONFIG_PATH: &str = "/etc/redis/redis.conf";
const TIMEOUT: Duration = Duration::from_secs(10);
/// Tek bir yanıt öğesi için üst sınır; panelin belleğini korur.
const MAX_BULK: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub enum Value {
    Simple(String),
    Int(i64),
    Bulk(Option<Vec<u8>>),
    Array(Option<Vec<Value>>),
}

impl Value {
    /// Metin olarak okunabilen yanıt (bozuk UTF-8 yer tutucuyla gösterilir).
    pub fn text(&self) -> String {
        match self {
            Value::Simple(text) => text.clone(),
            Value::Int(number) => number.to_string(),
            Value::Bulk(Some(bytes)) => String::from_utf8_lossy(bytes).into_owned(),
            _ => String::new(),
        }
    }

    pub fn int(&self) -> i64 {
        match self {
            Value::Int(number) => *number,
            other => other.text().parse().unwrap_or(0),
        }
    }

    pub fn items(&self) -> Vec<Value> {
        match self {
            Value::Array(Some(items)) => items.clone(),
            _ => Vec::new(),
        }
    }
}

/// Yapılandırma dosyasından bir yönergenin değerini okur.
fn config_value(raw: &str, key: &str) -> Option<String> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once(char::is_whitespace))
        .filter(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.trim().trim_matches('"').to_string())
        .next_back()
}

pub struct Client {
    stream: BufReader<TcpStream>,
}

impl Client {
    /// Yerel sunucuya bağlanır; parola tanımlıysa yapılandırma dosyasından okuyup kimlik doğrular.
    pub async fn connect() -> AppResult<Self> {
        let raw = std::fs::read_to_string(CONFIG_PATH).unwrap_or_default();
        let port: u16 = config_value(&raw, "port").and_then(|value| value.parse().ok()).unwrap_or(6379);
        let password = config_value(&raw, "requirepass").filter(|value| !value.is_empty());
        Self::connect_to(port, password.as_deref()).await
    }

    /// Yerel adresteki belirli bir örneğe bağlanır (hesap başına Redis örnekleri).
    pub async fn connect_to(port: u16, password: Option<&str>) -> AppResult<Self> {
        let stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(("127.0.0.1", port)))
            .await
            .map_err(|_| AppError::bad("Redis yanıt vermiyor"))?
            .map_err(|_| AppError::bad("Redis'e bağlanılamadı; servisin çalıştığını denetleyin"))?;
        let mut client = Self { stream: BufReader::new(stream) };
        if let Some(password) = password {
            client.command(&[b"AUTH", password.as_bytes()]).await?;
        }
        Ok(client)
    }

    /// Komutu gönderir ve yanıtını okur. Redis'in hata yanıtı `AppError` olarak döner.
    pub async fn command(&mut self, args: &[&[u8]]) -> AppResult<Value> {
        let mut request = format!("*{}\r\n", args.len()).into_bytes();
        for arg in args {
            request.extend_from_slice(format!("${}\r\n", arg.len()).as_bytes());
            request.extend_from_slice(arg);
            request.extend_from_slice(b"\r\n");
        }
        let exchange = async {
            self.stream.get_mut().write_all(&request).await?;
            self.read_value().await
        };
        tokio::time::timeout(TIMEOUT, exchange).await.map_err(|_| AppError::bad("Redis yanıt vermedi"))?
    }

    /// Metin argümanlarıyla komut göndermek için kısaltma.
    pub async fn run(&mut self, args: &[&str]) -> AppResult<Value> {
        let bytes: Vec<&[u8]> = args.iter().map(|arg| arg.as_bytes()).collect();
        self.command(&bytes).await
    }

    async fn read_line(&mut self) -> AppResult<String> {
        let mut line = String::new();
        self.stream.read_line(&mut line).await?;
        if line.is_empty() {
            return Err(AppError::bad("Redis bağlantısı kapandı"));
        }
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }

    fn read_value(&mut self) -> std::pin::Pin<Box<dyn std::future::Future<Output = AppResult<Value>> + Send + '_>> {
        Box::pin(async move {
            let line = self.read_line().await?;
            let (kind, rest) = line.split_at(1);
            let length = || rest.parse::<i64>().map_err(|_| AppError::internal("Redis yanıtı çözülemedi"));
            match kind {
                "+" => Ok(Value::Simple(rest.to_string())),
                "-" => Err(AppError::bad(format!("Redis: {rest}"))),
                ":" => Ok(Value::Int(length()?)),
                "$" => {
                    let size = length()?;
                    if size < 0 {
                        return Ok(Value::Bulk(None));
                    }
                    let size = size as usize;
                    if size > MAX_BULK {
                        return Err(AppError::bad("Değer panelde gösterilemeyecek kadar büyük"));
                    }
                    let mut buffer = vec![0u8; size + 2];
                    self.stream.read_exact(&mut buffer).await?;
                    buffer.truncate(size);
                    Ok(Value::Bulk(Some(buffer)))
                }
                "*" => {
                    let count = length()?;
                    if count < 0 {
                        return Ok(Value::Array(None));
                    }
                    let mut items = Vec::with_capacity((count as usize).min(1024));
                    for _ in 0..count {
                        items.push(self.read_value().await?);
                    }
                    Ok(Value::Array(Some(items)))
                }
                _ => Err(AppError::internal("Redis yanıtı çözülemedi")),
            }
        })
    }
}
