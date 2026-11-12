//! Kara liste (DNSBL) denetimi: sunucunun posta gönderebileceği genel IPv4 adresleri günde bir
//! kez bilinen kara listelerde sorgulanır. Sorgu, sistem çözücüsüyle `<ters-ip>.<liste>` adının
//! A kaydına bakmaktır; kayıt varsa adres listelenmiştir.
//!
//! Sonuç `settings` tablosunda saklanır; Sağlık merkezi o sonuca bakar (bkz. `health`).

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::db::{now, Db};
use crate::error::AppResult;
use crate::state::AppState;

/// Sorgulanan listeler: (alan adı, okunaklı ad).
pub const LISTS: &[(&str, &str)] = &[
    ("zen.spamhaus.org", "Spamhaus ZEN"),
    ("bl.spamcop.net", "SpamCop"),
    ("b.barracudacentral.org", "Barracuda"),
];

const RESULT_KEY: &str = "blacklist_result";
const CHECK_INTERVAL_SECS: i64 = 24 * 3600;
/// Bundan eski sonuç Sağlık merkezinde kullanılmaz (denetim uzun süredir çalışamıyor demektir).
pub const STALE_AFTER_SECS: i64 = 3 * 24 * 3600;
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(6);

/// IPv4 adresinin sorgu adı: sekizliler ters çevrilip listenin alan adı eklenir.
pub fn query_name(ip: Ipv4Addr, list: &str) -> String {
    let [a, b, c, d] = ip.octets();
    format!("{d}.{c}.{b}.{a}.{list}")
}

/// Bir sorgunun sonucu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "codes", rename_all = "snake_case")]
pub enum Verdict {
    /// Liste adresi içeriyor; dönen yanıt kodları (ör. `127.0.0.2`).
    Listed(Vec<String>),
    Clean,
    /// Liste sorguyu reddetti (Spamhaus'un 127.255.255.x yanıtları: genel çözücüden sorgu, sınır aşımı...).
    /// Listelenmiş sayılmaz.
    Refused,
    /// Yanıt yorumlanamadı ya da çözücü yanıt vermedi.
    Unknown,
}

/// Çözücünün döndürdüğü adresleri yorumlar.
///
/// Gerçek listeleme yanıtları `127.0.0.x` aralığındadır. Spamhaus, sorguyu cevaplayamadığında
/// `127.255.255.x` döner; bu bir listeleme değil hata bildirimidir. Loopback dışı bir yanıt
/// (NXDOMAIN'i reklam sayfasına çeviren çözücüler) güvenilmez sayılır.
pub fn interpret(answers: &[IpAddr]) -> Verdict {
    if answers.is_empty() {
        return Verdict::Clean;
    }
    let mut codes = Vec::new();
    let mut refused = false;
    for answer in answers {
        let IpAddr::V4(v4) = answer else { continue };
        let [a, b, c, _] = v4.octets();
        if a != 127 {
            continue;
        }
        if (a, b, c) == (127, 255, 255) {
            refused = true;
        } else {
            codes.push(v4.to_string());
        }
    }
    if !codes.is_empty() {
        Verdict::Listed(codes)
    } else if refused {
        Verdict::Refused
    } else {
        Verdict::Unknown
    }
}

/// Adres posta için sorgulanabilir mi? Özel, yerel ve belgeleme adresleri hiçbir listede yer almaz.
pub fn is_public(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    let carrier_grade_nat = a == 100 && (64..=127).contains(&b);
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || carrier_grade_nat
        || a == 0
        || a >= 240)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub ip: String,
    pub list: String,
    pub label: String,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Outcome {
    pub checked_at: i64,
    pub entries: Vec<Entry>,
    /// Genel adres olmadığı için sorgulanmayan adresler.
    pub skipped: Vec<String>,
}

