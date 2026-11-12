//! Haftalık özet: haftada bir (yapılandırılabilir gün ve saat; varsayılan pazartesi 08:00,
//! sunucu saati) yöneticilere son yedi günün özeti gönderilir: yapılan denetimler, kendiliğinden
//! düzeltmeler, bekleyen sorunlar, sertifika yenilemeleri, yedekler, disk büyümesi, en çok ziyaret
//! alan siteler ve bekleyen güncellemeler.
//!
//! Gönderim yöneticilerin kendi bildirim kanallarına (e-posta, Telegram) gider; e-posta düz metin
//! + sade HTML gövdelidir. Ayrıca isteğe bağlı bir ek alıcı adresi tanımlanabilir.

use std::time::Duration;

use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::{now, Db};
use crate::error::{AppError, AppResult};
use crate::health::Severity;
use crate::state::AppState;

const SETTINGS_KEY: &str = "weekly_report";
const LAST_KEY: &str = "weekly_report_last";
/// Zamanı geçmiş bir özet, panel bu süre içinde ayağa kalkarsa yine gönderilir.
const CATCH_UP_SECS: i64 = 24 * 3600;
/// Denetim sayaçları ve disk geçmişi bu kadar gün saklanır.
const RETENTION_DAYS: i64 = 90;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    /// 1 = Pazartesi … 7 = Pazar.
    pub weekday: i64,
    /// Gece yarısından itibaren dakika (sunucunun yerel saatiyle).
    pub at_minute: i64,
    /// Yöneticilerin bildirim kanallarına ek olarak gönderilecek e-posta adresi (isteğe bağlı).
    pub email_to: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self { enabled: true, weekday: 1, at_minute: 8 * 60, email_to: String::new() }
    }
}

impl Settings {
    pub fn validate(&self) -> AppResult<()> {
        if !(1..=7).contains(&self.weekday) || !(0..1440).contains(&self.at_minute) {
            return Err(AppError::bad("Geçersiz gün ya da saat"));
        }
        if !self.email_to.is_empty() && (self.email_to.len() > 254 || self.email_to.parse::<lettre::message::Mailbox>().is_err()) {
            return Err(AppError::bad("Geçersiz e-posta adresi"));
        }
        Ok(())
    }
}

pub fn load_settings(db: &Db) -> Settings {
    db.setting(SETTINGS_KEY).ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()
}

/// Son gönderimin sonucu.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Last {
    pub at: i64,
    pub delivered: usize,
    pub errors: Vec<String>,
}

pub fn load_last(db: &Db) -> Last {
    db.setting(LAST_KEY).ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()
}

fn save_last(db: &Db, last: &Last) {
    if let Ok(raw) = serde_json::to_string(last) {
        let _ = db.set_setting(LAST_KEY, &raw);
    }
}

/// Ayarı kaydeder. Zamanlama değiştiyse (ya da özet yeni açıldıysa) az önce kaçırılmış bir
/// özet hemen gönderilmesin diye son gönderim anı şimdiye çekilir.
pub fn save_settings(db: &Db, settings: &Settings) -> AppResult<()> {
    settings.validate()?;
    let old = load_settings(db);
    if old.enabled != settings.enabled || old.weekday != settings.weekday || old.at_minute != settings.at_minute {
        let mut last = load_last(db);
        last.at = now();
        save_last(db, &last);
    }
    let raw = serde_json::to_string(settings).map_err(|err| AppError::internal(err.to_string()))?;
    db.set_setting(SETTINGS_KEY, &raw)
}

// ---- Zamanlama ----

/// 1970-01-01 Perşembe'dir; gün numarasından ISO hafta günü (1 = Pazartesi).
fn iso_weekday(day: i64) -> i64 {
    (day + 3).rem_euclid(7) + 1
}

/// `now` anından önceki (ya da o ana eşit) son çalışma zamanı. `offset`: sunucu saatinin UTC'ye farkı (saniye).
pub fn previous_run(weekday: i64, at_minute: i64, now: i64, offset: i64) -> i64 {
    let local = now + offset;
    let today = local.div_euclid(86400);
    (0..=7)
        .map(|back| today - back)
        .filter(|day| iso_weekday(*day) == weekday)
        .map(|day| day * 86400 + at_minute * 60)
        .find(|at| *at <= local)
        .map(|at| at - offset)
        .unwrap_or(now)
}

