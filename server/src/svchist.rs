//! Servis ölçüm geçmişi: izlenen her systemd servisi için dakikada bir bellek, işlemci,
//! görev sayısı, durum ve yeniden başlama örneği alınır.
//!
//! Saklama düzeni sunucu ölçümleriyle (`sys::metrics`) aynıdır: dakikalık kayıtlar 48 saat,
//! onlardan her dakika yeniden hesaplanan saatlik ortalamalar 30 gün tutulur. Grafikler aralığa
//! göre seyreltilir (`bucketize`).
//!
//! Sağlık merkezi bu geçmişten iki şey çıkarır: kısa sürede çok kez yeniden başlayan servis ve
//! yedi gün boyunca düzenli büyüyen bellek (sızıntı şüphesi; bkz. `memory_trend`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::params;
use serde::Serialize;

use crate::db::{now, Db};
use crate::error::AppResult;
use crate::state::AppState;
use crate::sys::exec;

/// Dakikalık kayıtlar bu kadar saat, saatlik özetler bu kadar gün saklanır.
pub const MINUTE_RETENTION_HOURS: i64 = 48;
pub const HOUR_RETENTION_DAYS: i64 = 30;

/// Sızıntı ölçütü: en az bu kadar saatlik nokta ve bu kadar gün kapsam gerekir.
const LEAK_MIN_POINTS: usize = 72;
const LEAK_MIN_SPAN_SECS: i64 = 4 * 86400;
/// Uydurulan doğruya göre yedi gündeki artış, başlangıç değerinin en az bu oranı olmalı...
const LEAK_MIN_GROWTH_RATIO: f64 = 0.3;
/// ...ve en az bu kadar bayt (küçük servislerde birkaç MB'lık oynama sızıntı sayılmaz).
const LEAK_MIN_GROWTH_BYTES: f64 = 50.0 * 1_048_576.0;
/// Doğrusal uyum bu belirlilik katsayısının altındaysa artış "sürekli" sayılmaz
/// (düzenli yeniden başlayan servisin bellek grafiği testere dişi olur, uyumu düşüktür).
const LEAK_MIN_R2: f64 = 0.6;
/// Bir servis son 24 saatte bundan fazla yeniden başladıysa uyarılır.
pub const FLAPPING_LIMIT_24H: i64 = 3;

/// Panelin kendisi sağlık denetiminin izlediği listede yok; ölçüm geçmişi için eklenir.
const PANEL_UNIT: (&str, &str) = ("pixpanel.service", "PixPanel");

/// Geçmişi tutulan servisler ve okunaklı adları: sağlık merkezinin izlediği liste + panel.
pub fn monitored_units() -> Vec<(String, String)> {
    let mut units = crate::health::watched_units();
    units.push((PANEL_UNIT.0.to_string(), PANEL_UNIT.1.to_string()));
    units
}

pub fn is_monitored(unit: &str) -> bool {
    monitored_units().iter().any(|(name, _)| name == unit)
}

// ---- Okuma ----

/// Servisin o andaki durumu (kayıtta 0 / 1 / 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Active,
    Failed,
    Other,
}

impl State {
    fn code(self) -> i64 {
        match self {
            State::Active => 0,
            State::Failed => 1,
            State::Other => 2,
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "active" | "reloading" | "activating" => State::Active,
            "failed" => State::Failed,
            _ => State::Other,
        }
    }
}

/// `systemctl show` çıktısından okunan tek servis örneği.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub unit: String,
    pub state: State,
    /// cgroup bellek kullanımı (bayt); muhasebe kapalıysa `None`.
    pub memory: Option<u64>,
    /// Toplam işlemci süresi (nanosaniye); muhasebe kapalıysa `None`.
    pub cpu_ns: Option<u64>,
    pub tasks: Option<u64>,
    /// systemd'nin kendiliğinden yeniden başlatma sayacı (`NRestarts`).
    pub n_restarts: u64,
    /// Servisin son etkinleşme anı (açılıştan beri mikrosaniye); elle yeniden başlatmayı da yakalar.
    pub entered: u64,
}

/// systemd, ölçülmeyen değer için `[not set]` ya da `u64::MAX` döner.
fn parse_counter(value: Option<&&str>) -> Option<u64> {
    let value = value?.trim();
    let number: u64 = value.parse().ok()?;
    (number != u64::MAX).then_some(number)
}

