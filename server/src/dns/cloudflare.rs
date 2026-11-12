//! Cloudflare API istemcisi: bölgeler, DNS kayıtları ve birkaç bölge ayarı.
//!
//! Kimlik doğrulama, hesabın panelde kaydettiği API anahtarıyla (Bearer) yapılır.
//! Anahtar yalnızca istek başlığında taşınır; adreslere ve hata iletilerine girmez.

use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use super::Record;
use crate::error::{AppError, AppResult};

/// Panelin Cloudflare üzerinde yazabildiği kayıt türleri. SRV ve CAA, Cloudflare'de `content`
/// yerine yapılandırılmış `data` alanıyla yazılır (bkz. `Client::body`).
pub const WRITABLE_KINDS: &[&str] = &["A", "AAAA", "CNAME", "MX", "TXT", "NS", "SRV", "CAA"];

pub fn validate_token(token: &str) -> AppResult<()> {
    let valid = (20..=200).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if valid { Ok(()) } else { Err(AppError::bad("Geçersiz Cloudflare API anahtarı")) }
}

fn validate_id(id: &str) -> AppResult<()> {
    let valid = id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit());
    if valid { Ok(()) } else { Err(AppError::bad("Geçersiz Cloudflare kimliği")) }
}

pub struct Client {
    base: String,
    token: String,
    http: reqwest::Client,
}

#[derive(Debug, Serialize)]
pub struct RemoteZone {
    pub id: String,
    pub name: String,
}

impl Client {
    pub fn new(base: &str, token: &str) -> AppResult<Self> {
        validate_token(token)?;
        let http = reqwest::Client::builder().timeout(Duration::from_secs(20)).build().map_err(|err| AppError::internal(err.to_string()))?;
        Ok(Self { base: base.trim_end_matches('/').to_string(), token: token.to_string(), http })
    }

