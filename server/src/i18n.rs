//! Sunucunun ürettiği metinlerin çevirisi.
//!
//! Kaynak metin Türkçedir ve aynı zamanda çeviri anahtarıdır (arayüzdeki `t("…")` ile aynı
//! mantık). Kod her yerde Türkçe metin üretir; çeviri yalnızca metnin kullanıcıya ulaştığı
//! sınırda yapılır: API yanıtı (`translate_json`), canlı akışlar ve e-postalar. Böylece
//! veritabanında saklanan metinler (görev günlükleri, bulgular) tek dilde durur ve onları
//! okuyan her kullanıcı kendi dilinde görür.
//!
//! Sözlük `server/i18n/en.json` dosyasıdır ve ikiliye gömülür; `extra.json` kaynakta tek başına
//! geçmeyen, birleştirilerek kurulan metinlerin (`"[Kritik] {1}"` gibi) elle yazılmış şablonlarını tutar. Anahtarlar
//! `scripts/extract-server-i18n.py` ile kaynaktan çıkarılır. `format!` ile kurulan metinlerin
//! anahtarında yer tutucular kalır (`{domain} için posta zaten açık`); gelen metin bu şablonla
//! eşleştirilir, yakalanan değerler çevirideki aynı adlı yer tutuculara konur.
//! Çevirisi bulunmayan metin olduğu gibi (Türkçe) döner.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;

use axum::http::HeaderMap;
use serde_json::Value;

/// Çeviride iç içe inilecek en fazla düzey (`"{işlem}: {hata}"` gibi sarmalanmış metinler).
const MAX_DEPTH: u8 = 4;
/// Bundan uzun tek satırlar ileti değildir (dosya içeriği, komut çıktısı); şablonla eşleştirilmez.
const MAX_TEMPLATE_INPUT: usize = 4000;

/// Çevirideki bir parça: düz metin ya da kaynak şablondaki bir yer tutucunun sırası.
enum Piece {
    Text(String),
    Hole(usize),
}

struct Template {
    /// Yer tutucuların arasındaki sabit metinler; uzunluğu yer tutucu sayısından bir fazladır.
    literals: Vec<String>,
    target: Vec<Piece>,
    /// Sabit metnin uzunluğu: daha belirgin şablon önce denenir.
    weight: usize,
    /// Sabit metninde satır sonu olan şablonun yakaladığı değerler de çok satırlı olabilir.
    multiline: bool,
}

struct Dictionary {
    exact: HashMap<String, String>,
    templates: Vec<Template>,
}

/// Metindeki `{ad}` ya da `{1}` biçimindeki yer tutucuları bulur: (başlangıç, bitiş, ad).
fn holes(text: &str) -> Vec<(usize, usize, &str)> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(len) = text[i + 1..].find('}') {
                let name = &text[i + 1..i + 1 + len];
                if !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                    found.push((i, i + len + 2, name));
                    i += len + 2;
                    continue;
                }
            }
        }
        i += 1;
    }
    found
}

impl Dictionary {
    /// Sözlük dosyalarını birleştirerek okur; sonraki dosya öncekinin üstüne yazar.
    fn parse_all(sources: &[&str]) -> Self {
        let mut entries: HashMap<String, String> = HashMap::new();
        for json in sources {
            entries.extend(serde_json::from_str::<HashMap<String, String>>(json).unwrap_or_default());
        }
        Self::from_entries(entries)
    }

    #[cfg(test)]
    fn parse(json: &str) -> Self {
        Self::parse_all(&[json])
    }

