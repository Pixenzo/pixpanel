"""İngilizce sözlüğe çeviri ekler.

Kullanım: python scripts/add-translations.py <çeviriler.json>

JSON dosyası `{"Türkçe metin": "English text", ...}` biçimindedir. İsteğe bağlı
`"__extra__": {"web/src/dosya.tsx": ["metin", ...]}` anahtarı, kodda `t(değişken)` ile
çevrildiği için kendiliğinden bulunamayan metinleri ek listeye (`i18n-extra.json`) yazar.
Var olan anahtarlara dokunulmaz. Ardından `node scripts/extract-i18n.mjs --check` çalıştırın.
"""

import io
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DICTIONARY = os.path.join(HERE, "..", "src", "lib", "locales", "en.ts")
EXTRA = os.path.join(HERE, "i18n-extra.json")


def main() -> None:
    additions = json.load(io.open(sys.argv[1], encoding="utf-8"))
    extra_additions = additions.pop("__extra__", {})

    text = io.open(DICTIONARY, encoding="utf-8").read().rstrip()
    if not text.endswith("};"):
        raise SystemExit("sözlük dosyası beklenen biçimde bitmiyor")
    body = text[:-2].rstrip()
    lines = []
    for key, value in additions.items():
        quoted = json.dumps(key, ensure_ascii=False)
        if f"  {quoted}:" in body:
            continue
        lines.append(f"  {quoted}: {json.dumps(value, ensure_ascii=False)},")
    if lines:
        io.open(DICTIONARY, "w", encoding="utf-8", newline="\n").write(body + "\n" + "\n".join(lines) + "\n};\n")

    if extra_additions:
        extra = json.load(io.open(EXTRA, encoding="utf-8"))
        for source, texts in extra_additions.items():
            known = extra.setdefault(source, [])
            known.extend(item for item in texts if item not in known)
        io.open(EXTRA, "w", encoding="utf-8", newline="\n").write(json.dumps(extra, ensure_ascii=False, indent=2) + "\n")

    print(f"sözlüğe eklenen: {len(lines)}; ek listeye işlenen kaynak: {len(extra_additions)}")


if __name__ == "__main__":
    main()
