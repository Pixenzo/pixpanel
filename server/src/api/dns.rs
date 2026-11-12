//! DNS yönetimi uçları: bölgeler, kayıtlar, Cloudflare bağlantısı ve genel ayarlar.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::{self, AuthUser};
use crate::db::now;
use crate::dns::{self, cloudflare, local, Record, Zone};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::sys::services;

const AREA: &str = "dns";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(overview))
        .route("/setup", post(run_setup))
        .route("/settings", get(settings).put(save_settings))
        .route("/cloudflare", get(cloudflare_status).put(save_token).delete(remove_token))
        .route("/zones", post(create_zone))
        .route("/zones/{id}", get(zone_detail).delete(remove_zone))
        .route("/zones/{id}/records", post(create_record).put(update_record))
        .route("/zones/{id}/records/delete", post(delete_record))
        .route("/zones/{id}/sync", post(sync_zone))
        .route("/zones/{id}/dnssec", post(dnssec_switch))
        .route("/zones/{id}/export", get(export_zone))
        .route("/zones/{id}/import", post(import_zone))
        .route("/zones/{id}/cloudflare", post(cloudflare_settings))
        .route("/zones/{id}/purge", post(purge_cache))
        .route("/cluster", get(cluster_view).put(cluster_save))
        .route("/cluster/sync", post(cluster_sync))
        .route("/replica", axum::routing::put(replica_apply))
        .route("/replica/{primary}", axum::routing::delete(replica_detach))
}

#[derive(Serialize)]
struct Overview {
    /// Bu sunucudaki DNS (PowerDNS) kurulu ve çalışıyor mu?
    local_installed: bool,
    local_running: bool,
    cloudflare_connected: bool,
    server_ip: String,
    zones: Vec<Zone>,
    /// Bölge açılabilecek alan adları (hesabın siteleri).
    available_domains: Vec<String>,
}

/// Hesabın sitelerinden türetilen, henüz bölgesi olmayan kök alan adları.
fn available_domains(state: &AppState, owner_id: i64, zones: &[Zone]) -> AppResult<Vec<String>> {
    let names: Vec<String> = state.db.call(|conn| {
        let mut stmt = conn.prepare("SELECT domain FROM sites WHERE user_id = ?1 ORDER BY domain")?;
        stmt.query_map(params![owner_id], |row| row.get(0))?.collect()
    })?;
    let covered = |name: &str| zones.iter().any(|zone| name == zone.domain || name.ends_with(&format!(".{}", zone.domain)));
    // Alt alan adı, üst alan adı da listedeyse ayrıca önerilmez.
    Ok(names.iter().filter(|name| !covered(name) && !names.iter().any(|other| *name != other && name.ends_with(&format!(".{other}")))).cloned().collect())
}

