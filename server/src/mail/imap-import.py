#!/usr/bin/env python3
# PixPanel: bir posta kutusunu başka sunucudan IMAP ile içe aktarır.
#
# `vmail` kullanıcısıyla çalışır; ayarları (parola dahil) standart girdiden JSON olarak alır,
# iletileri kutunun Maildir klasörüne yazar. Karşı sunucudaki hiçbir şeyi değiştirmez (klasörler
# salt okunur açılır). Yeniden çalıştırılınca yalnızca yeni iletileri alır: klasör başına en son
# alınan UID kutunun klasöründeki durum dosyasında tutulur.
#
# Karşı sunucudan gelen her ad güvenilmezdir: klasör adları Maildir++ adına çevrilirken `/`, `..`
# ve denetim karakterleri temizlenir; ileti gövdesi yorumlanmadan dosyaya yazılır.

import base64
import hashlib
import imaplib
import json
import os
import re
import socket
import ssl
import sys
import time

imaplib._MAXLINE = 10 * 1024 * 1024
TIMEOUT = 120
MAX_MESSAGE = 150 * 1024 * 1024
SPECIAL = {"\\sent": "Sent", "\\drafts": "Drafts", "\\trash": "Trash", "\\junk": "Junk", "\\archive": "Archive"}
# Bütün iletileri yineleyen sanal klasörler (Gmail) alınmaz.
VIRTUAL = {"\\all", "\\flagged", "\\important", "\\noselect", "\\nonexistent"}
COMMON = {
    "sent": "Sent", "sent items": "Sent", "sent messages": "Sent", "sent mail": "Sent",
    "drafts": "Drafts", "draft": "Drafts",
    "trash": "Trash", "deleted items": "Trash", "deleted messages": "Trash",
    "junk": "Junk", "junk e-mail": "Junk", "spam": "Junk", "bulk mail": "Junk",
    "archive": "Archive", "archives": "Archive",
}
FLAG_LETTERS = {"\\seen": "S", "\\answered": "R", "\\flagged": "F", "\\draft": "D"}


def say(text):
    print(text, flush=True)


def fail(text):
    print(text, file=sys.stderr, flush=True)
    sys.exit(1)


def utf7_encode(name):
    """Klasör adını IMAP'in değiştirilmiş UTF-7 biçimine çevirir (ASCII adlar aynen kalır)."""
    out, buffer = [], []

    def flush():
        if buffer:
            raw = "".join(buffer).encode("utf-16-be")
            out.append("&" + base64.b64encode(raw).decode("ascii").rstrip("=").replace("/", ",") + "-")
            buffer.clear()

    for ch in name:
        if 0x20 <= ord(ch) <= 0x7E:
            flush()
            out.append("&-" if ch == "&" else ch)
        else:
            buffer.append(ch)
    flush()
    return "".join(out)


def quote(name):
    return '"' + name.replace("\\", "\\\\").replace('"', '\\"') + '"'


def parse_list_line(item):
    """LIST yanıtının bir satırı: (bayraklar, ayırıcı, ad). Çözülemezse None."""
    if isinstance(item, tuple):
        head, literal = item[0], item[1]
    else:
        head, literal = item, None
    match = re.match(rb'\((?P<flags>[^)]*)\) (?P<delim>"(?:[^"\\]|\\.)*"|NIL) ?(?P<name>.*)$', head, re.S)
    if not match:
        return None
    flags = {flag.lower() for flag in match.group("flags").decode("ascii", "replace").split()}
    delim = match.group("delim")
    delim = None if delim == b"NIL" else delim[1:-1].decode("ascii", "replace").replace("\\\\", "\\").replace('\\"', '"')
    if literal is not None:
        raw = literal
    else:
        raw = match.group("name").strip()
        if raw.startswith(b'"') and raw.endswith(b'"') and len(raw) >= 2:
            raw = re.sub(rb"\\(.)", rb"\1", raw[1:-1])
    try:
        name = raw.decode("ascii")
    except UnicodeDecodeError:
        name = raw.decode("utf-8", "replace")
    return flags, delim, name


