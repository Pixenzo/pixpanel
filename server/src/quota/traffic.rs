//! Aylık trafik sınırının uygulanması.
//!
//! Paketteki `traffic_gb`, hesabın bütün sitelerinin takvim ayındaki (UTC) toplam aktarımını
//! sınırlar. İstatistik toplayıcısı her çalıştığında (beş dakikada bir) her hesap için:
//!
//! - %80'e ulaşınca ve %100'e ulaşınca hesaba ve yöneticilere birer bildirim gider. Her düzey
//!   ayda yalnız bir kez bildirilir (`traffic_state`); ay değişince sayaç sıfırlanır.
//! - Paketin davranışı `suspend` ise sınır dolunca hesabın etkin siteleri askıya alınır. Bu siteler
//!   `traffic_suspensions` tablosuna işlenir; ay değişince, sınır yükselince ya da paket
//!   değişince yalnızca bu tabloda kayıtlı siteler kendiliğinden geri açılır. Yöneticinin elle
//!   askıya aldığı site tabloda olmadığından (elle askıya alınca kayıt da silinir) asla açılmaz.

use std::collections::HashSet;

use rusqlite::{params, OptionalExtension};

use crate::db::{now, Db};
use crate::error::AppResult;
use crate::hosting::sites::{self, Site};
use crate::state::AppState;

pub const ACTION_NOTIFY: &str = "notify";
pub const ACTION_SUSPEND: &str = "suspend";

/// Kotanın yüzde kaçında ilk uyarı gider.
pub const WARN_PERCENT: i64 = 80;

/// Aynı anda iki turun (toplayıcı döngüsü ve paket değişikliğinin tetiklediği tur) aynı hesabı
/// işleyip bildirimi iki kez göndermesini önler.
static ENFORCE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Kullanıma göre düzey: 0 (sorun yok), 1 (%80 aşıldı), 2 (sınır doldu).
pub fn threshold_level(used_bytes: i64, limit_gb: i64) -> i64 {
    let limit = limit_gb.saturating_mul(1024 * 1024 * 1024);
    if limit <= 0 {
        return 0;
    }
    if used_bytes >= limit {
        2
    } else if used_bytes.saturating_mul(100) >= limit.saturating_mul(WARN_PERCENT) {
        1
    } else {
        0
    }
}

/// Bir hesap için bir turun kararı.
#[derive(Debug, PartialEq)]
pub struct Decision {
    /// Şu anki düzey.
    pub level: i64,
    /// Gönderilecek bildirimin düzeyi (bu ay bu düzey daha önce bildirilmediyse).
    pub notify: Option<i64>,
    /// Bu ay için kaydedilecek "en son bildirilen düzey".
    pub warned: i64,
    /// Hesabın siteleri askıda olmalı mı?
    pub suspend: bool,
}

/// Kullanım, paket ve kayıtlı duruma göre ne yapılacağını belirler (saf işlev).
///
/// - `stored`: kayıtlı (ay, en son bildirilen düzey); başka bir aya aitse yok sayılır, yani
///   ay değişince bildirimler yeniden başlar.
/// - Düzey kayıtlı değerin altına inerse (sınır yükseltildi) kayıt da iner; böylece sınır
///   yeniden aşılırsa yeni bir bildirim gider.
pub fn decide(limit_gb: Option<i64>, action: Option<&str>, used_bytes: i64, stored: Option<(i64, i64)>, month: i64) -> Decision {
    let Some(limit_gb) = limit_gb else {
        return Decision { level: 0, notify: None, warned: 0, suspend: false };
    };
    let previous = stored.filter(|(stored_month, _)| *stored_month == month).map_or(0, |(_, warned)| warned);
    let level = threshold_level(used_bytes, limit_gb);
    Decision { level, notify: (level > previous).then_some(level), warned: level, suspend: level == 2 && action == Some(ACTION_SUSPEND) }
}

/// Bir sitenin durumu: kimliği, askıda mı, trafik yüzünden askıya alınmış olarak kayıtlı mı.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SiteState {
    pub id: i64,
    pub suspended: bool,
    pub recorded: bool,
}

/// Site başına yapılacaklar.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SitePlan {
    /// Askıya alınacak etkin siteler.
    pub suspend: Vec<i64>,
    /// Geri açılacak siteler (yalnız trafik yüzünden askıya alınmış olanlar).
    pub resume: Vec<i64>,
    /// Kaydı bayat kalanlar (site artık etkin): yalnız kayıt silinir.
    pub forget: Vec<i64>,
}

