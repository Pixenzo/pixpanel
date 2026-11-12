#!/usr/bin/env python3
"""API belgesi ile koddaki rotaları karşılaştırır.

`server/src/api/mod.rs` içindeki bağlama öneklerini ve her modülün `router()` işlevindeki
`.route("yol", get(...).post(...))` çağrılarını ayrıştırır; sonucu `docs/api.md` içindeki
uç tablolarıyla (`| YÖNTEM | `/yol` | ... |` satırları) karşılaştırır.

Kullanım:
    python scripts/check-api-docs.py           # fark varsa 1 ile çıkar
    python scripts/check-api-docs.py --list    # koddaki bütün uçları yazar (belge yazarken yardımcı)

Parametre adları karşılaştırmada önemsizdir (`{id}` ile `{site}` aynı sayılır); belgede
okunaklı ad kullanılabilir.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
API_DIR = ROOT / "server" / "src" / "api"
DOC = ROOT / "docs" / "api.md"
API_PREFIX = "/api/v1"
METHODS = ("get", "post", "put", "patch", "delete")


def strip_comments(text: str) -> str:
    """Tam satır yorumlarını atar (yorumların içindeki parantezler sayımı bozmasın)."""
    return "\n".join(line for line in text.splitlines() if not line.lstrip().startswith("//"))


def matching_paren(text: str, start: int) -> int:
    """`text[start]` bir `(` ise eşleşen `)` konumunu döner. Dizgi değişmezlerini atlar."""
    depth = 0
    index = start
    while index < len(text):
        char = text[index]
        if char == '"':
            index += 1
            while text[index] != '"':
                index += 2 if text[index] == "\\" else 1
        elif char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
            if depth == 0:
                return index
        index += 1
    raise ValueError("eşleşmeyen parantez")


def calls(text: str):
    """Bir zincirdeki `.ad(argümanlar)` çağrılarını (ad, argüman metni) olarak sıralar."""
    index = 0
    while True:
        match = re.compile(r"\.([A-Za-z_]+)\s*\(").search(text, index)
        if not match:
            return
        open_at = match.end() - 1
        close_at = matching_paren(text, open_at)
        yield match.group(1), text[open_at + 1 : close_at]
        index = close_at + 1


def function_body(text: str, name: str) -> str | None:
    match = re.search(r"pub fn " + name + r"\(\)\s*->\s*Router<AppState>\s*\{", text)
    if not match:
        return None
    depth = 1
    index = match.end()
    while depth:
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
        index += 1
    return text[match.end() : index - 1]


def module_routes(module: str, function: str) -> list[tuple[str, str]]:
    """Bir modülün yönlendiricisindeki (yol, yöntem) çiftleri; yol modüle göredir."""
    text = strip_comments((API_DIR / f"{module}.rs").read_text(encoding="utf-8"))
    body = function_body(text, function)
    if body is None:
        raise SystemExit(f"{module}.rs içinde {function}() bulunamadı")
    found = []
    for name, args in calls(body):
        if name != "route":
            continue
        path = re.match(r'\s*"([^"]*)"\s*,', args)
        if not path:
            raise SystemExit(f"{module}.rs: rota yolu okunamadı: {args[:60]}")
        handlers = args[path.end() :]
        for method in re.findall(r"\b(" + "|".join(METHODS) + r")\s*\(", handlers):
            found.append((path.group(1), method.upper()))
    return found


def join_path(prefix: str, path: str) -> str:
    joined = prefix.rstrip("/") + ("" if path in ("", "/") else path)
    return joined or "/"


def expression_routes(expr: str, prefix: str) -> list[tuple[str, str]]:
    """`modül::router()` ile başlayan, `.merge(..)`/`.nest("/p", ..)` zincirini açar."""
    base = re.match(r"\s*([a-z0-9_]+)::(router|hooks_router)\(\)", expr)
    if not base:
        raise SystemExit(f"mod.rs: tanınmayan ifade: {expr[:60]}")
    result = [(join_path(prefix, path), method) for path, method in module_routes(base.group(1), base.group(2))]
    for name, args in calls(expr[base.end() :]):
        if name == "merge":
            result += expression_routes(args, prefix)
        elif name == "nest":
            nested = re.match(r'\s*"([^"]*)"\s*,', args)
            result += expression_routes(args[nested.end() :], join_path(prefix, nested.group(1)))
    return result


def code_routes() -> set[tuple[str, str]]:
    text = strip_comments((API_DIR / "mod.rs").read_text(encoding="utf-8"))
    body = function_body(text, "router")
    routes: list[tuple[str, str]] = []
    for name, args in calls(body):
        if name == "nest":
            nested = re.match(r'\s*"([^"]*)"\s*,', args)
            routes += expression_routes(args[nested.end() :], nested.group(1))
    return {(method, normalize(path)) for path, method in routes}


def normalize(path: str) -> str:
    """Parametre adlarını yok sayar: `{id}` -> `{}`."""
    return re.sub(r"\{[^}]*\}", "{}", path)


def doc_routes() -> set[tuple[str, str]]:
    found = set()
    for line in DOC.read_text(encoding="utf-8").splitlines():
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) < 2 or not line.lstrip().startswith("|"):
            continue
        method, path = cells[0], cells[1]
        if method.upper() in {m.upper() for m in METHODS} and path.startswith("`/") and path.endswith("`"):
            found.add((method.upper(), normalize(path.strip("`"))))
    return found


def main() -> int:
    if sys.stdout.encoding and sys.stdout.encoding.lower() != "utf-8":
        sys.stdout.reconfigure(encoding="utf-8")
    code = code_routes()
    if "--list" in sys.argv:
        for method, path in sorted(code, key=lambda item: (item[1], item[0])):
            print(f"{method:6} {path}")
        return 0
    documented = doc_routes()
    missing = sorted(code - documented, key=lambda item: (item[1], item[0]))
    extra = sorted(documented - code, key=lambda item: (item[1], item[0]))
    print(f"Koddaki uç: {len(code)}, belgedeki uç: {len(documented)}")
    if missing:
        print(f"\nBelgede EKSİK ({len(missing)}):")
        for method, path in missing:
            print(f"  {method:6} {path}")
    if extra:
        print(f"\nBelgede FAZLA, kodda yok ({len(extra)}):")
        for method, path in extra:
            print(f"  {method:6} {path}")
    if missing or extra:
        print("\nFark var: docs/api.md dosyasını güncelleyin.")
        return 1
    print("Belge ile kod uyuşuyor.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