    /// İsteği gönderir ve yanıt zarfının tamamını döner (`result`, `result_info`...).
    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> AppResult<Value> {
        let mut request = self.http.request(method, format!("{}{path}", self.base)).bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|err| AppError::bad(format!("Cloudflare'e ulaşılamadı: {}", err.without_url())))?;
        let status = response.status();
        let value: Value = response.json().await.unwrap_or(Value::Null);
        if status.is_success() && value["success"].as_bool().unwrap_or(false) {
            return Ok(value);
        }
        let detail = value["errors"][0]["message"].as_str().map(str::to_string).unwrap_or_else(|| format!("HTTP {}", status.as_u16()));
        Err(match status.as_u16() {
            401 | 403 => AppError::bad(format!("Cloudflare anahtarı reddetti ({detail}). Anahtarın bu bölge için DNS düzenleme yetkisi olduğundan emin olun.")),
            _ => AppError::bad(format!("Cloudflare isteği kabul etmedi: {detail}")),
        })
    }

    pub async fn verify(&self) -> AppResult<()> {
        self.call(reqwest::Method::GET, "/user/tokens/verify", None).await.map(|_| ())
    }

    /// Anahtarın erişebildiği bölgeler (ilk 200).
    pub async fn zones(&self) -> AppResult<Vec<RemoteZone>> {
        let mut zones = Vec::new();
        for page in 1..=4 {
            let value = self.call(reqwest::Method::GET, &format!("/zones?per_page=50&page={page}"), None).await?;
            let items = value["result"].as_array().cloned().unwrap_or_default();
            zones.extend(items.iter().map(|zone| RemoteZone { id: zone["id"].as_str().unwrap_or_default().to_string(), name: zone["name"].as_str().unwrap_or_default().to_string() }));
            if value["result_info"]["total_pages"].as_u64().unwrap_or(1) <= page {
                break;
            }
        }
        Ok(zones)
    }

    fn record_from(item: &Value) -> Record {
        let kind = item["type"].as_str().unwrap_or_default().to_string();
        let mut content = item["content"].as_str().unwrap_or_default().to_string();
        let mut priority = item["priority"].as_u64().map(|value| value as u32);
        // Cloudflare TXT değerlerini tırnak içinde döndürebilir.
        if kind == "TXT" && content.len() >= 2 && content.starts_with('"') && content.ends_with('"') {
            content = content[1..content.len() - 1].replace("\" \"", "").replace("\\\"", "\"");
        }
        // SRV ve CAA'nın asıl verisi `data` alanındadır; panelin biçimine çevrilir.
        let data = &item["data"];
        match kind.as_str() {
            "SRV" if data.is_object() => {
                let target = data["target"].as_str().unwrap_or_default().trim_end_matches('.').to_ascii_lowercase();
                content = format!("{} {} {target}", data["weight"].as_u64().unwrap_or(0), data["port"].as_u64().unwrap_or(0));
                priority = data["priority"].as_u64().map(|value| value as u32).or(priority);
            }
            "SRV" => {
                // `data` yoksa içerik `ağırlık port hedef` (ya da başında öncelikle dört alan) olarak gelir.
                let parts: Vec<&str> = content.split_whitespace().collect();
                if let [first, weight, port, target] = parts.as_slice() {
                    priority = first.parse().ok().or(priority);
                    content = format!("{weight} {port} {}", target.trim_end_matches('.').to_ascii_lowercase());
                }
            }
            "CAA" if data.is_object() => {
                content = format!("{} {} \"{}\"", data["flags"].as_u64().unwrap_or(0), data["tag"].as_str().unwrap_or_default(), data["value"].as_str().unwrap_or_default());
            }
            _ => {}
        }
        Record {
            id: item["id"].as_str().unwrap_or_default().to_string(),
            kind,
            name: item["name"].as_str().unwrap_or_default().to_ascii_lowercase(),
            content,
            ttl: item["ttl"].as_u64().unwrap_or(1) as u32,
            priority,
            proxied: item["proxied"].as_bool(),
        }
    }

    pub async fn list(&self, zone_id: &str) -> AppResult<Vec<Record>> {
        validate_id(zone_id)?;
        let mut records = Vec::new();
        for page in 1..=50 {
            let value = self.call(reqwest::Method::GET, &format!("/zones/{zone_id}/dns_records?per_page=100&page={page}"), None).await?;
            records.extend(value["result"].as_array().into_iter().flatten().map(Self::record_from));
            if value["result_info"]["total_pages"].as_u64().unwrap_or(1) <= page {
                break;
            }
        }
        Ok(records)
    }

    fn body(record: &Record) -> AppResult<Value> {
        if !WRITABLE_KINDS.contains(&record.kind.as_str()) {
            return Err(AppError::bad(format!("{} kayıtları Cloudflare'de panelden düzenlenemiyor; Cloudflare panelini kullanın", record.kind)));
        }
        let proxied = record.proxied.unwrap_or(false) && matches!(record.kind.as_str(), "A" | "AAAA" | "CNAME");
        // Vekillenen kayıtlarda süreyi Cloudflare belirler (1 = otomatik).
        let ttl = if proxied { 1 } else { record.ttl };
        match record.kind.as_str() {
            // İçerik `ağırlık port hedef`; Cloudflare yapılandırılmış alan ister.
            "SRV" => {
                let parts: Vec<&str> = record.content.split_whitespace().collect();
                let [weight, port, target] = parts.as_slice() else { return Err(AppError::bad("SRV değeri “ağırlık port hedef” biçiminde olmalı")) };
                let (Ok(weight), Ok(port)) = (weight.parse::<u16>(), port.parse::<u16>()) else { return Err(AppError::bad("SRV ağırlığı ve portu sayı olmalı")) };
                return Ok(json!({
                    "type": "SRV",
                    "name": record.name,
                    "ttl": ttl,
                    "data": { "priority": record.priority.unwrap_or(10), "weight": weight, "port": port, "target": target },
                }));
            }
            // İçerik `bayrak etiket "değer"`.
            "CAA" => {
                let mut parts = record.content.splitn(3, ' ');
                let (flags, tag, value) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default(), parts.next().unwrap_or_default().trim_matches('"'));
                let Ok(flags) = flags.parse::<u8>() else { return Err(AppError::bad("CAA bayrağı sayı olmalı")) };
                return Ok(json!({
                    "type": "CAA",
                    "name": record.name,
                    "ttl": ttl,
                    "data": { "flags": flags, "tag": tag, "value": value },
                }));
            }
            _ => {}
        }
        let mut body = json!({
            "type": record.kind,
            "name": record.name,
            "content": record.content,
            "ttl": ttl,
        });
        if matches!(record.kind.as_str(), "A" | "AAAA" | "CNAME") {
            body["proxied"] = json!(proxied);
        }
        if record.kind == "MX" {
            body["priority"] = json!(record.priority.unwrap_or(10));
        }
        Ok(body)
    }

    pub async fn create(&self, zone_id: &str, record: &Record) -> AppResult<()> {
        validate_id(zone_id)?;
        self.call(reqwest::Method::POST, &format!("/zones/{zone_id}/dns_records"), Some(Self::body(record)?)).await.map(|_| ())
    }

    pub async fn update(&self, zone_id: &str, record_id: &str, record: &Record) -> AppResult<()> {
        validate_id(zone_id)?;
        validate_id(record_id)?;
        self.call(reqwest::Method::PUT, &format!("/zones/{zone_id}/dns_records/{record_id}"), Some(Self::body(record)?)).await.map(|_| ())
    }

    pub async fn delete(&self, zone_id: &str, record_id: &str) -> AppResult<()> {
        validate_id(zone_id)?;
        validate_id(record_id)?;
        self.call(reqwest::Method::DELETE, &format!("/zones/{zone_id}/dns_records/{record_id}"), None).await.map(|_| ())
    }

    /// Bölge ayarının değeri (ör. `ssl` → `full`, `development_mode` → `off`).
    pub async fn setting(&self, zone_id: &str, name: &str) -> AppResult<String> {
        validate_id(zone_id)?;
        let value = self.call(reqwest::Method::GET, &format!("/zones/{zone_id}/settings/{name}"), None).await?;
        Ok(value["result"]["value"].as_str().unwrap_or_default().to_string())
    }

    pub async fn set_setting(&self, zone_id: &str, name: &str, value: &str) -> AppResult<()> {
        validate_id(zone_id)?;
        self.call(reqwest::Method::PATCH, &format!("/zones/{zone_id}/settings/{name}"), Some(json!({ "value": value }))).await.map(|_| ())
    }

    pub async fn purge_cache(&self, zone_id: &str) -> AppResult<()> {
        validate_id(zone_id)?;
        self.call(reqwest::Method::POST, &format!("/zones/{zone_id}/purge_cache"), Some(json!({ "purge_everything": true }))).await.map(|_| ())
    }
}