/// Toplu `systemctl show` çıktısını servis başına örneğe çevirir. Kurulu olmayan birimler atlanır.
pub fn parse_readings(output: &str) -> Vec<Reading> {
    crate::health::parse_unit_blocks(output)
        .into_iter()
        .filter(|block| block.get("LoadState").copied() == Some("loaded"))
        .filter_map(|block| {
            Some(Reading {
                unit: block.get("Id")?.to_string(),
                state: State::parse(block.get("ActiveState").copied().unwrap_or("")),
                memory: parse_counter(block.get("MemoryCurrent")),
                cpu_ns: parse_counter(block.get("CPUUsageNSec")),
                tasks: parse_counter(block.get("TasksCurrent")),
                n_restarts: parse_counter(block.get("NRestarts")).unwrap_or(0),
                entered: parse_counter(block.get("ActiveEnterTimestampMonotonic")).unwrap_or(0),
            })
        })
        .collect()
}

/// İşlemci süresi farkından, tek çekirdeğe göre yüzde. Sayaç geriye gittiyse (servis yeniden
/// başladı) ya da süre geçmediyse `None`.
pub fn cpu_percent(previous_ns: u64, current_ns: u64, elapsed_secs: i64) -> Option<f64> {
    if current_ns < previous_ns || elapsed_secs <= 0 {
        return None;
    }
    Some((current_ns - previous_ns) as f64 / (elapsed_secs as f64 * 1e9) * 100.0)
}

/// İki örnek arasındaki yeniden başlama sayısı. `NRestarts` yalnızca systemd'nin kendiliğinden
/// yaptığı başlatmaları sayar ve elle başlatmada sıfırlanır; elle yeniden başlatmayı ise etkinleşme
/// anının değişmesinden anlarız. Önceki örnek yoksa (ilk ölçüm) sayılmaz.
pub fn restart_delta(previous: Option<(u64, u64)>, current: (u64, u64)) -> i64 {
    let Some((previous_restarts, previous_entered)) = previous else { return 0 };
    let automatic = current.0.saturating_sub(previous_restarts) as i64;
    let entered_changed = current.1 != 0 && previous_entered != 0 && current.1 != previous_entered;
    automatic.max(i64::from(entered_changed))
}

// ---- Kayıt ----

/// Bir dakikalık kayıt.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub unit: String,
    pub ts: i64,
    pub memory: Option<i64>,
    pub cpu: Option<f64>,
    pub tasks: Option<i64>,
    pub state: i64,
    pub n_restarts: i64,
    pub entered: i64,
    /// Bu dakikada yeniden başlama sayısı (`restart_delta`).
    pub restarts: i64,
}

/// Saatlik özet.
#[derive(Debug, Clone, PartialEq)]
pub struct HourRow {
    pub unit: String,
    pub ts: i64,
    pub memory: Option<f64>,
    pub memory_max: Option<i64>,
    pub cpu: Option<f64>,
    pub tasks: Option<f64>,
    pub failed_minutes: i64,
    pub samples: i64,
    pub restarts: i64,
}

fn average<T: Copy>(values: impl Iterator<Item = Option<T>>, convert: impl Fn(T) -> f64) -> Option<f64> {
    let (sum, count) = values.flatten().fold((0.0, 0u32), |(sum, count), value| (sum + convert(value), count + 1));
    (count > 0).then(|| sum / f64::from(count))
}

/// Bir saate düşen dakikalık kayıtların özeti (seyreltme). Kayıt yoksa `None`.
pub fn aggregate_hour(unit: &str, hour: i64, samples: &[Sample]) -> Option<HourRow> {
    if samples.is_empty() {
        return None;
    }
    Some(HourRow {
        unit: unit.to_string(),
        ts: hour,
        memory: average(samples.iter().map(|sample| sample.memory), |value| value as f64),
        memory_max: samples.iter().filter_map(|sample| sample.memory).max(),
        cpu: average(samples.iter().map(|sample| sample.cpu), |value| value),
        tasks: average(samples.iter().map(|sample| sample.tasks), |value| value as f64),
        failed_minutes: samples.iter().filter(|sample| sample.state == State::Failed.code()).count() as i64,
        samples: samples.len() as i64,
        restarts: samples.iter().map(|sample| sample.restarts).sum(),
    })
}

