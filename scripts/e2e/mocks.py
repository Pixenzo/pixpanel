#!/usr/bin/env python3
"""Bildirim sınamaları için sahte Telegram API'si (9911) ve SMTP alıcısı (2525).
Gelen her ileti /tmp/px-mock.log dosyasına yazılır."""
import json
import socketserver
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer

LOG = "/tmp/px-mock.log"


def log(kind, text):
    with open(LOG, "a", encoding="utf-8") as handle:
        handle.write(f"[{kind}] {text}\n")


class Telegram(BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0)))
        payload = json.loads(body or b"{}")
        log("telegram", f"{self.path.split('/')[-1]} chat={payload.get('chat_id')} text={payload.get('text', '')[:300]!r}")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.end_headers()
        self.wfile.write(b'{"ok":true}')

    def log_message(self, *args):
        pass


class Smtp(socketserver.StreamRequestHandler):
    def handle(self):
        self.wfile.write(b"220 mock ESMTP\r\n")
        in_data, lines = False, []
        for raw in self.rfile:
            line = raw.decode("utf-8", "replace").rstrip("\r\n")
            if in_data:
                if line == ".":
                    in_data = False
                    log("smtp", " | ".join(l for l in lines if l.lower().startswith(("to:", "subject:", "from:"))))
                    self.wfile.write(b"250 OK\r\n")
                else:
                    lines.append(line)
                continue
            verb = line[:4].upper()
            if verb in ("EHLO", "HELO"):
                self.wfile.write(b"250 mock\r\n")
            elif verb == "DATA":
                in_data, lines = True, []
                self.wfile.write(b"354 go\r\n")
            elif verb == "QUIT":
                self.wfile.write(b"221 bye\r\n")
                return
            else:
                self.wfile.write(b"250 OK\r\n")


socketserver.ThreadingTCPServer.allow_reuse_address = True
threading.Thread(target=socketserver.ThreadingTCPServer(("127.0.0.1", 2525), Smtp).serve_forever, daemon=True).start()
HTTPServer(("127.0.0.1", 9911), Telegram).serve_forever()
