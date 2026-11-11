# {{domain}} için PixPanel'in bıraktığı örnek uygulama.
# Kendi uygulamanızı yükleyip bu dosyayı silebilirsiniz. Uygulamanız PORT ortam
# değişkenindeki portu dinlemelidir; panel istekleri o porta iletir.
#
# Gerçek bir uygulamada bağımlılıklarınızı requirements.txt dosyasına yazın, panelden
# "Bağımlılıkları kur" deyin ve başlatma komutunu örneğin şöyle ayarlayın:
#   gunicorn app:app --bind 127.0.0.1:$PORT        (Flask, Django)
#   uvicorn app:app --host 127.0.0.1 --port $PORT  (FastAPI)
import os
import platform
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PAGE = """<!doctype html>
<html lang="tr">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{{domain}}</title>
<style>
  body { margin: 0; min-height: 100vh; display: grid; place-items: center; font-family: system-ui, sans-serif; background: #f2f5f8; color: #15202b; }
  main { max-width: 34rem; padding: 2rem; }
  h1 { font-size: 1.5rem; margin: 0 0 .5rem; }
  p { margin: 0; line-height: 1.5; color: #485868; }
  code { font-family: ui-monospace, monospace; color: #0b6b86; }
</style>
</head>
<body>
<main>
  <h1>{{domain}} yayında</h1>
  <p>Python %s çalışıyor. Uygulamanızı site klasörüne yükleyin ve başlatma komutunu panelden ayarlayın; bu örnek <code>app.py</code> dosyasını silebilirsiniz.</p>
</main>
</body>
</html>""" % platform.python_version()


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = PAGE.encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    address = (os.environ.get("HOST", "127.0.0.1"), int(os.environ.get("PORT", "8000")))
    ThreadingHTTPServer(address, Handler).serve_forever()