/// Servis başına bir önceki okuma; işlemci yüzdesi ve yeniden başlama farkı için.
struct Previous {
    ts: i64,
    cpu_ns: Option<u64>,
    n_restarts: u64,
    entered: u64,
}

static PREVIOUS: Mutex<Option<HashMap<String, Previous>>> = Mutex::new(None);

/// Okumaları kayıtlara çevirir; önceki okumayı günceller. Panel yeni açıldıysa önceki durum
/// veritabanındaki son kayıttan alınır (`seed`), böylece panel kapalıyken olan yeniden başlamalar
/// de sayılır.
fn to_samples(readings: &[Reading], ts: i64, seed: &HashMap<String, Previous>) -> Vec<Sample> {
    let mut guard = PREVIOUS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let memory = guard.get_or_insert_with(HashMap::new);
    let mut samples = Vec::new();
    for reading in readings {
        let previous = memory.get(&reading.unit).or_else(|| seed.get(&reading.unit));
        let cpu = match (previous.and_then(|previous| previous.cpu_ns.map(|ns| (ns, previous.ts))), reading.cpu_ns) {
            (Some((before, at)), Some(after)) => cpu_percent(before, after, ts - at),
            _ => None,
        };
        let restarts = restart_delta(previous.map(|previous| (previous.n_restarts, previous.entered)), (reading.n_restarts, reading.entered));
        samples.push(Sample {
            unit: reading.unit.clone(),
            ts,
            memory: reading.memory.map(|value| value as i64),
            cpu,
            tasks: reading.tasks.map(|value| value as i64),
            state: reading.state.code(),
            n_restarts: reading.n_restarts as i64,
            // Mikrosaniye değeri i64'e sığar (açılıştan beri geçen süre).
            entered: reading.entered as i64,
            restarts,
        });
        memory.insert(reading.unit.clone(), Previous { ts, cpu_ns: reading.cpu_ns, n_restarts: reading.n_restarts, entered: reading.entered });
    }
    samples
}

/// Panel yeni başladığında her servisin son kaydını okur.
fn load_seed(db: &Db) -> HashMap<String, Previous> {
    let rows = db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT unit, ts, n_restarts, entered FROM service_minutes m
             WHERE ts = (SELECT MAX(ts) FROM service_minutes WHERE unit = m.unit)",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    });
    // İşlemci sayacı saklanmadığından (yeniden başlatma sonrası ilk yüzde boş kalır) yalnızca sayaçlar tohumlanır.
    rows.unwrap_or_default()
        .into_iter()
        .map(|(unit, ts, n_restarts, entered)| (unit, Previous { ts, cpu_ns: None, n_restarts: n_restarts as u64, entered: entered as u64 }))
        .collect()
}

fn write_samples(db: &Db, samples: &[Sample], ts: i64) -> AppResult<()> {
    let hour = ts / 3600 * 3600;
    db.call(|conn| {
        let tx = conn.transaction()?;
        for sample in samples {
            tx.execute(
                "INSERT OR REPLACE INTO service_minutes (unit, ts, memory, cpu, tasks, state, n_restarts, entered, restarts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![sample.unit, sample.ts, sample.memory, sample.cpu, sample.tasks, sample.state, sample.n_restarts, sample.entered, sample.restarts],
            )?;
            // Saat dolmadan da o saatin noktası görünsün diye özet her dakika yeniden hesaplanır.
            let minutes = read_minutes(&tx, &sample.unit, hour, hour + 3600)?;
            if let Some(row) = aggregate_hour(&sample.unit, hour, &minutes) {
                tx.execute(
                    "INSERT OR REPLACE INTO service_hours (unit, ts, memory, memory_max, cpu, tasks, failed_minutes, samples, restarts)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![row.unit, row.ts, row.memory, row.memory_max, row.cpu, row.tasks, row.failed_minutes, row.samples, row.restarts],
                )?;
            }
        }
        tx.execute("DELETE FROM service_minutes WHERE ts < ?1", params![ts - MINUTE_RETENTION_HOURS * 3600])?;
        tx.execute("DELETE FROM service_hours WHERE ts < ?1", params![ts - HOUR_RETENTION_DAYS * 86400])?;
        tx.commit()
    })
}