def local_folder(flags, delim, name):
    """Karşıdaki klasörün Maildir++ adı; gelen kutusu için boş dize, alınmayacaksa None."""
    if name.upper() == "INBOX":
        return ""
    for flag, target in SPECIAL.items():
        if flag in flags:
            return target
    parts = name.split(delim) if delim else [name]
    if len(parts) > 1 and parts[0].upper() == "INBOX":
        parts = parts[1:]
    if len(parts) == 1 and parts[0].lower() in COMMON:
        return COMMON[parts[0].lower()]
    clean = []
    for part in parts:
        part = utf7_encode(part)
        part = re.sub(r"[\x00-\x1f\x7f/]", "_", part).replace(".", "_").strip()
        if not part or part.startswith("~"):
            return None
        clean.append(part)
    folder = ".".join(clean)
    if not folder or len(folder.encode()) > 200:
        return None
    return folder


def connect(cfg):
    host, ip, port = cfg["host"], cfg["ip"], int(cfg["port"])
    context = ssl.create_default_context()
    if not cfg.get("verify_tls", True):
        context.check_hostname = False
        context.verify_mode = ssl.CERT_NONE

    # Bağlantı, panelin doğruladığı adrese kurulur; sertifika yine sunucunun adıyla denetlenir.
    class Tls(imaplib.IMAP4_SSL):
        def _create_socket(self, timeout):
            sock = socket.create_connection((ip, self.port), TIMEOUT)
            return self.ssl_context.wrap_socket(sock, server_hostname=self.host)

    class Plain(imaplib.IMAP4):
        def _create_socket(self, timeout):
            return socket.create_connection((ip, self.port), TIMEOUT)

    try:
        if cfg["security"] == "ssl":
            conn = Tls(host, port, ssl_context=context, timeout=TIMEOUT)
        else:
            conn = Plain(host, port, timeout=TIMEOUT)
            conn.starttls(ssl_context=context)
    except ssl.SSLCertVerificationError:
        fail("Karşı sunucunun sertifikası doğrulanamadı. Sunucu adını denetleyin; sertifika kendinden imzalıysa “sertifikayı doğrulama” seçeneğini kullanın.")
    except (OSError, imaplib.IMAP4.error) as err:
        fail("Karşı sunucuya bağlanılamadı (%s). Adresi, bağlantı noktasını ve güvenlik türünü denetleyin." % type(err).__name__)

    user, password = cfg["username"], cfg["password"]
    try:
        try:
            user.encode("ascii"), password.encode("ascii")
            conn.login(user, password)
        except UnicodeEncodeError:
            conn.authenticate("PLAIN", lambda _: ("\0%s\0%s" % (user, password)).encode("utf-8"))
    except imaplib.IMAP4.error:
        fail("Karşı sunucu girişi reddetti. Kullanıcı adını ve parolayı denetleyin (bazı sağlayıcılar “uygulama parolası” ister).")
    return conn


def ensure_folder(maildir, folder):
    path = maildir if folder == "" else os.path.join(maildir, "." + folder)
    for sub in ("cur", "new", "tmp"):
        os.makedirs(os.path.join(path, sub), mode=0o700, exist_ok=True)
    if folder:
        subscriptions = os.path.join(maildir, "subscriptions")
        known = set()
        if os.path.isfile(subscriptions):
            with open(subscriptions, encoding="utf-8", errors="replace") as handle:
                known = {line.strip() for line in handle}
        if folder not in known:
            with open(subscriptions, "a", encoding="utf-8") as handle:
                handle.write(folder + "\n")
    return path


def fetch_message(conn, uid):
    """(bayrak harfleri, tarih, gövde) döner; ileti alınamazsa ya da silinmiş işaretliyse None."""
    typ, data = conn.uid("FETCH", str(uid), "(FLAGS INTERNALDATE BODY.PEEK[])")
    if typ != "OK" or not data:
        return None
    meta, body = b"", None
    for item in data:
        if isinstance(item, tuple):
            meta += item[0] + b" "
            if body is None:
                body = item[1]
        elif isinstance(item, bytes):
            meta += item + b" "
    if body is None or len(body) > MAX_MESSAGE:
        return None
    flags = {flag.decode("ascii", "replace").lower() for flag in imaplib.ParseFlags(meta)}
    if "\\deleted" in flags:
        return None
    letters = "".join(sorted(FLAG_LETTERS[flag] for flag in flags if flag in FLAG_LETTERS))
    when = imaplib.Internaldate2tuple(meta)
    stamp = time.mktime(when) if when else time.time()
    return letters, stamp, body.replace(b"\r\n", b"\n")


