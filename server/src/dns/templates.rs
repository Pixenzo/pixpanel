//! Hazır DNS kayıt kümeleri (bölge şablonları): Google Workspace, Microsoft 365, Yandex 360 postası,
//! alan adı doğrulama kayıtları ve "posta kullanılmıyor" koruması.
//!
//! Kayıtlar kodda sabittir; kullanıcıdan yalnızca birkaç alan gelir ve her biri sıkı biçimde
//! doğrulanır (sağlayıcı kimliği, doğrulama değeri, MX adresi). Üretilen her kayıt elle girilen kayıtla
//! aynı doğrulamadan (`normalize`) geçer. `preview` saf işlevdir (ağ ve disk yok); uygulama
//! `zonefile::apply` ile yapılır.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use super::zonefile::{self, canon, Key, Plan};
use super::{normalize, Record};
use crate::error::{AppError, AppResult};

/// Şablonun bir kaydının bölgedeki hangi mevcut kayıtlarla bir arada bulunamayacağı.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Owns {
    /// Başka kayıtla çakışmaz (ör. doğrulama TXT'si; aynı adda birden çok olabilir).
    Nothing,
    /// Aynı ad ve türdeki başka kayıtlar çakışır (ör. kök adın MX kayıtları).
    SameKind,
    /// Aynı adda, içeriği bu önekle başlayan TXT kayıtları çakışır (SPF, DMARC: bölgede tek olmalı).
    TxtPrefix(&'static str),
    /// Aynı adda her türden başka kayıt çakışır (CNAME başka kayıtla bir arada bulunamaz).
    AnyAtName,
}

struct Entry {
    record: Record,
    owns: Owns,
}

