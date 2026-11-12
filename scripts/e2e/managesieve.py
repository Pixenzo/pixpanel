#!/usr/bin/env python3
"""Küçük bir ManageSieve istemcisi (test makinesinde çalışır; yalnız 127.0.0.1:4190, STARTTLS).

  managesieve.py <kullanıcı> list                    -> betik adları (etkin olanın sonunda " ACTIVE")
  managesieve.py <kullanıcı> get <ad>                -> betik içeriği
  managesieve.py <kullanıcı> put <ad> <dosya|->      -> "ok" ya da "no: ..."
  managesieve.py <kullanıcı> activate <ad>           -> "ok" ya da "no: ..."
  managesieve.py <kullanıcı> delete <ad>             -> "ok" ya da "no: ..."
  managesieve.py - ping                              -> 4190 dinleniyor ve STARTTLS bildiriyor mu ("ok"/"fail")

Parola MAIL_PASS ortam değişkeninden okunur; ana makine MANAGESIEVE_HOST ile değiştirilebilir
(dışarıdan erişimin kapalı olduğunu sınamak için).
"""
import base64
import os
import re
import socket
import ssl
import sys

HOST = os.environ.get("MANAGESIEVE_HOST", "127.0.0.1")
PASSWORD = os.environ.get("MAIL_PASS", "")


class Client:
    def __init__(self):
        self.sock = socket.create_connection((HOST, 4190), timeout=15)
        self.reader = self.sock.makefile("rb")

    def read_response(self):
        """Yanıtı OK/NO/BYE satırına kadar okur; (durum, satırlar, sabit uzunluklu veriler) döner."""
        lines, literals = [], []
        while True:
            raw = self.reader.readline()
            if not raw:
                return "BYE", lines, literals
            line = raw.decode("utf-8", "replace").rstrip("\r\n")
            match = re.fullmatch(r".*\{(\d+)\+?\}", line)
            if match:
                size = int(match.group(1))
                data = self.reader.read(size)
                literals.append(data.decode("utf-8", "replace"))
                self.reader.readline()
                lines.append(line)
                continue
            lines.append(line)
            for status in ("OK", "NO", "BYE"):
                if line == status or line.startswith(status + " "):
                    return status, lines, literals

    def send(self, text):
        self.sock.sendall(text.encode("utf-8"))

    def starttls(self):
        self.read_response()
        self.send("STARTTLS\r\n")
        status, _, _ = self.read_response()
        if status != "OK":
            raise RuntimeError("STARTTLS reddedildi")
        context = ssl._create_unverified_context()
        self.sock = context.wrap_socket(self.sock, server_hostname=HOST)
        self.reader = self.sock.makefile("rb")
        self.read_response()

    def login(self, user):
        token = base64.b64encode(("\0%s\0%s" % (user, PASSWORD)).encode()).decode()
        self.send('AUTHENTICATE "PLAIN" "%s"\r\n' % token)
        status, lines, _ = self.read_response()
        if status != "OK":
            raise RuntimeError("giriş reddedildi: " + " ".join(lines)[:120])

    def command(self, text):
        self.send(text + "\r\n")
        return self.read_response()


def quote(value):
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def main():
    user, action = sys.argv[1], sys.argv[2]
    if action == "ping":
        try:
            client = Client()
            _, lines, _ = client.read_response()
            print("ok" if any("STARTTLS" in line for line in lines) else "fail")
        except Exception:  # noqa: BLE001
            print("fail")
        return
    try:
        client = Client()
        client.starttls()
        client.login(user)
    except Exception as error:  # noqa: BLE001
        print("no: " + str(error)[:160])
        return
    if action == "list":
        _, lines, _ = client.command("LISTSCRIPTS")
        for line in lines:
            match = re.match(r'^"((?:[^"\\]|\\.)*)"( ACTIVE)?$', line)
            if match:
                print(match.group(1) + (" ACTIVE" if match.group(2) else ""))
    elif action == "get":
        status, lines, literals = client.command("GETSCRIPT " + quote(sys.argv[3]))
        print(literals[0] if status == "OK" and literals else "no: " + " ".join(lines)[:160])
    elif action == "put":
        source = sys.stdin.read() if sys.argv[4] == "-" else open(sys.argv[4], encoding="utf-8").read()
        data = source.encode("utf-8")
        client.send("PUTSCRIPT %s {%d+}\r\n" % (quote(sys.argv[3]), len(data)))
        client.sock.sendall(data + b"\r\n")
        status, lines, _ = client.read_response()
        print("ok" if status == "OK" else "no: " + " ".join(lines)[:160])
    elif action in ("activate", "delete"):
        verb = "SETACTIVE" if action == "activate" else "DELETESCRIPT"
        status, lines, _ = client.command(verb + " " + quote(sys.argv[3]))
        print("ok" if status == "OK" else "no: " + " ".join(lines)[:160])
    client.command("LOGOUT")


if __name__ == "__main__":
    main()