async fn overview(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Overview>> {
    user.require(AREA)?;
    let zones = dns::list_zones(&state.db, &user)?;
    let local_installed = local::is_installed();
    Ok(Json(Overview {
        local_installed,
        local_running: local_installed && services::is_active(local::SERVICE).await,
        cloudflare_connected: dns::cloudflare_token(&state.db, user.owner_id())?.is_some(),
        server_ip: dns::server_ip(&state.db),
        available_domains: available_domains(&state, user.owner_id(), &zones)?,
        zones,
    }))
}

async fn run_setup(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    user.require_admin()?;
    if !local::is_installed() {
        return Err(AppError::bad("DNS sunucusu kurulu değil; önce Bileşenler sayfasından kurun"));
    }
    let _guard = state.sys_lock.lock().await;
    local::setup(&dns::cluster::transfer(&state.db)).await?;
    auth::audit(&state.db, &user, "dns.setup", "", "");
    Ok(Json(json!({ "ok": true })))
}

// ---- Genel ayarlar (yönetici) ----

#[derive(Serialize)]
struct Settings {
    server_ip: String,
    nameservers: Vec<String>,
    cloudflare_real_ip: bool,
}

fn settings_view(state: &AppState) -> Settings {
    Settings {
        server_ip: dns::server_ip(&state.db),
        nameservers: dns::configured_nameservers(&state.db),
        cloudflare_real_ip: dns::real_ip_enabled(&state.db),
    }
}

async fn settings(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Settings>> {
    user.require_admin()?;
    Ok(Json(settings_view(&state)))
}

#[derive(Deserialize)]
struct SettingsBody {
    server_ip: String,
    nameservers: Vec<String>,
    cloudflare_real_ip: bool,
}

async fn save_settings(State(state): State<AppState>, user: AuthUser, Json(body): Json<SettingsBody>) -> AppResult<Json<Settings>> {
    user.require_admin()?;
    dns::set_server_ip(&state.db, &body.server_ip)?;
    dns::set_nameservers(&state.db, &body.nameservers)?;
    // Açıkken yeniden kaydetmek adres aralıklarını da günceller.
    if body.cloudflare_real_ip || dns::real_ip_enabled(&state.db) {
        let _guard = state.sys_lock.lock().await;
        dns::set_real_ip(&state, body.cloudflare_real_ip).await?;
    }
    auth::audit(&state.db, &user, "dns.settings", "", "");
    Ok(Json(settings_view(&state)))
}

// ---- DNS kümesi (yönetici) ----

/// Küme görünümü: bu sunucunun ikincilleri (anahtarlar gizli) ve ikincili olduğu birinciller.
async fn cluster_json(state: &AppState) -> Value {
    let statuses = dns::cluster::statuses(&state.db);
    let secondaries: Vec<Value> = dns::cluster::secondaries(&state.db)
        .into_iter()
        .map(|item| {
            let status = statuses.get(&item.address);
            json!({
                "address": item.address,
                "panel_url": item.panel_url,
                "token_set": !item.token.is_empty(),
                "fingerprint": item.fingerprint,
                "last_sync_at": status.map(|status| status.at),
                "last_error": status.map(|status| status.error.clone()).unwrap_or_default(),
                "synced_zones": status.map(|status| status.zones),
                "conflicts": status.map(|status| status.conflicts.clone()).unwrap_or_default(),
            })
        })
        .collect();
    let replica_of: Vec<Value> = dns::cluster::replica_summary(&state.db).await.into_iter().map(|(primary, zones)| json!({ "primary": primary, "zones": zones })).collect();
    json!({ "local_installed": local::is_installed(), "secondaries": secondaries, "replica_of": replica_of })
}

async fn cluster_view(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    user.require_admin()?;
    Ok(Json(cluster_json(&state).await))
}

#[derive(Deserialize)]
struct ClusterBody {
    secondaries: Vec<dns::cluster::SecondaryInput>,
}

async fn cluster_save(State(state): State<AppState>, user: AuthUser, Json(body): Json<ClusterBody>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let _guard = state.sys_lock.lock().await;
    let detail = body.secondaries.iter().map(|item| item.address.trim().to_string()).collect::<Vec<_>>().join(", ");
    dns::cluster::save(&state, body.secondaries).await?;
    auth::audit(&state.db, &user, "dns.cluster", "", &detail);
    Ok(Json(cluster_json(&state).await))
}

async fn cluster_sync(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    user.require_admin()?;
    dns::cluster::sync(&state).await?;
    Ok(Json(cluster_json(&state).await))
}

#[derive(Deserialize)]
struct ReplicaBody {
    zones: Vec<String>,
    /// Bölgelerin çekileceği adres; verilmezse isteğin geldiği adres.
    primary: Option<String>,
}

/// Birincil panelin çağırdığı uç: bu sunucudaki ikincil bölgeleri gönderilen listeyle eşitler.
async fn replica_apply(State(state): State<AppState>, user: AuthUser, Json(body): Json<ReplicaBody>) -> AppResult<Json<dns::cluster::ReplicaReport>> {
    user.require_admin()?;
    let primary: std::net::IpAddr = body.primary.as_deref().unwrap_or(&user.ip).trim().parse().map_err(|_| AppError::bad("Birincil sunucunun adresi geçersiz"))?;
    let _guard = state.sys_lock.lock().await;
    let report = dns::cluster::replica_apply(&state, primary, &body.zones).await?;
    if !report.created.is_empty() || !report.removed.is_empty() {
        auth::audit(&state.db, &user, "dns.replica", &primary.to_string(), &format!("+{} -{}", report.created.len(), report.removed.len()));
    }
    Ok(Json(report))
}

async fn replica_detach(State(state): State<AppState>, user: AuthUser, Path(primary): Path<String>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let primary: std::net::IpAddr = primary.trim().parse().map_err(|_| AppError::bad("Birincil sunucunun adresi geçersiz"))?;
    let _guard = state.sys_lock.lock().await;
    let removed = dns::cluster::replica_detach(&state, primary).await?;
    auth::audit(&state.db, &user, "dns.replica.detach", &primary.to_string(), &format!("{removed} bölge"));
    Ok(Json(cluster_json(&state).await))
}

// ---- Cloudflare bağlantısı ----

#[derive(Serialize)]
struct RemoteZoneView {
    id: String,
    name: String,
    /// Bu bölge panelde zaten bağlı mı?
    linked: bool,
}

async fn cloudflare_status(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    if dns::cloudflare_token(&state.db, user.owner_id())?.is_none() {
        return Ok(Json(json!({ "connected": false, "zones": [] })));
    }
    let linked: Vec<String> = dns::list_zones(&state.db, &user)?.into_iter().map(|zone| zone.domain).collect();
    let zones: Vec<RemoteZoneView> = dns::cloudflare_client(&state, user.owner_id())?
        .zones()
        .await?
        .into_iter()
        .map(|zone| RemoteZoneView { linked: linked.contains(&zone.name), id: zone.id, name: zone.name })
        .collect();
    Ok(Json(json!({ "connected": true, "zones": zones })))
}

#[derive(Deserialize)]
struct TokenBody {
    token: String,
}

async fn save_token(State(state): State<AppState>, user: AuthUser, Json(body): Json<TokenBody>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let token = body.token.trim();
    // Anahtar kaydedilmeden önce Cloudflare'de sınanır.
    cloudflare::Client::new(&state.cfg.cloudflare_api, token)?.verify().await?;
    let owner = user.owner_id();
    state.db.call(|conn| {
        conn.execute(
            "INSERT INTO cloudflare_tokens (user_id, token) VALUES (?1, ?2) ON CONFLICT(user_id) DO UPDATE SET token = excluded.token",
            params![owner, token],
        )
    })?;
    auth::audit(&state.db, &user, "dns.cloudflare.connect", "", "");
    Ok(Json(json!({ "ok": true })))
}

async fn remove_token(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let owner = user.owner_id();
    let in_use: i64 = state.db.call(|conn| {
        conn.query_row("SELECT COUNT(*) FROM dns_zones WHERE user_id = ?1 AND provider = 'cloudflare'", params![owner], |row| row.get(0))
    })?;
    if in_use > 0 {
        return Err(AppError::conflict("Önce Cloudflare'e bağlı bölgelerin bağlantısını kaldırın"));
    }
    state.db.call(|conn| conn.execute("DELETE FROM cloudflare_tokens WHERE user_id = ?1", params![owner]))?;
    auth::audit(&state.db, &user, "dns.cloudflare.disconnect", "", "");
    Ok(Json(json!({ "ok": true })))
}

// ---- Bölgeler ----

#[derive(Deserialize)]
struct CreateZoneBody {
    domain: String,
    /// `local` ya da `cloudflare`.
    provider: String,
}

async fn create_zone(State(state): State<AppState>, user: AuthUser, Json(body): Json<CreateZoneBody>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let domain = dns::normalize_domain(&body.domain)?;
    let owner = user.owner_id();
    crate::quota::check(&state.db, owner, crate::quota::Resource::DnsZone)?;
    let exists: bool = state.db.call(|conn| conn.query_row("SELECT EXISTS(SELECT 1 FROM dns_zones WHERE domain = ?1)", params![domain], |row| row.get(0)))?;
    if exists {
        return Err(AppError::conflict(format!("{domain} için bir bölge zaten var")));
    }

    let _guard = state.sys_lock.lock().await;
    let (provider, cf_zone_id, vanity) = match body.provider.as_str() {
        "cloudflare" => {
            // Anahtarın bölgeye erişebilmesi, alan adının hesaba ait olduğunu kanıtlar.
            let remote = dns::cloudflare_client(&state, owner)?.zones().await?;
            let zone = remote.into_iter().find(|zone| zone.name == domain).ok_or_else(|| AppError::bad(format!("{domain} bu Cloudflare hesabında bulunamadı")))?;
            ("cloudflare", zone.id, false)
        }
        "local" => {
            if !local::is_installed() {
                return Err(AppError::bad("DNS sunucusu kurulu değil; yöneticinin Bileşenler sayfasından kurması gerekiyor"));
            }
            // Paylaşımlı sunucuda bir hesap başkasının alan adı için bölge açıp yanlış yanıtlar yayınlayabilirdi.
            if !user.is_admin() && !dns::owns_domain(&state.db, owner, &domain)? {
                return Err(AppError::bad("Bölge yalnızca hesabınızdaki bir sitenin alan adı için açılabilir"));
            }
            let (nameservers, vanity) = dns::nameservers(&state.db, &domain);
            local::create_zone(&domain, &nameservers).await?;
            ("local", String::new(), vanity)
        }
        _ => return Err(AppError::bad("Geçersiz DNS sağlayıcısı")),
    };
    let id = state.db.call(|conn| {
        conn.execute(
            "INSERT INTO dns_zones (user_id, domain, provider, cf_zone_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![owner, domain, provider, cf_zone_id, now()],
        )?;
        Ok(conn.last_insert_rowid())
    })?;
    let zone = dns::get_zone(&state.db, &user, id)?;
    let mut report = Vec::new();
    if provider == "local" {
        report.extend(dns::seed_local_zone(&state, &zone, vanity).await);
        // Yeni açılan yerel bölgeye hesabın site ve posta kayıtları da yazılır.
        report.extend(sync_records(&state, &zone).await?);
    }
    if provider == "local" {
        dns::cluster::sync_in_background(&state);
    }
    auth::audit(&state.db, &user, "dns.zone.create", &domain, provider);
    Ok(Json(json!({ "zone": zone, "report": report })))
}

/// Bölgenin kapsadığı siteler ve posta alan adları için gereken kayıtları ekler.
async fn sync_records(state: &AppState, zone: &Zone) -> AppResult<Vec<String>> {
    let suffix = format!("%.{}", zone.domain);
    let site_names: Vec<String> = state.db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT domain FROM sites WHERE user_id = ?1 AND (domain = ?2 OR domain LIKE ?3)
             UNION SELECT a.name FROM site_aliases a JOIN sites s ON s.id = a.site_id
                   WHERE s.user_id = ?1 AND (a.name = ?2 OR a.name LIKE ?3)",
        )?;
        stmt.query_map(params![zone.user_id, zone.domain, suffix], |row| row.get(0))?.collect()
    })?;
    let mut report = dns::add_site_records(state, zone.user_id, &site_names).await;
    let mail_domains: Vec<(String, String)> = state.db.call(|conn| {
        let mut stmt = conn.prepare("SELECT domain, dkim_record FROM mail_domains WHERE user_id = ?1 AND (domain = ?2 OR domain LIKE ?3)")?;
        stmt.query_map(params![zone.user_id, zone.domain, suffix], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
    })?;
    let mail_host = crate::mail::hostname(&state.db);
    for (domain, dkim) in mail_domains {
        report.extend(dns::add_mail_records(state, zone.user_id, &domain, &mail_host, crate::mail::DKIM_SELECTOR, &dkim).await);
    }
    Ok(report)
}