#[derive(Debug, Serialize)]
pub struct Choice {
    pub value: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Field {
    pub name: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    pub required: bool,
    /// Boş değilse alan bu seçeneklerden biri olmalıdır.
    pub choices: Vec<Choice>,
}

#[derive(Debug, Serialize)]
pub struct TemplateInfo {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub fields: Vec<Field>,
    /// Var olan hangi kayıtların değiştirilebileceği (arayüzdeki "değiştir" seçeneğinin açıklaması).
    pub replaces: &'static str,
}

const IDS: [&str; 5] = ["google_workspace", "microsoft_365", "yandex_360", "domain_verification", "no_mail"];

/// Şablonların listesi ve her birinin istediği alanlar.
pub fn list() -> Vec<TemplateInfo> {
    vec![
        TemplateInfo {
            id: "google_workspace",
            title: "Google Workspace postası",
            description: "Posta Google Workspace (Gmail) üzerinden alınır: Google'ın MX kayıtları ve SPF kaydı eklenir.",
            fields: vec![],
            replaces: "var olan MX ve SPF kayıtlarını",
        },
        TemplateInfo {
            id: "microsoft_365",
            title: "Microsoft 365 postası",
            description: "Posta Microsoft 365 (Exchange Online) üzerinden alınır: MX, SPF ve autodiscover kayıtları eklenir.",
            fields: vec![Field {
                name: "mx_host",
                label: "MX adresi (isteğe bağlı)",
                hint: "Boş bırakırsanız alan adından türetilir (ornek-com.mail.protection.outlook.com). Microsoft 365 yönetim merkezi farklı bir adres gösteriyorsa onu yazın.",
                required: false,
                choices: vec![],
            }],
            replaces: "var olan MX, SPF ve autodiscover kayıtlarını",
        },
        TemplateInfo {
            id: "yandex_360",
            title: "Yandex 360 postası",
            description: "Posta Yandex 360 üzerinden alınır: Yandex'in MX kaydı ve SPF kaydı eklenir.",
            fields: vec![],
            replaces: "var olan MX ve SPF kayıtlarını",
        },
        TemplateInfo {
            id: "domain_verification",
            title: "Alan adı doğrulama",
            description: "Google, Microsoft ya da Facebook'un size verdiği doğrulama değerini TXT kaydı olarak ekler.",
            fields: vec![
                Field {
                    name: "provider",
                    label: "Hizmet",
                    hint: "",
                    required: true,
                    choices: vec![
                        Choice { value: "google", label: "Google (Search Console, Workspace)" },
                        Choice { value: "microsoft", label: "Microsoft 365" },
                        Choice { value: "facebook", label: "Facebook / Meta" },
                    ],
                },
                Field {
                    name: "value",
                    label: "Doğrulama değeri",
                    hint: "Hizmetin gösterdiği değeri olduğu gibi yapıştırın (ör. google-site-verification=…, MS=ms…, facebook-domain-verification=…).",
                    required: true,
                    choices: vec![],
                },
            ],
            replaces: "",
        },
        TemplateInfo {
            id: "no_mail",
            title: "Posta kullanılmıyor",
            description: "Alan adı posta almaz ve göndermez: boş MX kaydı, her göndericiyi reddeden SPF ve DMARC (reject) kaydı eklenir. Sahte postayı engeller.",
            fields: vec![],
            replaces: "var olan MX, SPF ve DMARC kayıtlarını",
        },
    ]
}

// ---- Alan doğrulama ----

fn field_error(text: &str) -> AppError {
    AppError::bad(text.to_string())
}

/// Doğrulama değerini hizmete göre doğrular ve TXT içeriğine çevirir. Değer olduğu gibi bölgeye
/// yazıldığı için boşluk, tırnak, noktalı virgül ve kontrol karakteri hiçbir koşulda geçmez.
pub fn verification_value(provider: &str, raw: &str) -> AppResult<String> {
    let trimmed = raw.trim();
    let value = trimmed.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')).unwrap_or(trimmed);
    if value.is_empty() || value.len() > 200 || !value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'=' | b'_' | b'-')) {
        return Err(field_error("Doğrulama değeri yalnızca harf, rakam, =, _ ve - içerebilir; boşluk, tırnak ya da satır sonu olamaz"));
    }
    let token_ok = |token: &str, min: usize, max: usize, lowercase: bool| {
        (min..=max).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_alphanumeric() || (!lowercase && matches!(b, b'_' | b'-'))) && (!lowercase || !token.bytes().any(|b| b.is_ascii_uppercase()))
    };
    match provider {
        "google" => {
            let token = value.strip_prefix("google-site-verification=").unwrap_or(value);
            if !token_ok(token, 20, 100, false) {
                return Err(field_error("Google doğrulama değeri “google-site-verification=…” biçiminde olmalı"));
            }
            Ok(format!("google-site-verification={token}"))
        }
        "microsoft" => {
            let token = value.strip_prefix("MS=").unwrap_or(value);
            // `ms12345678` (eski biçim) ya da 32-64 basamaklı onaltılık özet.
            let numbered = token.strip_prefix("ms").is_some_and(|digits| (4..=12).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()));
            let hashed = (32..=64).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_hexdigit());
            if !(numbered || hashed) {
                return Err(field_error("Microsoft doğrulama değeri “MS=ms12345678” ya da “MS=” ile başlayan onaltılık bir değer olmalı"));
            }
            Ok(format!("MS={token}"))
        }
        "facebook" => {
            let token = value.strip_prefix("facebook-domain-verification=").unwrap_or(value);
            if !token_ok(token, 20, 40, true) {
                return Err(field_error("Facebook doğrulama değeri “facebook-domain-verification=…” biçiminde olmalı"));
            }
            Ok(format!("facebook-domain-verification={token}"))
        }
        _ => Err(field_error("Geçersiz hizmet; Google, Microsoft ya da Facebook seçin")),
    }
}

/// Microsoft 365'in alan adından türetilen MX adresi: `ornek.com.tr` → `ornek-com-tr.mail.protection.outlook.com`.
fn microsoft_mx_host(zone: &str) -> String {
    format!("{}.mail.protection.outlook.com", zone.replace('.', "-"))
}

