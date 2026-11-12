//! Hesap başına Redis: her hesap, isterse kendi Linux kullanıcısıyla çalışan ayrı bir
//! Redis örneği açar (`pixpanel-redis-<hesap>.service`).
//!
//! Ayrı örnek, hesapları birbirinden gerçekten ayırır: bir hesap diğerinin anahtarlarını
//! göremez, belleğini dolduran hesap yalnızca kendi önbelleğini boşaltır. Örnek yalnızca
//! `127.0.0.1`'i dinler ve bunu değiştiren ayar yoktur; yerel port sunucudaki bütün hesaplara
//! açık olduğundan panelin ürettiği uzun parola zorunludur. Yapılandırmayı çalışma anında
//! değiştiren komutlar (`CONFIG`, `MODULE`, `REPLICAOF`…) örnekte kapalıdır; müşteri dinlenen
//! adresi ya da parolayı Redis'in içinden değiştiremez.

use std::path::PathBuf;
use std::time::Duration;

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::auth;
use crate::db::{now, Db};
use crate::error::{AppError, AppResult};
use crate::hosting::{atomic_write, sysuser};
use crate::sys::redis::Client;
use crate::sys::{exec, services};

const CONFIG_DIR: &str = "/etc/pixpanel/redis";
const DATA_DIR: &str = "/var/lib/pixpanel/redis";
const RUNTIME_DIR: &str = "/run/pixpanel-redis";
/// Hesap örneklerine ayrılan port aralığı.
const PORT_RANGE: std::ops::Range<u16> = 16400..17400;
pub const DEFAULT_MEMORY_MB: u32 = 64;
pub const MAX_MEMORY_MB: u32 = 4096;
/// Çalışma anında yapılandırmayı ya da sunucunun rolünü değiştiren, örnekte kapatılan komutlar.
const DISABLED_COMMANDS: &[&str] = &["CONFIG", "MODULE", "REPLICAOF", "SLAVEOF", "DEBUG", "SHUTDOWN", "CLUSTER", "FAILOVER", "ACL"];

/// Bir hesabın Redis örneği.
#[derive(Debug, Clone, Serialize)]
pub struct Instance {
    pub user_id: i64,
    pub username: String,
    pub port: u16,
    pub password: String,
    pub memory_mb: u32,
    pub created_at: i64,
}

impl Instance {
    pub fn socket(&self) -> String {
        format!("{RUNTIME_DIR}/{}/redis.sock", self.username)
    }
}

/// Paketin hesabın Redis'ine getirdiği kural (`quota::Limits::redis_memory_mb`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackagePlan {
    /// Pakette sınır yok: örnek varsayılan belleğiyle açılır, yönetici örnek başına değiştirebilir.
    Unrestricted,
    /// Bellek sınırı paketten gelir (MB).
    Fixed(u32),
    /// Paket Redis içermiyor (0): örnek açılamaz, açıksa durdurulur.
    Denied,
}

/// Paketteki `redis_memory_mb` değerinden kural.
pub fn package_plan(limit: Option<i64>) -> PackagePlan {
    match limit {
        None => PackagePlan::Unrestricted,
        Some(0) => PackagePlan::Denied,
        Some(mb) => PackagePlan::Fixed(u32::try_from(mb).unwrap_or(MAX_MEMORY_MB).clamp(16, MAX_MEMORY_MB)),
    }
}

/// Hesabın paketinin adı ve Redis kuralı; paketsiz hesap ve yönetici için kural koymayan `Unrestricted`.
pub fn plan_for(db: &Db, user_id: i64) -> AppResult<(Option<String>, PackagePlan)> {
    Ok(match crate::quota::limits_for(db, user_id)? {
        Some((name, limits)) => (Some(name), package_plan(limits.redis_memory_mb)),
        None => (None, PackagePlan::Unrestricted),
    })
}

/// Paket Redis'e izin vermiyorsa nedenini söyleyen hata.
pub fn require_allowed(db: &Db, user_id: i64) -> AppResult<PackagePlan> {
    let (package, plan) = plan_for(db, user_id)?;
    if plan == PackagePlan::Denied {
        return Err(AppError::bad(format!("“{}” paketi Redis içermiyor.", package.unwrap_or_default())));
    }
    Ok(plan)
}

