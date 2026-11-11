//! Site erişilebilirlik geçmişi: sağlık denetiminin dakikalık site yoklamalarının
//! sonucu saklanır; site sayfasında erişilebilirlik yüzdesi ve yanıt süresi olarak gösterilir.

use rusqlite::params;
use serde::Serialize;

use crate::db::{now, Db};
use crate::error::AppResult;

/// Yoklama kayıtları bu kadar süre saklanır.
const RETENTION_SECS: i64 = 7 * 86400;
/// Grafikte gösterilen dilim sayısı (24 saat için 5 dakikalık dilimler).
const BUCKETS: i64 = 288;

/// Bir yoklamanın sonucu. `status` HTTP durum kodudur; yanıt alınamadıysa 0.
#[derive(Debug, Clone, Copy)]
pub struct Check {
    pub site_id: i64,
    pub status: u16,
    pub millis: u32,
}

impl Check {
    /// Site yanıt verdi ve sunucu hatası dönmedi.
    pub fn ok(&self) -> bool {
        self.status != 0 && self.status < 500
    }
}

/// Siteyi sunucunun içinden yoklar ve HTTP durum kodunu döner.
///
/// `https`: site ziyaretçileri HTTPS'e zorluyorsa 80. port her zaman yönlendirme döner
/// ve uygulama çökse de "çalışıyor" görünür; bu yüzden asıl içerik 443. porttan istenir.
/// İstek yerel adrese gittiğinden sertifikanın adı ve geçerliliği burada doğrulanmaz
/// (sertifika ayrı bir denetimin konusudur).
pub async fn probe(domain: &str, https: bool, timeout: std::time::Duration) -> Result<u16, reqwest::Error> {
    // Yoklamalar kendini tanıtır; istatistik toplayıcı bu istekleri ziyaret saymaz.
    let builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(timeout).user_agent(crate::stats::PROBE_USER_AGENT);
    if https {
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], 443));
        let client = builder.resolve(domain, address).danger_accept_invalid_certs(true).build()?;
        return Ok(client.get(format!("https://{domain}/")).send().await?.status().as_u16());
    }
    let client = builder.build()?;
    Ok(client.get("http://127.0.0.1/").header("Host", domain).send().await?.status().as_u16())
}

