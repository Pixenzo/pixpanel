#!/usr/bin/env python3
"""Sunucunun kullanıcıya gösterdiği Türkçe metinleri Rust kaynağından çıkarır.

Türkçe kaynak metin aynı zamanda çeviri anahtarıdır (arayüzdeki `t("…")` ile aynı mantık).
`format!` ile kurulan metinlerde yer tutucular anahtarda kalır: adlı olanlar `{domain}`,
adsız olanlar sırayla `{1}`, `{2}` olur; çeviride aynı adlarla kullanılır.

Kullanım:
  python scripts/extract-server-i18n.py            # eksik anahtarları JSON olarak yazar
  python scripts/extract-server-i18n.py --check    # eksik çeviri varsa hata koduyla çıkar
  python scripts/extract-server-i18n.py --all      # bulunan bütün anahtarları yazar
  python scripts/extract-server-i18n.py --unused   # sözlükte olup kaynakta bulunmayanlar
"""
import io
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "server", "src")
DICT = os.path.join(ROOT, "server", "i18n", "en.json")

TURKISH = re.compile(r"[çğıöşüÇĞİÖŞÜ]")
# Biçim dizesi alan makrolar: içlerindeki `{…}` yer tutucudur.
FORMAT_MACRO = re.compile(r"(format|bail|anyhow|ensure|write|writeln|println|eprintln|print|format_args)!\s*\($")
ERROR_ATTR = re.compile(r"#\[error\($")
# Türkçe harf içermese de ileti sayılan bağlamlar.
MESSAGE_CONTEXT = re.compile(r"(AppError::(bad|conflict|internal)|\.log|Finding::new|bail!|anyhow!)\s*\(\s*(format!\s*\(\s*)?$")
# Yalnızca sunucu günlüğüne giden ya da geliştiriciye dönük metinler.
SKIP_CONTEXT = re.compile(r"(tracing::\w+!|\b(trace|debug|info|warn|error)!|\.expect|panic!|unreachable!|assert(_eq|_ne)?!|debug_assert\w*!|todo!|unimplemented!)\s*\([^;{}]*$", re.S)
SKIP_OPEN = re.compile(r"(tracing::\w+!|\b(trace|debug|info|warn|error)!|\.expect|panic!|unreachable!|assert(_eq|_ne)?!|debug_assert\w*!|todo!|unimplemented!)\s*\(")
# Türkçeye özgü harf içermeyen Türkçe metinler ("Bekleyen sorunlar", "FTP sunucusu") bu sözcüklerle tanınır.
ASCII_MARKERS = set(
    """sunucusu bekleyen sorun sorunlar yok var zaten ve daha eklenti tema kutusu posta yedek yedekleme yedekler
    kurulumu eklendi eklenemedi silinemedi dolu bulgu sertifika sertifikalar siteleri bildirimi deneme hata bilinmeyen
    sonra dakika anahtar ekli onay bekliyor adres sorguyu reddetti bildirir hizmet portu hesap konteyner uygulama
    elle etkin olmayan kendisi panelin bu al siliniyor inceleme yedekte alan adi dosya eklentisi sunucu aktarma""".split()
)
ASCII_TEXT = re.compile(r"^[A-Za-z0-9 ,.:;()%'{}!?/+…–—“”\-\[\]]+$")
HOLE = re.compile(r"\{([^{}]*)\}")
# Diske yazılan yapılandırma dosyalarının ve betiklerin içeriği çevrilmez.
GENERATED_FILE = re.compile(r"(#|;|//|<\?php|\[Unit\]|\{BLOCK_START\}|UPDATE |SELECT |INSERT )")
# Komut satırı çıktısı sunucunun kendi dilinde kalır.
SKIP_FILES = {"cli.rs", "main.rs"}


def inside_skipped_macro(before):
    """Dize sabiti, günlük ya da doğrulama makrosunun hâlâ açık olan parantezinin içinde mi?"""
    match = None
    for match in SKIP_OPEN.finditer(before):
        pass
    if match is None:
        return False
    depth = 1
    for char in strip_strings(before[match.end() :]):
        if char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
            if depth == 0:
                return False
    return True


def strip_strings(code):
    """Parantez sayarken dize sabitlerinin içindeki parantezler sayılmasın."""
    return re.sub(r'"(\\.|[^"\\])*"', '""', code)


def ascii_turkish(text):
    """Türkçeye özgü harf içermeyen ama Türkçe olan metin mi? Tek sözcüklü küçük harfli değerler
    (klasör adı, tür kodu) sayılmaz."""
    if len(text) > 200 or not ASCII_TEXT.match(text) or text.startswith("/") or ".service" in text:
        return False
    words = re.findall(r"[A-Za-z]+", HOLE.sub("", text))
    if not any(word.lower() in ASCII_MARKERS for word in words):
        return False
    return len(words) + len(HOLE.findall(text)) >= 2 or text[:1].isupper()


def strip_tests(source):
    """Birim testleri dosyanın sonundadır; oradaki metinler kullanıcıya gösterilmez."""
    cut = source.find("#[cfg(test)]")
    return source if cut < 0 else source[:cut]


