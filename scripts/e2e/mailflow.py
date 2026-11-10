#!/usr/bin/env python3
"""Posta akışı sınama aracı (test makinesinde çalışır).

  mailflow.py send <oturum|-> <gönderen> <alıcı> <konu> [port]
  mailflow.py list <kullanıcı> [klasör]      -> konu satırları
  mailflow.py login <kullanıcı>              -> "ok" ya da "fail"

Parola MAIL_PASS ortam değişkeninden okunur.
"""
import imaplib
import os
import smtplib
import ssl
import sys
from email.header import decode_header, make_header
from email.message import EmailMessage

HOST = os.environ.get("MAIL_HOST", "127.0.0.1")
PASSWORD = os.environ.get("MAIL_PASS", "")
context = ssl._create_unverified_context()


def send(login, sender, recipient, subject, port=587):
    message = EmailMessage()
    message["From"], message["To"], message["Subject"] = sender, recipient, subject
    message.set_content("merhaba")
    try:
        if port == 465:
            smtp = smtplib.SMTP_SSL(HOST, port, context=context, timeout=20)
        else:
            smtp = smtplib.SMTP(HOST, port, timeout=20)
            if port != 25:
                smtp.starttls(context=context)
        with smtp:
            if login != "-":
                smtp.login(login, PASSWORD)
            smtp.send_message(message)
        return "sent"
    except Exception as error:  # noqa: BLE001
        return "rejected: " + str(error)[:120]


def subjects(user, folder="INBOX"):
    with imaplib.IMAP4_SSL(HOST, 993, ssl_context=context) as imap:
        imap.login(user, PASSWORD)
        status, _ = imap.select('"%s"' % folder, readonly=True)
        if status != "OK":
            return []
        _, data = imap.search(None, "ALL")
        found = []
        for number in data[0].split():
            _, parts = imap.fetch(number, "(BODY[HEADER.FIELDS (SUBJECT)])")
            raw = parts[0][1].decode("utf-8", "replace").strip()
            found.append(str(make_header(decode_header(raw.split(":", 1)[1].strip()))) if ":" in raw else raw)
        return found


if __name__ == "__main__":
    command = sys.argv[1]
    if command == "send":
        print(send(sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5], int(sys.argv[6]) if len(sys.argv) > 6 else 587))
    elif command == "list":
        print("\n".join(subjects(sys.argv[2], sys.argv[3] if len(sys.argv) > 3 else "INBOX")))
    elif command == "login":
        try:
            with imaplib.IMAP4_SSL(HOST, 993, ssl_context=context) as imap:
                imap.login(sys.argv[2], PASSWORD)
            print("ok")
        except Exception:  # noqa: BLE001
            print("fail")
