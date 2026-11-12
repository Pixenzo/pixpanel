//! Varsayılan yedek planı: yönetici bir yedek yeri, sıklık ve saklama ilkesi seçer; ayar açıksa
//! yeni oluşturulan her site kendiliğinden bu plana eklenir.
//!
//! Plan panelin yönettiği tek bir plandır ("Yeni siteler (varsayılan)"); kapsamı site kimlikleriyle
//! büyür. Site zaten başka bir zamanlanmış plana (ör. "tüm siteler") giriyorsa hiçbir şey yapılmaz.
//! Aynı mantık Sağlık merkezindeki "hiçbir yedek planına girmeyen site" bulgusunun tek tıkla
//! düzeltmesinde de kullanılır.

use serde::{Deserialize, Serialize};

use super::{Plan, Scope};
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::hosting::sites::Site;
use crate::state::AppState;

const SETTINGS_KEY: &str = "backup_default_plan";
const PLAN_NAME: &str = "Yeni siteler (varsayılan)";

/// Plan değişiklikleri sıraya girsin (art arda site açılırken aynı planın üstüne yazılmasın).
static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Yeni siteler kendiliğinden plana eklensin.
    pub enabled: bool,
    pub target_id: Option<i64>,
    /// `daily`, `weekly` ya da `monthly`.
    pub frequency: String,
    /// Gece yarısından itibaren dakika (sunucunun yerel saatiyle).
    pub at_minute: i64,
    /// 1 = Pazartesi … 7 = Pazar.
    pub weekday: i64,
    pub month_day: i64,
    pub keep_daily: i64,
    pub keep_weekly: i64,
    pub keep_monthly: i64,
    /// Panelin oluşturduğu planın kimliği (istemci değiştiremez).
    pub plan_id: Option<i64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            target_id: None,
            frequency: "daily".into(),
            at_minute: 180,
            weekday: 1,
            month_day: 1,
            keep_daily: 7,
            keep_weekly: 4,
            keep_monthly: 6,
            plan_id: None,
        }
    }
}

pub fn load(state: &AppState) -> Settings {
    state.db.setting(SETTINGS_KEY).ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()
}

fn save(state: &AppState, settings: &Settings) -> AppResult<()> {
    let raw = serde_json::to_string(settings).map_err(|err| AppError::internal(err.to_string()))?;
    state.db.set_setting(SETTINGS_KEY, &raw)
}

fn first_admin(state: &AppState) -> AppResult<i64> {
    state.db.call(|conn| conn.query_row("SELECT id FROM users WHERE role = 'admin' ORDER BY id LIMIT 1", [], |row| row.get(0)))
}

/// Ayarlardan, verilen sitelerle bir plan kurar (kaydetmez).
pub fn build_plan(settings: &Settings, owner: i64, target_id: i64, site_ids: Vec<i64>) -> Plan {
    Plan {
        id: 0,
        user_id: owner,
        name: PLAN_NAME.into(),
        target_id,
        scope: Scope { site_ids, ..Scope::default() },
        frequency: settings.frequency.clone(),
        at_minute: settings.at_minute,
        weekday: settings.weekday,
        month_day: settings.month_day,
        keep_daily: settings.keep_daily,
        keep_weekly: settings.keep_weekly,
        keep_monthly: settings.keep_monthly,
        enabled: true,
        next_run_at: None,
        last_run_at: None,
        last_status: String::new(),
        last_job_id: None,
        created_at: now(),
    }
}

/// Ayar gövdesini doğrular.
pub fn validate(state: &AppState, settings: &Settings) -> AppResult<()> {
    if !matches!(settings.frequency.as_str(), "daily" | "weekly" | "monthly") {
        return Err(AppError::bad("Sıklık günlük, haftalık ya da aylık olmalı"));
    }
    if let Some(id) = settings.target_id {
        super::load_target(&state.db, id).map_err(|_| AppError::bad("Seçilen yedek yeri bulunamadı"))?;
    } else if settings.enabled {
        return Err(AppError::bad("Önce bir yedek yeri seçin"));
    }
    // Plan doğrulaması (zaman ve saklama sınırları) örnek bir planla yapılır.
    build_plan(settings, 0, settings.target_id.unwrap_or(0), vec![0]).validate()
}

