//! Sistem ölçümleri: `/proc` üzerinden okunur ve kısa bir geçmiş bellekte tutulur.

use std::collections::VecDeque;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde::Serialize;

use crate::db::now;
use crate::sys::exec;

const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);
/// 30 dakikalık geçmiş.
const HISTORY_LEN: usize = 900;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Sample {
    pub ts: i64,
    /// İşlemci kullanımı (yüzde).
    pub cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub load1: f32,
    pub load5: f32,
    pub load15: f32,
    /// Ağ hızı (bayt/saniye).
    pub net_rx: u64,
    pub net_tx: u64,
}

#[derive(Clone, Default)]
pub struct Metrics(Arc<RwLock<VecDeque<Sample>>>);

impl Metrics {
    pub fn latest(&self) -> Sample {
        self.0.read().unwrap_or_else(|p| p.into_inner()).back().cloned().unwrap_or_default()
    }

    /// Son `count` örneği eskiden yeniye sıralı döner.
    pub fn history(&self, count: usize) -> Vec<Sample> {
        let guard = self.0.read().unwrap_or_else(|p| p.into_inner());
        let skip = guard.len().saturating_sub(count);
        guard.iter().skip(skip).cloned().collect()
    }

    /// Arka planda sürekli örnek toplayan görevi başlatır.
    pub fn spawn_sampler(&self) {
        let store = self.0.clone();
        tokio::spawn(async move {
            let mut prev_cpu = read_cpu_times();
            let mut prev_net = read_net_totals();
            let mut ticker = tokio::time::interval(SAMPLE_INTERVAL);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let cpu = read_cpu_times();
                let net = read_net_totals();
                let (mem_used, mem_total, swap_used, swap_total) = read_memory();
                let (load1, load5, load15) = read_load();
                let secs = SAMPLE_INTERVAL.as_secs().max(1);
                let sample = Sample {
                    ts: now(),
                    cpu: cpu_percent(prev_cpu, cpu),
                    mem_used,
                    mem_total,
                    swap_used,
                    swap_total,
                    load1,
                    load5,
                    load15,
                    net_rx: net.0.saturating_sub(prev_net.0) / secs,
                    net_tx: net.1.saturating_sub(prev_net.1) / secs,
                };
                prev_cpu = cpu;
                prev_net = net;
                // Açık arayüzler yeni örneği yoklamadan alır (yalnızca yöneticiler).
                crate::events::publish(crate::events::Event::Metrics { sample: sample.clone() }, None);
                let mut guard = store.write().unwrap_or_else(|p| p.into_inner());
                if guard.len() >= HISTORY_LEN {
                    guard.pop_front();
                }
                guard.push_back(sample);
            }
        });
    }
}

/// (meşgul, toplam) işlemci süreleri.
fn read_cpu_times() -> (u64, u64) {
    let Ok(stat) = std::fs::read_to_string("/proc/stat") else { return (0, 0) };
    let Some(line) = stat.lines().next() else { return (0, 0) };
    let values: Vec<u64> = line.split_whitespace().skip(1).filter_map(|v| v.parse().ok()).collect();
    let total: u64 = values.iter().take(8).sum();
    // idle + iowait boşta sayılır
    let idle = values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0);
    (total.saturating_sub(idle), total)
}

fn cpu_percent(prev: (u64, u64), current: (u64, u64)) -> f32 {
    let total = current.1.saturating_sub(prev.1);
    if total == 0 {
        return 0.0;
    }
    let busy = current.0.saturating_sub(prev.0);
    (busy as f32 / total as f32 * 100.0).clamp(0.0, 100.0)
}

/// (kullanılan, toplam, takas kullanılan, takas toplam) bayt cinsinden.
fn read_memory() -> (u64, u64, u64, u64) {
    let Ok(info) = std::fs::read_to_string("/proc/meminfo") else { return (0, 0, 0, 0) };
    let field = |name: &str| -> u64 {
        info.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|v| v.parse::<u64>().ok())
            .map(|kb| kb * 1024)
            .unwrap_or(0)
    };
    let total = field("MemTotal");
    let available = field("MemAvailable");
    let swap_total = field("SwapTotal");
    let swap_free = field("SwapFree");
    (total.saturating_sub(available), total, swap_total.saturating_sub(swap_free), swap_total)
}