pub fn is_available() -> bool {
    exec::binary_exists("redis-server")
}

/// Hesap adı dosya yollarına ve birim adına girdiği için yeniden doğrulanır.
fn check_name(username: &str) -> AppResult<()> {
    let valid = !username.is_empty()
        && username.len() <= 32
        && username.bytes().next().is_some_and(|first| first.is_ascii_lowercase())
        && username.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-');
    if valid { Ok(()) } else { Err(AppError::bad("Hesap adı Redis örneği için uygun değil")) }
}

pub fn unit_name(username: &str) -> String {
    format!("pixpanel-redis-{username}.service")
}

fn unit_path(username: &str) -> PathBuf {
    PathBuf::from(format!("/etc/systemd/system/{}", unit_name(username)))
}

fn config_path(username: &str) -> PathBuf {
    PathBuf::from(format!("{CONFIG_DIR}/{username}.conf"))
}

fn data_dir(username: &str) -> PathBuf {
    PathBuf::from(format!("{DATA_DIR}/{username}"))
}

pub fn get(db: &Db, user_id: i64) -> AppResult<Option<Instance>> {
    db.call(|conn| {
        conn.query_row(
            "SELECT r.user_id, u.username, r.port, r.password, r.memory_mb, r.created_at
             FROM redis_instances r JOIN users u ON u.id = r.user_id WHERE r.user_id = ?1",
            params![user_id],
            read_row,
        )
        .optional()
    })
}

pub fn list(db: &Db) -> AppResult<Vec<Instance>> {
    db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT r.user_id, u.username, r.port, r.password, r.memory_mb, r.created_at
             FROM redis_instances r JOIN users u ON u.id = r.user_id ORDER BY u.username",
        )?;
        let rows = stmt.query_map([], read_row)?;
        rows.collect()
    })
}

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Instance> {
    Ok(Instance { user_id: row.get(0)?, username: row.get(1)?, port: row.get(2)?, password: row.get(3)?, memory_mb: row.get(4)?, created_at: row.get(5)? })
}

/// Örneğin yapılandırma dosyası. Değerlerin hepsi panelin ürettiği ya da doğruladığı değerlerdir.
pub fn render_config(instance: &Instance) -> String {
    let mut conf = format!(
        "# PixPanel tarafından üretildi. Elle yapılan değişiklikler kaybolur.\n\
         bind 127.0.0.1\n\
         port {port}\n\
         protected-mode yes\n\
         requirepass {password}\n\
         unixsocket {socket}\n\
         unixsocketperm 700\n\
         daemonize no\n\
         logfile \"\"\n\
         dir {data}\n\
         databases 16\n\
         maxmemory {memory}mb\n\
         maxmemory-policy allkeys-lru\n\
         save 300 10\n\
         tcp-keepalive 300\n\
         timeout 0\n",
        port = instance.port,
        password = instance.password,
        socket = instance.socket(),
        data = data_dir(&instance.username).display(),
        memory = instance.memory_mb,
    );
    for command in DISABLED_COMMANDS {
        conf.push_str(&format!("rename-command {command} \"\"\n"));
    }
    conf
}

pub fn render_unit(instance: &Instance) -> String {
    let name = &instance.username;
    format!(
        "[Unit]\n\
         Description=PixPanel Redis ({name})\n\
         After=network.target\n\n\
         [Service]\n\
         Type=simple\n\
         User={name}\n\
         Group={name}\n\
         ExecStart=/usr/bin/redis-server {config}\n\
         RuntimeDirectory=pixpanel-redis/{name}\n\
         RuntimeDirectoryMode=0700\n\
         Restart=on-failure\n\
         RestartSec=2\n\
         # Redis'in kendi sınırı veriyi kapsar; süreç payıyla birlikte hesap bundan fazlasını tüketemez.\n\
         MemoryMax={memory}M\n\
         NoNewPrivileges=yes\n\
         PrivateTmp=yes\n\
         PrivateDevices=yes\n\
         ProtectSystem=strict\n\
         ProtectHome=yes\n\
         ReadWritePaths={data}\n\
         LimitNOFILE=10032\n\
         {port_rule}{slice_line}\n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        config = config_path(name).display(),
        port_rule = crate::hosting::portguard::deny_line(),
        // Hesabın işlemci/bellek sınırı varsa Redis de hesabın diliminde çalışır.
        slice_line = sysuser::lookup(name).map(|user| crate::hosting::resource_limits::redis_slice_line(user.uid)).unwrap_or_default(),
        memory = instance.memory_mb * 2 + 64,
        data = data_dir(name).display(),
    )
}