fn read_minutes(conn: &rusqlite::Connection, unit: &str, from: i64, to: i64) -> rusqlite::Result<Vec<Sample>> {
    let mut stmt = conn.prepare(
        "SELECT unit, ts, memory, cpu, tasks, state, n_restarts, entered, restarts FROM service_minutes
         WHERE unit = ?1 AND ts >= ?2 AND ts < ?3 ORDER BY ts",
    )?;
    let rows = stmt.query_map(params![unit, from, to], |row| {
        Ok(Sample {
            unit: row.get(0)?,
            ts: row.get(1)?,
            memory: row.get(2)?,
            cpu: row.get(3)?,
            tasks: row.get(4)?,
            state: row.get(5)?,
            n_restarts: row.get(6)?,
            entered: row.get(7)?,
            restarts: row.get(8)?,
        })
    })?;
    rows.collect()
}

/// Bir ölçüm turu: servisleri toplu okur ve kaydeder.
pub async fn collect(db: &Db) -> AppResult<()> {
    let units = monitored_units();
    let mut args = vec![
        "show",
        "--no-pager",
        "-p",
        "Id,LoadState,ActiveState,MemoryCurrent,CPUUsageNSec,TasksCurrent,NRestarts,ActiveEnterTimestampMonotonic",
        "--",
    ];
    args.extend(units.iter().map(|(unit, _)| unit.as_str()));
    let output = exec::run("systemctl", &args).await?;
    let readings = parse_readings(&output.stdout);
    let ts = now() / 60 * 60;
    let seed_needed = PREVIOUS.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).is_none();
    let seed = if seed_needed { load_seed(db) } else { HashMap::new() };
    let samples = to_samples(&readings, ts, &seed);
    write_samples(db, &samples, ts)
}

/// Ölçüm döngüsü: dakikada bir.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(25)).await;
        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        loop {
            ticker.tick().await;
            if let Err(err) = collect(&state.db).await {
                tracing::warn!(error = %err, "servis ölçümleri kaydedilemedi");
            }
        }
    });
}

// ---- Sorgu ----

/// Grafikteki bir nokta.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Point {
    pub ts: i64,
    /// Bellek (bayt).
    pub memory: Option<f64>,
    /// İşlemci (tek çekirdeğe göre yüzde).
    pub cpu: Option<f64>,
    pub tasks: Option<f64>,
    /// Aralıkta servis hata durumunda görüldü mü?
    pub failed: bool,
    pub restarts: i64,
}

/// Aralık adı → (kaynak: dakikalık mı, kapsanan süre, nokta başına saniye).
pub fn range_plan(range: &str) -> Option<(bool, i64, i64)> {
    Some(match range {
        "day" => (true, 86400, 300),
        "week" => (false, 7 * 86400, 3600),
        "month" => (false, 30 * 86400, 2 * 3600),
        _ => return None,
    })
}

/// Noktaları `bucket` saniyelik dilimlere indirger (grafik seyreltme): bellek, işlemci ve görev
/// ortalaması; hata ve yeniden başlama toplanır. Girdi eskiden yeniye sıralıdır.
pub fn bucketize(points: &[Point], bucket: i64) -> Vec<Point> {
    let mut groups: Vec<(i64, Vec<&Point>)> = Vec::new();
    for point in points {
        let start = point.ts.div_euclid(bucket) * bucket;
        match groups.last_mut() {
            Some((key, list)) if *key == start => list.push(point),
            _ => groups.push((start, vec![point])),
        }
    }
    groups
        .into_iter()
        .map(|(ts, list)| Point {
            ts,
            memory: average(list.iter().map(|point| point.memory), |value| value),
            cpu: average(list.iter().map(|point| point.cpu), |value| value),
            tasks: average(list.iter().map(|point| point.tasks), |value| value),
            failed: list.iter().any(|point| point.failed),
            restarts: list.iter().map(|point| point.restarts).sum(),
        })
        .collect()
}

fn minute_points(db: &Db, unit: &str, since: i64) -> AppResult<Vec<Point>> {
    let samples = db.call(|conn| read_minutes(conn, unit, since, i64::MAX))?;
    Ok(samples
        .into_iter()
        .map(|sample| Point {
            ts: sample.ts,
            memory: sample.memory.map(|value| value as f64),
            cpu: sample.cpu,
            tasks: sample.tasks.map(|value| value as f64),
            failed: sample.state == State::Failed.code(),
            restarts: sample.restarts,
        })
        .collect())
}