def literals(source):
    """Kaynaktaki dize sabitlerini (başlangıç konumu, ham içerik, ham mı) olarak verir."""
    i, n = 0, len(source)
    while i < n:
        c = source[i]
        if source.startswith("//", i):
            j = source.find("\n", i)
            i = n if j < 0 else j
        elif source.startswith("/*", i):
            j = source.find("*/", i)
            i = n if j < 0 else j + 2
        elif c == "'":
            # Karakter sabiti ya da ömür adı; `'"'` gibi sabitler dize sanılmasın.
            m = re.match(r"'(\\.[^']*|[^'\\])'", source[i:])
            i += m.end() if m else 1
        elif c == "r" and re.match(r'r#*"', source[i:]) and (i == 0 or not (source[i - 1].isalnum() or source[i - 1] == "_")):
            hashes = len(re.match(r"r(#*)", source[i:]).group(1))
            start = i + 1 + hashes + 1
            end = source.find('"' + "#" * hashes, start)
            if end < 0:
                return
            yield i, source[start:end], True
            i = end + 1 + hashes
        elif c == '"':
            j = i + 1
            while j < n and source[j] != '"':
                j += 2 if source[j] == "\\" else 1
            yield i, source[i + 1 : j], False
            i = j + 1
        else:
            i += 1


def unescape(raw):
    out, i = [], 0
    while i < len(raw):
        c = raw[i]
        if c != "\\":
            out.append(c)
            i += 1
            continue
        nxt = raw[i + 1] if i + 1 < len(raw) else ""
        if nxt == "\n":
            # Satır sonundaki ters bölü: satır sonu ve sonraki girinti atılır.
            i += 2
            while i < len(raw) and raw[i] in " \t\r\n":
                i += 1
            continue
        if nxt == "u":
            m = re.match(r"\\u\{([0-9a-fA-F]+)\}", raw[i:])
            out.append(chr(int(m.group(1), 16)))
            i += m.end()
            continue
        out.append({"n": "\n", "t": "\t", "r": "\r", "0": "\0", "\\": "\\", '"': '"', "'": "'"}.get(nxt, nxt))
        i += 2
    return "".join(out)


def to_key(text):
    """Biçim dizesini sözlük anahtarına çevirir; yer tutucu yoksa metni olduğu gibi verir."""
    counter = [0]

    def hole(match):
        name = match.group(1).split(":")[0].strip()
        if name == "" or name.isdigit():
            counter[0] += 1
            return "{%d}" % counter[0]
        return "{%s}" % name

    protected = text.replace("{{", "\x00").replace("}}", "\x01")
    return HOLE.sub(hole, protected).replace("\x00", "{").replace("\x01", "}")


def literal_length(key):
    return len(HOLE.sub("", key).strip())


def collect():
    found = {}
    for folder, _, files in os.walk(SRC):
        for name in sorted(files):
            if not name.endswith(".rs") or name in SKIP_FILES:
                continue
            path = os.path.join(folder, name)
            source = strip_tests(io.open(path, encoding="utf-8").read())
            for pos, raw, is_raw in literals(source):
                before = source[max(0, pos - 400) : pos]
                text = raw if is_raw else unescape(raw)
                in_message = bool(MESSAGE_CONTEXT.search(before))
                if not (TURKISH.search(text) or in_message or ascii_turkish(text)):
                    continue
                if inside_skipped_macro(before):
                    continue
                is_format = bool(FORMAT_MACRO.search(before.rstrip()) or ERROR_ATTR.search(before.rstrip()))
                if not is_format:
                    # Makronun ilk bağımsız değişkeni olmayan biçim dizeleri (`write!(out, "…")`, `ensure!(koşul, "…")`).
                    statement = re.split(r"[;{}]", before)[-1]
                    is_format = bool(re.search(r"\b(write|writeln|ensure)!\s*\([^\"]*$", statement))
                key = to_key(text) if is_format else text
                key = key.strip()
                if literal_length(key) < 3 or not re.search(r"[A-Za-zçğıöşüÇĞİÖŞÜ]{2}", key):
                    continue
                if GENERATED_FILE.match(key) or re.search(r"PixPanel tarafından (üretildi|yönetilir|yazıldı)", key):
                    continue
                found.setdefault(key, os.path.relpath(path, SRC).replace("\\", "/"))
    return found


def main():
    found = collect()
    dictionary = json.load(io.open(DICT, encoding="utf-8")) if os.path.exists(DICT) else {}
    mode = sys.argv[1] if len(sys.argv) > 1 else ""
    out = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8", newline="\n")
    if mode == "--all":
        json.dump(found, out, ensure_ascii=False, indent=1)
    elif mode == "--unused":
        json.dump(sorted(k for k in dictionary if k not in found), out, ensure_ascii=False, indent=1)
    else:
        missing = {k: v for k, v in found.items() if k not in dictionary}
        if mode == "--check":
            for key in missing:
                out.write("çevirisi yok: %s  (%s)\n" % (key.replace("\n", "\\n"), missing[key]))
            out.write("%d metin, %d eksik\n" % (len(found), len(missing)))
            out.flush()
            sys.exit(1 if missing else 0)
        json.dump(missing, out, ensure_ascii=False, indent=1)
    out.write("\n")
    out.flush()


if __name__ == "__main__":
    main()