async fn sync_zone(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let zone = dns::get_zone(&state.db, &user, id)?;
    let report = sync_records(&state, &zone).await?;
    auth::audit(&state.db, &user, "dns.zone.sync", &zone.domain, "");
    Ok(Json(json!({ "report": report })))
}

async fn zone_detail(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let zone = dns::get_zone(&state.db, &user, id)?;
    let records = dns::list_records(&state, &zone).await?;
    let (nameservers, _) = dns::nameservers(&state.db, &zone.domain);
    let cloudflare_info = if zone.provider == "cloudflare" {
        let client = dns::cloudflare_client(&state, zone.user_id)?;
        // Anahtarın ayar okuma yetkisi olmayabilir; o durumda ayarlar gösterilmez.
        json!({
            "ssl_mode": client.setting(&zone.cf_zone_id, "ssl").await.ok(),
            "development_mode": client.setting(&zone.cf_zone_id, "development_mode").await.ok().map(|value| value == "on"),
            "writable_kinds": cloudflare::WRITABLE_KINDS,
        })
    } else {
        Value::Null
    };
    // DNSSEC yalnızca bu sunucudaki bölgelerde yönetilir; durum okunamazsa kutu gösterilmez.
    let dnssec = if zone.provider == "local" { local::dnssec_status(&zone.domain).await.ok() } else { None };
    Ok(Json(json!({
        "zone": zone,
        "records": records,
        "dnssec": dnssec,
        "kinds": dns::KINDS,
        // Yerel bölgelerde alan adının kayıt şirketinde tanımlanması gereken ad sunucuları.
        "nameservers": if zone.provider == "local" { json!(nameservers) } else { Value::Null },
        "server_ip": dns::server_ip(&state.db),
        "cloudflare": cloudflare_info,
    })))
}

