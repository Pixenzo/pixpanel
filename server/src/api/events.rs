//! Canlı olay akışı (SSE). Arayüz tek bir bağlantıyla sunucu yükünü, görev
//! durumlarını ve kaynak değişikliklerini anlık alır.

use std::convert::Infallible;
use std::time::Duration;

use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::routing::get;
use axum::Router;
use futures_util::stream::{self, Stream, StreamExt};
use serde_json::json;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;

use crate::auth::AuthUser;
use crate::events;
use crate::state::AppState;

/// Akış bu süre sonunda kapanır; arayüz yeniden bağlanırken oturum baştan doğrulanır.
/// Böylece oturumu kapatılmış ya da yetkisi alınmış bir kullanıcı olay almayı sürdüremez.
const MAX_STREAM: Duration = Duration::from_secs(15 * 60);

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(stream_events))
}

async fn stream_events(user: AuthUser) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let hello = json!({ "type": "hello", "version": crate::update::VERSION }).to_string();
    let first = stream::once(async move { Ok(SseEvent::default().data(hello)) });

    let live = BroadcastStream::new(events::subscribe()).filter_map(move |item| {
        let line = match item {
            Ok(envelope) if envelope.visible_to(&user) => serde_json::to_string(&envelope.event).ok(),
            Ok(_) => None,
            // Dinleyici geride kaldı ve olay kaçırdı: arayüz her şeyi baştan sorgular.
            Err(BroadcastStreamRecvError::Lagged(_)) => Some(json!({ "type": "resync" }).to_string()),
        };
        async move { line.map(|data| Ok(SseEvent::default().data(data))) }
    });

    let stream = first.chain(live).take_until(tokio::time::sleep(MAX_STREAM));
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(20)))
}