/// Kaydeder; panelin oluşturduğu plan varsa yeni ayarlara uydurur.
pub async fn update(state: &AppState, mut settings: Settings) -> AppResult<Settings> {
    validate(state, &settings)?;
    let _turn = LOCK.lock().await;
    let current = load(state);
    settings.plan_id = current.plan_id;
    if let (Some(plan_id), Some(target_id)) = (settings.plan_id, settings.target_id) {
        if let Ok(mut plan) = super::load_plan(&state.db, plan_id) {
            plan.target_id = target_id;
            plan.frequency = settings.frequency.clone();
            plan.at_minute = settings.at_minute;
            plan.weekday = settings.weekday;
            plan.month_day = settings.month_day;
            plan.keep_daily = settings.keep_daily;
            plan.keep_weekly = settings.keep_weekly;
            plan.keep_monthly = settings.keep_monthly;
            plan.next_run_at = plan.next_run(now(), super::local_offset().await);
            super::save_plan(&state.db, &plan)?;
        }
    }
    save(state, &settings)?;
    Ok(settings)
}

/// Zamanlanmış (açık ve elle olmayan) hiçbir plana girmeyen siteler.
pub fn uncovered_ids(plans: &[Plan], ids: &[i64]) -> Vec<i64> {
    let scheduled: Vec<&Plan> = plans.iter().filter(|plan| plan.enabled && plan.frequency != "manual").collect();
    ids.iter().copied().filter(|id| !scheduled.iter().any(|plan| plan.scope.all_sites || plan.scope.site_ids.contains(id))).collect()
}

pub fn uncovered_sites<'a>(plans: &[Plan], sites: &'a [Site]) -> Vec<&'a Site> {
    let ids: Vec<i64> = sites.iter().map(|site| site.id).collect();
    let uncovered = uncovered_ids(plans, &ids);
    sites.iter().filter(|site| uncovered.contains(&site.id)).collect()
}

/// Panelin yönettiği planı döner; yoksa (ya da silinmişse) ayarlardan oluşturur.
async fn ensure_plan(state: &AppState, user_id: Option<i64>) -> AppResult<Plan> {
    let settings = load(state);
    if let Some(plan_id) = settings.plan_id {
        if let Ok(plan) = super::load_plan(&state.db, plan_id) {
            return Ok(plan);
        }
    }
    let target_id = match settings.target_id {
        Some(id) if super::load_target(&state.db, id).is_ok() => id,
        _ => super::list_targets(&state.db)?.into_iter().next().ok_or_else(|| AppError::bad("Önce bir yedek yeri ekleyin"))?.id,
    };
    let owner = match user_id {
        Some(id) => id,
        None => first_admin(state)?,
    };
    let mut plan = build_plan(&settings, owner, target_id, Vec::new());
    // Kapsam boşken plan doğrulanamaz; ilk site eklenirken doğrulanır.
    let existing = super::list_plans(&state.db)?;
    let mut name = PLAN_NAME.to_string();
    let mut suffix = 2;
    while existing.iter().any(|other| other.name == name) {
        name = format!("{PLAN_NAME} {suffix}");
        suffix += 1;
    }
    plan.name = name;
    Ok(plan)
}

/// Verilen siteleri plana ekler ve kaydeder; `(plan adı, eklenen site sayısı)` döner.
async fn add_to_plan(state: &AppState, user_id: Option<i64>, site_ids: &[i64]) -> AppResult<(String, usize)> {
    let mut plan = ensure_plan(state, user_id).await?;
    let mut added = 0;
    for id in site_ids {
        if !plan.scope.site_ids.contains(id) {
            plan.scope.site_ids.push(*id);
            added += 1;
        }
    }
    plan.validate()?;
    plan.next_run_at = plan.next_run(now(), super::local_offset().await);
    let name = plan.name.clone();
    let new_plan = plan.id == 0;
    let id = super::save_plan(&state.db, &plan)?;
    if new_plan {
        let mut settings = load(state);
        settings.plan_id = Some(id);
        save(state, &settings)?;
    }
    Ok((name, added))
}