/// `now` anından sonraki ilk çalışma zamanı.
pub fn next_run(weekday: i64, at_minute: i64, now: i64, offset: i64) -> i64 {
    let local = now + offset;
    let today = local.div_euclid(86400);
    (0..=7)
        .map(|ahead| today + ahead)
        .filter(|day| iso_weekday(*day) == weekday)
        .map(|day| day * 86400 + at_minute * 60)
        .find(|at| *at > local)
        .map(|at| at - offset)
        .unwrap_or(now)
}

/// Özet şimdi gönderilmeli mi? Son çalışma zamanı, son gönderimden sonra gelmiş ve çok eskimemiş olmalı.
pub fn is_due(settings: &Settings, last_sent: i64, now: i64, offset: i64) -> bool {
    if !settings.enabled {
        return false;
    }
    let scheduled = previous_run(settings.weekday, settings.at_minute, now, offset);
    scheduled > last_sent && now - scheduled < CATCH_UP_SECS
}

// ---- Sayaçlar ----

/// Bir denetim turunu sayar (Sağlık merkezi her turda çağırır).
pub fn note_run(db: &Db, checks: usize) {
    let day = now().div_euclid(86400);
    let saved = db.call(|conn| {
        conn.execute(
            "INSERT INTO health_runs (day, runs, checks) VALUES (?1, 1, ?2)
             ON CONFLICT(day) DO UPDATE SET runs = runs + 1, checks = checks + excluded.checks",
            params![day, checks as i64],
        )?;
        conn.execute("DELETE FROM health_runs WHERE day < ?1", params![day - RETENTION_DAYS])
    });
    if let Err(err) = saved {
        tracing::debug!(error = %err, "denetim sayacı yazılamadı");
    }
}

/// Disklerin bugünkü doluluğunu kaydeder (haftalık büyüme için).
async fn record_disks(db: &Db) {
    let day = now().div_euclid(86400);
    let disks = crate::sys::metrics::disks().await;
    let saved = db.call(|conn| {
        for disk in &disks {
            conn.execute(
                "INSERT OR REPLACE INTO health_disk (day, mount, used, total) VALUES (?1, ?2, ?3, ?4)",
                params![day, disk.mount, disk.used as i64, disk.total as i64],
            )?;
        }
        conn.execute("DELETE FROM health_disk WHERE day < ?1", params![day - RETENTION_DAYS])
    });
    if let Err(err) = saved {
        tracing::debug!(error = %err, "disk doluluğu yazılamadı");
    }
}

// ---- Özet verisi ----