async fn remove_zone(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let zone = dns::get_zone(&state.db, &user, id)?;
    let _guard = state.sys_lock.lock().await;
    // Cloudflare'deki bölgeye dokunulmaz; yalnızca panelle bağlantısı kaldırılır.
    if zone.provider == "local" && local::is_installed() {
        local::delete_zone(&zone.domain).await?;
    }
    state.db.call(|conn| conn.execute("DELETE FROM dns_zones WHERE id = ?1", params![id]))?;
    if zone.provider == "local" {
        dns::cluster::sync_in_background(&state);
    }
    auth::audit(&state.db, &user, "dns.zone.delete", &zone.domain, &zone.provider);
    Ok(Json(json!({ "ok": true })))
}

// ---- Kayıtlar ----

#[derive(Deserialize)]
struct RecordBody {
    record: Record,
}

#[derive(Deserialize)]
struct UpdateBody {
    old: Record,
    new: Record,
}

async fn records_reply(state: &AppState, zone: &Zone) -> AppResult<Json<Value>> {
    Ok(Json(json!({ "records": dns::list_records(state, zone).await? })))
}

async fn create_record(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<RecordBody>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let zone = dns::get_zone(&state.db, &user, id)?;
    let record = dns::normalize(&zone.domain, &body.record)?;
    dns::create_record(&state, &zone, &record).await?;
    auth::audit(&state.db, &user, "dns.record.create", &format!("{} {}", record.kind, record.name), &zone.domain);
    records_reply(&state, &zone).await
}

