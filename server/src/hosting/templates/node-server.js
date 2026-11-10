// {{domain}} için PixPanel'in bıraktığı örnek uygulama.
// Kendi uygulamanızı yükleyip bu dosyayı silebilirsiniz. Uygulamanız PORT ortam
// değişkenindeki portu dinlemelidir; panel istekleri o porta iletir.
const http = require("node:http");

const port = Number(process.env.PORT || 3000);
const host = process.env.HOST || "127.0.0.1";

const page = `<!doctype html>
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
  <p>Node.js ${process.version} çalışıyor. Uygulamanızı site klasörüne yükleyin ve başlatma komutunu panelden ayarlayın; bu örnek <code>server.js</code> dosyasını silebilirsiniz.</p>
</main>
</body>
</html>`;

http
  .createServer((request, response) => {
    response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    response.end(page);
  })
  .listen(port, host, () => console.log(`Dinleniyor: http://${host}:${port}`));