#[derive(Debug, Clone, PartialEq)]
pub struct DiskGrowth {
    pub mount: String,
    pub used: i64,
    pub total: i64,
    /// Son günlerdeki değişim (bayt) ve kaç günlük karşılaştırma olduğu; eski kayıt yoksa `None`.
    pub change: Option<(i64, i64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TopSite {
    pub domain: String,
    pub requests: i64,
    pub bytes: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub severity: Severity,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Digest {
    pub from: i64,
    pub to: i64,
    /// Sunucu saatinin UTC'ye farkı (tarihleri yerel yazmak için).
    pub offset: i64,
    pub runs: i64,
    pub checks: i64,
    pub auto_fixed: i64,
    pub auto_failed: i64,
    pub manual_fixes: i64,
    /// En son yapılan düzeltmeler: (özet, başarılı mı).
    pub fix_lines: Vec<(String, bool)>,
    pub findings: Vec<Finding>,
    pub cert_ok: i64,
    pub cert_failed: i64,
    pub backup_ok: i64,
    pub backup_failed: i64,
    pub disks: Vec<DiskGrowth>,
    pub top_sites: Vec<TopSite>,
    pub updates_total: usize,
    pub updates_security: usize,
    pub wordpress_sites: usize,
}

fn count_jobs(db: &Db, kind: &str, from: i64) -> AppResult<(i64, i64)> {
    db.call(|conn| {
        conn.query_row(
            "SELECT COALESCE(SUM(status = 'success'), 0), COALESCE(SUM(status = 'failed'), 0) FROM jobs WHERE kind = ?1 AND created_at >= ?2",
            params![kind, from],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
    })
}

/// Son yedi günün verisini toplar.
pub async fn gather(state: &AppState) -> AppResult<Digest> {
    let to = now();
    let from = to - 7 * 86400;
    let offset = crate::backup::local_offset().await;
    let db = &state.db;
    let from_day = from.div_euclid(86400);
    let (runs, checks): (i64, i64) = db.call(|conn| {
        conn.query_row("SELECT COALESCE(SUM(runs), 0), COALESCE(SUM(checks), 0) FROM health_runs WHERE day >= ?1", params![from_day], |row| Ok((row.get(0)?, row.get(1)?)))
    })?;
    let (auto_fixed, auto_failed, manual_fixes): (i64, i64, i64) = db.call(|conn| {
        conn.query_row(
            "SELECT COALESCE(SUM(automatic = 1 AND success = 1), 0), COALESCE(SUM(automatic = 1 AND success = 0), 0), COALESCE(SUM(automatic = 0), 0)
             FROM health_actions WHERE created_at >= ?1",
            params![from],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
    })?;
    let fix_lines: Vec<(String, bool)> = db.call(|conn| {
        let mut stmt = conn.prepare("SELECT summary, success FROM health_actions WHERE created_at >= ?1 ORDER BY id DESC LIMIT 8")?;
        let rows = stmt.query_map(params![from], |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)))?;
        rows.collect()
    })?;
    let findings = state.health.report().findings.into_iter().map(|finding| Finding { severity: finding.severity, title: finding.title }).collect();
    let (cert_ok, cert_failed) = count_jobs(db, "ssl.letsencrypt", from)?;
    let (backup_ok, backup_failed) = count_jobs(db, "backup", from)?;

    let mut disks = Vec::new();
    for disk in crate::sys::metrics::disks().await {
        let (used, total) = (disk.used as i64, disk.total as i64);
        let old: Option<(i64, i64)> = db.call(|conn| {
            use rusqlite::OptionalExtension;
            conn.query_row("SELECT day, used FROM health_disk WHERE mount = ?1 AND day >= ?2 ORDER BY day LIMIT 1", params![disk.mount, from_day - 1], |row| Ok((row.get(0)?, row.get(1)?))).optional()
        })?;
        let today = to.div_euclid(86400);
        let change = old.filter(|(day, _)| today - day >= 1).map(|(day, before)| (used - before, today - day));
        disks.push(DiskGrowth { mount: disk.mount, used, total, change });
    }

    let first_day = crate::stats::day_key(to - 6 * 86400);
    let top_sites = db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT s.domain, SUM(d.requests), SUM(d.bytes) FROM site_stats_daily d JOIN sites s ON s.id = d.site_id
             WHERE d.day >= ?1 GROUP BY d.site_id HAVING SUM(d.requests) > 0 ORDER BY SUM(d.requests) DESC LIMIT 5",
        )?;
        let rows = stmt.query_map(params![first_day], |row| Ok(TopSite { domain: row.get(0)?, requests: row.get(1)?, bytes: row.get(2)? }))?;
        rows.collect()
    })?;
    let (updates_total, updates_security) = state.health.pending_updates();
    Ok(Digest {
        from,
        to,
        offset,
        runs,
        checks,
        auto_fixed,
        auto_failed,
        manual_fixes,
        fix_lines,
        findings,
        cert_ok,
        cert_failed,
        backup_ok,
        backup_failed,
        disks,
        top_sites,
        updates_total,
        updates_security,
        wordpress_sites: state.health.wordpress_pending_sites(),
    })
}

// ---- Metin üretimi ----

/// `GG.AA.YYYY` (sunucu saatiyle).
pub fn format_date(unix: i64, offset: i64) -> String {
    let key = crate::stats::day_key(unix + offset);
    format!("{:02}.{:02}.{}", key % 100, key / 100 % 100, key / 10000)
}

/// Bayt sayısını okunur yazar (Türkçe ondalık virgül).
pub fn human_bytes(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let negative = bytes < 0;
    let mut value = bytes.unsigned_abs() as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let number = if unit == 0 { format!("{value:.0}") } else { format!("{value:.1}").replace('.', ",") };
    format!("{}{number} {}", if negative { "-" } else { "" }, UNITS[unit])
}

fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Critical => "Kritik",
        Severity::Warning => "Uyarı",
        Severity::Info => "Bilgi",
    }
}