    fn from_entries(entries: HashMap<String, String>) -> Self {
        let mut exact = HashMap::new();
        let mut templates = Vec::new();
        for (source, target) in entries {
            let source_holes = holes(&source);
            if source_holes.is_empty() {
                exact.insert(source, target);
                continue;
            }
            let mut literals = Vec::new();
            let mut names = Vec::new();
            let mut cursor = 0;
            for (start, end, name) in &source_holes {
                literals.push(source[cursor..*start].to_string());
                names.push(*name);
                cursor = *end;
            }
            literals.push(source[cursor..].to_string());
            // Art arda gelen iki yer tutucunun sınırı belirsizdir; böyle şablon eşleştirilmez.
            if literals[1..literals.len() - 1].iter().any(String::is_empty) {
                continue;
            }
            let mut pieces = Vec::new();
            let mut cursor = 0;
            for (start, end, name) in holes(&target) {
                // Çeviride kaynakta olmayan bir ad varsa düz metin sayılır.
                let Some(index) = names.iter().position(|n| *n == name) else { continue };
                pieces.push(Piece::Text(target[cursor..start].to_string()));
                pieces.push(Piece::Hole(index));
                cursor = end;
            }
            pieces.push(Piece::Text(target[cursor..].to_string()));
            let weight = literals.iter().map(String::len).sum();
            let multiline = literals.iter().any(|l| l.contains('\n'));
            templates.push(Template { literals, target: pieces, weight, multiline });
        }
        // Sabit metinle başlayan şablon önce denenir: iletiler çoğunlukla `"HATA: {iç}"` gibi önekle
        // sarmalanır ve iç ileti ayrıca çevrilir. Aynı sınıfta daha belirgin (uzun) şablon öndedir.
        templates.sort_by(|a, b| (b.literals[0].is_empty().cmp(&a.literals[0].is_empty())).reverse().then(b.weight.cmp(&a.weight)));
        Self { exact, templates }
    }
}

impl Template {
    /// Metin şablona uyuyorsa yer tutucuların karşılık geldiği parçaları verir.
    fn capture<'a>(&self, text: &'a str) -> Option<Vec<&'a str>> {
        let first = &self.literals[0];
        let last = &self.literals[self.literals.len() - 1];
        if text.len() < first.len() + last.len() || !text.starts_with(first.as_str()) || !text.ends_with(last.as_str()) {
            return None;
        }
        let body = &text[first.len()..text.len() - last.len()];
        let mut captures = Vec::with_capacity(self.literals.len() - 1);
        let mut cursor = 0;
        for literal in &self.literals[1..self.literals.len() - 1] {
            let at = body[cursor..].find(literal.as_str())?;
            captures.push(&body[cursor..cursor + at]);
            cursor += at + literal.len();
        }
        captures.push(&body[cursor..]);
        let acceptable = |value: &&str| !value.is_empty() && (self.multiline || !value.contains('\n'));
        if !captures.iter().all(acceptable) {
            return None;
        }
        // Yer tutucuyla başlayan şablon, önekle sarmalanmış bir metni (`"HATA: Kayıt bulunamadı"`)
        // ortasından yakalamasın: ilk değerde `": "` varsa bu şablon o metnin değildir.
        // Tek istisna, sabit metni yalnızca `": "` olan genel sarmalayıcıdır.
        if first.is_empty() && self.weight > 2 && captures[0].contains(": ") {
            return None;
        }
        Some(captures)
    }
}

static ENGLISH: OnceLock<Dictionary> = OnceLock::new();

fn dictionary(language: &str) -> Option<&'static Dictionary> {
    match language {
        "en" => Some(ENGLISH.get_or_init(|| Dictionary::parse_all(&[include_str!("../i18n/en.json"), include_str!("../i18n/extra.json")]))),
        _ => None,
    }
}

/// İsteği yapan hesabın dili. Çeviri katmanı bunu isteğe boş olarak ekler; kimlik doğrulanınca
/// `AuthUser` doldurur, yanıt dönerken katman okur. Böylece dil, hesabın kendi ayarından gelir
/// ve ayrıca veritabanına sorulmaz.
#[derive(Clone, Default)]
pub struct LanguageSlot(std::sync::Arc<OnceLock<&'static str>>);

impl LanguageSlot {
    /// Hesabın dilini kaydeder; dili henüz belirlenmemiş hesapta (boş değer) bir şey yapmaz.
    pub fn set(&self, language: &str) {
        if !language.trim().is_empty() {
            let _ = self.0.set(normalize(language));
        }
    }

    pub fn get(&self) -> Option<&'static str> {
        self.0.get().copied()
    }
}

