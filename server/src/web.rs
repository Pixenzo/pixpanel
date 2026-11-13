//! HTTP uygulaması: API, gömülü arayüz dosyaları ve güvenlik başlıkları.

use axum::extract::{Request, State};
use axum::http::header::{self, HeaderValue};
use axum::http::{StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;
use rust_embed::Embed;
use tower_http::compression::CompressionLayer;

use crate::api;
use crate::error::AppError;
use crate::state::AppState;

/// Derlenmiş arayüz dosyaları ikili dosyanın içine gömülür.
#[derive(Embed)]
#[folder = "../web/dist"]
struct Assets;

pub fn app(state: AppState) -> Router {
    Router::new()
        .nest("/api/v1", api::router().fallback(|| async { AppError::NotFound }).layer(middleware::from_fn(translate_response)))
        .route("/tools/{*path}", axum::routing::any(tools_proxy))
        .route("/webmail", axum::routing::get(|| async { axum::response::Redirect::permanent("/webmail/") }))
        .route("/webmail/", axum::routing::any(webmail_proxy))
        .route("/webmail/{*path}", axum::routing::any(webmail_proxy))
        .fallback(static_file)
        .layer(middleware::from_fn(security_headers))
        .layer(CompressionLayer::new())
        .with_state(state)
}

/// Çevrilecek JSON yanıtının üst sınırı; daha büyüğü (dosya içeriği, uzun listeler) olduğu gibi geçer.
const MAX_TRANSLATED_BODY: usize = 4 * 1024 * 1024;

/// API yanıtındaki sunucu metinlerini hesabın diline çevirir (bkz. `i18n`). Dil, isteği yapan
/// hesabın kendi ayarıdır; oturum yoksa ya da hesabın dili henüz belirlenmemişse isteğin
/// `Accept-Language` başlığına bakılır. Kod her yerde
/// Türkçe metin üretir; çeviri burada, yanıt istemciye giderken yapılır. Akışlar
/// (`text/event-stream`) ve JSON olmayan yanıtlar bu katmandan değişmeden geçer.
async fn translate_response(mut request: Request, next: Next) -> Response {
    let header_language = crate::i18n::request_language(request.headers());
    let slot = crate::i18n::LanguageSlot::default();
    request.extensions_mut().insert(slot.clone());
    // Bileşen listesinde `name` alanı sunucunun kendi metnidir ("Roundcube web postası").
    let extra_keys: &[&str] = if request.uri().path().contains("/components") { &["name"] } else { &[] };
    let response = next.run(request).await;
    let language = slot.get().unwrap_or(header_language);
    if language == "tr" {
        return response;
    }
    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    if !is_json {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_TRANSLATED_BODY).await {
        Ok(bytes) => bytes,
        // Sınırı aşan gövde yeniden okunamaz; istemciye anlaşılır bir hata döner.
        Err(_) => return AppError::internal("Yanıt çevrilemeyecek kadar büyük").into_response(),
    };
    let translated = serde_json::from_slice::<serde_json::Value>(&bytes).ok().and_then(|mut value| {
        crate::i18n::translate_json_with(language, &mut value, extra_keys).then(|| serde_json::to_vec(&value).ok()).flatten()
    });
    match translated {
        Some(body) => {
            parts.headers.remove(header::CONTENT_LENGTH);
            Response::from_parts(parts, axum::body::Body::from(body))
        }
        None => Response::from_parts(parts, axum::body::Body::from(bytes)),
    }
}

/// Veritabanı araçlarına (phpMyAdmin, Adminer) giden istekleri, panel oturumu
/// olan kullanıcılar için sunucunun içindeki araç sunucusuna aktarır.
async fn tools_proxy(State(state): State<AppState>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    if !crate::auth::session_is_valid(&parts, &state) {
        return (StatusCode::UNAUTHORIZED, "Bu sayfa için panelde oturum açmanız gerekiyor.").into_response();
    }
    forward_to_tools(parts, body, "/tools/", TOOLS_BODY_LIMIT).await
}

/// Web postası (Roundcube) herkese açıktır: posta kutusu sahipleri panel hesabı
/// olmadan, kendi adres ve parolalarıyla girer. Kimlik doğrulamayı Roundcube yapar.
async fn webmail_proxy(request: Request) -> Response {
    if !crate::dbtools::installed().roundcube {
        return (StatusCode::NOT_FOUND, "Web postası bu sunucuda kurulu değil.").into_response();
    }
    let (parts, body) = request.into_parts();
    forward_to_tools(parts, body, "/webmail/", WEBMAIL_BODY_LIMIT).await
}

/// Oturumlu araçlara (veritabanı dökümü yükleme) izin verilen gövde boyutu.
const TOOLS_BODY_LIMIT: u64 = 1024 * 1024 * 1024;
/// Web postası oturumsuz erişilir; gövde bir e-postanın ekleriyle sınırlıdır.
const WEBMAIL_BODY_LIMIT: u64 = 64 * 1024 * 1024;

/// İsteği sunucunun içindeki araç sunucusuna aktarır ve yanıtını akıtarak döner.
///
/// `prefix`: iletilen yolun kalması gereken yer. Hedef adres çözümlenirken `..` parçaları
/// sadeleşir; denetlenmeseydi oturumsuz erişilen `/webmail/../tools/…` yolu, oturum isteyen
/// veritabanı araçlarına ulaşırdı. Bu yüzden denetim, sadeleşmiş adres üzerinde yapılır.
async fn forward_to_tools(parts: axum::http::request::Parts, body: axum::body::Body, prefix: &str, body_limit: u64) -> Response {
    let path_and_query = parts.uri.path_and_query().map(|value| value.as_str()).unwrap_or("/");
    let target = format!("http://127.0.0.1:{}{path_and_query}", crate::dbtools::PORT);
    let stays_inside = url::Url::parse(&target).is_ok_and(|url| {
        let path = url.path().to_ascii_lowercase();
        url.path().starts_with(prefix) && !path.contains("%2e") && !path.contains("%2f") && !path.contains("%5c") && !path.contains('\\')
    });
    if !stays_inside {
        return StatusCode::NOT_FOUND.into_response();
    }
    // Bildirilen boyut sınırı aşıyorsa istek hemen geri çevrilir. Boyut bildirilmeyen (parçalı ya da
    // HTTP/2) gövde ise akarken sayılır ve sınır aşılınca kesilir (aşağıda).
    let declared = parts.headers.get(header::CONTENT_LENGTH).and_then(|value| value.to_str().ok()).and_then(|value| value.parse::<u64>().ok());
    if declared.is_some_and(|length| length > body_limit) {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    let host = parts.headers.get(header::HOST).and_then(|value| value.to_str().ok()).unwrap_or_default().to_string();

    let client = match reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build() {
        Ok(client) => client,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    // Gerçek adres bağlantının kendisinden alınır; istemcinin gönderdiği başlıklara güvenilmez.
    let client_ip = crate::auth::client_ip(&parts);
    let mut forwarded = client.request(parts.method.clone(), target).header("x-forwarded-host", host).header("x-real-ip", client_ip);
    for (name, value) in &parts.headers {
        // Bağlantıya özgü başlıklar aktarılmaz; panelin oturum çerezi de araçlara sızdırılmaz.
        if matches!(name.as_str(), "host" | "connection" | "transfer-encoding" | "upgrade" | "x-forwarded-host" | "x-real-ip" | "x-forwarded-for" | "cookie") {
            continue;
        }
        forwarded = forwarded.header(name, value);
    }
    let panel_cookie = format!("{}=", crate::auth::SESSION_COOKIE);
    let cookies: Vec<&str> = parts
        .headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .map(str::trim)
        .filter(|pair| !pair.is_empty() && !pair.starts_with(&panel_cookie))
        .collect();
    if !cookies.is_empty() {
        forwarded = forwarded.header(header::COOKIE, cookies.join("; "));
    }
    let mut received: u64 = 0;
    let limited = futures_util::StreamExt::map(body.into_data_stream(), move |chunk| {
        let chunk = chunk.map_err(std::io::Error::other)?;
        received += chunk.len() as u64;
        if received > body_limit {
            return Err(std::io::Error::other("istek gövdesi sınırı aştı"));
        }
        Ok(chunk)
    });
    let upstream = match forwarded.body(reqwest::Body::wrap_stream(limited)).send().await {
        Ok(response) => response,
        Err(_) => return (StatusCode::BAD_GATEWAY, "Veritabanı aracına ulaşılamadı.").into_response(),
    };

    let mut response = Response::builder().status(upstream.status());
    for (name, value) in upstream.headers() {
        if !matches!(name.as_str(), "connection" | "transfer-encoding" | "keep-alive") {
            response = response.header(name, value);
        }
    }
    response
        .body(axum::body::Body::from_stream(upstream.bytes_stream()))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

async fn static_file(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Assets::get(path) {
        Some(file) => serve_asset(path, file),
        // Tek sayfalı uygulama: bilinmeyen yollar arayüzün yönlendiricisine bırakılır.
        None => match Assets::get("index.html") {
            Some(file) => serve_asset("index.html", file),
            None => (StatusCode::NOT_FOUND, "Arayüz dosyaları bulunamadı").into_response(),
        },
    }
}

fn serve_asset(path: &str, file: rust_embed::EmbeddedFile) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    // Vite çıktısındaki dosya adları içerik özetini taşır; süresiz önbelleklenebilir.
    let cache = if path.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
    (
        [(header::CONTENT_TYPE, mime.as_ref().to_string()), (header::CACHE_CONTROL, cache.to_string())],
        file.data,
    )
        .into_response()
}

async fn security_headers(request: Request, next: Next) -> Response {
    // Vekillenen araçlar kendi güvenlik başlıklarını gönderir; panelin katı içerik
    // ilkesi onların satır içi betiklerini engellerdi.
    let proxied_tool = request.uri().path().starts_with("/tools/");
    let webmail = request.uri().path().starts_with("/webmail");
    let mut response = next.run(request).await;
    if proxied_tool {
        return response;
    }
    let headers = response.headers_mut();
    let mut set = |name: &'static str, value: &'static str| {
        headers.insert(name, HeaderValue::from_static(value));
    };
    set("x-content-type-options", "nosniff");
    if webmail {
        // Roundcube kendi satır içi betiklerini kullanır; panelin içerik ilkesi uygulanmaz.
        set("x-frame-options", "SAMEORIGIN");
        return response;
    }
    set("x-frame-options", "DENY");
    set("referrer-policy", "same-origin");
    set(
        "content-security-policy",
        "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
         font-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'",
    );
    response
}