pub fn subject(digest: &Digest) -> String {
    format!("PixPanel haftalık özet ({} – {})", format_date(digest.from, digest.offset), format_date(digest.to, digest.offset))
}

fn percent(used: i64, total: i64) -> i64 {
    if total <= 0 { 0 } else { (used as f64 * 100.0 / total as f64).round() as i64 }
}

fn disk_line(disk: &DiskGrowth) -> String {
    let growth = match disk.change {
        Some((delta, days)) => format!("son {days} günde {}{}", if delta >= 0 { "+" } else { "" }, human_bytes(delta)),
        None => "büyüme için henüz yeterli kayıt yok".to_string(),
    };
    format!("{}: %{} dolu ({} / {}), {growth}", disk.mount, percent(disk.used, disk.total), human_bytes(disk.used), human_bytes(disk.total))
}

/// Maddeler halinde bölümler: (başlık, satırlar). Düz metin ve HTML aynı içerikten üretilir.
fn sections(digest: &Digest) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();

    let mut checks = vec![format!("{} denetim turu yapıldı ({} ayrı kontrol).", digest.runs, digest.checks)];
    checks.push(format!(
        "Panelin kendiliğinden yaptığı düzeltmeler: {} başarılı, {} başarısız. Elle yapılan düzeltme: {}.",
        digest.auto_fixed, digest.auto_failed, digest.manual_fixes
    ));
    for (summary, success) in &digest.fix_lines {
        checks.push(format!("  {} {summary}", if *success { "+" } else { "!" }));
    }
    out.push(("Denetimler ve düzeltmeler".into(), checks));

    let pending = if digest.findings.is_empty() {
        vec!["Bekleyen sorun yok.".to_string()]
    } else {
        let mut lines = vec![format!("{} bekleyen bulgu var.", digest.findings.len())];
        lines.extend(digest.findings.iter().take(10).map(|finding| format!("[{}] {}", severity_label(finding.severity), finding.title)));
        if digest.findings.len() > 10 {
            lines.push(format!("… ve {} bulgu daha (Genel bakış sayfasında).", digest.findings.len() - 10));
        }
        lines
    };
    out.push(("Bekleyen sorunlar".into(), pending));

    out.push((
        "Sertifikalar ve yedekler".into(),
        vec![
            format!("Sertifika yenilemeleri: {} başarılı, {} başarısız.", digest.cert_ok, digest.cert_failed),
            format!("Yedekler: {} başarılı, {} başarısız.", digest.backup_ok, digest.backup_failed),
        ],
    ));

    let disks = if digest.disks.is_empty() { vec!["Disk bilgisi alınamadı.".to_string()] } else { digest.disks.iter().map(disk_line).collect() };
    out.push(("Disk".into(), disks));

    let top = if digest.top_sites.is_empty() {
        vec!["Son yedi günde kayıtlı ziyaret yok.".to_string()]
    } else {
        digest
            .top_sites
            .iter()
            .enumerate()
            .map(|(index, site)| format!("{}. {} — {} istek, {} aktarım", index + 1, site.domain, site.requests, human_bytes(site.bytes)))
            .collect()
    };
    out.push(("En çok ziyaret alan siteler".into(), top));

    let mut updates = Vec::new();
    if digest.updates_total == 0 {
        updates.push("Bekleyen paket güncellemesi yok.".to_string());
    } else {
        updates.push(format!("{} paket güncellemesi bekliyor ({} güvenlik).", digest.updates_total, digest.updates_security));
    }
    if digest.wordpress_sites > 0 {
        updates.push(format!("{} WordPress sitesinde güncelleme var.", digest.wordpress_sites));
    }
    out.push(("Bekleyen güncellemeler".into(), updates));
    out
}