/// Panelin desteklediği dil kodu; tanınmayan değer Türkçe sayılır.
pub fn normalize(language: &str) -> &'static str {
    let tag = language.trim().to_ascii_lowercase();
    if tag == "en" || tag.starts_with("en-") || tag.starts_with("en_") {
        "en"
    } else {
        "tr"
    }
}

/// İsteğin `Accept-Language` başlığındaki ilk dil. Yalnızca hesabın dili bilinmediğinde
/// kullanılır: oturum açılmamış isteklerde (giriş ekranı) ve dili henüz belirlenmemiş hesapta.
/// Başlık da yoksa metinler Türkçe kalır.
pub fn request_language(headers: &HeaderMap) -> &'static str {
    let value = headers.get(axum::http::header::ACCEPT_LANGUAGE).and_then(|v| v.to_str().ok()).unwrap_or("");
    let first = value.split(',').next().unwrap_or("").split(';').next().unwrap_or("");
    normalize(first)
}

/// Metni verilen dile çevirir; çevirisi yoksa metnin kendisini döner.
pub fn translate<'a>(language: &str, text: &'a str) -> Cow<'a, str> {
    match dictionary(language) {
        Some(dictionary) => translate_with(dictionary, text, 0),
        None => Cow::Borrowed(text),
    }
}

fn translate_with<'a>(dictionary: &Dictionary, text: &'a str, depth: u8) -> Cow<'a, str> {
    if let Some(found) = dictionary.exact.get(text) {
        return Cow::Owned(found.clone());
    }
    if depth >= MAX_DEPTH || !text.chars().any(char::is_alphabetic) {
        return Cow::Borrowed(text);
    }
    // Baştaki ve sondaki boşluklar (girintili günlük satırları) çeviriye engel olmasın.
    let trimmed = text.trim();
    if trimmed.len() != text.len() && !trimmed.is_empty() {
        let start = text.len() - text.trim_start().len();
        let inner = translate_with(dictionary, trimmed, depth);
        return match inner {
            Cow::Borrowed(_) => Cow::Borrowed(text),
            Cow::Owned(inner) => Cow::Owned(format!("{}{inner}{}", &text[..start], &text[start + trimmed.len()..])),
        };
    }
    let has_newline = text.contains('\n');
    if text.len() <= MAX_TEMPLATE_INPUT {
        for template in &dictionary.templates {
            if has_newline && !template.multiline {
                continue;
            }
            let Some(captures) = template.capture(text) else { continue };
            let mut out = String::with_capacity(text.len() + 16);
            for piece in &template.target {
                match piece {
                    Piece::Text(literal) => out.push_str(literal),
                    // Tek sözcüklük değerler addır (alan adı, kullanıcı adı); yalnızca
                    // içinde boşluk olanlar yeniden çevrilmeye çalışılır.
                    Piece::Hole(index) => {
                        let value = captures[*index];
                        if value.contains(char::is_whitespace) {
                            out.push_str(&translate_with(dictionary, value, depth + 1));
                        } else {
                            out.push_str(value);
                        }
                    }
                }
            }
            return Cow::Owned(out);
        }
    }
    if has_newline {
        // Çok satırlı metin (görev günlüğü, ayrıntılı hata) satır satır çevrilir.
        let mut changed = false;
        let lines: Vec<Cow<str>> = text
            .split('\n')
            .map(|line| {
                let translated = translate_with(dictionary, line, depth + 1);
                changed |= matches!(translated, Cow::Owned(_));
                translated
            })
            .collect();
        if changed {
            return Cow::Owned(lines.join("\n"));
        }
    }
    Cow::Borrowed(text)
}