async fn update_record(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<UpdateBody>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let zone = dns::get_zone(&state.db, &user, id)?;
    let old = dns::normalize(&zone.domain, &body.old)?;
    let new = dns::normalize(&zone.domain, &body.new)?;
    dns::update_record(&state, &zone, &old, &new).await?;
    auth::audit(&state.db, &user, "dns.record.update", &format!("{} {}", new.kind, new.name), &zone.domain);
    records_reply(&state, &zone).await
}

async fn delete_record(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<RecordBody>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let zone = dns::get_zone(&state.db, &user, id)?;
    let record = dns::normalize(&zone.domain, &body.record)?;
    // Yerel bölgede ad sunucusu kayıtlarının hepsi silinirse bölge yanıt veremez hâle gelir.
    if zone.provider == "local" && record.kind == "NS" && record.name == zone.domain {
        let remaining = dns::list_records(&state, &zone).await?.iter().filter(|other| other.kind == "NS" && other.name == zone.domain).count();
        if remaining <= 1 {
            return Err(AppError::bad("Bölgenin son ad sunucusu kaydı silinemez"));
        }
    }
    dns::delete_record(&state, &zone, &record).await?;
    auth::audit(&state.db, &user, "dns.record.delete", &format!("{} {}", record.kind, record.name), &zone.domain);
    records_reply(&state, &zone).await
}

// ---- DNSSEC (yerel bölgeler) ----

#[derive(Deserialize)]
struct DnssecBody {
    enabled: bool,
}

async fn dnssec_switch(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<DnssecBody>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let zone = dns::get_zone(&state.db, &user, id)?;
    if zone.provider != "local" {
        return Err(AppError::bad("DNSSEC yalnızca bu sunucudaki bölgelerde yönetilir; Cloudflare bölgelerinde Cloudflare panelini kullanın"));
    }
    let _guard = state.sys_lock.lock().await;
    if body.enabled {
        local::dnssec_enable(&zone.domain).await?;
    } else {
        local::dnssec_disable(&zone.domain).await?;
    }
    // İkincillere imzalı (ya da imzasız) bölgenin yeni seri numarası bildirilir.
    dns::announce(&state, &zone).await;
    auth::audit(&state.db, &user, if body.enabled { "dns.dnssec.enable" } else { "dns.dnssec.disable" }, &zone.domain, "");
    Ok(Json(json!({ "dnssec": local::dnssec_status(&zone.domain).await? })))
}

// ---- Bölge dosyası: dışa ve içe aktarma ----