fn microsoft_mx_override(raw: &str) -> AppResult<String> {
    let host = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    let valid = host.ends_with(".mail.protection.outlook.com")
        && host.len() <= 120
        && host.split('.').all(|label| !label.is_empty() && !label.starts_with('-') && !label.ends_with('-') && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
    if valid { Ok(host) } else { Err(field_error("Microsoft 365 MX adresi “….mail.protection.outlook.com” ile bitmeli ve yalnızca harf, rakam ve - içermeli")) }
}

// ---- Kayıt üretimi ----

fn entry(kind: &str, name: &str, content: &str, priority: Option<u32>, owns: Owns) -> Entry {
    let mut record = Record::new(kind, name, content);
    record.priority = priority;
    // Doğrulama ve biçimlendirme `build` içinde, bölge adı bilinirken yapılır.
    Entry { record, owns }
}

/// Şablonun kayıtlarını üretir. `fields` yalnızca şablonun tanımladığı alanları içerebilir.
fn build(id: &str, zone: &str, fields: &HashMap<String, String>) -> AppResult<Vec<Entry>> {
    let info = list().into_iter().find(|info| info.id == id).ok_or(AppError::NotFound)?;
    for (name, value) in fields {
        let known = info.fields.iter().find(|field| field.name == name).ok_or_else(|| AppError::bad(format!("Bilinmeyen alan: {}", name.chars().take(40).collect::<String>())))?;
        if value.len() > 300 {
            return Err(AppError::bad(format!("“{}” alanı çok uzun", known.label)));
        }
    }
    for field in &info.fields {
        let given = fields.get(field.name).map(|value| value.trim()).unwrap_or("");
        if field.required && given.is_empty() {
            return Err(AppError::bad(format!("“{}” alanı gerekli", field.label)));
        }
        if !field.choices.is_empty() && !given.is_empty() && !field.choices.iter().any(|choice| choice.value == given) {
            return Err(AppError::bad(format!("“{}” alanı için geçersiz seçim", field.label)));
        }
    }
    let field = |name: &str| fields.get(name).map(|value| value.trim()).unwrap_or("");

    let mut entries = Vec::new();
    match id {
        "google_workspace" => {
            for (priority, host) in [(1, "aspmx.l.google.com"), (5, "alt1.aspmx.l.google.com"), (5, "alt2.aspmx.l.google.com"), (10, "alt3.aspmx.l.google.com"), (10, "alt4.aspmx.l.google.com")] {
                entries.push(entry("MX", zone, host, Some(priority), Owns::SameKind));
            }
            entries.push(entry("TXT", zone, "v=spf1 include:_spf.google.com ~all", None, Owns::TxtPrefix("v=spf1")));
        }
        "microsoft_365" => {
            let host = match field("mx_host") {
                "" => microsoft_mx_host(zone),
                custom => microsoft_mx_override(custom)?,
            };
            entries.push(entry("MX", zone, &host, Some(0), Owns::SameKind));
            entries.push(entry("TXT", zone, "v=spf1 include:spf.protection.outlook.com -all", None, Owns::TxtPrefix("v=spf1")));
            entries.push(entry("CNAME", &format!("autodiscover.{zone}"), "autodiscover.outlook.com", None, Owns::AnyAtName));
        }
        "yandex_360" => {
            entries.push(entry("MX", zone, "mx.yandex.net", Some(10), Owns::SameKind));
            entries.push(entry("TXT", zone, "v=spf1 include:_spf.yandex.net ~all", None, Owns::TxtPrefix("v=spf1")));
        }
        "domain_verification" => {
            let value = verification_value(field("provider"), field("value"))?;
            entries.push(entry("TXT", zone, &value, None, Owns::Nothing));
        }
        "no_mail" => {
            entries.push(entry("MX", zone, ".", Some(0), Owns::SameKind));
            entries.push(entry("TXT", zone, "v=spf1 -all", None, Owns::TxtPrefix("v=spf1")));
            entries.push(entry("TXT", &format!("_dmarc.{zone}"), "v=DMARC1; p=reject; adkim=s; aspf=s", None, Owns::TxtPrefix("v=dmarc1")));
        }
        _ => return Err(AppError::NotFound),
    }
    // Var olan doğrulamadan geçir; sağlayıcıya yalnızca normalize edilmiş kayıt gider.
    for item in &mut entries {
        item.record = normalize(zone, &item.record)?;
    }
    Ok(entries)
}

/// Şablon kimliği geçerli mi (404 için)?
pub fn exists(id: &str) -> bool {
    IDS.contains(&id)
}

// ---- Önizleme ----

#[derive(Debug)]
pub struct Preview {
    /// Eklenecek, zaten var olan ve (değiştirme açıksa) silinecek kayıtlar.
    pub plan: Plan,
    /// Şablonla bir arada bulunamayan var olan kayıtlar (ör. başka bir MX kümesi).
    pub conflicts: Vec<Record>,
    /// Çakışma var ve değiştirme istenmedi: şablon uygulanamaz.
    pub blocked: bool,
}

fn is_owned(existing: &Record, entries: &[Entry]) -> bool {
    entries.iter().any(|entry| {
        let same_name = existing.name.eq_ignore_ascii_case(&entry.record.name);
        match entry.owns {
            Owns::Nothing => false,
            Owns::SameKind => same_name && existing.kind == entry.record.kind,
            Owns::TxtPrefix(prefix) => same_name && existing.kind == "TXT" && existing.content.trim().to_ascii_lowercase().starts_with(&prefix.to_ascii_lowercase()),
            Owns::AnyAtName => same_name,
        }
    })
}

/// Şablonun bölgeye ne yapacağını hesaplar. Şablonla aynı olan kayıtlar "zaten var" sayılır; şablonun
/// sahiplendiği (MX, SPF, DMARC, autodiscover) ama içeriği farklı olan kayıtlar çakışmadır. Değiştirme
/// açıksa çakışanlar silinir, kapalıysa şablon uygulanamaz (iki farklı MX kümesi posta akışını bozar).
pub fn preview(zone: &str, id: &str, fields: &HashMap<String, String>, current: &[Record], replace: bool) -> AppResult<Preview> {
    let entries = build(id, zone, fields)?;
    let template_keys: HashSet<Key> = entries.iter().map(|entry| canon(&entry.record)).collect();

    let mut conflicts: Vec<Record> = Vec::new();
    for existing in current {
        let apex_ns = existing.kind == "NS" && existing.name == zone;
        if !apex_ns && is_owned(existing, &entries) && !template_keys.contains(&canon(existing)) && !conflicts.iter().any(|seen| canon(seen) == canon(existing)) {
            conflicts.push(existing.clone());
        }
    }

    let removed: HashSet<Key> = if replace { conflicts.iter().map(canon).collect() } else { HashSet::new() };
    let remaining: Vec<Record> = current.iter().filter(|record| !removed.contains(&canon(record))).cloned().collect();
    let incoming: Vec<(usize, Record)> = entries.iter().enumerate().map(|(index, entry)| (index + 1, entry.record.clone())).collect();
    let mut plan = zonefile::plan(zone, &incoming, &remaining, false);
    if replace {
        plan.remove = conflicts.clone();
    }
    let blocked = !replace && !conflicts.is_empty();
    Ok(Preview { plan, conflicts, blocked })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZONE: &str = "ornek.com";

    fn none() -> HashMap<String, String> {
        HashMap::new()
    }

    fn fields(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect()
    }

    fn record(kind: &str, name: &str, content: &str, priority: Option<u32>) -> Record {
        let mut record = Record::new(kind, name, content);
        record.priority = priority;
        normalize(ZONE, &record).unwrap()
    }

    fn shown(records: &[Record]) -> Vec<String> {
        let mut list: Vec<String> = records.iter().map(|r| format!("{} {} {} {:?}", r.kind, r.name, r.content, r.priority)).collect();
        list.sort();
        list
    }

    #[test]
    fn every_template_builds_valid_records() {
        let verification = fields(&[("provider", "google"), ("value", "google-site-verification=abcdefghijklmnopqrstuvwxyz0123456789ABCD")]);
        for info in list() {
            let given = if info.id == "domain_verification" { verification.clone() } else { none() };
            let entries = build(info.id, ZONE, &given).unwrap_or_else(|err| panic!("{}: {err}", info.id));
            assert!(!entries.is_empty(), "{}", info.id);
            // Her kayıt zaten normalize edilmiştir: yeniden normalize aynı sonucu verir.
            for item in &entries {
                assert_eq!(normalize(ZONE, &item.record).unwrap(), item.record, "{}", info.id);
            }
        }
        assert!(IDS.iter().all(|id| list().iter().any(|info| info.id == *id)));
    }

    #[test]
    fn google_workspace_records() {
        let entries = build("google_workspace", ZONE, &none()).unwrap();
        let records: Vec<Record> = entries.into_iter().map(|entry| entry.record).collect();
        let list = shown(&records);
        assert!(list.contains(&"MX ornek.com aspmx.l.google.com Some(1)".to_string()), "{list:?}");
        assert!(list.contains(&"MX ornek.com alt4.aspmx.l.google.com Some(10)".to_string()), "{list:?}");
        assert!(list.contains(&"TXT ornek.com v=spf1 include:_spf.google.com ~all None".to_string()), "{list:?}");
        assert_eq!(records.iter().filter(|r| r.kind == "MX").count(), 5);
    }

    #[test]
    fn microsoft_365_derives_mx_host_from_the_zone() {
        assert_eq!(microsoft_mx_host("ornek.com.tr"), "ornek-com-tr.mail.protection.outlook.com");
        let records: Vec<Record> = build("microsoft_365", ZONE, &none()).unwrap().into_iter().map(|entry| entry.record).collect();
        let list = shown(&records);
        assert!(list.contains(&"MX ornek.com ornek-com.mail.protection.outlook.com Some(0)".to_string()), "{list:?}");
        assert!(list.contains(&"CNAME autodiscover.ornek.com autodiscover.outlook.com None".to_string()), "{list:?}");
        assert!(list.iter().any(|item| item.contains("include:spf.protection.outlook.com")), "{list:?}");
        let custom: Vec<Record> = build("microsoft_365", ZONE, &fields(&[("mx_host", "Sirket-Ornek.mail.protection.outlook.com.")])).unwrap().into_iter().map(|entry| entry.record).collect();
        assert!(shown(&custom).contains(&"MX ornek.com sirket-ornek.mail.protection.outlook.com Some(0)".to_string()));
        for bad in ["mail.ornek.net", "x.mail.protection.outlook.com.evil.example", "a b.mail.protection.outlook.com", "-x.mail.protection.outlook.com", "x\n.mail.protection.outlook.com"] {
            assert!(build("microsoft_365", ZONE, &fields(&[("mx_host", bad)])).is_err(), "{bad}");
        }
    }

    #[test]
    fn no_mail_uses_null_mx_and_rejecting_policies() {
        let records: Vec<Record> = build("no_mail", ZONE, &none()).unwrap().into_iter().map(|entry| entry.record).collect();
        let list = shown(&records);
        assert!(list.contains(&"MX ornek.com . Some(0)".to_string()), "{list:?}");
        assert!(list.contains(&"TXT ornek.com v=spf1 -all None".to_string()), "{list:?}");
        assert!(list.iter().any(|item| item.starts_with("TXT _dmarc.ornek.com v=DMARC1; p=reject")), "{list:?}");
    }

    #[test]
    fn verification_values_are_validated_strictly() {
        let google = "google-site-verification=abcdefghijklmnopqrstuvwxyz0123456789ABCD";
        assert_eq!(verification_value("google", google).unwrap(), google);
        // Tırnaklı yapıştırma ve yalnızca belirteç de kabul edilir.
        assert_eq!(verification_value("google", &format!("\"{google}\"")).unwrap(), google);
        assert_eq!(verification_value("google", "abcdefghijklmnopqrstuvwxyz0123456789ABCD").unwrap(), google);
        assert_eq!(verification_value("microsoft", "MS=ms12345678").unwrap(), "MS=ms12345678");
        assert_eq!(verification_value("microsoft", "ms12345678").unwrap(), "MS=ms12345678");
        assert!(verification_value("microsoft", "MS=E4A68B9AB2BB9670BCE15412F62916164C0B20BB").is_ok());
        assert_eq!(verification_value("facebook", "facebook-domain-verification=abcdefghij0123456789klmno").unwrap(), "facebook-domain-verification=abcdefghij0123456789klmno");
        for bad in [
            "",
            "google-site-verification=abc",
            "google-site-verification=abcdefghijklmnopqrstuvwxyz\n0123456789",
            "google-site-verification=abcdefghijklmnopqrstuvwxyz0123456789\"; v=spf1 +all",
            "google-site-verification=abcdefghijklmnopqrstuvwxyz0123456789 v=spf1 +all",
            "google-site-verification=abcdefghijklmnopqrstuvwxyz0123456789;",
            "google-site-verification=abcdefghijklmnopqrstuvwxyz0123456789\\",
        ] {
            assert!(verification_value("google", bad).is_err(), "{bad:?}");
        }
        assert!(verification_value("microsoft", "MS=ms1").is_err());
        assert!(verification_value("microsoft", "MS=zzzz").is_err());
        assert!(verification_value("facebook", "facebook-domain-verification=ABCDEFGHIJ0123456789KLMNO").is_err(), "Facebook değeri küçük harfli olmalı");
        assert!(verification_value("tiktok", "abcdefghijklmnopqrstuvwxyz").is_err());
    }

    #[test]
    fn fields_are_checked_before_building() {
        let ok = fields(&[("provider", "google"), ("value", "google-site-verification=abcdefghijklmnopqrstuvwxyz0123456789ABCD")]);
        assert!(build("domain_verification", ZONE, &ok).is_ok());
        assert!(build("domain_verification", ZONE, &fields(&[("provider", "google")])).is_err(), "zorunlu alan eksik");
        assert!(build("domain_verification", ZONE, &fields(&[("provider", "x"), ("value", "abcdefghijklmnopqrstuvwxyz")])).is_err(), "geçersiz seçim");
        assert!(build("google_workspace", ZONE, &fields(&[("herhangi", "x")])).is_err(), "bilinmeyen alan");
        assert!(matches!(build("yok", ZONE, &none()), Err(AppError::NotFound)));
        assert!(!exists("yok") && exists("no_mail"));
    }

    #[test]
    fn preview_on_empty_zone_adds_everything() {
        let preview = preview(ZONE, "google_workspace", &none(), &[], false).unwrap();
        assert_eq!((preview.plan.add.len(), preview.plan.existing.len(), preview.conflicts.len(), preview.blocked), (6, 0, 0, false));
    }

    #[test]
    fn preview_reports_existing_records_and_applying_twice_changes_nothing() {
        let first = preview(ZONE, "google_workspace", &none(), &[], false).unwrap();
        let after_first = first.plan.add.clone();
        let second = preview(ZONE, "google_workspace", &none(), &after_first, false).unwrap();
        assert_eq!((second.plan.add.len(), second.plan.existing.len(), second.blocked), (0, 6, false));
    }

    #[test]
    fn conflicting_mx_and_spf_block_unless_replace_is_set() {
        let current = vec![
            record("MX", ZONE, "mail.ornek.com", Some(10)),
            record("TXT", ZONE, "v=spf1 mx ~all", None),
            record("TXT", ZONE, "google-site-verification=abcdefghijklmnopqrstuvwxyz0123456789ABCD", None),
            record("A", "mail.ornek.com", "203.0.113.5", None),
        ];
        let blocked = preview(ZONE, "google_workspace", &none(), &current, false).unwrap();
        assert!(blocked.blocked);
        assert_eq!(shown(&blocked.conflicts), vec!["MX ornek.com mail.ornek.com Some(10)", "TXT ornek.com v=spf1 mx ~all None"]);
        assert!(blocked.plan.remove.is_empty());

        let replaced = preview(ZONE, "google_workspace", &none(), &current, true).unwrap();
        assert!(!replaced.blocked);
        assert_eq!(shown(&replaced.plan.remove), shown(&blocked.conflicts));
        assert_eq!(replaced.plan.add.len(), 6);
        // Doğrulama TXT'si ve A kaydı çakışma sayılmaz, silinmez.
        assert!(replaced.plan.remove.iter().all(|r| r.kind != "A" && !r.content.starts_with("google-site")));
    }

    #[test]
    fn microsoft_autodiscover_conflicts_with_any_record_at_that_name() {
        let current = vec![record("CNAME", "autodiscover.ornek.com", "mail.ornek.com", None)];
        let result = preview(ZONE, "microsoft_365", &none(), &current, false).unwrap();
        assert!(result.blocked);
        assert_eq!(shown(&result.conflicts), vec!["CNAME autodiscover.ornek.com mail.ornek.com None"]);
        let replaced = preview(ZONE, "microsoft_365", &none(), &current, true).unwrap();
        assert!(replaced.plan.add.iter().any(|r| r.kind == "CNAME" && r.content == "autodiscover.outlook.com"));
    }

    #[test]
    fn verification_never_conflicts_and_apex_ns_is_never_replaced() {
        let current = vec![record("TXT", ZONE, "v=spf1 mx ~all", None), record("NS", ZONE, "ns1.ornek.com", None)];
        let fields = fields(&[("provider", "microsoft"), ("value", "MS=ms12345678")]);
        let result = preview(ZONE, "domain_verification", &fields, &current, false).unwrap();
        assert_eq!((result.blocked, result.plan.add.len()), (false, 1));
        let no_mail = preview(ZONE, "no_mail", &none(), &current, true).unwrap();
        assert!(no_mail.plan.remove.iter().all(|r| r.kind != "NS"));
        assert_eq!(no_mail.plan.remove.len(), 1, "yalnızca eski SPF silinir");
    }
}