/// Sunucunun kendi yazdığı metni taşıyan JSON alanları. Kullanıcının girdiği değerleri taşıyan
/// alanlar (ad, alan adı, açıklama, dosya içeriği) burada yer almaz: bir veritabanının adı
/// rastlantıyla sözlükteki bir metne eşit diye çevrilmemelidir.
const TEXT_KEYS: &[&str] = &[
    "message",
    "title",
    "detail",
    "details",
    "label",
    "line",
    "log",
    "error",
    "last_error",
    "check_error",
    "summary",
    "hint",
    "reason",
    "note",
    "notes",
    "warning",
    "warnings",
    "problems",
    "issues",
    "steps",
    "fix",
    "fix_label",
    "status_text",
    "result",
    "advice",
    "explanation",
    "blocked_reason",
    "skipped",
    // Bu dördü kullanıcı metni de taşıyabilir (zamanlı görevin açıklaması gibi); çeviri yalnızca
    // sözlükteki bir metinle birebir örtüşünce yapıldığı için kullanıcının yazdığı cümle değişmez.
    "description",
    "purpose",
    "replaces",
    "in_use",
];

/// API yanıtındaki sunucu metinlerini çevirir. Değişiklik olduysa `true` döner.
pub fn translate_json(language: &str, value: &mut Value) -> bool {
    translate_json_with(language, value, &[])
}

/// `translate_json` gibi; ayrıca `extra_keys` içindeki alanları da metin sayar. Yalnızca o alanın
/// sunucunun kendi metnini taşıdığı bilinen yanıtlarda kullanılır (bileşen listesindeki `name`).
pub fn translate_json_with(language: &str, value: &mut Value, extra_keys: &[&str]) -> bool {
    match dictionary(language) {
        Some(dictionary) => walk(dictionary, value, false, extra_keys),
        None => false,
    }
}

/// Değeri JSON metnine çevirir; sunucu metinleri verilen dile çevrilir (canlı akışlar için).
/// Kaynak dilde değer doğrudan yazılır, böylece alanların sırası da değişmez.
pub fn to_json_string<T: serde::Serialize>(language: &str, value: &T) -> String {
    if dictionary(language).is_none() {
        return serde_json::to_string(value).unwrap_or_default();
    }
    let mut json = serde_json::to_value(value).unwrap_or_default();
    translate_json(language, &mut json);
    json.to_string()
}