/// Askıya alma kararına göre site planı. Kayıtsız askıdaki site (yöneticinin elle askıya aldığı)
/// hiçbir koşulda açılmaz.
pub fn plan_sites(sites: &[SiteState], suspend: bool) -> SitePlan {
    let mut plan = SitePlan::default();
    for site in sites {
        match (suspend, site.suspended, site.recorded) {
            (true, false, _) => plan.suspend.push(site.id),
            (false, true, true) => plan.resume.push(site.id),
            (false, false, true) => plan.forget.push(site.id),
            _ => {}
        }
    }
    plan
}

fn load_state(db: &Db, account_id: i64) -> AppResult<Option<(i64, i64)>> {
    db.call(|conn| conn.query_row("SELECT month, warned_level FROM traffic_state WHERE user_id = ?1", params![account_id], |row| Ok((row.get(0)?, row.get(1)?))).optional())
}

fn save_state(db: &Db, account_id: i64, month: i64, warned: i64) -> AppResult<()> {
    db.call(|conn| {
        conn.execute(
            "INSERT INTO traffic_state (user_id, month, warned_level) VALUES (?1, ?2, ?3)
             ON CONFLICT(user_id) DO UPDATE SET month = excluded.month, warned_level = excluded.warned_level",
            params![account_id, month, warned],
        )
    })?;
    Ok(())
}

/// Hesabın trafik yüzünden askıya alınmış kayıtlı siteleri.
fn recorded_sites(db: &Db, account_id: i64) -> AppResult<HashSet<i64>> {
    db.call(|conn| {
        let mut stmt = conn.prepare("SELECT site_id FROM traffic_suspensions WHERE user_id = ?1")?;
        let rows = stmt.query_map(params![account_id], |row| row.get(0))?;
        rows.collect()
    })
}

/// Siteyi trafik yüzünden askıya alınanlar listesinden çıkarır (yönetici elle askıya aldığında ya da
/// site geri açıldığında çağrılır); böylece site kendiliğinden geri açılmaz.
pub fn forget_site(db: &Db, site_id: i64) {
    if let Err(err) = db.call(|conn| conn.execute("DELETE FROM traffic_suspensions WHERE site_id = ?1", params![site_id])) {
        tracing::warn!(error = %err, site_id, "trafik askı kaydı silinemedi");
    }
}

/// Hesabın trafik sınırı yüzünden şu an askıda olan sitelerinin sayısı.
pub fn suspended_site_count(db: &Db, account_id: i64) -> AppResult<i64> {
    db.call(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM traffic_suspensions t JOIN sites s ON s.id = t.site_id WHERE t.user_id = ?1 AND s.status = 'suspended'",
            params![account_id],
            |row| row.get(0),
        )
    })
}

/// Sistem adına denetim kaydı (kullanıcı eylemi değil, panelin kendiliğinden yaptığı işlem).
fn audit_system(db: &Db, action: &str, target: &str, detail: &str, account_id: i64) {
    let result = db.call(|conn| {
        conn.execute(
            "INSERT INTO audit_log (user_id, username, action, target, detail, ip, created_at) VALUES (NULL, 'system', ?1, ?2, ?3, '', ?4)",
            params![action, target, detail, now()],
        )
    });
    if let Err(err) = result {
        tracing::warn!(error = %err, action, "denetim kaydı yazılamadı");
    }
    crate::events::changed(action, account_id);
}