fn read_load() -> (f32, f32, f32) {
    let raw = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let mut parts = raw.split_whitespace().map(|v| v.parse::<f32>().unwrap_or(0.0));
    (parts.next().unwrap_or(0.0), parts.next().unwrap_or(0.0), parts.next().unwrap_or(0.0))
}

/// Döngü arayüzü hariç toplam (alınan, gönderilen) bayt.
fn read_net_totals() -> (u64, u64) {
    let Ok(raw) = std::fs::read_to_string("/proc/net/dev") else { return (0, 0) };
    raw.lines()
        .skip(2)
        .filter_map(|line| {
            let (name, rest) = line.split_once(':')?;
            if name.trim() == "lo" {
                return None;
            }
            let values: Vec<u64> = rest.split_whitespace().filter_map(|v| v.parse().ok()).collect();
            Some((*values.first()?, *values.get(8)?))
        })
        .fold((0, 0), |acc, (rx, tx)| (acc.0 + rx, acc.1 + tx))
}

#[derive(Debug, Serialize)]
pub struct Disk {
    pub filesystem: String,
    pub fs_type: String,
    pub mount: String,
    pub total: u64,
    pub used: u64,
    pub available: u64,
}

/// Gerçek dosya sistemlerinin doluluk bilgisi.
pub async fn disks() -> Vec<Disk> {
    let args = [
        "-B1",
        "--output=source,fstype,size,used,avail,target",
        "-x", "tmpfs", "-x", "devtmpfs", "-x", "overlay", "-x", "squashfs", "-x", "efivarfs",
    ];
    let Ok(output) = exec::run("df", &args).await else { return Vec::new() };
    output
        .stdout
        .lines()
        .skip(1)
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 6 {
                return None;
            }
            Some(Disk {
                filesystem: cols[0].to_string(),
                fs_type: cols[1].to_string(),
                total: cols[2].parse().ok()?,
                used: cols[3].parse().ok()?,
                available: cols[4].parse().ok()?,
                mount: cols[5..].join(" "),
            })
        })
        .filter(|disk| disk.total > 0)
        .collect()
}

#[derive(Debug, Serialize)]
pub struct HostInfo {
    pub hostname: String,
    pub os: String,
    pub kernel: String,
    pub cpu_model: String,
    pub cpu_cores: usize,
    pub uptime_secs: u64,
    pub virtualization: String,
    pub panel_version: &'static str,
}

pub async fn host_info() -> HostInfo {
    let read = |path: &str| std::fs::read_to_string(path).unwrap_or_default();
    let os = read("/etc/os-release")
        .lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim_matches('"').to_string())
        .unwrap_or_else(|| "Linux".into());
    let cpuinfo = read("/proc/cpuinfo");
    let cpu_model = cpuinfo
        .lines()
        .find_map(|line| line.strip_prefix("model name"))
        .and_then(|rest| rest.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_default();
    let cpu_cores = cpuinfo.lines().filter(|line| line.starts_with("processor")).count().max(1);
    let uptime_secs = read("/proc/uptime")
        .split_whitespace()
        .next()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0) as u64;
    let virtualization = exec::run("systemd-detect-virt", &[])
        .await
        .map(|out| out.stdout.trim().to_string())
        .unwrap_or_default();
    HostInfo {
        hostname: read("/proc/sys/kernel/hostname").trim().to_string(),
        os,
        kernel: read("/proc/sys/kernel/osrelease").trim().to_string(),
        cpu_model,
        cpu_cores,
        uptime_secs,
        virtualization,
        panel_version: env!("CARGO_PKG_VERSION"),
    }
}

// ---- Uzun dönem geçmiş ----

/// Dakikalık kayıtlar bu kadar gün, saatlik özetler `HOUR_RETENTION_DAYS` gün saklanır.
const MINUTE_RETENTION_DAYS: i64 = 14;
const HOUR_RETENTION_DAYS: i64 = 400;

/// Geçmiş grafiğindeki bir nokta: o aralığın ortalaması.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ArchivePoint {
    pub ts: i64,
    pub cpu: f64,
    /// Bellek kullanımı (yüzde).
    pub mem: f64,
    pub load1: f64,
    pub net_rx: i64,
    pub net_tx: i64,
}