/// Yeni açılan siteyi, ayar açıksa varsayılan plana ekler. Site zaten zamanlanmış bir plana
/// giriyorsa ya da yedekleme kurulu değilse hiçbir şey yapmaz. Plan adını döner.
pub async fn attach_site(state: &AppState, site_id: i64) -> AppResult<Option<String>> {
    if !load(state).enabled || !super::is_installed() {
        return Ok(None);
    }
    let _turn = LOCK.lock().await;
    let plans = super::list_plans(&state.db)?;
    if uncovered_ids(&plans, &[site_id]).is_empty() {
        return Ok(None);
    }
    let (name, _) = add_to_plan(state, None, &[site_id]).await?;
    Ok(Some(name))
}

/// Hiçbir plana girmeyen bütün siteleri varsayılan plana ekler (Sağlık merkezi düzeltmesi).
/// `(plan adı, eklenen site sayısı)` döner.
pub async fn cover_uncovered(state: &AppState, user_id: Option<i64>) -> AppResult<(String, usize)> {
    let _turn = LOCK.lock().await;
    let plans = super::list_plans(&state.db)?;
    let sites: Vec<Site> = super::all_sites(&state.db)?.into_iter().filter(|site| site.status == "active").collect();
    let ids = uncovered_ids(&plans, &sites.iter().map(|site| site.id).collect::<Vec<_>>());
    if ids.is_empty() {
        return Err(AppError::bad("Plana eklenecek site kalmamış"));
    }
    add_to_plan(state, user_id, &ids).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(frequency: &str, enabled: bool, scope: Scope) -> Plan {
        let mut plan = build_plan(&Settings::default(), 1, 1, Vec::new());
        plan.frequency = frequency.into();
        plan.enabled = enabled;
        plan.scope = scope;
        plan
    }

    #[test]
    fn default_settings_build_a_valid_plan() {
        let settings = Settings::default();
        let plan = build_plan(&settings, 1, 3, vec![7]);
        assert!(plan.validate().is_ok());
        assert_eq!((plan.frequency.as_str(), plan.at_minute, plan.keep_daily), ("daily", 180, 7));
        assert_eq!(plan.scope.site_ids, vec![7]);
        assert!(plan.next_run(1_794_300_000, 3 * 3600).is_some(), "zamanlanmış plan bir sonraki koşuyu hesaplar");
    }

    #[test]
    fn invalid_schedule_is_rejected_by_plan_validation() {
        let settings = Settings { at_minute: 5000, ..Settings::default() };
        assert!(build_plan(&settings, 1, 1, vec![1]).validate().is_err());
        let settings = Settings { keep_daily: 999, ..Settings::default() };
        assert!(build_plan(&settings, 1, 1, vec![1]).validate().is_err());
    }

    #[test]
    fn uncovered_sites_ignore_manual_and_disabled_plans() {
        let sites = [1, 2, 3];
        let plans = vec![
            plan("daily", true, Scope { site_ids: vec![1], ..Scope::default() }),
            plan("manual", true, Scope { site_ids: vec![2], ..Scope::default() }),
            plan("daily", false, Scope { site_ids: vec![3], ..Scope::default() }),
        ];
        assert_eq!(uncovered_ids(&plans, &sites), vec![2, 3]);
        // "Tüm siteler" kapsamındaki zamanlanmış plan hepsini kapsar.
        let all = vec![plan("weekly", true, Scope { all_sites: true, ..Scope::default() })];
        assert!(uncovered_ids(&all, &sites).is_empty());
        assert_eq!(uncovered_ids(&[], &sites).len(), 3);
    }
}
