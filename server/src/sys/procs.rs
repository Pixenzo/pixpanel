//! Süreç listesi: `/proc` altından okunur.

use std::collections::HashMap;
use std::time::Duration;

use serde::Serialize;

/// Linux'ta USER_HZ pratikte her zaman 100'dür.
const CLOCK_TICKS: f32 = 100.0;
const PAGE_SIZE: u64 = 4096;
const CPU_SAMPLE_WINDOW: Duration = Duration::from_millis(300);

#[derive(Debug, Serialize)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    pub user: String,
    pub name: String,
    pub command: String,
    pub state: String,
    pub cpu: f32,
    /// Fiziksel bellek kullanımı (bayt).
    pub memory: u64,
    pub threads: u32,
    /// Sürecin başlangıcından bu yana geçen süre (saniye).
    pub elapsed_secs: u64,
}

struct RawStat {
    name: String,
    state: String,
    ppid: u32,
    cpu_ticks: u64,
    threads: u32,
    start_ticks: u64,
    rss_pages: u64,
}

fn list_pids() -> Vec<u32> {
    std::fs::read_dir("/proc")
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn read_stat(pid: u32) -> Option<RawStat> {
    let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Süreç adı parantez içinde olup boşluk içerebilir; son ')' karakterinden bölünür.
    let open = raw.find('(')?;
    let close = raw.rfind(')')?;
    let name = raw.get(open + 1..close)?.to_string();
    let rest: Vec<&str> = raw.get(close + 1..)?.split_whitespace().collect();
    // `rest[0]` stat(5) içindeki 3. alana (state) karşılık gelir.
    let num = |index: usize| rest.get(index).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    Some(RawStat {
        name,
        state: rest.first()?.to_string(),
        ppid: num(1) as u32,
        cpu_ticks: num(11) + num(12),
        threads: num(17) as u32,
        start_ticks: num(19),
        rss_pages: num(21),
    })
}

fn read_uid(pid: u32) -> Option<u32> {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn read_command(pid: u32) -> String {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|bytes| {
            String::from_utf8_lossy(&bytes)
                .split('\0')
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

/// uid → kullanıcı adı eşlemesi.
pub fn user_names() -> HashMap<u32, String> {
    std::fs::read_to_string("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut parts = line.split(':');
            let name = parts.next()?;
            let uid = parts.nth(1)?.parse().ok()?;
            Some((uid, name.to_string()))
        })
        .collect()
}

/// Sürecin ne kadar süredir çalıştığı.
///
/// Konteynerlerde (LXC) `/proc/uptime` konteynere, `stat` içindeki başlangıç
/// zamanı ise ana makineye göredir; ikisi birbirinden çıkarılamaz. Bu yüzden
/// önce `/proc/<pid>` klasörünün oluşturulma zamanına bakılır.
fn elapsed_secs(pid: u32, start_ticks: u64) -> u64 {
    let from_dir = std::fs::metadata(format!("/proc/{pid}"))
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|started| started.elapsed().ok())
        .map(|elapsed| elapsed.as_secs());
    from_dir.unwrap_or_else(|| {
        let uptime: f64 = std::fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|raw| raw.split_whitespace().next()?.parse().ok())
            .unwrap_or(0.0);
        (uptime - start_ticks as f64 / CLOCK_TICKS as f64).max(0.0) as u64
    })
}

/// Çalışan süreçleri işlemci kullanımıyla birlikte listeler.
/// İşlemci yüzdesi iki kısa aralıklı ölçümün farkından hesaplanır.
pub async fn list() -> Vec<Process> {
    let before: HashMap<u32, u64> =
        list_pids().into_iter().filter_map(|pid| Some((pid, read_stat(pid)?.cpu_ticks))).collect();
    tokio::time::sleep(CPU_SAMPLE_WINDOW).await;

    let users = user_names();
    let window = CPU_SAMPLE_WINDOW.as_secs_f32();

    let mut processes: Vec<Process> = list_pids()
        .into_iter()
        .filter_map(|pid| {
            let stat = read_stat(pid)?;
            let uid = read_uid(pid).unwrap_or(0);
            let delta = before.get(&pid).map(|prev| stat.cpu_ticks.saturating_sub(*prev)).unwrap_or(0);
            let command = read_command(pid);
            Some(Process {
                pid,
                ppid: stat.ppid,
                user: users.get(&uid).cloned().unwrap_or_else(|| uid.to_string()),
                command: if command.is_empty() { format!("[{}]", stat.name) } else { command },
                name: stat.name,
                state: stat.state,
                cpu: delta as f32 / CLOCK_TICKS / window * 100.0,
                memory: stat.rss_pages * PAGE_SIZE,
                threads: stat.threads,
                elapsed_secs: elapsed_secs(pid, stat.start_ticks),
            })
        })
        .collect();
    processes.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.memory.cmp(&a.memory)));
    processes
}