/// Son bir dakikanın örneklerinin ortalamasını kalıcı geçmişe yazar ve içinde bulunulan
/// saatin özetini günceller. Dakikada bir çağrılır.
pub fn archive_minute(db: &crate::db::Db, samples: &[Sample]) -> crate::error::AppResult<()> {
    if samples.is_empty() {
        return Ok(());
    }
    let count = samples.len() as f64;
    let average = |value: fn(&Sample) -> f64| samples.iter().map(value).sum::<f64>() / count;
    let ts = now() / 60 * 60;
    let hour = ts / 3600 * 3600;
    db.call(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO metrics_minutes (ts, cpu, mem_used, mem_total, load1, net_rx, net_tx) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                ts,
                average(|sample| f64::from(sample.cpu)),
                average(|sample| sample.mem_used as f64) as i64,
                samples[samples.len() - 1].mem_total as i64,
                average(|sample| f64::from(sample.load1)),
                average(|sample| sample.net_rx as f64) as i64,
                average(|sample| sample.net_tx as f64) as i64,
            ],
        )?;
        // Saat dolmadan da o saatin noktası grafikte görünsün diye özet her dakika yeniden hesaplanır.
        conn.execute(
            "INSERT OR REPLACE INTO metrics_hours (ts, cpu, mem_used, mem_total, load1, net_rx, net_tx)
             SELECT ?1, AVG(cpu), CAST(AVG(mem_used) AS INTEGER), MAX(mem_total), AVG(load1), CAST(AVG(net_rx) AS INTEGER), CAST(AVG(net_tx) AS INTEGER)
             FROM metrics_minutes WHERE ts >= ?1 AND ts < ?1 + 3600",
            rusqlite::params![hour],
        )?;
        conn.execute("DELETE FROM metrics_minutes WHERE ts < ?1", rusqlite::params![ts - MINUTE_RETENTION_DAYS * 86400])?;
        conn.execute("DELETE FROM metrics_hours WHERE ts < ?1", rusqlite::params![ts - HOUR_RETENTION_DAYS * 86400])
    })?;
    Ok(())
}

/// Bir geçmiş aralığının kaynağı ve çözünürlüğü: (tablo, kapsanan süre, nokta başına saniye).
fn archive_plan(range: &str) -> Option<(&'static str, i64, i64)> {
    Some(match range {
        "day" => ("metrics_minutes", 86400, 300),
        "week" => ("metrics_minutes", 7 * 86400, 1800),
        "month" => ("metrics_hours", 30 * 86400, 2 * 3600),
        "year" => ("metrics_hours", 365 * 86400, 86400),
        _ => return None,
    })
}

/// İstenen aralığın noktaları, eskiden yeniye. Aralık adı geçersizse `None`.
pub fn archive(db: &crate::db::Db, range: &str) -> crate::error::AppResult<Option<Vec<ArchivePoint>>> {
    let Some((table, span, bucket)) = archive_plan(range) else { return Ok(None) };
    let since = now() - span;
    // Tablo adı yukarıdaki sabitlerden gelir; kullanıcı girdisi sorguya girmez.
    let sql = format!(
        "SELECT ts / ?1 * ?1 AS bucket, AVG(cpu), AVG(CASE WHEN mem_total > 0 THEN mem_used * 100.0 / mem_total ELSE 0 END), AVG(load1), AVG(net_rx), AVG(net_tx)
         FROM {table} WHERE ts >= ?2 GROUP BY bucket ORDER BY bucket"
    );
    let points = db.call(|conn| {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params![bucket, since], |row| {
            Ok(ArchivePoint {
                ts: row.get(0)?,
                cpu: row.get(1)?,
                mem: row.get(2)?,
                load1: row.get(3)?,
                net_rx: row.get::<_, f64>(4)? as i64,
                net_tx: row.get::<_, f64>(5)? as i64,
            })
        })?;
        rows.collect()
    })?;
    Ok(Some(points))
}

#[cfg(test)]
mod archive_tests {
    use super::*;

    #[test]
    fn every_range_stays_under_four_hundred_points() {
        for range in ["day", "week", "month", "year"] {
            let (_, span, bucket) = archive_plan(range).unwrap();
            assert!(span / bucket <= 400, "{range}: {} nokta", span / bucket);
        }
        assert!(archive_plan("decade").is_none());
        // Dakikalık kayıtlar, onlardan okunan en uzun aralığı (hafta) karşılar.
        assert!(MINUTE_RETENTION_DAYS * 86400 >= archive_plan("week").unwrap().1);
        assert!(HOUR_RETENTION_DAYS * 86400 >= archive_plan("year").unwrap().1);
    }
}