/// Cloudflare'in uç sunucularının adres aralıkları (son güncelleme alınamazsa kullanılır).
pub const FALLBACK_RANGES: &[&str] = &[
    "173.245.48.0/20", "103.21.244.0/22", "103.22.200.0/22", "103.31.4.0/22", "141.101.64.0/18", "108.162.192.0/18",
    "190.93.240.0/20", "188.114.96.0/20", "197.234.240.0/22", "198.41.128.0/17", "162.158.0.0/15", "104.16.0.0/13",
    "104.24.0.0/14", "172.64.0.0/13", "131.0.72.0/22", "2400:cb00::/32", "2606:4700::/32", "2803:f800::/32",
    "2405:b500::/32", "2405:8100::/32", "2a06:98c0::/29", "2c0f:f248::/32",
];

/// Satırın geçerli bir adres aralığı (`adres/önek`) olup olmadığını denetler.
pub fn is_cidr(line: &str) -> bool {
    line.split_once('/').is_some_and(|(address, prefix)| {
        address.parse::<std::net::IpAddr>().is_ok_and(|ip| prefix.parse::<u8>().is_ok_and(|bits| bits <= if ip.is_ipv4() { 32 } else { 128 }))
    })
}

/// Güncel adres aralıklarını Cloudflare'den indirir; alınamazsa gömülü listeyi döner.
pub async fn fetch_ranges(base: &str) -> Vec<String> {
    let fallback = || FALLBACK_RANGES.iter().map(|range| range.to_string()).collect::<Vec<_>>();
    let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(15)).build() else { return fallback() };
    let Ok(response) = client.get(format!("{}/ips", base.trim_end_matches('/'))).send().await else { return fallback() };
    let Ok(value) = response.json::<Value>().await else { return fallback() };
    let ranges: Vec<String> = ["ipv4_cidrs", "ipv6_cidrs"]
        .iter()
        .flat_map(|key| value["result"][key].as_array().cloned().unwrap_or_default())
        .filter_map(|item| item.as_str().map(str::to_string))
        .filter(|range| is_cidr(range))
        .collect();
    // Beklenmedik biçimde kısa bir liste, gerçek ziyaretçi adreslerinin kaybolmasına yol açardı.
    if ranges.len() >= 10 { ranges } else { fallback() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srv_and_caa_are_written_with_data_field() {
        let mut srv = Record::new("SRV", "_sip._tcp.ornek.com", "5 5060 sip.ornek.com");
        srv.priority = Some(10);
        let body = Client::body(&srv).unwrap();
        assert_eq!(body["data"], json!({ "priority": 10, "weight": 5, "port": 5060, "target": "sip.ornek.com" }));
        assert!(body.get("content").is_none());
        assert_eq!(body["name"], "_sip._tcp.ornek.com");

        let caa = Record::new("CAA", "ornek.com", "0 issue \"letsencrypt.org\"");
        let body = Client::body(&caa).unwrap();
        assert_eq!(body["data"], json!({ "flags": 0, "tag": "issue", "value": "letsencrypt.org" }));
        assert!(Client::body(&Record::new("SRV", "_x._tcp.ornek.com", "5060 sip.ornek.com")).is_err());
        assert!(Client::body(&Record::new("PTR", "x.ornek.com", "y.ornek.com")).is_err());
    }

    #[test]
    fn srv_and_caa_are_read_back_in_panel_format() {
        let srv = json!({ "id": "a", "type": "SRV", "name": "_SIP._tcp.ornek.com", "content": "5 5060 sip.ornek.com", "ttl": 300, "priority": 10,
            "data": { "priority": 10, "weight": 5, "port": 5060, "target": "sip.ornek.com" } });
        let record = Client::record_from(&srv);
        assert_eq!((record.content.as_str(), record.priority, record.name.as_str()), ("5 5060 sip.ornek.com", Some(10), "_sip._tcp.ornek.com"));
        let caa = json!({ "id": "b", "type": "CAA", "name": "ornek.com", "content": "0 issue \"letsencrypt.org\"", "ttl": 1,
            "data": { "flags": 0, "tag": "issue", "value": "letsencrypt.org" } });
        assert_eq!(Client::record_from(&caa).content, "0 issue \"letsencrypt.org\"");
        // `data` olmadan gelen SRV (öncelikli dört alan)
        let plain = json!({ "id": "c", "type": "SRV", "name": "_a._tcp.ornek.com", "content": "7 5 5060 Sip.Ornek.com.", "ttl": 300 });
        let record = Client::record_from(&plain);
        assert_eq!((record.content.as_str(), record.priority), ("5 5060 sip.ornek.com", Some(7)));
    }
}