/// Düz metin gövde.
pub fn render_text(digest: &Digest) -> String {
    let mut text = format!("{}\n", subject(digest));
    for (title, lines) in sections(digest) {
        text.push_str(&format!("\n{title}\n{}\n", "-".repeat(title.chars().count())));
        for line in lines {
            text.push_str(&format!("{line}\n"));
        }
    }
    text.push_str("\nBu ileti PixPanel tarafından her hafta gönderilir; Sağlık merkezi ayarlarından kapatabilirsiniz.\n");
    text
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Sade HTML gövde (satır içi biçimli; e-posta istemcileri stil bloklarını sevmez).
pub fn render_html(digest: &Digest) -> String {
    let mut html = String::from("<!doctype html><html><body style=\"font-family:Arial,Helvetica,sans-serif;color:#1f2933;max-width:640px;margin:0 auto;padding:16px\">");
    html.push_str(&format!("<h2 style=\"margin:0 0 12px\">{}</h2>", escape(&subject(digest))));
    for (title, lines) in sections(digest) {
        html.push_str(&format!("<h3 style=\"margin:20px 0 6px;font-size:15px;border-bottom:1px solid #d9dee3;padding-bottom:4px\">{}</h3><ul style=\"margin:0;padding-left:20px\">", escape(&title)));
        for line in lines {
            html.push_str(&format!("<li style=\"margin:2px 0\">{}</li>", escape(line.trim_start())));
        }
        html.push_str("</ul>");
    }
    html.push_str("<p style=\"margin-top:24px;font-size:12px;color:#6b7782\">Bu ileti PixPanel tarafından her hafta gönderilir; Sağlık merkezi ayarlarından kapatabilirsiniz.</p></body></html>");
    html
}

// ---- Gönderim ----

#[derive(Debug, Serialize)]
pub struct Delivery {
    pub subject: String,
    pub text: String,
    pub html: String,
    /// Gönderilen e-posta adresleri.
    pub emailed: Vec<String>,
    pub telegram: usize,
    pub errors: Vec<String>,
}

fn admins(db: &Db) -> Vec<i64> {
    db.call(|conn| {
        let mut stmt = conn.prepare("SELECT id FROM users WHERE role = 'admin' AND status = 'active' ORDER BY id")?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    })
    .unwrap_or_default()
}

/// Özeti üretir; `deliver` doğruysa kanallara da gönderir.
pub async fn build(state: &AppState, deliver: bool) -> AppResult<Delivery> {
    let digest = gather(state).await?;
    let mut delivery = Delivery { subject: subject(&digest), text: render_text(&digest), html: render_html(&digest), emailed: Vec::new(), telegram: 0, errors: Vec::new() };
    if !deliver {
        return Ok(delivery);
    }
    let smtp = crate::notify::load_smtp(&state.db).unwrap_or_default();
    let mut recipients: Vec<String> = Vec::new();
    let settings = load_settings(&state.db);
    if !settings.email_to.is_empty() {
        recipients.push(settings.email_to.clone());
    }
    let admin_ids = admins(&state.db);
    for admin in &admin_ids {
        crate::webhooks::emit("weekly_report", Some(*admin), &delivery.subject, json!({ "subject": delivery.subject, "body": delivery.text }));
        let Ok(channels) = crate::notify::load_channels(&state.db, *admin) else { continue };
        if channels.email_enabled && !channels.email_to.is_empty() && !recipients.contains(&channels.email_to) {
            recipients.push(channels.email_to.clone());
        }
        if channels.telegram_enabled {
            let text = format!("{}\n\n{}", delivery.subject, delivery.text);
            match crate::notify::send_telegram(&state.cfg.telegram_api, &channels.telegram_token, &channels.telegram_chat_id, &text).await {
                Ok(()) => delivery.telegram += 1,
                Err(err) => delivery.errors.push(err.to_string()),
            }
        }
    }
    for to in recipients {
        match crate::notify::send_email_html(&smtp, &to, &delivery.subject, &delivery.text, &delivery.html).await {
            Ok(()) => delivery.emailed.push(to),
            Err(err) => delivery.errors.push(format!("{to}: {err}")),
        }
    }
    if delivery.emailed.is_empty() && delivery.telegram == 0 && delivery.errors.is_empty() {
        delivery.errors.push("Gönderilecek kanal yok: bir yöneticinin Bildirimler ayarında e-posta ya da Telegram açık olmalı ya da ek alıcı adresi girilmeli.".into());
    }
    save_last(&state.db, &Last { at: now(), delivered: delivery.emailed.len() + delivery.telegram, errors: delivery.errors.clone() });
    Ok(delivery)
}

/// Zamanı geldiyse özeti gönderir. Gönderim hatalı olsa da aynı hafta yeniden denenmez.
pub async fn run_if_due(state: &AppState) {
    let settings = load_settings(&state.db);
    if !settings.enabled {
        return;
    }
    let last = load_last(&state.db);
    if last.at == 0 {
        // İlk çalışma (ya da özellik yeni geldi): geçmişe dönük bir özet gönderilmez, takvim bu andan başlar.
        save_last(&state.db, &Last { at: now(), ..last });
        return;
    }
    let offset = crate::backup::local_offset().await;
    if !is_due(&settings, last.at, now(), offset) {
        return;
    }
    // Önce işaretlenir: gönderim yarıda kalsa bile her dakika yeniden denenmez.
    save_last(&state.db, &Last { at: now(), delivered: 0, errors: Vec::new() });
    match build(state, true).await {
        Ok(delivery) => tracing::info!(emailed = delivery.emailed.len(), telegram = delivery.telegram, errors = delivery.errors.len(), "haftalık özet gönderildi"),
        Err(err) => tracing::warn!(error = %err, "haftalık özet hazırlanamadı"),
    }
}

/// Zamanlayıcı: dakikada bir bakar, saatte bir disk doluluğunu kaydeder.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let mut tick: u64 = 0;
        loop {
            if tick % 60 == 0 {
                record_disks(&state.db).await;
            }
            run_if_due(&state).await;
            tick += 1;
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-11-09 Pazartesi 00:00 UTC.
    const MONDAY: i64 = 1_794_182_400;
    const UTC3: i64 = 3 * 3600;

    #[test]
    fn weekday_numbers_follow_iso() {
        assert_eq!(iso_weekday(0), 4, "1970-01-01 Perşembe");
        assert_eq!(iso_weekday(MONDAY / 86400), 1);
        assert_eq!(iso_weekday(MONDAY / 86400 + 6), 7);
    }

    #[test]
    fn next_run_is_the_coming_monday_morning() {
        // Pazartesi 08:00 (UTC+3) = 05:00 UTC. Pazartesi 04:00 UTC'de bir sonraki çalışma aynı gün.
        let same_day = next_run(1, 8 * 60, MONDAY + 4 * 3600, UTC3);
        assert_eq!(same_day, MONDAY + 5 * 3600);
        // 05:00 UTC'den sonra gelecek hafta.
        let next_week = next_run(1, 8 * 60, MONDAY + 5 * 3600, UTC3);
        assert_eq!(next_week, MONDAY + 7 * 86400 + 5 * 3600);
        // Perşembe için aynı hafta.
        assert_eq!(next_run(4, 0, MONDAY + 3600, 0), MONDAY + 3 * 86400);
    }

    #[test]
    fn previous_run_is_the_latest_scheduled_moment() {
        let at = MONDAY + 5 * 3600; // Pazartesi 08:00 yerel
        assert_eq!(previous_run(1, 8 * 60, at, UTC3), at, "tam zamanında kendisi");
        assert_eq!(previous_run(1, 8 * 60, at + 3 * 86400, UTC3), at);
        assert_eq!(previous_run(1, 8 * 60, at - 1, UTC3), at - 7 * 86400, "bir saniye önce geçen haftaki");
        // Gün sınırı: yerel saat farkı yüzünden UTC'de önceki güne düşen çalışma.
        assert_eq!(previous_run(1, 60, MONDAY + 3600, UTC3), MONDAY - 2 * 3600);
    }

    #[test]
    fn report_is_due_once_per_week() {
        let settings = Settings::default(); // Pazartesi 08:00
        let at = MONDAY + 5 * 3600;
        assert!(!is_due(&settings, 0, at - 60, UTC3), "zamanı gelmedi: önceki hafta çok eski, yakalama penceresi dışında");
        assert!(is_due(&settings, at - 7 * 86400, at + 30, UTC3), "zamanı geldi");
        assert!(!is_due(&settings, at + 1, at + 120, UTC3), "bu hafta gönderildi");
        assert!(is_due(&settings, at - 7 * 86400, at + 2 * 3600, UTC3), "panel geç ayağa kalktıysa yakalanır");
        assert!(!is_due(&settings, at - 7 * 86400, at + 30 * 3600, UTC3), "bir günden eski kaçırılmış özet gönderilmez");
        let off = Settings { enabled: false, ..settings };
        assert!(!is_due(&off, 0, at + 30, UTC3));
    }

    #[test]
    fn settings_are_validated() {
        assert!(Settings::default().validate().is_ok());
        assert!(Settings { weekday: 0, ..Settings::default() }.validate().is_err());
        assert!(Settings { weekday: 8, ..Settings::default() }.validate().is_err());
        assert!(Settings { at_minute: 1440, ..Settings::default() }.validate().is_err());
        assert!(Settings { email_to: "bozuk".into(), ..Settings::default() }.validate().is_err());
        assert!(Settings { email_to: "yonetici@ornek.com".into(), ..Settings::default() }.validate().is_ok());
    }

    fn digest() -> Digest {
        Digest {
            from: MONDAY - 7 * 86400,
            to: MONDAY,
            offset: 0,
            runs: 10_080,
            checks: 241_920,
            auto_fixed: 3,
            auto_failed: 1,
            manual_fixes: 2,
            fix_lines: vec![("nginx çöktü (panel düzeltti)".into(), true), ("Sertifika alınamadı".into(), false)],
            findings: vec![
                Finding { severity: Severity::Critical, title: "Disk dolmak üzere: /".into() },
                Finding { severity: Severity::Info, title: "<script>alert(1)</script> kullanıyor".into() },
            ],
            cert_ok: 5,
            cert_failed: 1,
            backup_ok: 7,
            backup_failed: 0,
            disks: vec![
                DiskGrowth { mount: "/".into(), used: 18 * 1_073_741_824, total: 40 * 1_073_741_824, change: Some((1_395_864_371, 6)) },
                DiskGrowth { mount: "/data".into(), used: 1, total: 100, change: None },
            ],
            top_sites: vec![TopSite { domain: "a.test".into(), requests: 12_345, bytes: 1_288_490_189 }],
            updates_total: 12,
            updates_security: 3,
            wordpress_sites: 2,
        }
    }

    #[test]
    fn text_report_covers_every_section() {
        let text = render_text(&digest());
        assert!(text.starts_with("PixPanel haftalık özet (02.11.2026 – 09.11.2026)"), "{text}");
        for expected in [
            "10080 denetim turu yapıldı (241920 ayrı kontrol)",
            "3 başarılı, 1 başarısız. Elle yapılan düzeltme: 2",
            "+ nginx çöktü (panel düzeltti)",
            "! Sertifika alınamadı",
            "2 bekleyen bulgu var",
            "[Kritik] Disk dolmak üzere: /",
            "Sertifika yenilemeleri: 5 başarılı, 1 başarısız",
            "Yedekler: 7 başarılı, 0 başarısız",
            "/: %45 dolu (18,0 GB / 40,0 GB), son 6 günde +1,3 GB",
            "büyüme için henüz yeterli kayıt yok",
            "1. a.test — 12345 istek, 1,2 GB aktarım",
            "12 paket güncellemesi bekliyor (3 güvenlik)",
            "2 WordPress sitesinde güncelleme var",
        ] {
            assert!(text.contains(expected), "eksik: {expected}\n{text}");
        }
    }

    #[test]
    fn empty_week_still_produces_a_readable_report() {
        let empty = Digest {
            runs: 0,
            checks: 0,
            auto_fixed: 0,
            auto_failed: 0,
            manual_fixes: 0,
            fix_lines: vec![],
            findings: vec![],
            cert_ok: 0,
            cert_failed: 0,
            backup_ok: 0,
            backup_failed: 0,
            disks: vec![],
            top_sites: vec![],
            updates_total: 0,
            updates_security: 0,
            wordpress_sites: 0,
            ..digest()
        };
        let text = render_text(&empty);
        assert!(text.contains("Bekleyen sorun yok.") && text.contains("Bekleyen paket güncellemesi yok.") && text.contains("kayıtlı ziyaret yok"));
    }

    #[test]
    fn html_report_escapes_titles() {
        let html = render_html(&digest());
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"), "başlıktaki etiketler kaçışlanmalı");
        assert!(!html.contains("<script>"));
        assert!(html.contains("<li"));
    }

    #[test]
    fn bytes_are_humanised() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1536), "1,5 KB");
        assert_eq!(human_bytes(-1_048_576), "-1,0 MB");
        assert_eq!(format_date(MONDAY, 0), "09.11.2026");
        assert_eq!(format_date(MONDAY - 1, 3 * 3600), "09.11.2026", "yerel saat farkı gün sınırını aşar");
    }
}
