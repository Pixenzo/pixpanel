//! Bağımlılıksız, hoşgörülü XML taraması (Plesk `backup_info_*.xml` için).
//!
//! Yalnızca başlangıç/bitiş etiketlerini ve öznitelik değerlerini çıkarır; ad alanı, DTD ya da
//! varlık tanımı işlemez (dış varlıklar hiçbir zaman çözülmez). Öznitelik değerlerinde `>` ve
//! tırnak içi her şey güvenle değer sayılır; yorumlar, CDATA ve işlem yönergeleri atlanır.

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Start { name: String, attrs: Vec<(String, String)>, self_closing: bool },
    End { name: String },
}

impl Event {
    pub fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
        attrs.iter().find(|(name, _)| name == key).map(|(_, value)| value.as_str())
    }
}

/// `&amp;`, `&lt;`, `&gt;`, `&quot;`, `&apos;` ve sayısal karakter başvurularını çözer; tanınmayan
/// başvurular olduğu gibi kalır.
pub fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let replaced = rest.find(';').filter(|end| *end <= 10).and_then(|end| {
            let name = &rest[1..end];
            let ch = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => name
                    .strip_prefix("#x")
                    .or_else(|| name.strip_prefix("#X"))
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .or_else(|| name.strip_prefix('#').and_then(|number| number.parse::<u32>().ok()))
                    .and_then(char::from_u32),
            };
            ch.map(|ch| (ch, end + 1))
        });
        match replaced {
            Some((ch, used)) => {
                out.push(ch);
                rest = &rest[used..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

/// Metindeki etiketleri sırayla çıkarır. Bozuk biçimde sessizce olabildiğince ilerler.
pub fn scan(text: &str) -> Vec<Event> {
    let bytes = text.as_bytes();
    let mut events = Vec::new();
    let mut index = 0;
    while let Some(offset) = text[index..].find('<') {
        index += offset;
        let rest = &text[index..];
        if rest.starts_with("<!--") {
            index += rest.find("-->").map_or(rest.len(), |end| end + 3);
        } else if rest.starts_with("<![CDATA[") {
            index += rest.find("]]>").map_or(rest.len(), |end| end + 3);
        } else if rest.starts_with("<?") {
            index += rest.find("?>").map_or(rest.len(), |end| end + 2);
        } else if rest.starts_with("<!") {
            // DOCTYPE: iç alt küme (`[ ... ]`) varsa onun bitişine kadar atlanır.
            let end = match (rest.find('['), rest.find('>')) {
                (Some(open), Some(close)) if open < close => rest.find("]>").map_or(rest.len(), |end| end + 2),
                (_, Some(close)) => close + 1,
                _ => rest.len(),
            };
            index += end;
        } else if rest.starts_with("</") {
            let end = rest.find('>').unwrap_or(rest.len());
            events.push(Event::End { name: rest[2..end].trim().to_string() });
            index = (index + end + 1).min(text.len());
        } else {
            // Başlangıç etiketi.
            let mut cursor = index + 1;
            let name_start = cursor;
            while cursor < bytes.len() && !is_space(bytes[cursor]) && bytes[cursor] != b'/' && bytes[cursor] != b'>' {
                cursor += 1;
            }
            let name = text[name_start..cursor].to_string();
            let mut attrs = Vec::new();
            let mut self_closing = false;
            loop {
                while cursor < bytes.len() && is_space(bytes[cursor]) {
                    cursor += 1;
                }
                if cursor >= bytes.len() {
                    break;
                }
                match bytes[cursor] {
                    b'>' => {
                        cursor += 1;
                        break;
                    }
                    b'/' => {
                        self_closing = true;
                        cursor += 1;
                        continue;
                    }
                    _ => {}
                }
                let key_start = cursor;
                while cursor < bytes.len() && !is_space(bytes[cursor]) && !matches!(bytes[cursor], b'=' | b'>' | b'/') {
                    cursor += 1;
                }
                let key = text[key_start..cursor].to_string();
                while cursor < bytes.len() && is_space(bytes[cursor]) {
                    cursor += 1;
                }
                let mut value = String::new();
                if cursor < bytes.len() && bytes[cursor] == b'=' {
                    cursor += 1;
                    while cursor < bytes.len() && is_space(bytes[cursor]) {
                        cursor += 1;
                    }
                    if cursor < bytes.len() && (bytes[cursor] == b'"' || bytes[cursor] == b'\'') {
                        let quote = bytes[cursor];
                        cursor += 1;
                        let value_start = cursor;
                        while cursor < bytes.len() && bytes[cursor] != quote {
                            cursor += 1;
                        }
                        value = decode_entities(&text[value_start..cursor]);
                        cursor = (cursor + 1).min(bytes.len());
                    } else {
                        let value_start = cursor;
                        while cursor < bytes.len() && !is_space(bytes[cursor]) && bytes[cursor] != b'>' {
                            cursor += 1;
                        }
                        value = decode_entities(&text[value_start..cursor]);
                    }
                }
                // Anahtar yalnızca `=` ile başlayan bozuk öznitelikte boş kalır; `=` zaten tüketildiği
                // için ilerleme her zaman sağlanır ve imleç hep ASCII sınırında durur.
                if !key.is_empty() {
                    attrs.push((key, value));
                }
            }
            if !name.is_empty() {
                events.push(Event::Start { name, attrs, self_closing });
            }
            index = cursor;
        }
        if index >= text.len() {
            break;
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attributes_are_parsed_safely() {
        let xml = r#"<?xml version="1.0"?><!-- <domain name="yorum.com"> --><backup-info><domain name="ornek.com" note='a &amp; b &lt;x&gt;' tricky="a>b"><database name="db_1" type="mysql"/></domain></backup-info>"#;
        let events = scan(xml);
        let starts: Vec<(&str, &[(String, String)])> = events
            .iter()
            .filter_map(|event| match event {
                Event::Start { name, attrs, .. } => Some((name.as_str(), attrs.as_slice())),
                _ => None,
            })
            .collect();
        assert_eq!(starts.iter().map(|(name, _)| *name).collect::<Vec<_>>(), ["backup-info", "domain", "database"]);
        let domain = starts[1].1;
        assert_eq!(Event::attr(domain, "name"), Some("ornek.com"));
        assert_eq!(Event::attr(domain, "note"), Some("a & b <x>"));
        assert_eq!(Event::attr(domain, "tricky"), Some("a>b"));
        assert_eq!(Event::attr(starts[2].1, "type"), Some("mysql"));
        assert!(matches!(events.last(), Some(Event::End { name }) if name == "backup-info"));
        // Yorumun içindeki etiket görülmez.
        assert!(!events.iter().any(|event| matches!(event, Event::Start { attrs, .. } if Event::attr(attrs, "name") == Some("yorum.com"))));
    }

    #[test]
    fn entities_and_malformed_input() {
        assert_eq!(decode_entities("&#65;&#x42;&quot;&apos;&bilinmeyen; & &amp"), "AB\"'&bilinmeyen; & &amp");
        // Bozuk girdi paniğe ya da sonsuz döngüye yol açmaz.
        for text in ["<", "<a", "<a b", "<a b=", "<a b='x", "<a =\"x\">", "<!DOCTYPE x [ <!ENTITY e 'y'> ]><r a=\"&e;\"/>", "</", "<a b c d/>"] {
            let _ = scan(text);
        }
        let events = scan("<!DOCTYPE x [ <!ENTITY e 'y'> ]><r a=\"&e;\"/>");
        assert!(matches!(&events[0], Event::Start { name, attrs, self_closing: true } if name == "r" && attrs[0].1 == "&e;"));
    }
}
