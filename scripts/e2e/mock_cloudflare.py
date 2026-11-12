#!/usr/bin/env python3
"""Sınamalar için sahte Cloudflare API'si (127.0.0.1:9912).

Gerçek API'nin panelin kullandığı uçlarını taklit eder: anahtar doğrulama, bölge
listesi, DNS kayıtları (ekle/güncelle/sil), bölge ayarları ve önbellek temizleme.
Geçerli anahtar: TOKEN. Yapılan her yazma /tmp/px-cf.log dosyasına düşer.
"""
import json
import re
import uuid
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs, urlparse

TOKEN = "cf_test_token_0123456789abcdefghij"
ZONE_ID = "0123456789abcdef0123456789abcdef"
ZONES = [{"id": ZONE_ID, "name": "bulut.test"}, {"id": "f" * 32, "name": "baska.test"}]
RECORDS = {ZONE_ID: [], "f" * 32: []}
SETTINGS = {"ssl": "full", "development_mode": "off"}
LOG = "/tmp/px-cf.log"


def log(text):
    with open(LOG, "a", encoding="utf-8") as handle:
        handle.write(text + "\n")


def with_structured_data(body):
    """SRV/CAA gövdesini doğrular; `content` ve `priority` alanlarını gerçek API gibi `data`dan türetir."""
    kind = body.get("type")
    if kind not in ("SRV", "CAA"):
        if "content" not in body:
            raise ValueError("content is required")
        return body
    data = body.get("data")
    if not isinstance(data, dict) or "content" in body:
        raise ValueError("Invalid request: SRV and CAA records are written with a data object only")
    result = {k: v for k, v in body.items() if k != "data"}
    if kind == "SRV":
        for field in ("priority", "weight", "port"):
            if not isinstance(data.get(field), int):
                raise ValueError(f"data.{field} must be an integer")
        if not isinstance(data.get("target"), str) or not data["target"]:
            raise ValueError("data.target is required")
        result["content"] = f"{data['weight']} {data['port']} {data['target']}"
        result["priority"] = data["priority"]
    else:
        if not isinstance(data.get("flags"), int) or data.get("tag") not in ("issue", "issuewild", "iodef") or not data.get("value"):
            raise ValueError("invalid CAA data")
        result["content"] = f"{data['flags']} {data['tag']} \"{data['value']}\""
    result["data"] = data
    return result


class Handler(BaseHTTPRequestHandler):
    def reply(self, status, result=None, errors=None, info=None):
        body = {"success": errors is None, "errors": errors or [], "messages": [], "result": result}
        if info:
            body["result_info"] = info
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def handle_request(self, method):
        url = urlparse(self.path)
        path, query = url.path, parse_qs(url.query)
        length = int(self.headers.get("content-length", 0))
        body = json.loads(self.rfile.read(length) or b"{}") if length else {}
        if path == "/ips":
            return self.reply(200, {"ipv4_cidrs": ["173.245.48.0/20", "103.21.244.0/22", "103.22.200.0/22", "103.31.4.0/22", "141.101.64.0/18", "108.162.192.0/18", "190.93.240.0/20", "188.114.96.0/20"], "ipv6_cidrs": ["2400:cb00::/32", "2606:4700::/32", "2803:f800::/32"]})
        if self.headers.get("authorization") != f"Bearer {TOKEN}":
            return self.reply(403, errors=[{"code": 9109, "message": "Invalid access token"}])
        if path == "/user/tokens/verify":
            return self.reply(200, {"status": "active"})
        if path == "/zones":
            return self.reply(200, ZONES, info={"page": 1, "total_pages": 1})
        match = re.fullmatch(r"/zones/([0-9a-f]{32})/dns_records(?:/([0-9a-f]{32}))?", path)
        if match:
            zone, record_id = match.groups()
            records = RECORDS.setdefault(zone, [])
            if method == "GET":
                page = int(query.get("page", ["1"])[0])
                return self.reply(200, records if page == 1 else [], info={"page": page, "total_pages": 1})
            if method in ("POST", "PUT"):
                # Gerçek API gibi: SRV ve CAA `content` değil yapılandırılmış `data` ister.
                try:
                    body = with_structured_data(body)
                except ValueError as err:
                    return self.reply(400, errors=[{"code": 9005, "message": str(err)}])
            if method == "POST":
                if any(r["type"] == body["type"] and r["name"] == body["name"] and r["content"] == body["content"] for r in records):
                    return self.reply(400, errors=[{"code": 81058, "message": "An identical record already exists."}])
                record = {"id": uuid.uuid4().hex, **body}
                records.append(record)
                log(f"create {body['type']} {body['name']} {body['content'][:40]} proxied={body.get('proxied')} ttl={body.get('ttl')}")
                if "data" in body:
                    log(f"data {json.dumps(body['data'], sort_keys=True)}")
                return self.reply(200, record)
            existing = next((r for r in records if r["id"] == record_id), None)
            if existing is None:
                return self.reply(404, errors=[{"code": 81044, "message": "Record does not exist."}])
            if method == "PUT":
                existing.update(body)
                if "data" in body:
                    log(f"data {json.dumps(body['data'], sort_keys=True)}")
                log(f"update {body['type']} {body['name']} {body['content'][:40]} proxied={body.get('proxied')}")
                return self.reply(200, existing)
            if method == "DELETE":
                records.remove(existing)
                log(f"delete {existing['type']} {existing['name']}")
                return self.reply(200, {"id": record_id})
        match = re.fullmatch(r"/zones/([0-9a-f]{32})/settings/(ssl|development_mode)", path)
        if match:
            name = match.group(2)
            if method == "PATCH":
                SETTINGS[name] = body["value"]
                log(f"setting {name}={body['value']}")
            return self.reply(200, {"id": name, "value": SETTINGS[name]})
        if re.fullmatch(r"/zones/[0-9a-f]{32}/purge_cache", path) and method == "POST":
            log("purge everything" if body.get("purge_everything") else "purge partial")
            return self.reply(200, {"id": ZONE_ID})
        return self.reply(404, errors=[{"code": 7003, "message": "No route for that URI"}])

    def do_GET(self):
        self.handle_request("GET")

    def do_POST(self):
        self.handle_request("POST")

    def do_PUT(self):
        self.handle_request("PUT")

    def do_PATCH(self):
        self.handle_request("PATCH")

    def do_DELETE(self):
        self.handle_request("DELETE")

    def log_message(self, *args):
        pass


HTTPServer(("127.0.0.1", 9912), Handler).serve_forever()