/// Bir turun yoklamalarını kaydeder ve süresi geçmiş kayıtları siler.
pub fn record(db: &Db, checks: &[Check]) -> AppResult<()> {
    let at = now();
    db.call(|conn| {
        let tx = conn.transaction()?;
        {
            let mut insert = tx.prepare("INSERT INTO site_checks (site_id, checked_at, status, millis) VALUES (?1, ?2, ?3, ?4)")?;
            for check in checks {
                insert.execute(params![check.site_id, at, check.status, check.millis])?;
            }
        }
        tx.execute("DELETE FROM site_checks WHERE checked_at < ?1", params![at - RETENTION_SECS])?;
        tx.commit()
    })
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Bucket {
    /// Dilimin başlangıcı (Unix saniyesi).
    pub at: i64,
    /// Dilimdeki yoklama sayısı; 0 ise o aralıkta veri yok.
    pub checks: u32,
    pub failures: u32,
    /// Başarılı yoklamaların ortalama yanıt süresi (ms).
    pub millis: u32,
}

#[derive(Debug, Serialize)]
pub struct History {
    /// Son 24 saatte başarılı yoklamaların yüzdesi; veri yoksa `None`.
    pub uptime_day: Option<f64>,
    pub uptime_week: Option<f64>,
    /// Son 24 saatin ortalama yanıt süresi (ms).
    pub average_millis: Option<u32>,
    /// Son yoklamanın durumu.
    pub last_status: Option<u16>,
    pub last_checked_at: Option<i64>,
    /// Son 24 saat, eskiden yeniye.
    pub buckets: Vec<Bucket>,
}

fn percent(ok: i64, total: i64) -> Option<f64> {
    (total > 0).then(|| (ok as f64 / total as f64 * 1000.0).round() / 10.0)
}

/// Yoklamaları eşit aralıklı dilimlere böler. `rows`: (an, durum, ms), eskiden yeniye.
fn bucketize(rows: &[(i64, u16, u32)], start: i64, span: i64) -> Vec<Bucket> {
    let width = (span / BUCKETS).max(1);
    let mut buckets: Vec<Bucket> = (0..BUCKETS).map(|index| Bucket { at: start + index * width, checks: 0, failures: 0, millis: 0 }).collect();
    let mut totals = vec![0u64; BUCKETS as usize];
    for &(at, status, millis) in rows {
        let index = ((at - start) / width).clamp(0, BUCKETS - 1) as usize;
        let check = Check { site_id: 0, status, millis };
        buckets[index].checks += 1;
        if check.ok() {
            totals[index] += u64::from(millis);
        } else {
            buckets[index].failures += 1;
        }
    }
    for (bucket, total) in buckets.iter_mut().zip(totals) {
        let ok = bucket.checks - bucket.failures;
        bucket.millis = if ok > 0 { (total / u64::from(ok)) as u32 } else { 0 };
    }
    buckets
}

pub fn history(db: &Db, site_id: i64) -> AppResult<History> {
    let current = now();
    let day_start = current - 86400;
    let (rows, week): (Vec<(i64, u16, u32)>, (i64, i64)) = db.call(|conn| {
        let mut stmt = conn.prepare("SELECT checked_at, status, millis FROM site_checks WHERE site_id = ?1 AND checked_at >= ?2 ORDER BY checked_at")?;
        let rows = stmt.query_map(params![site_id, day_start], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let week = conn.query_row(
            "SELECT COALESCE(SUM(status BETWEEN 1 AND 499), 0), COUNT(*) FROM site_checks WHERE site_id = ?1 AND checked_at >= ?2",
            params![site_id, current - RETENTION_SECS],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok((rows, week))
    })?;
    let ok: Vec<&(i64, u16, u32)> = rows.iter().filter(|(_, status, millis)| Check { site_id, status: *status, millis: *millis }.ok()).collect();
    let average_millis = (!ok.is_empty()).then(|| (ok.iter().map(|(_, _, millis)| u64::from(*millis)).sum::<u64>() / ok.len() as u64) as u32);
    Ok(History {
        uptime_day: percent(ok.len() as i64, rows.len() as i64),
        uptime_week: percent(week.0, week.1),
        average_millis,
        last_status: rows.last().map(|(_, status, _)| *status),
        last_checked_at: rows.last().map(|(at, _, _)| *at),
        buckets: bucketize(&rows, day_start, 86400),
    })
}

/// Site kartları için: son 24 saatin erişilebilirliği ve son yanıt süresi.
pub fn glance(db: &Db, site_id: i64) -> AppResult<(Option<f64>, Option<u32>)> {
    db.call(|conn| {
        let (ok, total): (i64, i64) = conn.query_row(
            "SELECT COALESCE(SUM(status BETWEEN 1 AND 499), 0), COUNT(*) FROM site_checks WHERE site_id = ?1 AND checked_at >= ?2",
            params![site_id, now() - 86400],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let last: Option<u32> = conn
            .query_row("SELECT millis FROM site_checks WHERE site_id = ?1 AND status BETWEEN 1 AND 499 ORDER BY checked_at DESC LIMIT 1", params![site_id], |row| row.get(0))
            .ok();
        Ok((percent(ok, total), last))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_errors_and_no_response_count_as_down() {
        assert!(Check { site_id: 1, status: 200, millis: 10 }.ok());
        assert!(Check { site_id: 1, status: 404, millis: 10 }.ok(), "sayfa bulunamadı, site ayakta");
        assert!(Check { site_id: 1, status: 301, millis: 10 }.ok());
        assert!(!Check { site_id: 1, status: 502, millis: 10 }.ok());
        assert!(!Check { site_id: 1, status: 0, millis: 0 }.ok());
    }

    #[test]
    fn percent_is_rounded_to_one_decimal() {
        assert_eq!(percent(1439, 1440), Some(99.9));
        assert_eq!(percent(10, 10), Some(100.0));
        assert_eq!(percent(0, 0), None);
    }

    #[test]
    fn checks_fall_into_time_buckets() {
        let start = 1_000_000;
        // İlk beş dakikada iki başarılı (100 ve 300 ms) ve bir başarısız yoklama; son dilimde bir başarılı.
        let rows = vec![(start + 10, 200, 100), (start + 70, 200, 300), (start + 130, 502, 5), (start + 86_399, 200, 50)];
        let buckets = bucketize(&rows, start, 86400);
        assert_eq!(buckets.len(), 288);
        assert_eq!(buckets[0], Bucket { at: start, checks: 3, failures: 1, millis: 200 });
        assert_eq!(buckets[1].checks, 0);
        assert_eq!(buckets[287], Bucket { at: start + 287 * 300, checks: 1, failures: 0, millis: 50 });
    }
}