impl Outcome {
    /// Listelenmiş adresler ve hangi listelerde olduğu (okunaklı adlarla).
    pub fn listed(&self) -> Vec<(String, Vec<String>)> {
        let mut found: Vec<(String, Vec<String>)> = Vec::new();
        for entry in self.entries.iter().filter(|entry| matches!(entry.verdict, Verdict::Listed(_))) {
            match found.iter_mut().find(|(ip, _)| *ip == entry.ip) {
                Some((_, lists)) => lists.push(entry.label.clone()),
                None => found.push((entry.ip.clone(), vec![entry.label.clone()])),
            }
        }
        found
    }

    /// Sorgulanan her adres için en az bir liste sorguyu reddetti mi?
    pub fn refused_lists(&self) -> Vec<String> {
        let mut names: Vec<String> = self.entries.iter().filter(|entry| entry.verdict == Verdict::Refused).map(|entry| entry.label.clone()).collect();
        names.sort();
        names.dedup();
        names
    }
}

pub fn load(db: &Db) -> Outcome {
    db.setting(RESULT_KEY).ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()
}

fn save(db: &Db, outcome: &Outcome) -> AppResult<()> {
    let raw = serde_json::to_string(outcome).map_err(|err| crate::error::AppError::internal(err.to_string()))?;
    db.set_setting(RESULT_KEY, &raw)
}

/// Posta gönderebilecek adresler: sunucudaki IPv4 adresleri (NAT arkasındaysa dış karşılıkları) ve
/// DNS kayıtlarında kullanılan genel adres. İkinci değer: genel olmadığı için atlananlar.
pub async fn sender_ips(db: &Db) -> (Vec<Ipv4Addr>, Vec<String>) {
    let mut candidates: Vec<Ipv4Addr> = Vec::new();
    if let Ok(list) = crate::addresses::list(db).await {
        for item in list.into_iter().filter(|item| item.present && item.family == "ipv4") {
            let text = if item.public_address.is_empty() { item.address } else { item.public_address };
            if let Ok(ip) = text.parse() {
                candidates.push(ip);
            }
        }
    }
    if let Ok(ip) = crate::dns::server_ip(db).parse::<Ipv4Addr>() {
        candidates.push(ip);
    }
    candidates.sort();
    candidates.dedup();
    let (public, private): (Vec<_>, Vec<_>) = candidates.into_iter().partition(|ip| is_public(*ip));
    (public, private.iter().map(Ipv4Addr::to_string).collect())
}

/// Tek bir adı sistem çözücüsüyle sorgular.
async fn lookup(name: &str) -> Verdict {
    match tokio::time::timeout(LOOKUP_TIMEOUT, tokio::net::lookup_host((name, 0))).await {
        Err(_) => Verdict::Unknown,
        Ok(Ok(addresses)) => interpret(&addresses.map(|address| address.ip()).collect::<Vec<_>>()),
        // Kayıt yok (NXDOMAIN) çözücüde hata olarak görünür; geçici çözücü arızaları ayrılır.
        Ok(Err(err)) => {
            let message = err.to_string().to_lowercase();
            if message.contains("temporary") || message.contains("try again") || message.contains("server fail") || message.contains("timed out") {
                Verdict::Unknown
            } else {
                Verdict::Clean
            }
        }
    }
}

/// Bütün adresleri bütün listelerde sorgular ve sonucu saklar.
pub async fn run(db: &Db) -> AppResult<Outcome> {
    let (ips, skipped) = sender_ips(db).await;
    let mut entries = Vec::new();
    for ip in ips {
        for (list, label) in LISTS {
            let verdict = lookup(&query_name(ip, list)).await;
            entries.push(Entry { ip: ip.to_string(), list: (*list).to_string(), label: (*label).to_string(), verdict });
        }
    }
    let outcome = Outcome { checked_at: now(), entries, skipped };
    save(db, &outcome)?;
    Ok(outcome)
}