fn gb_text(bytes: i64) -> String {
    format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

/// Düzeye göre bildirimin konusu ve metni.
fn message(level: i64, username: &str, used: i64, limit_gb: i64, suspends: bool) -> (String, String) {
    let used_text = gb_text(used);
    if level >= 2 {
        let consequence = if suspends {
            "Paketiniz gereği sitelerinizin yayını durduruldu (askıya alındı); ay başında ya da paketiniz yükseltildiğinde kendiliğinden yeniden açılır."
        } else {
            "Siteleriniz çalışmaya devam ediyor."
        };
        (
            format!("{username}: aylık trafik sınırı doldu"),
            format!("Hesabın bu ayki trafiği {used_text}; paketin aylık sınırı {limit_gb} GB. {consequence}"),
        )
    } else {
        (
            format!("{username}: aylık trafik sınırının %{WARN_PERCENT}'ine ulaşıldı"),
            format!("Hesabın bu ayki trafiği {used_text}; paketin aylık sınırı {limit_gb} GB (yüzde {WARN_PERCENT} aşıldı). Sınır dolduğunda paketin davranışı uygulanır; paketi yükseltmeyi düşünün."),
        )
    }
}

/// Bildirimi hesaba ve etkin yöneticilere gönderir.
async fn notify(state: &AppState, account_id: i64, level: i64, used: i64, limit_gb: i64, suspends: bool) {
    let username: String = state
        .db
        .call(|conn| conn.query_row("SELECT username FROM users WHERE id = ?1", params![account_id], |row| row.get(0)))
        .unwrap_or_default();
    let (subject, body) = message(level, &username, used, limit_gb, suspends);
    let admins: Vec<i64> = state
        .db
        .call(|conn| {
            let mut stmt = conn.prepare("SELECT id FROM users WHERE role = 'admin' AND status = 'active'")?;
            let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
            rows.collect()
        })
        .unwrap_or_default();
    let mut recipients = vec![account_id];
    recipients.extend(admins.into_iter().filter(|admin| *admin != account_id));
    for recipient in recipients {
        crate::notify::send_event(state, recipient, "traffic_warning", &format!("PixPanel: {subject}"), &body).await;
    }
}

/// Planı uygular: siteleri askıya alır ya da geri açar. `sys_lock` altında, site her seferinde
/// yeniden okunur (plan sırasında elle değiştirilmiş olabilir).
async fn apply_plan(state: &AppState, account_id: i64, username: &str, month: i64, plan: &SitePlan) {
    for id in &plan.forget {
        forget_site(&state.db, *id);
    }
    let mut suspended: Vec<String> = Vec::new();
    for id in &plan.suspend {
        let _guard = state.sys_lock.lock().await;
        let Ok(Some(site)) = sites::load(&state.db, *id) else { continue };
        if site.status != "active" {
            continue;
        }
        if switch_status(state, &site, "suspended").await {
            let recorded = state.db.call(|conn| {
                conn.execute(
                    "INSERT INTO traffic_suspensions (site_id, user_id, month, created_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(site_id) DO UPDATE SET user_id = excluded.user_id, month = excluded.month, created_at = excluded.created_at",
                    params![site.id, account_id, month, now()],
                )
            });
            if let Err(err) = recorded {
                tracing::error!(error = %err, domain = %site.domain, "trafik askı kaydı yazılamadı; site elle açılmalı");
            }
            suspended.push(site.domain.clone());
        }
    }
    if !suspended.is_empty() {
        audit_system(&state.db, "account.traffic_suspend", username, &suspended.join(", "), account_id);
    }
    let mut resumed: Vec<String> = Vec::new();
    for id in &plan.resume {
        let _guard = state.sys_lock.lock().await;
        let Ok(Some(site)) = sites::load(&state.db, *id) else { continue };
        // Kayıt, bu arada yönetici siteyi elle askıya aldıysa zaten silinmiştir.
        let still_recorded = state.db.call(|conn| conn.query_row("SELECT EXISTS(SELECT 1 FROM traffic_suspensions WHERE site_id = ?1)", params![site.id], |row| row.get::<_, bool>(0))).unwrap_or(false);
        if !still_recorded {
            continue;
        }
        if site.status != "suspended" {
            forget_site(&state.db, site.id);
            continue;
        }
        if switch_status(state, &site, "active").await {
            forget_site(&state.db, site.id);
            resumed.push(site.domain.clone());
        }
    }
    if !resumed.is_empty() {
        audit_system(&state.db, "account.traffic_resume", username, &resumed.join(", "), account_id);
    }
}

/// Sitenin durumunu değiştirip sisteme uygular; uygulanamazsa eski hâline döndürür. Başarıyı döner.
async fn switch_status(state: &AppState, site: &Site, status: &str) -> bool {
    let mut changed = site.clone();
    changed.status = status.to_string();
    if let Err(err) = sites::save(&state.db, &changed) {
        tracing::error!(error = %err, domain = %site.domain, "site durumu kaydedilemedi");
        return false;
    }
    if let Err(err) = sites::provision(&changed).await {
        tracing::error!(error = %err, domain = %site.domain, status, "site durumu uygulanamadı; eski hâline dönülüyor");
        let _ = sites::save(&state.db, site);
        if let Err(restore) = sites::provision(site).await {
            tracing::error!(error = %restore, domain = %site.domain, "site eski ayarlarına döndürülemedi");
        }
        return false;
    }
    true
}

async fn enforce_locked(state: &AppState, account_id: i64) -> AppResult<()> {
    let limits = super::limits_for(&state.db, account_id)?.map(|(_, limits)| limits);
    let limit_gb = limits.as_ref().and_then(|limits| limits.traffic_gb);
    let action = limits.and_then(|limits| limits.traffic_action);
    let month = crate::stats::month_key(now());
    let used = crate::stats::account_month_bytes(&state.db, account_id)?;
    let stored = load_state(&state.db, account_id)?;
    let decision = decide(limit_gb, action.as_deref(), used, stored, month);
    // Sınırı olmayan ve kaydı da olmayan hesaba hiçbir şey yazılmaz.
    if limit_gb.is_some() || stored.is_some() {
        if stored != Some((month, decision.warned)) {
            save_state(&state.db, account_id, month, decision.warned)?;
        }
    }
    if let (Some(level), Some(limit_gb)) = (decision.notify, limit_gb) {
        notify(state, account_id, level, used, limit_gb, action.as_deref() == Some(ACTION_SUSPEND)).await;
    }
    let account_sites = sites::list_by_user(&state.db, account_id)?;
    let recorded = recorded_sites(&state.db, account_id)?;
    let states: Vec<SiteState> = account_sites.iter().map(|site| SiteState { id: site.id, suspended: site.status != "active", recorded: recorded.contains(&site.id) }).collect();
    // Hesabı olmayan (silinmiş) siteler tablodan zaten düşer; kayıtlı ama listede olmayan site yoktur.
    let plan = plan_sites(&states, decision.suspend);
    if plan != SitePlan::default() {
        let username: String = state.db.call(|conn| conn.query_row("SELECT username FROM users WHERE id = ?1", params![account_id], |row| row.get(0)))?;
        apply_plan(state, account_id, &username, month, &plan).await;
    }
    Ok(())
}

/// Bir hesabın trafik sınırını denetler ve gerekeni yapar.
pub async fn enforce_account(state: &AppState, account_id: i64) -> AppResult<()> {
    let _turn = ENFORCE_LOCK.lock().await;
    enforce_locked(state, account_id).await
}

/// Bütün müşteri ve bayi hesaplarının trafik sınırını denetler (yöneticinin ve alt hesapların
/// sınırı yoktur). Hata bir hesapta kalır, öbürleri işlenmeye devam eder.
pub async fn enforce_all(state: &AppState) {
    let _turn = ENFORCE_LOCK.lock().await;
    let accounts: Vec<i64> = match state.db.call(|conn| {
        let mut stmt = conn.prepare("SELECT id FROM users WHERE role IN ('user', 'reseller') ORDER BY id")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    }) {
        Ok(accounts) => accounts,
        Err(err) => {
            tracing::warn!(error = %err, "trafik sınırı için hesaplar okunamadı");
            return;
        }
    };
    for account in accounts {
        if let Err(err) = enforce_locked(state, account).await {
            tracing::warn!(error = %err, account, "trafik sınırı denetlenemedi");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: i64 = 1024 * 1024 * 1024;

    #[test]
    fn threshold_levels() {
        assert_eq!(threshold_level(0, 100), 0);
        assert_eq!(threshold_level(79 * GB, 100), 0);
        assert_eq!(threshold_level(80 * GB, 100), 1);
        assert_eq!(threshold_level(99 * GB, 100), 1);
        assert_eq!(threshold_level(100 * GB, 100), 2);
        assert_eq!(threshold_level(500 * GB, 100), 2);
        // %80 sınırı yuvarlanmaz: bir bayt eksik hâlâ 0.
        assert_eq!(threshold_level(80 * GB - 1, 100), 0);
        // Sınırı sıfır olan paket uyarı üretmez.
        assert_eq!(threshold_level(5 * GB, 0), 0);
    }

    #[test]
    fn no_limit_means_no_action() {
        let decision = decide(None, Some(ACTION_SUSPEND), 900 * GB, Some((202611, 2)), 202611);
        assert_eq!(decision, Decision { level: 0, notify: None, warned: 0, suspend: false });
    }

    #[test]
    fn notifications_are_sent_once_per_level_per_month() {
        let month = 202611;
        // %80: bir kez.
        let first = decide(Some(100), None, 80 * GB, None, month);
        assert_eq!((first.notify, first.warned, first.suspend), (Some(1), 1, false));
        // Aynı düzeyde yinelenmez.
        let again = decide(Some(100), None, 85 * GB, Some((month, first.warned)), month);
        assert_eq!((again.notify, again.warned), (None, 1));
        // %100: ikinci ve son bildirim.
        let full = decide(Some(100), None, 100 * GB, Some((month, again.warned)), month);
        assert_eq!((full.notify, full.warned), (Some(2), 2));
        let repeat = decide(Some(100), None, 150 * GB, Some((month, full.warned)), month);
        assert_eq!((repeat.notify, repeat.warned), (None, 2));
        // Yalnız bildirim davranışında asla askıya alma istenmez.
        assert!(!full.suspend && !repeat.suspend);
    }

    #[test]
    fn jumping_straight_to_full_sends_a_single_notification() {
        let decision = decide(Some(100), Some(ACTION_NOTIFY), 120 * GB, None, 202611);
        assert_eq!((decision.notify, decision.warned, decision.suspend), (Some(2), 2, false));
    }

    #[test]
    fn new_month_restarts_notifications_and_lifts_suspension() {
        // Ekimde sınır dolmuş, bildirilmiş ve askıya alınmış.
        let october = decide(Some(100), Some(ACTION_SUSPEND), 130 * GB, Some((202610, 2)), 202610);
        assert_eq!((october.notify, october.suspend), (None, true));
        // Kasımın ilk turunda kullanım sıfırdan başlar: askı kalkmalı, kayıtlı düzey eski aya ait olduğundan yok sayılır.
        let november = decide(Some(100), Some(ACTION_SUSPEND), 2 * GB, Some((202610, 2)), 202611);
        assert_eq!((november.level, november.notify, november.warned, november.suspend), (0, None, 0, false));
        // Kasımda yeniden %80'e gelince eski aydan kalan kayıt bildirimi engellemez.
        let later = decide(Some(100), Some(ACTION_SUSPEND), 81 * GB, Some((202610, 2)), 202611);
        assert_eq!((later.notify, later.warned), (Some(1), 1));
    }

    #[test]
    fn raising_the_limit_lifts_suspension_and_rearms_notifications() {
        let month = 202611;
        let suspended = decide(Some(100), Some(ACTION_SUSPEND), 110 * GB, Some((month, 2)), month);
        assert!(suspended.suspend);
        // Sınır 200 GB'a çıkarıldı: askı kalkar, kayıt düzeye iner.
        let raised = decide(Some(200), Some(ACTION_SUSPEND), 110 * GB, Some((month, 2)), month);
        assert_eq!((raised.level, raised.notify, raised.warned, raised.suspend), (0, None, 0, false));
        // Davranış askıdan yalnız bildirime çevrilirse de askı kalkar.
        let notify_only = decide(Some(100), Some(ACTION_NOTIFY), 110 * GB, Some((month, 2)), month);
        assert!(!notify_only.suspend);
        // Paket kaldırıldı: sınır yok, askı kalkar.
        assert!(!decide(None, None, 110 * GB, Some((month, 2)), month).suspend);
        // Sınır yeniden aşılırsa yeni bildirim gider.
        let again = decide(Some(200), Some(ACTION_SUSPEND), 200 * GB, Some((month, raised.warned)), month);
        assert_eq!((again.notify, again.suspend), (Some(2), true));
    }

    #[test]
    fn plan_suspends_only_active_sites() {
        let sites = [
            SiteState { id: 1, suspended: false, recorded: false },
            SiteState { id: 2, suspended: true, recorded: false }, // yöneticinin elle askıya aldığı
            SiteState { id: 3, suspended: true, recorded: true },  // zaten trafik yüzünden askıda
            SiteState { id: 4, suspended: false, recorded: true }, // yönetici elle açmış; yeniden askıya alınır
        ];
        let plan = plan_sites(&sites, true);
        assert_eq!(plan, SitePlan { suspend: vec![1, 4], resume: vec![], forget: vec![] });
    }

    #[test]
    fn plan_never_resumes_manually_suspended_sites() {
        let sites = [
            SiteState { id: 1, suspended: true, recorded: true },   // trafik yüzünden: açılır
            SiteState { id: 2, suspended: true, recorded: false },  // elle askıya alınmış: dokunulmaz
            SiteState { id: 3, suspended: false, recorded: true },  // bayat kayıt: yalnız silinir
            SiteState { id: 4, suspended: false, recorded: false }, // sıradan etkin site
        ];
        let plan = plan_sites(&sites, false);
        assert_eq!(plan, SitePlan { suspend: vec![], resume: vec![1], forget: vec![3] });
    }

    #[test]
    fn messages_describe_the_consequence() {
        let (subject, body) = message(1, "musteri", 85 * GB, 100, true);
        assert!(subject.contains("%80") && body.contains("100 GB"));
        let (_, suspended) = message(2, "musteri", 101 * GB, 100, true);
        assert!(suspended.contains("askıya alındı"));
        let (_, kept) = message(2, "musteri", 101 * GB, 100, false);
        assert!(kept.contains("çalışmaya devam ediyor"));
    }
}
