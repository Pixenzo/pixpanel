//! Karantina: Rspamd'nin reddetme eşiğini aşan iletilerin reddedilmek yerine tutulması.
//!
//! Tasarım: Rspamd `quarantine_on_reject` ile reddedilecek iletiler için milter "karantina"
//! komutunu gönderir; Postfix de iletiyi kendi **bekletme (hold) kuyruğuna** alır. Ek bir
//! depolama ya da ayrıştırma katmanı yoktur: panel `postqueue -j` ile listeler, `postcat` ile
//! görüntüler, `postsuper -H` ile bırakır, `postsuper -d` ile siler. İletiler Postfix'in kendi
//! kuyruk dizininde (`/var/spool/postfix/hold`, yalnız postfix kullanıcısı okuyabilir) durur.
//!
//! Sahiplik kuralı (sade olanı): yönetici bütün karantinayı görür. Diğer hesaplar yalnızca
//! **bütün alıcıları** kendi görebildiği posta alan adlarında olan iletileri görür; bir alıcısı
//! bile başka hesabın (ya da dış bir adresin) olan ileti onlara hiç gösterilmez — ne listede, ne
//! içerik olarak, ne de bırakma/silme işleminde. Böylece başka hesabın iletisi sızmaz.

use std::collections::HashSet;
use std::process::Stdio;
use std::time::Duration;

use base64::Engine;
use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncReadExt;

use crate::error::{AppError, AppResult};
use crate::sys::exec;

/// Karantinadaki iletinin saklanma süresi (14 gün).
pub const RETENTION_SECS: i64 = 14 * 24 * 3600;
/// İleti görüntülemede okunan en fazla bayt.
pub const VIEW_LIMIT: usize = 64 * 1024;
/// Listede özetlenen (konusu okunan) en fazla ileti sayısı; en yeniden başlanır.
pub const MAX_LISTED: usize = 300;
/// Toplu silmede tek istekte kabul edilen en fazla kimlik.
pub const MAX_BULK: usize = 500;
/// Konu ve puan için ileti başından okunan bayt sayısı.
const SUMMARY_READ: usize = 32 * 1024;

/// Postfix kuyruk kimliği: kısa (onaltılık) ya da uzun (harf-rakam) biçim.
pub fn validate_id(id: &str) -> AppResult<()> {
    let valid = (6..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric());
    if valid { Ok(()) } else { Err(AppError::bad("Geçersiz kuyruk kimliği")) }
}

/// Bekletme kuyruğundaki bir ileti (zarf bilgisi).
#[derive(Debug, Clone, PartialEq)]
pub struct Held {
    pub id: String,
    pub arrived_at: i64,
    pub size: i64,
    pub sender: String,
    pub recipients: Vec<String>,
}

/// `postqueue -j` çıktısından yalnızca bekletme kuyruğundaki iletileri ayıklar.
pub fn parse_held(json_lines: &str) -> Vec<Held> {
    let mut list: Vec<Held> = json_lines
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| entry["queue_name"].as_str() == Some("hold"))
        .filter_map(|entry| {
            let id = entry["queue_id"].as_str()?.to_string();
            validate_id(&id).ok()?;
            Some(Held {
                id,
                arrived_at: entry["arrival_time"].as_i64().unwrap_or(0),
                size: entry["message_size"].as_i64().unwrap_or(0),
                sender: entry["sender"].as_str().unwrap_or("").to_string(),
                recipients: entry["recipients"]
                    .as_array()
                    .map(|list| list.iter().filter_map(|r| r["address"].as_str()).map(|a| a.to_ascii_lowercase()).collect())
                    .unwrap_or_default(),
            })
        })
        .collect();
    list.sort_by(|a, b| b.arrived_at.cmp(&a.arrived_at));
    list
}

pub async fn list_held() -> AppResult<Vec<Held>> {
    let output = exec::run_with_timeout("postqueue", &["-j"], Duration::from_secs(30)).await?;
    if !output.success() {
        return Err(AppError::internal(format!("Posta kuyruğu okunamadı: {}", output.stderr.trim())));
    }
    Ok(parse_held(&output.stdout))
}

