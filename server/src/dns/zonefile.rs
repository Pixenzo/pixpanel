//! BIND bölge dosyası: ayrıştırma, üretme ve içe aktarma planı.
//!
//! `parse`, `plan` ve `export` saf işlevlerdir (ağ ve disk yok); birim testleri bunlara yazılmıştır.
//! Yalnızca `apply` sağlayıcıya yazar. Dosyadan gelen her kayıt, elle girilen kayıtla aynı
//! doğrulamadan (`normalize`) geçer; dosya içeriği hiçbir yerde kabuğa ya da yapılandırma
//! dosyasına yazılmaz, sağlayıcıya yalnızca doğrulanmış kayıt olarak gider.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::Ipv6Addr;

use serde::Serialize;

use super::{normalize, Record, Zone};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// İçe aktarılabilecek en büyük metin.
pub const MAX_BYTES: usize = 512 * 1024;
/// Bir dosyadan alınabilecek en çok kayıt.
pub const MAX_RECORDS: usize = 5000;
/// Önizlemede ayrıca listelenen atlanmış satır sayısı (fazlası yalnızca sayılır).
const MAX_SKIPPED_LISTED: usize = 1000;
/// Cloudflare'e tek seferde yazılabilecek kayıt sayısı (API hız sınırı yüzünden).
pub const MAX_CLOUDFLARE_ADDS: usize = 500;
/// Bir seferde sağlayıcıya gönderilen küme sayısı (PowerDNS).
const CHUNK: usize = 100;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Skipped {
    pub line: usize,
    pub text: String,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct Parsed {
    /// (kaynak satır numarası, doğrulanmış kayıt)
    pub records: Vec<(usize, Record)>,
    pub skipped: Vec<Skipped>,
    /// Atlanan toplam sayı (listelenenden fazla olabilir).
    pub skipped_total: usize,
    /// Kullanıcıya bildirilen, kayıt atlamayan ayarlamalar (ör. TTL aralığa çekildi).
    pub notes: Vec<String>,
}

// ---- Sözcüklere ayırma ----

struct Token {
    bytes: Vec<u8>,
    quoted: bool,
    /// Kaçış dizisi (`\.`, `\032`) içeriyor mu? Adlarda desteklenmez.
    escaped: bool,
}

impl Token {
    fn empty() -> Self {
        Self { bytes: Vec::new(), quoted: false, escaped: false }
    }

    fn text(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }
}

struct Entry {
    line: usize,
    /// Satır boşlukla başlıyorsa kayıt adı bir önceki kaydın adıdır.
    indented: bool,
    tokens: Vec<Token>,
    error: Option<String>,
}

impl Entry {
    fn new(line: usize) -> Self {
        Self { line, indented: false, tokens: Vec::new(), error: None }
    }
}

fn flush(token: &mut Option<Token>, entry: &mut Entry) {
    if let Some(done) = token.take() {
        entry.tokens.push(done);
    }
}

/// `\` ile başlayan kaçışı çözer (`\"`, `\\`, `\DDD`); `*i` kaçışın üzerindeki konumdur.
fn push_escape(bytes: &[u8], i: &mut usize, out: &mut Vec<u8>) {
    let rest = &bytes[*i + 1..];
    if rest.len() >= 3 && rest[..3].iter().all(u8::is_ascii_digit) {
        let value = u32::from(rest[0] - b'0') * 100 + u32::from(rest[1] - b'0') * 10 + u32::from(rest[2] - b'0');
        if value <= 255 {
            out.push(value as u8);
            *i += 4;
        } else {
            out.push(b'\\');
            *i += 1;
        }
        return;
    }
    match rest.first() {
        Some(&c) if c != b'\n' => {
            out.push(c);
            *i += 2;
        }
        _ => {
            out.push(b'\\');
            *i += 1;
        }
    }
}

/// Metni mantıksal kayıtlara böler: yorumlar (`;`) atılır, tırnaklı metinler tek sözcük olur,
/// parantezli çok satırlı kayıtlar tek kayıt sayılır.
fn tokenize(text: &str) -> Vec<Entry> {
    let bytes = text.as_bytes();
    let mut entries = Vec::new();
    let mut entry = Entry::new(1);
    let mut token: Option<Token> = None;
    let mut depth = 0usize;
    let mut line = 1usize;
    let mut at_line_start = true;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if at_line_start && depth == 0 {
            entry.line = line;
            entry.indented = b == b' ' || b == b'\t';
        }
        at_line_start = false;
        match b {
            b'"' => {
                flush(&mut token, &mut entry);
                i += 1;
                let mut value = Vec::new();
                let (mut escaped, mut closed) = (false, false);
                while i < bytes.len() {
                    match bytes[i] {
                        b'"' => {
                            closed = true;
                            i += 1;
                            break;
                        }
                        // Tırnak satır sonunda kapanmamışsa kayıt hatalıdır; sonraki satırdan devam edilir.
                        b'\n' => break,
                        b'\\' => {
                            escaped = true;
                            push_escape(bytes, &mut i, &mut value);
                        }
                        other => {
                            value.push(other);
                            i += 1;
                        }
                    }
                }
                if !closed {
                    entry.error.get_or_insert_with(|| "Tırnak kapanmamış".to_string());
                }
                entry.tokens.push(Token { bytes: value, quoted: true, escaped });
            }
            b';' => {
                flush(&mut token, &mut entry);
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'(' => {
                flush(&mut token, &mut entry);
                depth += 1;
                i += 1;
            }
            b')' => {
                flush(&mut token, &mut entry);
                if depth == 0 {
                    entry.error.get_or_insert_with(|| "Fazladan kapanış parantezi".to_string());
                } else {
                    depth -= 1;
                }
                i += 1;
            }
            b'\n' => {
                flush(&mut token, &mut entry);
                i += 1;
                line += 1;
                if depth == 0 {
                    let done = std::mem::replace(&mut entry, Entry::new(line));
                    if !done.tokens.is_empty() || done.error.is_some() {
                        entries.push(done);
                    }
                    at_line_start = true;
                }
            }
            b' ' | b'\t' | b'\r' => {
                flush(&mut token, &mut entry);
                i += 1;
            }
            b'\\' => {
                let current = token.get_or_insert_with(Token::empty);
                current.escaped = true;
                push_escape(bytes, &mut i, &mut current.bytes);
            }
            other => {
                token.get_or_insert_with(Token::empty).bytes.push(other);
                i += 1;
            }
        }
    }
    flush(&mut token, &mut entry);
    if depth > 0 {
        entry.error.get_or_insert_with(|| "Parantez kapanmamış".to_string());
    }
    if !entry.tokens.is_empty() || entry.error.is_some() {
        entries.push(entry);
    }
    entries
}

// ---- Alan ayrıştırıcıları ----

/// `3600`, `1h`, `1h30m`, `2d` gibi BIND süre yazımı.
fn parse_ttl(text: &str) -> Option<u32> {
    if !text.as_bytes().first()?.is_ascii_digit() {
        return None;
    }
    let (mut total, mut number): (u64, Option<u64>) = (0, None);
    for c in text.chars() {
        if let Some(digit) = c.to_digit(10) {
            number = Some(number.unwrap_or(0).saturating_mul(10).saturating_add(u64::from(digit)));
        } else {
            let unit = match c.to_ascii_lowercase() {
                's' => 1,
                'm' => 60,
                'h' => 3600,
                'd' => 86400,
                'w' => 604800,
                _ => return None,
            };
            total = total.saturating_add(number.take()?.saturating_mul(unit));
        }
    }
    if let Some(rest) = number {
        total = total.saturating_add(rest);
    }
    Some(total.min(u64::from(u32::MAX)) as u32)
}

/// Ad sözcüğünü, geçerli `$ORIGIN`e göre sonda noktasız küçük harfli tam ada çevirir.
fn absolute(token: &Token, origin: &str) -> Result<String, String> {
    if token.quoted || token.escaped {
        return Err("Ad tırnak ya da kaçış dizisi içeriyor".into());
    }
    let text = token.text().ok_or("Ad geçerli bir metin değil")?;
    let name = if text == "@" {
        origin.to_string()
    } else if let Some(full) = text.strip_suffix('.') {
        full.to_string()
    } else {
        format!("{text}.{origin}")
    };
    if name.is_empty() {
        return Err("Boş ad".into());
    }
    Ok(name.to_ascii_lowercase())
}

fn plain(token: &Token) -> Result<String, String> {
    token.text().map(str::to_string).ok_or_else(|| "Değer geçerli bir metin değil".to_string())
}

fn number<T: std::str::FromStr>(token: &Token, what: &str) -> Result<T, String> {
    token.text().and_then(|text| text.parse().ok()).ok_or_else(|| format!("{what} geçersiz"))
}

/// Kayıt türüne göre (içerik, öncelik) üretir. İçerik, panelin kendi biçimidir (bkz. `Record`).
fn build(kind: &str, rdata: &[Token], origin: &str) -> Result<(String, Option<u32>), String> {
    let wrong = || format!("{kind} kaydının alan sayısı beklenenden farklı");
    match kind {
        "A" | "AAAA" => {
            let [value] = rdata else { return Err(wrong()) };
            Ok((plain(value)?, None))
        }
        "CNAME" | "NS" => {
            let [target] = rdata else { return Err(wrong()) };
            Ok((absolute(target, origin)?, None))
        }
        "MX" => {
            let [preference, host] = rdata else { return Err(wrong()) };
            // Boş MX (`0 .`) olduğu gibi kalır.
            let target = if !host.quoted && host.text() == Some(".") { ".".to_string() } else { absolute(host, origin)? };
            Ok((target, Some(u32::from(number::<u16>(preference, "MX önceliği")?))))
        }
        "TXT" => {
            if rdata.is_empty() {
                return Err(wrong());
            }
            // Birden çok metin parçası BIND'de ek boşluk olmadan birleşir.
            let joined: Vec<u8> = rdata.iter().flat_map(|part| part.bytes.iter().copied()).collect();
            String::from_utf8(joined).map(|text| (text, None)).map_err(|_| "TXT değeri geçerli bir metin değil".to_string())
        }
        "SRV" => {
            let [priority, weight, port, target] = rdata else { return Err(wrong()) };
            let (priority, weight, port) = (number::<u16>(priority, "SRV önceliği")?, number::<u16>(weight, "SRV ağırlığı")?, number::<u16>(port, "SRV portu")?);
            Ok((format!("{weight} {port} {}", absolute(target, origin)?), Some(u32::from(priority))))
        }
        "CAA" => {
            let [flags, tag, value] = rdata else { return Err(wrong()) };
            let flags = number::<u8>(flags, "CAA bayrağı")?;
            Ok((format!("{flags} {} \"{}\"", plain(tag)?.to_ascii_lowercase(), plain(value)?), None))
        }
        _ => Err(format!("Desteklenmeyen kayıt türü: {kind}")),
    }
}

fn excerpt(lines: &[&str], line: usize) -> String {
    let raw = lines.get(line.saturating_sub(1)).copied().unwrap_or_default().trim();
    let mut shown: String = raw.chars().filter(|c| !c.is_control()).take(120).collect();
    if raw.chars().count() > 120 {
        shown.push('…');
    }
    shown
}

fn in_zone(name: &str, zone: &str) -> bool {
    name == zone || name.ends_with(&format!(".{zone}"))
}

/// BIND bölge dosyası metnini ayrıştırır. Boyut ya da kayıt sayısı sınırı aşılırsa hata döner;
/// tek tek bozuk ya da desteklenmeyen satırlar `skipped` içinde listelenir.
pub fn parse(zone: &str, text: &str) -> AppResult<Parsed> {
    if text.len() > MAX_BYTES {
        return Err(AppError::bad(format!("Bölge dosyası en çok {} KB olabilir", MAX_BYTES / 1024)));
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text.lines().collect();
    let mut parsed = Parsed::default();
    let mut origin = zone.to_string();
    let (mut dollar_ttl, mut last_ttl): (Option<u32>, Option<u32>) = (None, None);
    let mut last_owner: Option<String> = None;
    let mut seen: HashSet<(String, String, String, u32)> = HashSet::new();
    let mut clamped = 0usize;

    for entry in tokenize(text) {
        let skip = |parsed: &mut Parsed, reason: String| {
            parsed.skipped_total += 1;
            if parsed.skipped.len() < MAX_SKIPPED_LISTED {
                parsed.skipped.push(Skipped { line: entry.line, text: excerpt(&lines, entry.line), reason });
            }
        };
        if let Some(error) = &entry.error {
            skip(&mut parsed, error.clone());
            continue;
        }
        let tokens = &entry.tokens;
        // Yönergeler
        if !entry.indented && !tokens[0].quoted && tokens[0].bytes.first() == Some(&b'$') {
            let directive = tokens[0].text().unwrap_or_default().to_ascii_uppercase();
            match (directive.as_str(), tokens.get(1)) {
                ("$ORIGIN", Some(value)) if tokens.len() == 2 => match absolute(value, &origin) {
                    Ok(next) => origin = next,
                    Err(reason) => skip(&mut parsed, format!("$ORIGIN geçersiz: {reason}")),
                },
                ("$TTL", Some(value)) if tokens.len() == 2 => match value.text().and_then(parse_ttl) {
                    Some(ttl) => dollar_ttl = Some(ttl),
                    None => skip(&mut parsed, "$TTL değeri geçersiz".into()),
                },
                ("$ORIGIN" | "$TTL", _) => skip(&mut parsed, format!("{directive} yönergesi geçersiz")),
                _ => skip(&mut parsed, format!("{directive} yönergesi desteklenmiyor")),
            }
            continue;
        }

        let mut index = 0;
        let owner = if entry.indented {
            match &last_owner {
                Some(owner) => owner.clone(),
                None => {
                    skip(&mut parsed, "Kayıt adı yok (önceki bir kayıt bulunmuyor)".into());
                    continue;
                }
            }
        } else {
            index = 1;
            match absolute(&tokens[0], &origin) {
                Ok(owner) => {
                    last_owner = Some(owner.clone());
                    owner
                }
                Err(reason) => {
                    skip(&mut parsed, reason);
                    continue;
                }
            }
        };

        // İsteğe bağlı TTL ve sınıf (sırası serbest).
        let (mut explicit_ttl, mut class_seen, mut bad_class) = (None, false, false);
        while let Some(token) = tokens.get(index) {
            let Some(word) = token.text().filter(|_| !token.quoted) else { break };
            let ttl = if explicit_ttl.is_none() { parse_ttl(word) } else { None };
            if ttl.is_some() {
                explicit_ttl = ttl;
            } else if !class_seen && matches!(word.to_ascii_uppercase().as_str(), "IN" | "CH" | "HS" | "CS") {
                class_seen = true;
                bad_class = !word.eq_ignore_ascii_case("IN");
            } else {
                break;
            }
            index += 1;
        }
        if bad_class {
            skip(&mut parsed, "Yalnızca IN sınıfı desteklenir".into());
            continue;
        }
        let Some(kind_token) = tokens.get(index) else {
            skip(&mut parsed, "Kayıt türü yok".into());
            continue;
        };
        let kind = kind_token.text().unwrap_or_default().to_ascii_uppercase();
        if kind == "SOA" {
            skip(&mut parsed, "SOA kaydı sunucu tarafından yönetilir".into());
            continue;
        }
        if !super::KINDS.contains(&kind.as_str()) {
            skip(&mut parsed, format!("Desteklenmeyen kayıt türü: {}", kind.chars().take(16).collect::<String>()));
            continue;
        }
        if !in_zone(&owner, zone) {
            skip(&mut parsed, format!("Ad {zone} bölgesinin dışında"));
            continue;
        }
        let (content, priority) = match build(&kind, &tokens[index + 1..], &origin) {
            Ok(built) => built,
            Err(reason) => {
                skip(&mut parsed, reason);
                continue;
            }
        };
        if let Some(ttl) = explicit_ttl {
            last_ttl = Some(ttl);
        }
        let wanted = explicit_ttl.or(dollar_ttl).or(last_ttl).unwrap_or(3600);
        let ttl = wanted.clamp(60, 86400);
        let mut record = Record::new(&kind, &owner, &content);
        record.ttl = ttl;
        record.priority = priority;
        let record = match normalize(zone, &record) {
            Ok(record) => record,
            Err(err) => {
                skip(&mut parsed, err.to_string());
                continue;
            }
        };
        if !seen.insert(canon(&record)) {
            skip(&mut parsed, "Dosyada aynı kayıt birden fazla kez var".into());
            continue;
        }
        if ttl != wanted {
            clamped += 1;
        }
        if parsed.records.len() >= MAX_RECORDS {
            return Err(AppError::bad(format!("Bir seferde en çok {MAX_RECORDS} kayıt aktarılabilir")));
        }
        parsed.records.push((entry.line, record));
    }
    if clamped > 0 {
        parsed.notes.push(format!("{clamped} kaydın TTL değeri 60 ile 86400 saniye aralığına çekildi"));
    }
    Ok(parsed)
}

// ---- Karşılaştırma ve plan ----

pub(super) type Key = (String, String, String, u32);

/// Kaydın karşılaştırma anahtarı: sağlayıcıların yazım farkları (IPv6 biçimi, büyük/küçük harf) yok sayılır.
pub(super) fn canon(record: &Record) -> Key {
    let content = match record.kind.as_str() {
        "AAAA" => record.content.trim().parse::<Ipv6Addr>().map(|ip| ip.to_string()).unwrap_or_else(|_| record.content.trim().to_string()),
        "CNAME" | "NS" | "MX" => record.content.trim().trim_end_matches('.').to_ascii_lowercase(),
        "SRV" => record.content.split_whitespace().collect::<Vec<_>>().join(" ").trim_end_matches('.').to_ascii_lowercase(),
        "CAA" => record.content.split_whitespace().collect::<Vec<_>>().join(" "),
        _ => record.content.trim().to_string(),
    };
    let priority = if matches!(record.kind.as_str(), "MX" | "SRV") { record.priority.unwrap_or(0) } else { 0 };
    (record.kind.clone(), record.name.to_ascii_lowercase(), content, priority)
}

#[derive(Debug, Default, Serialize)]
pub struct Plan {
    /// Eklenecek kayıtlar.
    pub add: Vec<Record>,
    /// Dosyadaki, bölgede zaten bulunan kayıtlar.
    pub existing: Vec<Record>,
    /// Değiştirme kipinde silinecek var olan kayıtlar.
    pub remove: Vec<Record>,
    pub skipped: Vec<Skipped>,
}

fn describe(record: &Record) -> String {
    let text: String = format!("{} {} {}", record.kind, record.name, record.content).chars().filter(|c| !c.is_control()).take(120).collect();
    text
}

/// Dosyadaki kayıtlarla bölgedeki kayıtları karşılaştırır.
///
/// - Bölgede aynısı olan kayıt "zaten var" sayılır.
/// - Kök ad sunucusu (NS) kayıtları panelin yönettiği düzenektir; bölgede yoksa eklenmez
///   (başka bir sağlayıcının ad sunucularını taşımak bölgeyi bozardı).
/// - `replace` açıksa, dosyada bulunan her (ad, tür) kümesindeki, dosyada olmayan var olan kayıtlar silinir.
/// - Aynı adda CNAME ile başka türde kayıt bir arada bulunamayacağından çakışanlar atlanır.
pub fn plan(zone: &str, incoming: &[(usize, Record)], current: &[Record], replace: bool) -> Plan {
    let mut plan = Plan::default();
    let current_keys: HashSet<Key> = current.iter().map(canon).collect();
    let is_apex_ns = |record: &Record| record.kind == "NS" && record.name == zone;

    let mut file_keys: HashSet<Key> = HashSet::new();
    let mut touched: HashSet<(String, String)> = HashSet::new();
    let mut candidates: Vec<(usize, &Record)> = Vec::new();
    for (line, record) in incoming {
        let key = canon(record);
        if current_keys.contains(&key) {
            plan.existing.push(record.clone());
        } else if is_apex_ns(record) {
            plan.skipped.push(Skipped { line: *line, text: describe(record), reason: "Kök ad sunucusu kayıtları panel tarafından yönetilir; eklenmedi".into() });
            continue;
        } else {
            candidates.push((*line, record));
        }
        file_keys.insert(key);
        if !is_apex_ns(record) {
            touched.insert((record.name.clone(), record.kind.clone()));
        }
    }

    if replace {
        for record in current {
            if !is_apex_ns(record) && touched.contains(&(record.name.clone(), record.kind.clone())) && !file_keys.contains(&canon(record)) {
                plan.remove.push(record.clone());
            }
        }
    }

    // Silme sonrası her adda kalan türler; CNAME çakışması bunlara göre denetlenir.
    let removed: HashSet<Key> = plan.remove.iter().map(canon).collect();
    let mut kinds_at: HashMap<String, HashSet<String>> = HashMap::new();
    for record in current.iter().filter(|record| !removed.contains(&canon(record))) {
        kinds_at.entry(record.name.clone()).or_default().insert(record.kind.clone());
    }
    for (line, record) in candidates {
        let kinds = kinds_at.entry(record.name.clone()).or_default();
        let conflict = if record.kind == "CNAME" { !kinds.is_empty() } else { kinds.contains("CNAME") };
        if conflict {
            plan.skipped.push(Skipped { line, text: describe(record), reason: "Aynı adda CNAME ile başka türde kayıt bir arada bulunamaz".into() });
            continue;
        }
        kinds.insert(record.kind.clone());
        plan.add.push(record.clone());
    }
    plan
}

// ---- Dışa aktarma ----

/// TXT değerini 200 baytlık tırnaklı parçalara böler (BIND'de 255 baytlık sınır vardır).
fn quote_txt(content: &str) -> String {
    let mut parts = Vec::new();
    let mut start = 0;
    while start < content.len() {
        let mut end = (start + 200).min(content.len());
        while !content.is_char_boundary(end) {
            end -= 1;
        }
        parts.push(format!("\"{}\"", content[start..end].replace('\\', "\\\\").replace('"', "\\\"")));
        start = end;
    }
    if parts.is_empty() { "\"\"".to_string() } else { parts.join(" ") }
}

/// Kayıtlardan BIND bölge dosyası üretir. SOA satırı gösterim içindir (içe aktarırken atlanır).
pub fn export(zone: &str, records: &[Record], serial: u32) -> String {
    let relative = |name: &str| if name == zone { "@".to_string() } else { name.strip_suffix(&format!(".{zone}")).unwrap_or(name).to_string() };
    let order = |kind: &str| super::KINDS.iter().position(|item| *item == kind).unwrap_or(usize::MAX);
    let mut sorted: Vec<&Record> = records.iter().filter(|record| super::KINDS.contains(&record.kind.as_str())).collect();
    sorted.sort_by(|a, b| (a.name != zone, &a.name, order(&a.kind), &a.content).cmp(&(b.name != zone, &b.name, order(&b.kind), &b.content)));

    let primary = sorted.iter().find(|record| record.kind == "NS" && record.name == zone).map(|record| format!("{}.", record.content)).unwrap_or_else(|| format!("ns1.{zone}."));
    let mut out = format!("; {zone} bölge dosyası (PixPanel)\n$ORIGIN {zone}.\n$TTL 3600\n");
    out.push_str(&format!("@\t3600\tIN\tSOA\t{primary} hostmaster.{zone}. {serial} 10800 3600 604800 3600\n"));
    for record in sorted {
        let ttl = if record.ttl == 1 { 300 } else { record.ttl };
        let data = match record.kind.as_str() {
            "CNAME" | "NS" => format!("{}.", record.content),
            "MX" if record.content == "." => format!("{} .", record.priority.unwrap_or(0)),
            "MX" => format!("{} {}.", record.priority.unwrap_or(10), record.content),
            "SRV" => {
                let mut parts = record.content.split_whitespace();
                let (weight, port, target) = (parts.next().unwrap_or("0"), parts.next().unwrap_or("0"), parts.next().unwrap_or("."));
                format!("{} {weight} {port} {target}.", record.priority.unwrap_or(0))
            }
            "TXT" => quote_txt(&record.content),
            _ => record.content.clone(),
        };
        out.push_str(&format!("{}\t{ttl}\tIN\t{}\t{data}\n", relative(&record.name), record.kind));
    }
    out
}

// ---- Uygulama ----

#[derive(Debug, Default, Serialize)]
pub struct Outcome {
    pub added: usize,
    pub removed: usize,
    pub failed: Vec<String>,
}

fn fail(outcome: &mut Outcome, message: String) {
    if outcome.failed.len() < 50 {
        outcome.failed.push(message);
    }
}

/// Planı sağlayıcıya yazar. Yerel bölgede kayıt kümeleri toplu yazılır; bir toplu istek reddedilirse
/// küme küme yeniden denenir ve yalnızca gerçekten yazılamayanlar `failed` içinde bildirilir.
pub async fn apply(state: &AppState, zone: &Zone, plan: &Plan, current: &[Record]) -> Outcome {
    let mut outcome = Outcome::default();
    if plan.add.is_empty() && plan.remove.is_empty() {
        return outcome;
    }
    if zone.provider == "cloudflare" {
        let client = match super::cloudflare_client(state, zone.user_id) {
            Ok(client) => client,
            Err(err) => {
                fail(&mut outcome, err.to_string());
                return outcome;
            }
        };
        for record in &plan.remove {
            match client.delete(&zone.cf_zone_id, &record.id).await {
                Ok(()) => outcome.removed += 1,
                Err(err) => fail(&mut outcome, format!("{}: silinemedi ({err})", describe(record))),
            }
        }
        for record in &plan.add {
            match client.create(&zone.cf_zone_id, record).await {
                Ok(()) => outcome.added += 1,
                Err(err) => fail(&mut outcome, format!("{}: eklenemedi ({err})", describe(record))),
            }
        }
        return outcome;
    }

    // Yerel bölge: dokunulan her (ad, tür) kümesinin son hâli hesaplanır.
    let removed: HashSet<Key> = plan.remove.iter().map(canon).collect();
    struct Set {
        records: Vec<Record>,
        added: usize,
        removed: usize,
    }
    let mut sets: BTreeMap<(String, String), Set> = BTreeMap::new();
    for record in plan.add.iter().chain(plan.remove.iter()) {
        sets.entry((record.name.clone(), record.kind.clone())).or_insert_with(|| Set { records: Vec::new(), added: 0, removed: 0 });
    }
    for record in current {
        if let Some(set) = sets.get_mut(&(record.name.clone(), record.kind.clone())) {
            if !removed.contains(&canon(record)) {
                set.records.push(record.clone());
            }
        }
    }
    for record in &plan.remove {
        if let Some(set) = sets.get_mut(&(record.name.clone(), record.kind.clone())) {
            set.removed += 1;
        }
    }
    for record in &plan.add {
        if let Some(set) = sets.get_mut(&(record.name.clone(), record.kind.clone())) {
            set.records.push(record.clone());
            set.added += 1;
        }
    }

    let entries: Vec<((String, String), Set)> = sets.into_iter().collect();
    for chunk in entries.chunks(CHUNK) {
        let batch: Vec<(String, String, Vec<Record>)> = chunk.iter().map(|((name, kind), set)| (name.clone(), kind.clone(), set.records.clone())).collect();
        if super::local::write_sets(&zone.domain, &batch).await.is_ok() {
            outcome.added += chunk.iter().map(|(_, set)| set.added).sum::<usize>();
            outcome.removed += chunk.iter().map(|(_, set)| set.removed).sum::<usize>();
            continue;
        }
        for (key, set) in chunk {
            match super::local::write_sets(&zone.domain, &[(key.0.clone(), key.1.clone(), set.records.clone())]).await {
                Ok(()) => {
                    outcome.added += set.added;
                    outcome.removed += set.removed;
                }
                Err(err) => fail(&mut outcome, format!("{} {}: yazılamadı ({err})", key.1, key.0)),
            }
        }
    }
    if outcome.added + outcome.removed > 0 {
        super::announce(state, zone).await;
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
; örnek bölge
$ORIGIN ornek.com.
$TTL 1h
@       IN  SOA ns1.ornek.com. hostmaster.ornek.com. (
                2026111201 ; serial
                3h 1h 1w 1h )
        IN  NS  ns1.ornek.com.
        IN  NS  ns2
@       300 IN  A     203.0.113.10
www         IN  CNAME @
mail        IN  A     203.0.113.11
@           IN  MX    10 mail
@           IN  MX    20 mail2.example.net.
@           IN  TXT   "v=spf1 mx" " ~all"
_dmarc      IN  TXT   "v=DMARC1; p=none; rua=mailto:a@ornek.com" ; yorum
_sip._tcp   IN  SRV   5 10 5060 sip
@           IN  CAA   0 issue "letsencrypt.org"
ipv6        IN  AAAA  2001:DB8:0:0:0:0:0:1
dış         IN  A     203.0.113.99
x.baska.org. IN A     203.0.113.5
@           IN  SPF   "v=spf1"
@           IN  LOC   1 2 3
"#;

    fn kinds(parsed: &Parsed) -> Vec<String> {
        parsed.records.iter().map(|(_, r)| format!("{} {} {} {:?} {}", r.kind, r.name, r.content, r.priority, r.ttl)).collect()
    }

    #[test]
    fn null_mx_survives_parse_and_export() {
        let parsed = parse("ornek.com", "@ 3600 IN MX 0 .\n@ IN MX 10 mail\n").unwrap();
        let list = kinds(&parsed);
        assert!(list.contains(&"MX ornek.com . Some(0) 3600".to_string()), "{list:?}");
        assert!(list.contains(&"MX ornek.com mail.ornek.com Some(10) 3600".to_string()), "{list:?}");
        let records: Vec<Record> = parsed.records.into_iter().map(|(_, record)| record).collect();
        let text = export("ornek.com", &records, 1);
        assert!(text.contains("MX\t0 .\n"), "{text}");
        assert!(!text.contains(". .") && !text.contains(".."), "{text}");
        assert_eq!(parse("ornek.com", &text).unwrap().records.len(), 2);
    }

    #[test]
    fn parses_realistic_zone() {
        let parsed = parse("ornek.com", SAMPLE).unwrap();
        let list = kinds(&parsed);
        assert!(list.contains(&"NS ornek.com ns1.ornek.com None 3600".to_string()), "{list:?}");
        assert!(list.contains(&"NS ornek.com ns2.ornek.com None 3600".to_string()), "{list:?}");
        assert!(list.contains(&"A ornek.com 203.0.113.10 None 300".to_string()), "{list:?}");
        assert!(list.contains(&"CNAME www.ornek.com ornek.com None 3600".to_string()), "{list:?}");
        assert!(list.contains(&"MX ornek.com mail.ornek.com Some(10) 3600".to_string()), "{list:?}");
        assert!(list.contains(&"MX ornek.com mail2.example.net Some(20) 3600".to_string()), "{list:?}");
        assert!(list.contains(&"TXT ornek.com v=spf1 mx ~all None 3600".to_string()), "{list:?}");
        assert!(list.iter().any(|item| item.starts_with("TXT _dmarc.ornek.com v=DMARC1; p=none; rua=mailto:a@ornek.com")), "{list:?}");
        assert!(list.contains(&"SRV _sip._tcp.ornek.com 10 5060 sip.ornek.com Some(5) 3600".to_string()), "{list:?}");
        assert!(list.contains(&"CAA ornek.com 0 issue \"letsencrypt.org\" None 3600".to_string()), "{list:?}");
        assert!(list.contains(&"AAAA ipv6.ornek.com 2001:db8::1 None 3600".to_string()), "{list:?}");
        // SOA, bölge dışı ad, SPF ve LOC atlanır; Türkçe harfli ad geçersizdir.
        let reasons: Vec<&str> = parsed.skipped.iter().map(|s| s.reason.as_str()).collect();
        assert!(reasons.iter().any(|r| r.contains("SOA")), "{reasons:?}");
        assert!(reasons.iter().any(|r| r.contains("dışında")), "{reasons:?}");
        assert!(reasons.iter().any(|r| r.contains("SPF")), "{reasons:?}");
        assert!(reasons.iter().any(|r| r.contains("LOC")), "{reasons:?}");
        assert!(parsed.skipped.iter().any(|s| s.text.starts_with("dış")), "{:?}", parsed.skipped);
        assert_eq!(parsed.records.len(), 12, "{list:?}");
    }

    #[test]
    fn origin_directive_changes_relative_names() {
        let parsed = parse("ornek.com", "$ORIGIN alt.ornek.com.\nwww IN A 1.2.3.4\n@ IN A 1.2.3.5\n").unwrap();
        let list = kinds(&parsed);
        assert_eq!(list, vec!["A www.alt.ornek.com 1.2.3.4 None 3600", "A alt.ornek.com 1.2.3.5 None 3600"]);
    }

    #[test]
    fn ttl_units_and_inheritance() {
        assert_eq!(parse_ttl("1h30m"), Some(5400));
        assert_eq!(parse_ttl("2d"), Some(172800));
        assert_eq!(parse_ttl("90"), Some(90));
        assert_eq!(parse_ttl("h"), None);
        assert_eq!(parse_ttl("1x"), None);
        let parsed = parse("ornek.com", "a 600 IN A 1.1.1.1\nb IN A 1.1.1.2\n").unwrap();
        assert_eq!(parsed.records[1].1.ttl, 600);
        // Aralık dışı süreler çekilir ve bildirilir.
        let parsed = parse("ornek.com", "a 604800 IN A 1.1.1.1\nb 0 IN A 1.1.1.2\n").unwrap();
        assert_eq!((parsed.records[0].1.ttl, parsed.records[1].1.ttl), (86400, 60));
        assert_eq!(parsed.notes.len(), 1);
    }

    #[test]
    fn quoted_txt_with_escapes_and_semicolons() {
        let parsed = parse("ornek.com", "t IN TXT \"a;b \\\"q\\\" \\\\ \\065\"\n").unwrap();
        assert_eq!(parsed.records[0].1.content, "a;b \"q\" \\ A");
        let parsed = parse("ornek.com", "t IN TXT unquoted ; comment\n").unwrap();
        assert_eq!(parsed.records[0].1.content, "unquoted");
    }

    #[test]
    fn crlf_and_bom_are_accepted() {
        let parsed = parse("ornek.com", "\u{feff}a IN A 1.1.1.1\r\nb IN A 1.1.1.2\r\n").unwrap();
        assert_eq!(parsed.records.len(), 2);
    }

    #[test]
    fn broken_lines_are_skipped_not_fatal() {
        let text = "ok IN A 1.1.1.1\nbad IN A 999.1.1.1\nq IN TXT \"kapanmamis\nok2 IN A 1.1.1.2\nparen IN SOA a b ( 1 2\n";
        let parsed = parse("ornek.com", text).unwrap();
        let names: Vec<&str> = parsed.records.iter().map(|(_, r)| r.name.as_str()).collect();
        assert_eq!(names, vec!["ok.ornek.com", "ok2.ornek.com"]);
        assert_eq!(parsed.skipped.len(), 3, "{:?}", parsed.skipped);
        assert_eq!(parsed.skipped[0].line, 2);
    }

    #[test]
    fn injection_attempts_are_rejected_or_harmless() {
        // Ad ve değerlerdeki denetim karakterleri, kaçış dizileri ve ek satırlar kayıt olamaz.
        let text = "a\\010b IN A 1.1.1.1\nx IN A 1.1.1.1\\n2.2.2.2\nc IN CNAME hedef.ornek.net.\\032x\n\"q\" IN A 1.1.1.1\nt IN TXT \"a\\010b\"\nm IN MX 10 \"mail.ornek.com\"\n";
        let parsed = parse("ornek.com", text).unwrap();
        assert!(parsed.records.is_empty(), "{:?}", parsed.records);
        // TXT içindeki tırnak ve ters eğik çizgi yalnızca değer olarak kalır.
        let parsed = parse("ornek.com", "t IN TXT \"x\\\" ; IN A 6.6.6.6\"\n").unwrap();
        assert_eq!(parsed.records.len(), 1);
        assert_eq!(parsed.records[0].1.content, "x\" ; IN A 6.6.6.6");
    }

    #[test]
    fn size_and_count_limits() {
        assert!(parse("ornek.com", &"a".repeat(MAX_BYTES + 1)).is_err());
        let many: String = (0..MAX_RECORDS + 1).map(|i| format!("h{i} IN A 10.0.{}.{}\n", i / 250, i % 250)).collect();
        assert!(many.len() < MAX_BYTES);
        assert!(parse("ornek.com", &many).is_err());
        let ok: String = (0..MAX_RECORDS).map(|i| format!("h{i} IN A 10.0.{}.{}\n", i / 250, i % 250)).collect();
        assert_eq!(parse("ornek.com", &ok).unwrap().records.len(), MAX_RECORDS);
    }

    #[test]
    fn duplicate_lines_are_listed_once() {
        let parsed = parse("ornek.com", "a IN A 1.1.1.1\na IN A 1.1.1.1\nb IN AAAA 2001:db8::1\nb IN AAAA 2001:0DB8:0:0:0:0:0:1\n").unwrap();
        assert_eq!(parsed.records.len(), 2);
        assert_eq!(parsed.skipped.len(), 2);
    }

    #[test]
    fn export_then_parse_roundtrip() {
        let mut mx = Record::new("MX", "ornek.com", "mail.ornek.com");
        mx.priority = Some(10);
        let mut srv = Record::new("SRV", "_sip._tcp.ornek.com", "10 5060 sip.ornek.com");
        srv.priority = Some(5);
        let long = format!("v=DKIM1; k=rsa; p={}", "Ab\"\\".repeat(150));
        let records = vec![
            Record::new("A", "ornek.com", "203.0.113.10"),
            Record::new("NS", "ornek.com", "ns1.ornek.com"),
            Record::new("CNAME", "www.ornek.com", "ornek.com"),
            mx,
            srv,
            Record::new("TXT", "ornek.com", "v=spf1 mx ~all"),
            Record::new("TXT", "pix._domainkey.ornek.com", &long),
            Record::new("CAA", "ornek.com", "0 issue \"letsencrypt.org\""),
            Record::new("AAAA", "v6.ornek.com", "2001:db8::1"),
        ];
        let text = export("ornek.com", &records, 2026111201);
        let parsed = parse("ornek.com", &text).unwrap();
        assert_eq!(parsed.skipped.len(), 1, "yalnızca SOA atlanmalı: {:?}", parsed.skipped);
        assert!(parsed.skipped[0].reason.contains("SOA"));
        let result = plan("ornek.com", &parsed.records, &records, false);
        assert!(result.add.is_empty(), "{:?}", result.add);
        assert_eq!(result.existing.len(), records.len());
        assert!(result.skipped.is_empty());
    }

    fn incoming(zone: &str, text: &str) -> Vec<(usize, Record)> {
        parse(zone, text).unwrap().records
    }

    #[test]
    fn plan_adds_missing_and_keeps_existing() {
        let current = vec![Record::new("A", "ornek.com", "1.1.1.1"), Record::new("NS", "ornek.com", "ns1.ornek.com")];
        let file = incoming("ornek.com", "@ IN A 1.1.1.1\n@ IN A 2.2.2.2\nwww IN A 3.3.3.3\n@ IN NS ns.baska.net.\n@ IN NS NS1.ornek.com.\nsub IN NS ns.baska.net.\n");
        let result = plan("ornek.com", &file, &current, false);
        let added: Vec<String> = result.add.iter().map(|r| format!("{} {} {}", r.kind, r.name, r.content)).collect();
        assert_eq!(added, vec!["A ornek.com 2.2.2.2", "A www.ornek.com 3.3.3.3", "NS sub.ornek.com ns.baska.net"]);
        assert_eq!(result.existing.len(), 2);
        assert!(result.remove.is_empty());
        assert_eq!(result.skipped.len(), 1);
        assert!(result.skipped[0].reason.contains("Kök ad sunucusu"));
    }

    #[test]
    fn replace_mode_removes_only_touched_sets() {
        let current = vec![
            Record::new("A", "ornek.com", "1.1.1.1"),
            Record::new("A", "ornek.com", "9.9.9.9"),
            Record::new("A", "eski.ornek.com", "8.8.8.8"),
            Record::new("TXT", "ornek.com", "kalacak"),
            Record::new("NS", "ornek.com", "ns1.ornek.com"),
        ];
        let file = incoming("ornek.com", "@ IN A 1.1.1.1\n@ IN A 2.2.2.2\n@ IN NS ns.baska.net.\n");
        let result = plan("ornek.com", &file, &current, true);
        assert_eq!(result.add.len(), 1);
        let removed: Vec<&str> = result.remove.iter().map(|r| r.content.as_str()).collect();
        assert_eq!(removed, vec!["9.9.9.9"], "ad sunucusu, TXT ve başka adlar silinmemeli");
    }

    #[test]
    fn cname_conflicts_are_skipped() {
        let current = vec![Record::new("A", "var.ornek.com", "1.1.1.1"), Record::new("CNAME", "takma.ornek.com", "ornek.com")];
        let file = incoming("ornek.com", "var IN CNAME hedef.ornek.net.\ntakma IN A 2.2.2.2\nyeni IN CNAME a.ornek.net.\nyeni IN CNAME b.ornek.net.\nyeni2 IN A 1.1.1.1\nyeni2 IN CNAME c.ornek.net.\n");
        let result = plan("ornek.com", &file, &current, false);
        let added: Vec<String> = result.add.iter().map(|r| format!("{} {}", r.kind, r.name)).collect();
        assert_eq!(added, vec!["CNAME yeni.ornek.com", "A yeni2.ornek.com"]);
        assert_eq!(result.skipped.len(), 4);
    }
}