fn walk(dictionary: &Dictionary, value: &mut Value, is_text: bool, extra_keys: &[&str]) -> bool {
    match value {
        Value::String(text) if is_text => match translate_with(dictionary, text, 0) {
            Cow::Owned(translated) => {
                *text = translated;
                true
            }
            Cow::Borrowed(_) => false,
        },
        // Metin alanının değeri dizi ise (uyarı listesi gibi) öğeleri de metindir.
        Value::Array(items) => items.iter_mut().fold(false, |changed, item| walk(dictionary, item, is_text, extra_keys) | changed),
        Value::Object(map) => map.iter_mut().fold(false, |changed, (key, item)| {
            let text_field = TEXT_KEYS.contains(&key.as_str()) || extra_keys.contains(&key.as_str());
            // Metin alanının altındaki nesnelerde yalnızca yine bilinen alanlar çevrilir.
            walk(dictionary, item, text_field && !item.is_object(), extra_keys) | changed
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dictionary() -> Dictionary {
        Dictionary::parse(
            r#"{
                "Kayıt bulunamadı": "Record not found",
                "{domain} için posta zaten açık": "Mail is already enabled for {domain}",
                "{1}: {2}": "{1}: {2}",
                "Yedek alınamadı: {err}": "Backup failed: {err}",
                "{used} / {limit} MB kullanılıyor": "{used} of {limit} MB in use",
                "{err}\n\nSon satırlar:\n{1}": "{err}\n\nLast lines:\n{1}",
                "komut başarısız oldu": "command failed",
                "{a}{b} bitişik": "{a}{b} adjacent"
            }"#,
        )
    }

    fn tr(text: &str) -> String {
        translate_with(&dictionary(), text, 0).into_owned()
    }

    #[test]
    fn exact_and_template_matches() {
        assert_eq!(tr("Kayıt bulunamadı"), "Record not found");
        assert_eq!(tr("ornek.com için posta zaten açık"), "Mail is already enabled for ornek.com");
        assert_eq!(tr("12 / 500 MB kullanılıyor"), "12 of 500 MB in use");
    }

    #[test]
    fn unknown_text_is_returned_as_is() {
        assert_eq!(tr("Bilinmeyen bir ileti"), "Bilinmeyen bir ileti");
        assert_eq!(tr(""), "");
        assert_eq!(tr("12345"), "12345");
    }

    #[test]
    fn nested_messages_are_translated_but_names_are_not() {
        // Sarmalanan hata da çevrilir.
        assert_eq!(tr("Yedek alınamadı: komut başarısız oldu"), "Backup failed: command failed");
        // Tek sözcüklük değer addır; sözlükte karşılığı olsa da dokunulmaz.
        let dict = Dictionary::parse(r#"{"Yedek": "Backup", "{name} veritabanı silindi": "Database {name} deleted"}"#);
        assert_eq!(translate_with(&dict, "Yedek veritabanı silindi", 0), "Database Yedek deleted");
    }

    #[test]
    fn more_specific_template_wins() {
        assert_eq!(tr("Yedek alınamadı: disk dolu"), "Backup failed: disk dolu");
    }

    #[test]
    fn multiline_text_is_translated_line_by_line() {
        assert_eq!(tr("başladı\nKayıt bulunamadı\n  komut başarısız oldu"), "başladı\nRecord not found\n  command failed");
        assert_eq!(tr("komut başarısız oldu\n\nSon satırlar:\nKayıt bulunamadı"), "command failed\n\nLast lines:\nRecord not found");
    }

    #[test]
    fn wrapped_message_is_not_captured_from_the_middle() {
        let dict = Dictionary::parse(
            r#"{"HATA: {1}": "ERROR: {1}", "{1} bulunamadı": "{1} not found", "Kayıt bulunamadı": "Record not found", "{1}: {2}": "{1}: {2}"}"#,
        );
        assert_eq!(translate_with(&dict, "HATA: Kayıt bulunamadı", 0), "ERROR: Record not found");
        assert_eq!(translate_with(&dict, "Dosya bulunamadı", 0), "Dosya not found");
        assert_eq!(translate_with(&dict, "Site: Kayıt bulunamadı", 0), "Site: Record not found");
    }

    #[test]
    fn adjacent_holes_are_skipped() {
        assert_eq!(tr("xy bitişik"), "xy bitişik");
    }

    #[test]
    fn json_only_known_fields_are_translated() {
        let dict = dictionary();
        let mut value = serde_json::json!({
            "name": "Kayıt bulunamadı",
            "error": { "code": "not_found", "message": "Kayıt bulunamadı" },
            "warnings": ["komut başarısız oldu", "başka"],
            "items": [{ "title": "Kayıt bulunamadı", "domain": "komut başarısız oldu" }]
        });
        assert!(walk(&dict, &mut value, false, &[]));
        assert_eq!(value["name"], "Kayıt bulunamadı");
        assert_eq!(value["error"]["message"], "Record not found");
        assert_eq!(value["warnings"][0], "command failed");
        assert_eq!(value["items"][0]["title"], "Record not found");
        assert_eq!(value["items"][0]["domain"], "komut başarısız oldu");
    }

    #[test]
    fn request_language_reads_first_tag() {
        let mut headers = HeaderMap::new();
        assert_eq!(request_language(&headers), "tr");
        headers.insert(axum::http::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9,tr;q=0.8".parse().unwrap());
        assert_eq!(request_language(&headers), "en");
        headers.insert(axum::http::header::ACCEPT_LANGUAGE, "tr-TR,en;q=0.9".parse().unwrap());
        assert_eq!(request_language(&headers), "tr");
    }

    #[test]
    fn language_slot_keeps_the_account_language() {
        let slot = LanguageSlot::default();
        assert_eq!(slot.get(), None);
        slot.set("");
        assert_eq!(slot.get(), None, "dili belirlenmemiş hesap başlığa bırakılır");
        slot.clone().set("en");
        assert_eq!(slot.get(), Some("en"));
    }

    #[test]
    fn embedded_dictionary_loads() {
        let english = super::dictionary("en").unwrap();
        assert!(english.exact.len() > 500);
        assert!(english.templates.len() > 300);
        assert_eq!(translate("en", "Kayıt bulunamadı"), "Record not found");
        assert_eq!(translate("tr", "Kayıt bulunamadı"), "Kayıt bulunamadı");
    }
}