// ---- Sahiplik ----

fn recipient_domain(address: &str) -> &str {
    address.rsplit_once('@').map(|(_, domain)| domain).unwrap_or("")
}

/// Bütün alıcıları verilen alan adlarındaysa doğru döner (alıcısız ileti kimsenin değildir).
pub fn all_recipients_within(recipients: &[String], domains: &HashSet<String>) -> bool {
    !recipients.is_empty() && recipients.iter().all(|address| domains.contains(recipient_domain(address)))
}

/// İletiyi isteği yapan görebilir mi? `domains`: yönetici dışındakilerin görebildiği alan adları;
/// `None` yönetici demektir (hepsi).
pub fn visible(held: &Held, domains: Option<&HashSet<String>>) -> bool {
    match domains {
        None => true,
        Some(domains) => all_recipients_within(&held.recipients, domains),
    }
}

// ---- Süre ----

/// Saklama süresini aşan iletilerin kimlikleri.
pub fn expired_ids(held: &[Held], now: i64) -> Vec<String> {
    held.iter().filter(|message| message.arrived_at > 0 && now - message.arrived_at > RETENTION_SECS).map(|message| message.id.clone()).collect()
}

/// Süresi dolan karantina iletilerini siler; silinen sayısını döner.
pub async fn purge_expired(now: i64) -> AppResult<usize> {
    let held = list_held().await?;
    let mut removed = 0;
    for id in expired_ids(&held, now) {
        if delete(&id).await.is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

// ---- İşlemler ----

/// İletiyi karantinadan bırakır (normal teslimata alır).
pub async fn release(id: &str) -> AppResult<()> {
    validate_id(id)?;
    exec::run_ok("postsuper", &["-H", id]).await?;
    // Bırakılan ileti ertelenmiş kuyruğa düşer ve bir sonraki kuyruk turunu (dakikalar) bekler;
    // hemen teslim edilmesi istenir.
    let _ = exec::run("postqueue", &["-i", id]).await;
    Ok(())
}

/// İletiyi siler (yalnızca bekletme kuyruğunda aranır).
pub async fn delete(id: &str) -> AppResult<()> {
    validate_id(id)?;
    exec::run_ok("postsuper", &["-d", id, "hold"]).await?;
    Ok(())
}

/// İletinin başlıklarını ve gövdesini en çok `limit` bayt okur; ileti daha uzunsa `true` döner.
/// Süreç yeterince okununca öldürülür; büyük ileti belleğe alınmaz.
pub async fn read_message(id: &str, limit: usize) -> AppResult<(String, bool)> {
    validate_id(id)?;
    let mut cmd = exec::base_command("postcat");
    cmd.args(["-q", "-h", "-b", id]).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|err| AppError::internal(format!("postcat başlatılamadı: {err}")))?;
    let mut stdout = child.stdout.take().ok_or_else(|| AppError::internal("postcat çıktısı alınamadı"))?;
    let mut buffer = Vec::with_capacity(limit.min(8192));
    let reading = async {
        // Sınırdan bir bayt fazla okunur: fazlası varsa kesildiği anlaşılır.
        (&mut stdout).take(limit as u64 + 1).read_to_end(&mut buffer).await
    };
    let result = tokio::time::timeout(Duration::from_secs(15), reading).await;
    let _ = child.kill().await;
    match result {
        Ok(Ok(_)) => {}
        Ok(Err(err)) => return Err(AppError::internal(format!("İleti okunamadı: {err}"))),
        Err(_) => return Err(AppError::internal("İleti okunurken zaman aşımı")),
    }
    let truncated = buffer.len() > limit;
    buffer.truncate(limit);
    Ok((String::from_utf8_lossy(&buffer).into_owned(), truncated))
}

// ---- Başlık özeti ----

/// Başlık bölümünde `name` başlığının (katlanmış satırlar birleştirilmiş) değeri.
pub fn header_value(raw: &str, name: &str) -> Option<String> {
    let mut found: Option<String> = None;
    for line in raw.lines() {
        // Başlık bölümü ilk boş satırda biter.
        if line.is_empty() {
            break;
        }
        let continuation = line.starts_with(' ') || line.starts_with('\t');
        match &mut found {
            Some(value) if continuation => {
                value.push(' ');
                value.push_str(line.trim());
            }
            Some(_) => break,
            None => {
                if let Some((key, value)) = line.split_once(':') {
                    if key.eq_ignore_ascii_case(name) {
                        found = Some(value.trim().to_string());
                    }
                }
            }
        }
    }
    found
}

/// RFC 2047 kodlu sözcükleri (`=?utf-8?B?...?=`, `=?utf-8?Q?...?=`) çözer; tanınmayanı olduğu gibi bırakır.
pub fn decode_header(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    let mut previous_encoded = false;
    while let Some(start) = rest.find("=?") {
        let (before, tail) = rest.split_at(start);
        match decode_word(tail) {
            Some((text, used)) => {
                // Kodlu sözcükler arasındaki boşluk atılır (RFC 2047 §6.2).
                if !(previous_encoded && before.trim().is_empty()) {
                    out.push_str(before);
                }
                out.push_str(&text);
                rest = &tail[used..];
                previous_encoded = true;
            }
            None => {
                out.push_str(before);
                out.push_str("=?");
                rest = &tail[2..];
                previous_encoded = false;
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_word(input: &str) -> Option<(String, usize)> {
    let body = input.strip_prefix("=?")?;
    let end = body.find("?=")?;
    let word = &body[..end];
    let mut parts = word.splitn(3, '?');
    let (charset, encoding, text) = (parts.next()?, parts.next()?, parts.next()?);
    let bytes = match encoding {
        "B" | "b" => base64::engine::general_purpose::STANDARD.decode(text.as_bytes()).ok()?,
        "Q" | "q" => {
            let raw = text.as_bytes();
            let mut bytes = Vec::with_capacity(raw.len());
            let mut index = 0;
            while index < raw.len() {
                match raw[index] {
                    b'_' => bytes.push(b' '),
                    b'=' if index + 2 < raw.len() => {
                        let value = u8::from_str_radix(std::str::from_utf8(&raw[index + 1..index + 3]).ok()?, 16).ok()?;
                        bytes.push(value);
                        index += 2;
                    }
                    other => bytes.push(other),
                }
                index += 1;
            }
            bytes
        }
        _ => return None,
    };
    let text = match charset.to_ascii_lowercase().as_str() {
        "iso-8859-1" | "latin1" => bytes.iter().map(|b| *b as char).collect(),
        _ => String::from_utf8_lossy(&bytes).into_owned(),
    };
    Some((text, 2 + end + 2))
}

/// Konu başlığı (çözülmüş, denetim karakterlerinden arındırılmış ve kısaltılmış).
pub fn subject_of(raw: &str) -> String {
    let decoded = header_value(raw, "Subject").map(|value| decode_header(&value)).unwrap_or_default();
    let clean: String = decoded.chars().filter(|c| !c.is_control()).collect();
    clean.chars().take(200).collect()
}

/// `X-Spamd-Result: default: False [4.00 / 15.00]; ...` başlığındaki puan.
pub fn score_from_headers(raw: &str) -> Option<f64> {
    let value = header_value(raw, "X-Spamd-Result")?;
    let open = value.find('[')?;
    let close = value[open..].find(']')? + open;
    value[open + 1..close].split('/').next()?.trim().parse().ok()
}

/// Rspamd geçmişindeki bir satır (puan eşleştirmesi için).
#[derive(Debug, Clone)]
pub struct HistoryRow {
    pub sender: String,
    pub subject: String,
    pub score: f64,
    pub action: String,
}

/// Geçmişte (gönderen + konu) uyan, reddedilmiş/karantinaya alınmış en yeni satırın puanı.
pub fn score_from_history(history: &[HistoryRow], sender: &str, subject: &str) -> Option<f64> {
    history
        .iter()
        .find(|row| row.sender.eq_ignore_ascii_case(sender) && row.subject == subject && matches!(row.action.as_str(), "reject" | "quarantine"))
        .map(|row| row.score)
}

/// Rspamd denetleyicisinin geçmişini okur (alınamazsa boş).
pub async fn fetch_history() -> Vec<HistoryRow> {
    let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(5)).build() else { return Vec::new() };
    let Ok(response) = client.get("http://127.0.0.1:11334/history").send().await else { return Vec::new() };
    let Ok(value) = response.json::<Value>().await else { return Vec::new() };
    value["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some(HistoryRow {
                sender: row["sender_smtp"].as_str()?.to_string(),
                subject: row["subject"].as_str().unwrap_or("").to_string(),
                score: row["score"].as_f64()?,
                action: row["action"].as_str().unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// Listede gösterilen ileti satırı.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub id: String,
    pub arrived_at: i64,
    pub size: i64,
    pub sender: String,
    pub recipients: Vec<String>,
    pub subject: String,
    /// Rspamd puanı; bulunamadıysa boş.
    pub score: Option<f64>,
}

/// İletinin konusunu ve puanını okuyup liste satırını kurar.
pub async fn summarize(held: &Held, history: &[HistoryRow]) -> Summary {
    let head = read_message(&held.id, SUMMARY_READ).await.map(|(text, _)| text).unwrap_or_default();
    let subject = subject_of(&head);
    let score = score_from_headers(&head).or_else(|| score_from_history(history, &held.sender, &subject));
    Summary {
        id: held.id.clone(),
        arrived_at: held.arrived_at,
        size: held.size,
        sender: held.sender.clone(),
        recipients: held.recipients.clone(),
        subject,
        score,
    }
}

// ---- Rspamd tarafı ----

const BLOCK_BEGIN: &str = "# PixPanel karantina (başla)";
const BLOCK_END: &str = "# PixPanel karantina (bitti)";

/// Rspamd'nin `worker-proxy.inc` yerel dosyasındaki panelin bloğunu açar ya da kaldırır; dosyanın
/// geri kalanına dokunmaz. Dönen `None` dosyanın silinmesi gerektiğini (boş kaldı) belirtir.
pub fn proxy_config(existing: Option<&str>, enabled: bool) -> Option<String> {
    let mut kept: Vec<&str> = Vec::new();
    let mut inside = false;
    for line in existing.unwrap_or("").lines() {
        if line.trim() == BLOCK_BEGIN {
            inside = true;
        } else if line.trim() == BLOCK_END {
            inside = false;
        } else if !inside {
            kept.push(line);
        }
    }
    while kept.last().is_some_and(|line| line.trim().is_empty()) {
        kept.pop();
    }
    let mut content = kept.join("\n");
    if enabled {
        if !content.is_empty() {
            content.push_str("\n\n");
        }
        content.push_str(&format!("{BLOCK_BEGIN}\nquarantine_on_reject = true;\n{BLOCK_END}"));
    }
    if content.trim().is_empty() {
        None
    } else {
        content.push('\n');
        Some(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domains(list: &[&str]) -> HashSet<String> {
        list.iter().map(|d| d.to_string()).collect()
    }

    fn held(id: &str, arrived_at: i64, recipients: &[&str]) -> Held {
        Held { id: id.into(), arrived_at, size: 10, sender: "x@y.z".into(), recipients: recipients.iter().map(|r| r.to_string()).collect() }
    }

    #[test]
    fn queue_id_validation() {
        for good in ["ABC123", "4Xyz7a1B2cD", "A1B2C3D4E5F6G7H8I9J0"] {
            assert!(validate_id(good).is_ok(), "{good}");
        }
        for bad in ["", "ABC12", "A1B2C3D4E5F6G7H8I9J0K", "ALL;rm", "ab cd12", "../etc/x", "ab\ncd12", "ÇABC123"] {
            assert!(validate_id(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn only_hold_queue_is_parsed() {
        let raw = r#"{"queue_name":"hold","queue_id":"ABC123","arrival_time":100,"message_size":500,"sender":"a@b.c","recipients":[{"address":"Ali@Posta.Test","delay_reason":"x"}]}
{"queue_name":"deferred","queue_id":"DEF456","arrival_time":200,"message_size":1,"sender":"","recipients":[]}
{"queue_name":"hold","queue_id":"bad;id","arrival_time":300,"message_size":1,"sender":"","recipients":[]}
not json"#;
        let list = parse_held(raw);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "ABC123");
        assert_eq!(list[0].recipients, vec!["ali@posta.test"]);
    }

    #[test]
    fn mixed_recipient_message_is_hidden_from_customers() {
        let mine = domains(&["posta.test"]);
        assert!(visible(&held("ABC123", 1, &["a@posta.test"]), Some(&mine)));
        assert!(visible(&held("ABC123", 1, &["a@posta.test", "b@posta.test"]), Some(&mine)));
        assert!(!visible(&held("ABC123", 1, &["a@posta.test", "c@baska.test"]), Some(&mine)));
        assert!(!visible(&held("ABC123", 1, &["c@baska.test"]), Some(&mine)));
        assert!(!visible(&held("ABC123", 1, &[]), Some(&mine)));
        // `@` içermeyen ya da alan adı taklidi yapan alıcı eşleşmez.
        assert!(!visible(&held("ABC123", 1, &["a@posta.test.evil.test"]), Some(&mine)));
        assert!(!visible(&held("ABC123", 1, &["posta.test"]), Some(&mine)));
        assert!(visible(&held("ABC123", 1, &["c@baska.test"]), None));
    }

    #[test]
    fn expiry_uses_fourteen_days() {
        let now = 10_000_000;
        let list = vec![held("OLD123", now - RETENTION_SECS - 1, &["a@b.c"]), held("NEW123", now - RETENTION_SECS + 60, &["a@b.c"]), held("ZERO12", 0, &["a@b.c"])];
        assert_eq!(expired_ids(&list, now), vec!["OLD123"]);
    }

    #[test]
    fn subject_decoding() {
        let raw = "From: a@b.c\nSubject: =?UTF-8?B?w4dhxJ9yxLE=?=\n  devam\nTo: x@y.z\n\nbody Subject: yok\n";
        assert_eq!(subject_of(raw), "Çağrı devam");
        assert_eq!(decode_header("=?utf-8?Q?Merhaba_d=C3=BCnya?="), "Merhaba dünya");
        assert_eq!(decode_header("=?utf-8?B?TWVy?= =?utf-8?B?aGFiYQ==?="), "Merhaba");
        assert_eq!(decode_header("düz konu =? bozuk"), "düz konu =? bozuk");
        assert_eq!(decode_header("=?utf-8?Q?x=zz?="), "=?utf-8?Q?x=zz?=");
        // Gövdedeki "Subject:" başlık sayılmaz.
        assert_eq!(subject_of("From: a@b.c\n\nSubject: gövde\n"), "");
    }

    #[test]
    fn score_parsing() {
        let raw = "X-Spamd-Result: default: False [4.50 / 15.00];\n\tBAYES_SPAM(5.00)[99%];\nSubject: x\n\n";
        assert_eq!(score_from_headers(raw), Some(4.5));
        assert_eq!(score_from_headers("Subject: x\n\n"), None);
        let history = vec![
            HistoryRow { sender: "A@B.C".into(), subject: "konu".into(), score: 20.0, action: "reject".into() },
            HistoryRow { sender: "a@b.c".into(), subject: "konu".into(), score: 1.0, action: "no action".into() },
        ];
        assert_eq!(score_from_history(&history, "a@b.c", "konu"), Some(20.0));
        assert_eq!(score_from_history(&history, "a@b.c", "baska"), None);
    }

    #[test]
    fn proxy_block_is_idempotent_and_preserves_other_lines() {
        let on = proxy_config(None, true).unwrap();
        assert!(on.contains("quarantine_on_reject = true;"));
        assert_eq!(proxy_config(Some(&on), true).unwrap(), on);
        assert_eq!(proxy_config(Some(&on), false), None);
        let custom = "milter = yes;\ntimeout = 120s;\n";
        let with = proxy_config(Some(custom), true).unwrap();
        assert!(with.starts_with("milter = yes;\ntimeout = 120s;\n\n# PixPanel karantina"));
        assert_eq!(proxy_config(Some(&with), false).unwrap(), custom);
        assert_eq!(proxy_config(Some(&with), true).unwrap(), with);
    }
}