fn hour_points(db: &Db, unit: &str, since: i64) -> AppResult<Vec<Point>> {
    db.call(|conn| {
        let mut stmt = conn.prepare("SELECT ts, memory, cpu, tasks, failed_minutes, restarts FROM service_hours WHERE unit = ?1 AND ts >= ?2 ORDER BY ts")?;
        let rows = stmt.query_map(params![unit, since], |row| {
            Ok(Point { ts: row.get(0)?, memory: row.get(1)?, cpu: row.get(2)?, tasks: row.get(3)?, failed: row.get::<_, i64>(4)? > 0, restarts: row.get(5)? })
        })?;
        rows.collect()
    })
}

/// Grafik noktaları (aralığa göre seyreltilmiş). Aralık adı geçersizse `None`.
pub fn history(db: &Db, unit: &str, range: &str) -> AppResult<Option<Vec<Point>>> {
    let Some((minutes, span, bucket)) = range_plan(range) else { return Ok(None) };
    let since = now() - span;
    let raw = if minutes { minute_points(db, unit, since)? } else { hour_points(db, unit, since)? };
    Ok(Some(bucketize(&raw, bucket)))
}

/// Son `secs` saniyedeki yeniden başlama sayısı. Saatlik özet, dakikalık kayıtları da kapsadığından
/// tek kaynaktır (çift sayım olmaz); 48 saatten kısa pencerelerde saat sınırı en çok bir saat taşar.
pub fn restarts_within(db: &Db, unit: &str, secs: i64) -> AppResult<i64> {
    let since = (now() - secs) / 3600 * 3600;
    db.call(|conn| conn.query_row("SELECT COALESCE(SUM(restarts), 0) FROM service_hours WHERE unit = ?1 AND ts >= ?2", params![unit, since], |row| row.get(0)))
}

/// Yedi günlük saatlik bellek ortalamaları (sızıntı ölçütü için).
pub fn memory_series(db: &Db, unit: &str, secs: i64) -> AppResult<Vec<(i64, f64)>> {
    Ok(hour_points(db, unit, now() - secs)?.into_iter().filter_map(|point| point.memory.map(|memory| (point.ts, memory))).collect())
}

/// Belleğin doğrusal eğilimi.
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct Trend {
    /// Uydurulan doğruya göre pencere boyunca artış (bayt).
    pub growth_bytes: f64,
    /// Artışın başlangıç değerine oranı.
    pub growth_ratio: f64,
    /// Doğrusal uyumun belirlilik katsayısı (0–1).
    pub r2: f64,
    pub span_days: f64,
}

/// En küçük kareler doğrusu: yeterli veri yoksa `None`.
pub fn memory_trend(points: &[(i64, f64)]) -> Option<Trend> {
    if points.len() < LEAK_MIN_POINTS {
        return None;
    }
    let first = points.first()?.0;
    let last = points.last()?.0;
    if last - first < LEAK_MIN_SPAN_SECS {
        return None;
    }
    let count = points.len() as f64;
    let xs: Vec<f64> = points.iter().map(|(ts, _)| (*ts - first) as f64 / 86400.0).collect();
    let mean_x = xs.iter().sum::<f64>() / count;
    let mean_y = points.iter().map(|(_, y)| *y).sum::<f64>() / count;
    let sxx: f64 = xs.iter().map(|x| (x - mean_x).powi(2)).sum();
    if sxx <= f64::EPSILON {
        return None;
    }
    let sxy: f64 = xs.iter().zip(points).map(|(x, (_, y))| (x - mean_x) * (y - mean_y)).sum();
    let syy: f64 = points.iter().map(|(_, y)| (y - mean_y).powi(2)).sum();
    let slope = sxy / sxx;
    let intercept = mean_y - slope * mean_x;
    let span_days = (last - first) as f64 / 86400.0;
    let growth = slope * span_days;
    let r2 = if syy <= f64::EPSILON { 0.0 } else { (sxy * sxy / (sxx * syy)).clamp(0.0, 1.0) };
    let growth_ratio = if intercept > 1_048_576.0 { growth / intercept } else if growth > 0.0 { f64::INFINITY } else { 0.0 };
    Some(Trend { growth_bytes: growth, growth_ratio, r2, span_days })
}