/// Günde bir denetim; posta bileşeni kurulu değilse atlanır.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(240)).await;
        loop {
            if crate::mail::is_installed() && now() - load(&state.db).checked_at >= CHECK_INTERVAL_SECS {
                if let Err(err) = run(&state.db).await {
                    tracing::warn!(error = %err, "kara liste denetimi yapılamadı");
                } else {
                    // Sonuç Sağlık merkezine hemen yansısın.
                    crate::health::run_checks(&state).await;
                }
            }
            tokio::time::sleep(Duration::from_secs(30 * 60)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn query_name_reverses_the_octets() {
        assert_eq!(query_name(Ipv4Addr::new(203, 0, 113, 7), "zen.spamhaus.org"), "7.113.0.203.zen.spamhaus.org");
        assert_eq!(query_name(Ipv4Addr::new(1, 2, 3, 4), "bl.spamcop.net"), "4.3.2.1.bl.spamcop.net");
    }

    #[test]
    fn listing_answers_are_recognised() {
        assert_eq!(interpret(&[ip("127.0.0.2")]), Verdict::Listed(vec!["127.0.0.2".into()]));
        assert_eq!(interpret(&[ip("127.0.0.4"), ip("127.0.0.10")]), Verdict::Listed(vec!["127.0.0.4".into(), "127.0.0.10".into()]));
        assert_eq!(interpret(&[]), Verdict::Clean);
    }

    #[test]
    fn spamhaus_refusals_are_not_listings() {
        assert_eq!(interpret(&[ip("127.255.255.254")]), Verdict::Refused, "genel çözücüden sorgu");
        assert_eq!(interpret(&[ip("127.255.255.252")]), Verdict::Refused);
        assert_eq!(interpret(&[ip("127.255.255.255")]), Verdict::Refused, "sorgu sınırı aşıldı");
        // Gerçek bir listeleme yanıtı varsa hata kodu onu gizlemez.
        assert_eq!(interpret(&[ip("127.255.255.254"), ip("127.0.0.3")]), Verdict::Listed(vec!["127.0.0.3".into()]));
    }

    #[test]
    fn hijacked_or_foreign_answers_are_unknown() {
        // NXDOMAIN'i reklam adresine çeviren çözücü.
        assert_eq!(interpret(&[ip("93.184.216.34")]), Verdict::Unknown);
        assert_eq!(interpret(&[ip("2001:db8::1")]), Verdict::Unknown);
    }

    #[test]
    fn only_routable_addresses_are_queried() {
        for private in ["10.1.2.3", "172.16.0.1", "192.168.1.25", "127.0.0.1", "169.254.1.1", "100.64.0.1", "0.0.0.0", "224.0.0.1", "203.0.113.5", "255.255.255.255"] {
            assert!(!is_public(private.parse().unwrap()), "{private}");
        }
        for public in ["8.8.8.8", "93.184.216.34", "100.63.0.1", "172.32.0.1"] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn listed_addresses_are_grouped_with_their_lists() {
        let entry = |ip: &str, label: &str, verdict: Verdict| Entry { ip: ip.into(), list: "x".into(), label: label.into(), verdict };
        let outcome = Outcome {
            checked_at: 1,
            entries: vec![
                entry("8.8.8.8", "Spamhaus ZEN", Verdict::Listed(vec!["127.0.0.2".into()])),
                entry("8.8.8.8", "SpamCop", Verdict::Listed(vec!["127.0.0.2".into()])),
                entry("8.8.8.8", "Barracuda", Verdict::Clean),
                entry("9.9.9.9", "Spamhaus ZEN", Verdict::Refused),
            ],
            skipped: vec![],
        };
        assert_eq!(outcome.listed(), vec![("8.8.8.8".to_string(), vec!["Spamhaus ZEN".to_string(), "SpamCop".to_string()])]);
        assert_eq!(outcome.refused_lists(), vec!["Spamhaus ZEN".to_string()]);
    }
}