/// Bölgeyi BIND bölge dosyası olarak verir (her sağlayıcıda, kayıtlardan üretilir).
async fn export_zone(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let zone = dns::get_zone(&state.db, &user, id)?;
    let records = dns::list_records(&state, &zone).await?;
    let serial = u32::try_from(now()).unwrap_or(1);
    Ok(Json(json!({
        "filename": format!("{}.zone", zone.domain),
        "content": dns::zonefile::export(&zone.domain, &records, serial),
    })))
}

#[derive(Deserialize)]
struct ImportBody {
    text: String,
    /// Dosyadaki her (ad, tür) kümesinin var olan kayıtlarını dosyadakilerle değiştir.
    #[serde(default)]
    replace: bool,
    /// Yanlışsa yalnızca önizleme döner, hiçbir şey yazılmaz.
    #[serde(default)]
    apply: bool,
}

async fn import_zone(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<ImportBody>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let zone = dns::get_zone(&state.db, &user, id)?;
    let parsed = dns::zonefile::parse(&zone.domain, &body.text)?;
    let current = dns::list_records(&state, &zone).await?;
    let plan = dns::zonefile::plan(&zone.domain, &parsed.records, &current, body.replace);
    if zone.provider == "cloudflare" && plan.add.len() > dns::zonefile::MAX_CLOUDFLARE_ADDS {
        return Err(AppError::bad(format!("Cloudflare bölgelerine tek seferde en çok {} kayıt aktarılabilir", dns::zonefile::MAX_CLOUDFLARE_ADDS)));
    }
    let mut skipped = parsed.skipped.clone();
    skipped.extend(plan.skipped.iter().cloned());
    let mut reply = json!({
        "add": plan.add,
        "existing_count": plan.existing.len(),
        "remove": plan.remove,
        "skipped": skipped,
        "skipped_total": parsed.skipped_total + plan.skipped.len(),
        "notes": parsed.notes,
        "result": Value::Null,
    });
    if body.apply {
        let outcome = dns::zonefile::apply(&state, &zone, &plan, &current).await;
        auth::audit(
            &state.db,
            &user,
            "dns.zone.import",
            &zone.domain,
            &format!("+{} -{}{}{}", outcome.added, outcome.removed, if body.replace { " değiştirme" } else { "" }, if outcome.failed.is_empty() { String::new() } else { format!(" başarısız {}", outcome.failed.len()) }),
        );
        reply["result"] = json!(outcome);
        reply["records"] = json!(dns::list_records(&state, &zone).await?);
    }
    Ok(Json(reply))
}

// ---- Cloudflare bölge ayarları ----

#[derive(Deserialize)]
struct CloudflareSettingsBody {
    /// `off`, `flexible`, `full` ya da `strict`.
    ssl_mode: Option<String>,
    development_mode: Option<bool>,
}

fn cloudflare_zone(state: &AppState, user: &AuthUser, id: i64) -> AppResult<Zone> {
    let zone = dns::get_zone(&state.db, user, id)?;
    if zone.provider == "cloudflare" { Ok(zone) } else { Err(AppError::bad("Bu bölge Cloudflare'de değil")) }
}

async fn cloudflare_settings(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<CloudflareSettingsBody>,
) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let zone = cloudflare_zone(&state, &user, id)?;
    let client = dns::cloudflare_client(&state, zone.user_id)?;
    if let Some(mode) = &body.ssl_mode {
        if !matches!(mode.as_str(), "off" | "flexible" | "full" | "strict") {
            return Err(AppError::bad("Geçersiz SSL kipi"));
        }
        client.set_setting(&zone.cf_zone_id, "ssl", mode).await?;
    }
    if let Some(enabled) = body.development_mode {
        client.set_setting(&zone.cf_zone_id, "development_mode", if enabled { "on" } else { "off" }).await?;
    }
    auth::audit(&state.db, &user, "dns.cloudflare.settings", &zone.domain, "");
    Ok(Json(json!({ "ok": true })))
}

async fn purge_cache(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let zone = cloudflare_zone(&state, &user, id)?;
    dns::cloudflare_client(&state, zone.user_id)?.purge_cache(&zone.cf_zone_id).await?;
    auth::audit(&state.db, &user, "dns.cloudflare.purge", &zone.domain, "");
    Ok(Json(json!({ "ok": true })))
}