def main():
    cfg = json.load(sys.stdin)
    maildir = cfg["maildir"]
    home = os.path.dirname(maildir)
    os.umask(0o077)
    os.makedirs(home, mode=0o700, exist_ok=True)
    ensure_folder(maildir, "")

    state_dir = os.path.join(home, ".pixpanel-import")
    os.makedirs(state_dir, mode=0o700, exist_ok=True)
    key = hashlib.sha256(("%s|%s" % (cfg["host"], cfg["username"])).encode()).hexdigest()[:24]
    state_path = os.path.join(state_dir, key + ".json")
    state = {}
    if os.path.isfile(state_path):
        try:
            with open(state_path, encoding="utf-8") as handle:
                state = json.load(handle)
        except (OSError, ValueError):
            state = {}

    conn = connect(cfg)
    say("Bağlantı kuruldu, klasörler okunuyor")
    typ, listing = conn.list()
    if typ != "OK":
        fail("Karşı sunucudaki klasörler listelenemedi.")
    folders = []
    for item in listing:
        if item is None:
            continue
        parsed = parse_list_line(item)
        if not parsed:
            continue
        flags, delim, name = parsed
        if flags & VIRTUAL:
            continue
        target = local_folder(flags, delim, name)
        if target is None:
            say("Atlandı (ad desteklenmiyor): %s" % name[:80])
            continue
        folders.append((name, target))

    total, skipped, sequence = 0, 0, 0
    for name, target in folders:
        try:
            typ, _ = conn.select(quote(name), readonly=True)
        except imaplib.IMAP4.error:
            typ = "NO"
        if typ != "OK":
            say("Atlandı (açılamadı): %s" % name[:80])
            continue
        validity = (conn.response("UIDVALIDITY")[1] or [b""])[0]
        validity = validity.decode("ascii", "replace") if isinstance(validity, bytes) else ""
        record = state.get(name, {})
        last = record.get("last_uid", 0) if record.get("uidvalidity") == validity else 0
        typ, found = conn.uid("SEARCH", None, "UID", "%d:*" % (last + 1))
        uids = sorted(int(part) for part in (found[0] or b"").split() if part.isdigit() and int(part) > last) if typ == "OK" else []
        label = "Gelen kutusu" if target == "" else target
        if not uids:
            say("%s: yeni ileti yok" % label)
            continue
        path = ensure_folder(maildir, target)
        count = 0
        for uid in uids:
            try:
                message = fetch_message(conn, uid)
            except (imaplib.IMAP4.error, OSError) as err:
                # Bağlantı koptuysa o ana kadar alınanlar kaydedilir; yeniden çalıştırınca kaldığı yerden sürer.
                state[name] = {"uidvalidity": validity, "last_uid": last}
                with open(state_path, "w", encoding="utf-8") as handle:
                    json.dump(state, handle)
                fail("Bağlantı koptu (%s). %d ileti alındı; yeniden başlatırsanız kaldığı yerden sürer." % (type(err).__name__, total + count))
            if message is None:
                skipped += 1
                last = uid
                continue
            letters, stamp, body = message
            sequence += 1
            base = "%d.M%dP%dQ%d.pixpanel-import,S=%d" % (int(time.time()), int(time.time() * 1000000) % 1000000, os.getpid(), sequence, len(body))
            temporary = os.path.join(path, "tmp", base)
            with open(temporary, "wb") as handle:
                handle.write(body)
            os.utime(temporary, (stamp, stamp))
            os.rename(temporary, os.path.join(path, "cur", base + ":2," + letters))
            count += 1
            last = uid
            if count % 200 == 0:
                say("%s: %d / %d" % (label, count, len(uids)))
                state[name] = {"uidvalidity": validity, "last_uid": last}
                with open(state_path, "w", encoding="utf-8") as handle:
                    json.dump(state, handle)
        state[name] = {"uidvalidity": validity, "last_uid": last}
        with open(state_path, "w", encoding="utf-8") as handle:
            json.dump(state, handle)
        total += count
        say("%s: %d ileti alındı" % (label, count))

    try:
        conn.logout()
    except Exception:
        pass
    say("Toplam %d ileti alındı%s." % (total, (", %d ileti atlandı (silinmiş işaretli ya da çok büyük)" % skipped) if skipped else ""))


if __name__ == "__main__":
    main()