/// Eğilim sızıntı şüphesi sayılır mı?
pub fn is_leak_suspect(trend: &Trend) -> bool {
    trend.growth_bytes >= LEAK_MIN_GROWTH_BYTES && trend.growth_ratio >= LEAK_MIN_GROWTH_RATIO && trend.r2 >= LEAK_MIN_R2
}

/// Servis ayrıntısında gösterilen özet.
#[derive(Debug, Serialize)]
pub struct Summary {
    pub restarts_24h: i64,
    pub restarts_7d: i64,
    pub trend: Option<Trend>,
    pub leak_suspect: bool,
    /// Son kayıt.
    pub latest: Option<Point>,
}

pub fn summary(db: &Db, unit: &str) -> AppResult<Summary> {
    let series = memory_series(db, unit, 7 * 86400)?;
    let trend = memory_trend(&series);
    let latest = minute_points(db, unit, now() - 600)?.pop();
    Ok(Summary {
        restarts_24h: restarts_within(db, unit, 86400)?,
        restarts_7d: restarts_within(db, unit, 7 * 86400)?,
        leak_suspect: trend.as_ref().is_some_and(is_leak_suspect),
        trend,
        latest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ts: i64, memory: Option<i64>, cpu: Option<f64>, state: State, restarts: i64) -> Sample {
        Sample { unit: "nginx.service".into(), ts, memory, cpu, tasks: Some(5), state: state.code(), n_restarts: 0, entered: 0, restarts }
    }

    #[test]
    fn systemctl_output_is_parsed_per_unit() {
        let output = "Id=nginx.service\nLoadState=loaded\nActiveState=active\nMemoryCurrent=52428800\nCPUUsageNSec=1500000000\nTasksCurrent=5\nNRestarts=2\nActiveEnterTimestampMonotonic=123456\n\n\
                      Id=redis-server.service\nLoadState=loaded\nActiveState=failed\nMemoryCurrent=[not set]\nCPUUsageNSec=18446744073709551615\nTasksCurrent=[not set]\nNRestarts=0\nActiveEnterTimestampMonotonic=0\n\n\
                      Id=rspamd.service\nLoadState=not-found\nActiveState=inactive\n";
        let readings = parse_readings(output);
        assert_eq!(readings.len(), 2, "kurulu olmayan birim atlanır");
        assert_eq!(readings[0].memory, Some(52_428_800));
        assert_eq!(readings[0].cpu_ns, Some(1_500_000_000));
        assert_eq!(readings[0].n_restarts, 2);
        assert_eq!(readings[1].state, State::Failed);
        assert_eq!(readings[1].memory, None, "[not set] ölçülmemiş demektir");
        assert_eq!(readings[1].cpu_ns, None, "u64::MAX ölçülmemiş demektir");
        assert_eq!(readings[1].tasks, None);
    }

    #[test]
    fn cpu_percent_comes_from_counter_difference() {
        // 60 saniyede 30 saniyelik işlemci süresi = tek çekirdeğin yarısı.
        assert_eq!(cpu_percent(10_000_000_000, 40_000_000_000, 60), Some(50.0));
        assert_eq!(cpu_percent(40_000_000_000, 10_000_000_000, 60), None, "sayaç geriye gittiyse servis yeniden başlamıştır");
        assert_eq!(cpu_percent(1, 2, 0), None);
    }

    #[test]
    fn restarts_are_detected_from_counter_and_activation_time() {
        assert_eq!(restart_delta(None, (5, 100)), 0, "ilk ölçüm sayılmaz");
        assert_eq!(restart_delta(Some((2, 100)), (2, 100)), 0);
        assert_eq!(restart_delta(Some((2, 100)), (4, 300)), 2, "systemd iki kez yeniden başlatmış");
        assert_eq!(restart_delta(Some((0, 100)), (0, 900)), 1, "elle yeniden başlatma etkinleşme anından anlaşılır");
        assert_eq!(restart_delta(Some((3, 100)), (0, 900)), 1, "elle başlatma sayacı sıfırlar");
        assert_eq!(restart_delta(Some((0, 0)), (0, 900)), 0, "etkinleşme anı bilinmiyorsa sayılmaz");
    }

    #[test]
    fn minutes_are_thinned_into_hourly_averages() {
        let minutes = vec![
            sample(0, Some(100), Some(10.0), State::Active, 0),
            sample(60, Some(300), None, State::Active, 1),
            sample(120, None, Some(30.0), State::Failed, 2),
        ];
        let row = aggregate_hour("nginx.service", 0, &minutes).unwrap();
        assert_eq!(row.memory, Some(200.0), "ölçülmeyen dakika ortalamayı bozmaz");
        assert_eq!(row.memory_max, Some(300));
        assert_eq!(row.cpu, Some(20.0));
        assert_eq!(row.failed_minutes, 1);
        assert_eq!(row.samples, 3);
        assert_eq!(row.restarts, 3);
        assert!(aggregate_hour("nginx.service", 0, &[]).is_none());
    }

    #[test]
    fn graph_points_are_thinned_per_bucket() {
        let point = |ts: i64, memory: f64, failed: bool, restarts: i64| Point { ts, memory: Some(memory), cpu: None, tasks: Some(2.0), failed, restarts };
        let raw: Vec<Point> = (0..10).map(|minute| point(minute * 60, 100.0 + minute as f64, minute == 7, i64::from(minute == 3))).collect();
        // Beş dakikalık dilimler: 0–4 ve 5–9.
        let thinned = bucketize(&raw, 300);
        assert_eq!(thinned.len(), 2);
        assert_eq!(thinned[0].ts, 0);
        assert_eq!(thinned[0].memory, Some(102.0));
        assert_eq!(thinned[0].cpu, None);
        assert!(!thinned[0].failed && thinned[1].failed, "hata, görüldüğü dilimi işaretler");
        assert_eq!((thinned[0].restarts, thinned[1].restarts), (1, 0));
        assert!(bucketize(&[], 300).is_empty());
        // Her aralık grafikte makul sayıda nokta verir ve saklama süresi kaynağı karşılar.
        for range in ["day", "week", "month"] {
            let (minutes, span, bucket) = range_plan(range).unwrap();
            assert!(span / bucket <= 400, "{range}");
            let retention = if minutes { MINUTE_RETENTION_HOURS * 3600 } else { HOUR_RETENTION_DAYS * 86400 };
            assert!(retention >= span, "{range}: saklama süresi aralığı karşılamıyor");
        }
        assert!(range_plan("year").is_none());
    }

    fn series(points: usize, value: impl Fn(usize) -> f64) -> Vec<(i64, f64)> {
        (0..points).map(|hour| (hour as i64 * 3600, value(hour))).collect()
    }

    #[test]
    fn steadily_growing_memory_is_a_leak_suspect() {
        const MIB: f64 = 1_048_576.0;
        // 7 günde 200 MiB'tan 500 MiB'a, küçük oynamalarla düzenli büyüme.
        let growing = series(168, |hour| 200.0 * MIB + hour as f64 * (300.0 * MIB / 167.0) + ((hour % 5) as f64) * MIB);
        let trend = memory_trend(&growing).unwrap();
        assert!(trend.growth_ratio > 1.0 && trend.r2 > 0.95, "{trend:?}");
        assert!(is_leak_suspect(&trend));
    }

    #[test]
    fn flat_noisy_or_sawtooth_memory_is_not_a_leak() {
        const MIB: f64 = 1_048_576.0;
        let flat = series(168, |hour| 300.0 * MIB + ((hour * 7) % 11) as f64 * MIB);
        assert!(!is_leak_suspect(&memory_trend(&flat).unwrap()));
        // Her gün yeniden başlayıp tekrar büyüyen servis: yükseliş var ama doğrusal değil.
        let sawtooth = series(168, |hour| 100.0 * MIB + (hour % 24) as f64 * 20.0 * MIB);
        assert!(!is_leak_suspect(&memory_trend(&sawtooth).unwrap()));
        // Küçük servisin birkaç MB'lık büyümesi sızıntı sayılmaz.
        let tiny = series(168, |hour| 5.0 * MIB + hour as f64 * 0.05 * MIB);
        assert!(!is_leak_suspect(&memory_trend(&tiny).unwrap()));
        // Yetersiz veri.
        assert!(memory_trend(&series(40, |hour| hour as f64 * MIB)).is_none());
        assert!(memory_trend(&series(100, |_| MIB)).is_none() || !is_leak_suspect(&memory_trend(&series(100, |_| MIB)).unwrap()));
    }
}