/// Var olan örneğin birim dosyasını güncel ayarlarla yeniden yazar; servisi yeniden başlatmaz ve
/// `daemon-reload` çağırmaz (çağıran toplu yapar). Birim yoksa ya da içerik aynıysa `false` döner.
/// Kaynak sınırı değişince dilim satırını eklemek/kaldırmak için kullanılır; değişiklik servisin
/// bir sonraki başlatılışında geçerli olur.
pub fn rewrite_unit(instance: &Instance) -> AppResult<bool> {
    check_name(&instance.username)?;
    let path = unit_path(&instance.username);
    if !path.exists() {
        return Ok(false);
    }
    let wanted = render_unit(instance);
    if std::fs::read_to_string(&path).is_ok_and(|current| current == wanted) {
        return Ok(false);
    }
    atomic_write(&path, wanted.as_bytes(), 0o644)?;
    Ok(true)
}

/// Kullanılmayan bir port seçer: kayıtlı örneklerin portları ve o an dinlenen portlar atlanır.
fn pick_port(db: &Db) -> AppResult<u16> {
    let taken: Vec<u16> = db.call(|conn| {
        let mut stmt = conn.prepare("SELECT port FROM redis_instances")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    })?;
    PORT_RANGE
        .into_iter()
        .find(|port| !taken.contains(port) && std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok())
        .ok_or_else(|| AppError::internal("Redis örneği için boş port kalmadı"))
}

pub fn validate_memory(memory_mb: u32) -> AppResult<u32> {
    if (16..=MAX_MEMORY_MB).contains(&memory_mb) { Ok(memory_mb) } else { Err(AppError::bad(format!("Bellek sınırı 16 ile {MAX_MEMORY_MB} MB arasında olmalı"))) }
}

