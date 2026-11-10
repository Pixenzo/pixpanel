//! Posta kutusu kurallarından Sieve betiği üretimi.
//!
//! Kullanıcı filtrelerini, otomatik yanıtı ve yönlendirmeyi arayüzde
//! yapılandırır; panel bunları tek bir Sieve betiğine çevirir. Kullanıcı metni
//! betiğe yalnızca dize sabiti olarak, kaçışlanarak girer.

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

const MAX_FILTERS: usize = 50;
const MAX_CONDITIONS: usize = 10;
const MAX_ACTIONS: usize = 5;
pub const MAX_FORWARDS: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct AutoReply {
    pub enabled: bool,
    pub subject: String,
    pub body: String,
    /// `YYYY-AA-GG`; boşsa sınır yok.
    pub start_date: String,
    pub end_date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Condition {
    /// `from`, `to`, `cc`, `subject`, `header` ya da `size_over`.
    pub field: String,
    /// `field` = `header` ise başlığın adı.
    #[serde(default)]
    pub header: String,
    /// `contains`, `not_contains`, `is` ya da `matches` (joker: `*` ve `?`).
    #[serde(default)]
    pub op: String,
    /// Aranan metin; `size_over` için megabayt cinsinden sayı.
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Action {
    /// `move`, `copy`, `mark_read`, `flag`, `forward` ya da `discard`.
    pub kind: String,
    /// `move`/`copy` için klasör, `forward` için adres.
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Filter {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Koşulların hepsi mi (`true`) yoksa herhangi biri mi (`false`) sağlanmalı.
    #[serde(default = "default_true")]
    pub match_all: bool,
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
    /// Bu kural uyduysa sonraki kurallara bakılmaz.
    #[serde(default = "default_true")]
    pub stop: bool,
}

fn default_true() -> bool {
    true
}

/// Sieve dize sabiti: ters eğik çizgi ve tırnak kaçışlanır.
fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn no_control(value: &str, allow_newlines: bool) -> bool {
    !value.chars().any(|c| c.is_control() && !(allow_newlines && (c == '\n' || c == '\r')))
}

fn validate_text(label: &str, value: &str, max: usize) -> AppResult<()> {
    if value.trim().is_empty() {
        return Err(AppError::bad(format!("{label} boş olamaz")));
    }
    if value.len() > max || !no_control(value, false) {
        return Err(AppError::bad(format!("{label} geçersiz ya da çok uzun")));
    }
    Ok(())
}

pub fn validate_folder(value: &str) -> AppResult<()> {
    validate_text("Klasör adı", value, 100)?;
    if value.starts_with(['.', '/']) || value.contains("..") || value.contains('*') || value.contains('%') {
        return Err(AppError::bad("Geçersiz klasör adı"));
    }
    Ok(())
}

fn validate_date(value: &str) -> AppResult<()> {
    let bytes = value.as_bytes();
    let shaped = bytes.len() == 10
        && bytes.iter().enumerate().all(|(index, b)| if index == 4 || index == 7 { *b == b'-' } else { b.is_ascii_digit() });
    if shaped { Ok(()) } else { Err(AppError::bad("Tarih YYYY-AA-GG biçiminde olmalı")) }
}

impl AutoReply {
    pub fn validate(&self) -> AppResult<()> {
        if !self.enabled && self.subject.is_empty() && self.body.is_empty() {
            return Ok(());
        }
        validate_text("Otomatik yanıt konusu", &self.subject, 200)?;
        if self.body.trim().is_empty() || self.body.len() > 4000 || !no_control(&self.body, true) {
            return Err(AppError::bad("Otomatik yanıt metni boş olamaz ve 4000 karakteri aşamaz"));
        }
        for date in [&self.start_date, &self.end_date] {
            if !date.is_empty() {
                validate_date(date)?;
            }
        }
        if !self.start_date.is_empty() && !self.end_date.is_empty() && self.start_date > self.end_date {
            return Err(AppError::bad("Otomatik yanıtın bitiş tarihi başlangıçtan önce olamaz"));
        }
        Ok(())
    }
}

pub fn validate_filters(filters: &[Filter], validate_address: impl Fn(&str) -> AppResult<String>) -> AppResult<()> {
    if filters.len() > MAX_FILTERS {
        return Err(AppError::bad(format!("En fazla {MAX_FILTERS} filtre tanımlanabilir")));
    }
    for filter in filters {
        validate_text("Filtre adı", &filter.name, 100)?;
        if filter.conditions.is_empty() || filter.conditions.len() > MAX_CONDITIONS {
            return Err(AppError::bad(format!("“{}” filtresinde 1-{MAX_CONDITIONS} koşul olmalı", filter.name)));
        }
        if filter.actions.is_empty() || filter.actions.len() > MAX_ACTIONS {
            return Err(AppError::bad(format!("“{}” filtresinde 1-{MAX_ACTIONS} işlem olmalı", filter.name)));
        }
        for condition in &filter.conditions {
            match condition.field.as_str() {
                "size_over" => {
                    let megabytes: u32 = condition.value.trim().parse().map_err(|_| AppError::bad("Boyut koşulu megabayt cinsinden bir sayı olmalı"))?;
                    if !(1..=1000).contains(&megabytes) {
                        return Err(AppError::bad("Boyut koşulu 1-1000 MB arasında olmalı"));
                    }
                }
                "from" | "to" | "cc" | "subject" | "header" => {
                    if !matches!(condition.op.as_str(), "contains" | "not_contains" | "is" | "matches") {
                        return Err(AppError::bad("Geçersiz karşılaştırma türü"));
                    }
                    validate_text("Koşul değeri", &condition.value, 250)?;
                    if condition.field == "header" {
                        let name = condition.header.as_str();
                        let name_ok = !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
                        if !name_ok {
                            return Err(AppError::bad("Geçersiz başlık adı (örnek: List-Id)"));
                        }
                    }
                }
                _ => return Err(AppError::bad("Geçersiz koşul alanı")),
            }
        }
        for action in &filter.actions {
            match action.kind.as_str() {
                "move" | "copy" => validate_folder(&action.value)?,
                "forward" => {
                    validate_address(&action.value)?;
                }
                "mark_read" | "flag" | "discard" => {}
                _ => return Err(AppError::bad("Geçersiz filtre işlemi")),
            }
        }
    }
    Ok(())
}

fn render_condition(condition: &Condition) -> String {
    if condition.field == "size_over" {
        return format!("size :over {}M", condition.value.trim());
    }
    let (negate, comparator) = match condition.op.as_str() {
        "not_contains" => (true, ":contains"),
        "is" => (false, ":is"),
        "matches" => (false, ":matches"),
        _ => (false, ":contains"),
    };
    let test = match condition.field.as_str() {
        "from" | "to" | "cc" => format!("address {comparator} {} {}", quote(&condition.field), quote(&condition.value)),
        "subject" => format!("header {comparator} \"subject\" {}", quote(&condition.value)),
        _ => format!("header {comparator} {} {}", quote(&condition.header), quote(&condition.value)),
    };
    if negate { format!("not {test}") } else { test }
}

fn render_action(action: &Action) -> String {
    match action.kind.as_str() {
        "move" => format!("fileinto :create {};", quote(&action.value)),
        "copy" => format!("fileinto :copy :create {};", quote(&action.value)),
        "mark_read" => "addflag \"\\\\Seen\";".to_string(),
        "flag" => "addflag \"\\\\Flagged\";".to_string(),
        "forward" => format!("redirect :copy {};", quote(&action.value)),
        _ => "discard;".to_string(),
    }
}

/// Kutunun bütün kurallarını tek betikte birleştirir. Girdiler doğrulanmış olmalıdır.
///
/// Sıra: filtreler, otomatik yanıt, yönlendirme. Bir filtre iletiyi durdurursa
/// (ör. siler ya da klasöre taşıyıp `stop` derse) sonrakiler çalışmaz.
pub fn render(address: &str, filters: &[Filter], auto_reply: &AutoReply, forward_to: &[String], forward_keep: bool) -> String {
    let mut out = String::from(
        "# PixPanel tarafından üretildi. Elle yapılan değişiklikler kaybolur.\n\
         require [\"fileinto\", \"mailbox\", \"imap4flags\", \"copy\", \"vacation\", \"date\", \"relational\"];\n\n",
    );
    for filter in filters.iter().filter(|filter| filter.enabled) {
        let tests: Vec<String> = filter.conditions.iter().map(render_condition).collect();
        let test = if tests.len() == 1 {
            tests[0].clone()
        } else {
            format!("{}({})", if filter.match_all { "allof" } else { "anyof" }, tests.join(", "))
        };
        // Yorum satırına giren ad tek satıra indirgenmiştir (denetim karakteri içeremez).
        out.push_str(&format!("# {}\nif {test} {{\n", filter.name.replace('\n', " ")));
        for action in &filter.actions {
            out.push_str(&format!("    {}\n", render_action(action)));
        }
        if filter.stop {
            out.push_str("    stop;\n");
        }
        out.push_str("}\n\n");
    }

    if auto_reply.enabled {
        let mut tests = Vec::new();
        if !auto_reply.start_date.is_empty() {
            tests.push(format!("currentdate :value \"ge\" \"date\" {}", quote(&auto_reply.start_date)));
        }
        if !auto_reply.end_date.is_empty() {
            tests.push(format!("currentdate :value \"le\" \"date\" {}", quote(&auto_reply.end_date)));
        }
        let vacation = format!(
            "vacation :days 1 :subject {} :addresses [{}] {};",
            quote(&auto_reply.subject),
            quote(address),
            quote(&auto_reply.body.replace("\r\n", "\n"))
        );
        match tests.len() {
            0 => out.push_str(&format!("{vacation}\n\n")),
            1 => out.push_str(&format!("if {} {{\n    {vacation}\n}}\n\n", tests[0])),
            _ => out.push_str(&format!("if allof({}) {{\n    {vacation}\n}}\n\n", tests.join(", "))),
        }
    }

    for (index, target) in forward_to.iter().enumerate() {
        // Kopya saklanmayacaksa son yönlendirme iletinin kutuya düşmesini engeller.
        let copy = if forward_keep || index + 1 < forward_to.len() { " :copy" } else { "" };
        out.push_str(&format!("redirect{copy} {};\n", quote(target)));
    }
    out
}

/// Kutunun betik gerektiren bir kuralı var mı?
pub fn is_needed(filters: &[Filter], auto_reply: &AutoReply, forward_to: &[String]) -> bool {
    filters.iter().any(|filter| filter.enabled) || auto_reply.enabled || !forward_to.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(conditions: Vec<Condition>, actions: Vec<Action>) -> Filter {
        Filter { name: "Deneme".into(), enabled: true, match_all: true, conditions, actions, stop: true }
    }

    fn condition(field: &str, op: &str, value: &str) -> Condition {
        Condition { field: field.into(), header: String::new(), op: op.into(), value: value.into() }
    }

    #[test]
    fn user_text_cannot_break_out_of_string() {
        let filters = vec![filter(
            vec![condition("subject", "contains", "a\"; discard; #")],
            vec![Action { kind: "move".into(), value: "Klasör\\\"x".into() }],
        )];
        let script = render("ali@ornek.com", &filters, &AutoReply::default(), &[], true);
        assert!(script.contains(r#"header :contains "subject" "a\"; discard; #""#), "{script}");
        assert!(script.contains(r#"fileinto :create "Klasör\\\"x";"#), "{script}");
    }

    #[test]
    fn validation_rejects_bad_input() {
        let ok_address = |value: &str| Ok(value.to_string());
        let newline = vec![filter(vec![condition("subject", "contains", "a\nb")], vec![Action { kind: "discard".into(), value: String::new() }])];
        assert!(validate_filters(&newline, ok_address).is_err());
        let bad_header = vec![filter(
            vec![Condition { field: "header".into(), header: "X\" :is".into(), op: "is".into(), value: "x".into() }],
            vec![Action { kind: "flag".into(), value: String::new() }],
        )];
        assert!(validate_filters(&bad_header, ok_address).is_err());
        let traversal = vec![filter(vec![condition("from", "is", "a@b.c")], vec![Action { kind: "move".into(), value: "../../x".into() }])];
        assert!(validate_filters(&traversal, ok_address).is_err());
        let size = vec![filter(vec![condition("size_over", "", "5M; discard")], vec![Action { kind: "flag".into(), value: String::new() }])];
        assert!(validate_filters(&size, ok_address).is_err());
    }

    #[test]
    fn forward_without_copy_drops_local_delivery_only_on_last() {
        let targets = vec!["a@x.com".to_string(), "b@x.com".to_string()];
        let script = render("ali@ornek.com", &[], &AutoReply::default(), &targets, false);
        assert!(script.contains("redirect :copy \"a@x.com\";\nredirect \"b@x.com\";"), "{script}");
    }
}
