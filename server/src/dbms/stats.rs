//! Veritabanı boyutları ve tablo sayıları.
//!
//! Her sayfa açılışında sunucuya sorgu atılmasın diye sonuç kısa süre bellekte tutulur.
//! Veritabanı açılıp kapanınca ya da içe aktarma/kopyalama bitince önbellek boşaltılır;
//! saatlik disk ölçümü (bkz. `quota::measure_disk`) her zaman taze değer ister.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{mariadb, postgres, Engine};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DbStat {
    /// Veri + dizin boyutu (bayt).
    pub size: u64,
    pub tables: u64,
}

/// Veritabanı adı → boyut ve tablo sayısı.
pub type Stats = HashMap<String, DbStat>;

/// Önbelleğin geçerlilik süresi.
const TTL: Duration = Duration::from_secs(300);

static CACHE: Mutex<Option<HashMap<Engine, (Instant, Stats)>>> = Mutex::new(None);
/// Aynı anda tek ölçüm yürüsün; sayfayı art arda açanlar sunucuyu yormasın.
static LOADING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn cached(engine: Engine) -> Option<Stats> {
    let cache = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (at, stats) = cache.as_ref()?.get(&engine)?;
    (at.elapsed() < TTL).then(|| stats.clone())
}

fn store(engine: Engine, stats: &Stats) {
    let mut cache = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.get_or_insert_with(HashMap::new).insert(engine, (Instant::now(), stats.clone()));
}

/// Önbelleği boşaltır (veritabanı açıldı, silindi ya da içeriği değişti).
pub fn invalidate(engine: Engine) {
    let mut cache = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(map) = cache.as_mut() {
        map.remove(&engine);
    }
}

async fn load(engine: Engine) -> Option<Stats> {
    let result = match engine {
        Engine::Mariadb => mariadb::stats().await,
        Engine::Postgresql => postgres::stats().await,
    };
    match result {
        Ok(stats) => {
            store(engine, &stats);
            Some(stats)
        }
        Err(err) => {
            tracing::warn!(engine = engine.as_str(), error = %err, "veritabanı boyutları okunamadı");
            None
        }
    }
}

/// Önbellekteki (yoksa yeni ölçülen) değerler. Okunamazsa boş döner.
pub async fn get(engine: Engine) -> Stats {
    if let Some(hit) = cached(engine) {
        return hit;
    }
    let _loading = LOADING.lock().await;
    // Beklerken başka bir istek ölçümü bitirmiş olabilir.
    if let Some(hit) = cached(engine) {
        return hit;
    }
    load(engine).await.unwrap_or_default()
}

/// Önbelleğe bakmadan yeniden ölçer.
pub async fn refresh(engine: Engine) -> Stats {
    let _loading = LOADING.lock().await;
    load(engine).await.unwrap_or_default()
}

/// Sorgu çıktısındaki `ad<TAB>boyut<TAB>tablo sayısı` satırlarını ayrıştırır.
pub fn parse_rows(output: &str) -> Stats {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let name = parts.next()?.trim();
            let size = parts.next()?.trim().parse::<f64>().ok()?;
            let tables = parts.next().and_then(|value| value.trim().parse::<u64>().ok()).unwrap_or(0);
            (!name.is_empty()).then(|| (name.to_string(), DbStat { size: size.max(0.0) as u64, tables }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_parsed() {
        let stats = parse_rows("blog\t16384\t3\nbos\t0\t0\nbozuk satir\nsayi\tx\t1\nondalik\t1024.0\t2\n");
        assert_eq!(stats["blog"], DbStat { size: 16384, tables: 3 });
        assert_eq!(stats["bos"], DbStat { size: 0, tables: 0 });
        assert_eq!(stats["ondalik"], DbStat { size: 1024, tables: 2 });
        assert!(!stats.contains_key("sayi"));
        assert_eq!(stats.len(), 3);
    }
}