/// Dosyaları yazar ve servisi (yeniden) başlatır.
async fn apply(instance: &Instance) -> AppResult<()> {
    check_name(&instance.username)?;
    let user = sysuser::ensure(&instance.username).await?;
    std::fs::create_dir_all(CONFIG_DIR)?;
    std::fs::create_dir_all(DATA_DIR)?;
    let data = data_dir(&instance.username);
    std::fs::create_dir_all(&data)?;
    std::os::unix::fs::chown(&data, Some(user.uid), Some(user.gid))?;
    std::fs::set_permissions(&data, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    // Yapılandırma parolayı içerir: yalnızca root ve örneğin kendi kullanıcısı okur.
    let config = config_path(&instance.username);
    atomic_write(&config, render_config(instance).as_bytes(), 0o640)?;
    std::os::unix::fs::chown(&config, Some(0), Some(user.gid))?;
    atomic_write(&unit_path(&instance.username), render_unit(instance).as_bytes(), 0o644)?;
    exec::run_ok("systemctl", &["daemon-reload"]).await?;
    let unit = unit_name(&instance.username);
    exec::run_ok("systemctl", &["enable", "--quiet", "--", &unit]).await?;
    exec::run_ok("systemctl", &["restart", "--", &unit]).await?;
    // Servis "başladı" dese de Redis'in bağlantı kabul etmesi kısa bir süre alır.
    for _ in 0..40 {
        if connect(instance).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    Err(AppError::internal("Redis örneği başlatıldı ama yanıt vermiyor"))
}

/// Hesap için örnek açar; zaten varsa onu döner.
pub async fn enable(db: &Db, user_id: i64) -> AppResult<Instance> {
    if !is_available() {
        return Err(AppError::bad("Redis bu sunucuda kurulu değil; yöneticinin Bileşenler sayfasından kurması gerekiyor"));
    }
    // Paket Redis içermiyorsa (ya da hesabın örneği paket yüzünden durdurulmuşsa) açılamaz.
    let plan = require_allowed(db, user_id)?;
    if let Some(existing) = get(db, user_id)? {
        return Ok(existing);
    }
    let username: String = db.call(|conn| conn.query_row("SELECT username FROM users WHERE id = ?1", params![user_id], |row| row.get(0)))?;
    check_name(&username)?;
    let instance = Instance { user_id, username, port: pick_port(db)?, password: auth::random_token(24), memory_mb: match plan { PackagePlan::Fixed(memory) => memory, _ => DEFAULT_MEMORY_MB }, created_at: now() };
    db.call(|conn| {
        conn.execute(
            "INSERT INTO redis_instances (user_id, port, password, memory_mb, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![instance.user_id, instance.port, instance.password, instance.memory_mb, instance.created_at],
        )
    })?;
    if let Err(err) = apply(&instance).await {
        // Yarım kalan kurulum iz bırakmaz.
        let _ = remove_files(&instance.username, true).await;
        db.call(|conn| conn.execute("DELETE FROM redis_instances WHERE user_id = ?1", params![user_id]))?;
        return Err(err);
    }
    Ok(instance)
}

async fn remove_files(username: &str, delete_data: bool) -> AppResult<()> {
    check_name(username)?;
    let unit = unit_name(username);
    let _ = exec::run("systemctl", &["disable", "--now", "--quiet", "--", &unit]).await;
    for path in [unit_path(username), config_path(username)] {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
    }
    let _ = exec::run("systemctl", &["daemon-reload"]).await;
    let data = data_dir(username);
    if delete_data && data.exists() {
        std::fs::remove_dir_all(data)?;
    }
    Ok(())
}

/// Örneği kapatır: süreç durur, verisi silinir.
pub async fn disable(db: &Db, user_id: i64) -> AppResult<()> {
    let Some(instance) = get(db, user_id)? else { return Ok(()) };
    remove_files(&instance.username, true).await?;
    db.call(|conn| conn.execute("DELETE FROM redis_instances WHERE user_id = ?1", params![user_id]))?;
    Ok(())
}

/// Hesap silinirken çağrılır: kullanıcı adı artık veritabanında olmayabileceği için adla çalışır.
pub async fn remove_for_username(db: &Db, user_id: i64, username: &str) {
    let _ = db.call(|conn| conn.execute("DELETE FROM redis_instances WHERE user_id = ?1", params![user_id]));
    if unit_path(username).exists() || config_path(username).exists() {
        if let Err(err) = remove_files(username, true).await {
            tracing::warn!(account = %username, error = %err, "hesabın Redis örneği kaldırılamadı");
        }
    }
}

/// Parolayı yeniler; eski parolayı kullanan uygulamaların ayarı güncellenmelidir.
pub async fn rotate_password(db: &Db, user_id: i64) -> AppResult<Instance> {
    let mut instance = get(db, user_id)?.ok_or_else(|| AppError::bad("Bu hesap için Redis açık değil"))?;
    instance.password = auth::random_token(24);
    db.call(|conn| conn.execute("UPDATE redis_instances SET password = ?1 WHERE user_id = ?2", params![instance.password, user_id]))?;
    apply(&instance).await?;
    Ok(instance)
}

/// Bellek sınırını değiştirir (yönetici).
pub async fn set_memory(db: &Db, user_id: i64, memory_mb: u32) -> AppResult<Instance> {
    // Sınır pakette tanımlıysa hesap başına elle değiştirilemez; paket değişince eşitlenir.
    if let (Some(package), PackagePlan::Fixed(_) | PackagePlan::Denied) = plan_for(db, user_id)? {
        return Err(AppError::bad(format!("Bu hesabın Redis bellek sınırı “{package}” paketinden geliyor; paketi değiştirin")));
    }
    let memory_mb = validate_memory(memory_mb)?;
    let mut instance = get(db, user_id)?.ok_or_else(|| AppError::bad("Bu hesap için Redis açık değil"))?;
    instance.memory_mb = memory_mb;
    db.call(|conn| conn.execute("UPDATE redis_instances SET memory_mb = ?1 WHERE user_id = ?2", params![memory_mb, user_id]))?;
    apply(&instance).await?;
    Ok(instance)
}

/// Hesabın paketindeki Redis kuralını var olan örneğe uygular (paket atanınca/değişince ve panel
/// açılışında; `resource_limits::sync` çağırır, `sys_lock` çağıranda tutulur):
/// - `Fixed`: `maxmemory` paketin değerine çekilir (değişmişse yapılandırma yazılıp örnek yeniden başlar);
/// - `Denied`: servis durdurulur ve açılışta başlamaz; veri ve kayıt kalır (paket yeniden izin verirse örnek
///   kendiliğinden yeniden başlar), hesap Redis'i kullanamaz;
/// - `Unrestricted`: bellek olduğu gibi kalır; paket yüzünden durdurulmuşsa yeniden başlatılır.
pub async fn sync_package_limit(db: &Db, user_id: i64) -> AppResult<()> {
    let Some(mut instance) = get(db, user_id)? else { return Ok(()) };
    check_name(&instance.username)?;
    let (_, plan) = plan_for(db, user_id)?;
    let unit = unit_name(&instance.username);
    let enabled = exec::run("systemctl", &["is-enabled", "--quiet", "--", &unit]).await.map(|out| out.success()).unwrap_or(false);
    match plan {
        PackagePlan::Denied => {
            if enabled || services::is_active(&unit).await {
                let _ = exec::run("systemctl", &["disable", "--now", "--quiet", "--", &unit]).await;
            }
        }
        PackagePlan::Fixed(_) | PackagePlan::Unrestricted => {
            let target = match plan {
                PackagePlan::Fixed(memory) => memory,
                _ => instance.memory_mb,
            };
            if target != instance.memory_mb || !enabled {
                instance.memory_mb = target;
                db.call(|conn| conn.execute("UPDATE redis_instances SET memory_mb = ?1 WHERE user_id = ?2", params![target, user_id]))?;
                apply(&instance).await?;
            }
        }
    }
    Ok(())
}

/// Örneğe bağlanır ve kimlik doğrular.
pub async fn connect(instance: &Instance) -> AppResult<Client> {
    Client::connect_to(instance.port, Some(&instance.password)).await
}

pub async fn is_running(instance: &Instance) -> bool {
    services::is_active(&unit_name(&instance.username)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Instance {
        Instance { user_id: 7, username: "musteri".into(), port: 16400, password: "a".repeat(48), memory_mb: 64, created_at: 0 }
    }

    #[test]
    fn config_binds_only_locally_and_locks_runtime_changes() {
        let conf = render_config(&sample());
        assert!(conf.contains("bind 127.0.0.1\n"));
        assert!(!conf.contains("0.0.0.0"));
        assert!(conf.contains(&format!("requirepass {}\n", "a".repeat(48))));
        assert!(conf.contains("maxmemory 64mb\n"));
        assert!(conf.contains("unixsocket /run/pixpanel-redis/musteri/redis.sock\n"));
        for command in ["CONFIG", "MODULE", "REPLICAOF", "DEBUG", "SHUTDOWN", "ACL"] {
            assert!(conf.contains(&format!("rename-command {command} \"\"\n")), "{command}");
        }
    }

    #[test]
    fn unit_runs_as_the_account() {
        let unit = render_unit(&sample());
        assert!(unit.contains("User=musteri\n"));
        assert!(unit.contains("ExecStart=/usr/bin/redis-server /etc/pixpanel/redis/musteri.conf\n"));
        assert!(unit.contains("MemoryMax=192M\n"));
        assert!(unit.contains("ReadWritePaths=/var/lib/pixpanel/redis/musteri\n"));
    }

    #[test]
    fn account_names_are_checked() {
        for bad in ["", "../x", "a b", "Ali", "1abc", "a;b", &"a".repeat(33)] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
        assert!(check_name("musteri-1_a").is_ok());
    }

    #[test]
    fn package_memory_decides_the_redis_plan() {
        assert_eq!(package_plan(None), PackagePlan::Unrestricted);
        assert_eq!(package_plan(Some(0)), PackagePlan::Denied, "0: Redis pakette yok");
        assert_eq!(package_plan(Some(256)), PackagePlan::Fixed(256));
        // Doğrulamadan kaçan değerler uç sınırlara çekilir.
        assert_eq!(package_plan(Some(5)), PackagePlan::Fixed(16));
        assert_eq!(package_plan(Some(1_000_000)), PackagePlan::Fixed(MAX_MEMORY_MB));
    }

    #[test]
    fn memory_limits() {
        assert!(validate_memory(15).is_err());
        assert!(validate_memory(MAX_MEMORY_MB + 1).is_err());
        assert_eq!(validate_memory(128).unwrap(), 128);
    }
}
