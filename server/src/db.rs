//! SQLite bağlantısı ve şema göçleri.
//!
//! Panelin durumu tek bir SQLite dosyasında tutulur. Sorgular çok kısa sürdüğü
//! için bağlantı basit bir kilit arkasında paylaşılır.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use rusqlite::Connection;

use crate::error::AppResult;

/// Şema göçleri. Sıra numarası `PRAGMA user_version` ile izlenir;
/// var olan bir göç ASLA değiştirilmez, yenisi sona eklenir.
const MIGRATIONS: &[&str] = &[
    // 1: çekirdek tablolar
    r#"
    CREATE TABLE users (
        id            INTEGER PRIMARY KEY,
        username      TEXT NOT NULL UNIQUE COLLATE NOCASE,
        email         TEXT NOT NULL DEFAULT '',
        password_hash TEXT NOT NULL,
        role          TEXT NOT NULL CHECK (role IN ('admin', 'reseller', 'user', 'member')),
        parent_id     INTEGER REFERENCES users(id) ON DELETE CASCADE,
        permissions   TEXT NOT NULL DEFAULT '[]',
        totp_secret   TEXT,
        status        TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended')),
        language      TEXT NOT NULL DEFAULT 'tr',
        created_at    INTEGER NOT NULL,
        last_login_at INTEGER
    );

    CREATE TABLE sessions (
        id         INTEGER PRIMARY KEY,
        user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        token_hash TEXT NOT NULL UNIQUE,
        ip         TEXT NOT NULL DEFAULT '',
        user_agent TEXT NOT NULL DEFAULT '',
        created_at INTEGER NOT NULL,
        expires_at INTEGER NOT NULL
    );
    CREATE INDEX sessions_user ON sessions(user_id);

    CREATE TABLE api_tokens (
        id           INTEGER PRIMARY KEY,
        user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        name         TEXT NOT NULL,
        token_hash   TEXT NOT NULL UNIQUE,
        prefix       TEXT NOT NULL,
        read_only    INTEGER NOT NULL DEFAULT 0,
        allowed_ips  TEXT NOT NULL DEFAULT '',
        last_used_at INTEGER,
        expires_at   INTEGER,
        created_at   INTEGER NOT NULL
    );
    CREATE INDEX api_tokens_user ON api_tokens(user_id);

    CREATE TABLE audit_log (
        id         INTEGER PRIMARY KEY,
        user_id    INTEGER,
        username   TEXT NOT NULL DEFAULT '',
        action     TEXT NOT NULL,
        target     TEXT NOT NULL DEFAULT '',
        detail     TEXT NOT NULL DEFAULT '',
        ip         TEXT NOT NULL DEFAULT '',
        created_at INTEGER NOT NULL
    );
    CREATE INDEX audit_log_created ON audit_log(created_at);

    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );

    CREATE TABLE jobs (
        id          INTEGER PRIMARY KEY,
        kind        TEXT NOT NULL,
        title       TEXT NOT NULL,
        status      TEXT NOT NULL CHECK (status IN ('running', 'success', 'failed')),
        log         TEXT NOT NULL DEFAULT '',
        user_id     INTEGER,
        created_at  INTEGER NOT NULL,
        finished_at INTEGER
    );
    CREATE INDEX jobs_created ON jobs(created_at);
    "#,
    // 2: siteler
    r#"
    CREATE TABLE sites (
        id              INTEGER PRIMARY KEY,
        user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
        domain          TEXT NOT NULL UNIQUE,
        kind            TEXT NOT NULL CHECK (kind IN ('php', 'static', 'proxy', 'node')),
        public_dir      TEXT NOT NULL DEFAULT 'public',
        php_version     TEXT,
        php_settings    TEXT NOT NULL DEFAULT '{}',
        proxy_target    TEXT,
        proxy_websocket INTEGER NOT NULL DEFAULT 1,
        ssl_mode        TEXT NOT NULL DEFAULT 'none',
        force_https     INTEGER NOT NULL DEFAULT 0,
        hsts            INTEGER NOT NULL DEFAULT 0,
        nginx_custom    TEXT NOT NULL DEFAULT '',
        status          TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended')),
        created_at      INTEGER NOT NULL
    );
    CREATE INDEX sites_user ON sites(user_id);

    CREATE TABLE site_aliases (
        name    TEXT PRIMARY KEY,
        site_id INTEGER NOT NULL REFERENCES sites(id) ON DELETE CASCADE
    );
    CREATE INDEX site_aliases_site ON site_aliases(site_id);
    "#,
    // 3: veritabanları
    r#"
    CREATE TABLE databases (
        id         INTEGER PRIMARY KEY,
        user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
        engine     TEXT NOT NULL CHECK (engine IN ('mariadb', 'postgresql')),
        name       TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        UNIQUE (engine, name)
    );
    CREATE INDEX databases_user ON databases(user_id);

    CREATE TABLE database_users (
        id         INTEGER PRIMARY KEY,
        user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
        engine     TEXT NOT NULL CHECK (engine IN ('mariadb', 'postgresql')),
        name       TEXT NOT NULL,
        host       TEXT NOT NULL DEFAULT '',
        created_at INTEGER NOT NULL,
        UNIQUE (engine, name, host)
    );
    CREATE INDEX database_users_user ON database_users(user_id);

    CREATE TABLE database_grants (
        database_id INTEGER NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
        db_user_id  INTEGER NOT NULL REFERENCES database_users(id) ON DELETE CASCADE,
        access      TEXT NOT NULL CHECK (access IN ('full', 'readwrite', 'readonly')),
        PRIMARY KEY (database_id, db_user_id)
    );
    "#,
    // 4: zamanlanmış görevler
    r#"
    CREATE TABLE cron_jobs (
        id         INTEGER PRIMARY KEY,
        run_as     TEXT NOT NULL,
        schedule   TEXT NOT NULL,
        command    TEXT NOT NULL,
        comment    TEXT NOT NULL DEFAULT '',
        enabled    INTEGER NOT NULL DEFAULT 1,
        created_at INTEGER NOT NULL
    );
    CREATE INDEX cron_jobs_run_as ON cron_jobs(run_as);
    "#,
    // 5: Git ile yayınlama ve bildirimler
    r#"
    CREATE TABLE site_git (
        site_id       INTEGER PRIMARY KEY REFERENCES sites(id) ON DELETE CASCADE,
        repo_url      TEXT NOT NULL,
        branch        TEXT NOT NULL,
        auto_deploy   INTEGER NOT NULL DEFAULT 1,
        script        TEXT NOT NULL DEFAULT '',
        webhook_token TEXT NOT NULL UNIQUE,
        public_key    TEXT NOT NULL DEFAULT '',
        created_at    INTEGER NOT NULL
    );

    CREATE TABLE deployments (
        id             INTEGER PRIMARY KEY,
        site_id        INTEGER NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
        source         TEXT NOT NULL CHECK (source IN ('manual', 'webhook', 'rollback')),
        commit_hash    TEXT NOT NULL DEFAULT '',
        commit_subject TEXT NOT NULL DEFAULT '',
        commit_author  TEXT NOT NULL DEFAULT '',
        status         TEXT NOT NULL CHECK (status IN ('running', 'success', 'failed')),
        job_id         INTEGER,
        started_at     INTEGER NOT NULL,
        finished_at    INTEGER
    );
    CREATE INDEX deployments_site ON deployments(site_id, id);

    CREATE TABLE user_notify (
        user_id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
        config  TEXT NOT NULL
    );
    "#,
    // 6: posta
    r#"
    CREATE TABLE mail_domains (
        id          INTEGER PRIMARY KEY,
        user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
        domain      TEXT NOT NULL UNIQUE,
        catch_all   TEXT NOT NULL DEFAULT '',
        dkim_record TEXT NOT NULL DEFAULT '',
        created_at  INTEGER NOT NULL
    );
    CREATE INDEX mail_domains_user ON mail_domains(user_id);

    CREATE TABLE mailboxes (
        id            INTEGER PRIMARY KEY,
        domain_id     INTEGER NOT NULL REFERENCES mail_domains(id) ON DELETE CASCADE,
        local         TEXT NOT NULL,
        password_hash TEXT NOT NULL,
        quota_mb      INTEGER NOT NULL DEFAULT 1024,
        enabled       INTEGER NOT NULL DEFAULT 1,
        forward_to    TEXT NOT NULL DEFAULT '',
        forward_keep  INTEGER NOT NULL DEFAULT 1,
        auto_reply    TEXT NOT NULL DEFAULT '{}',
        filters       TEXT NOT NULL DEFAULT '[]',
        created_at    INTEGER NOT NULL,
        UNIQUE (domain_id, local)
    );

    CREATE TABLE mail_aliases (
        id           INTEGER PRIMARY KEY,
        domain_id    INTEGER NOT NULL REFERENCES mail_domains(id) ON DELETE CASCADE,
        local        TEXT NOT NULL,
        destinations TEXT NOT NULL,
        created_at   INTEGER NOT NULL,
        UNIQUE (domain_id, local)
    );
    "#,
    // 7: veritabanı araçları için açılan geçici hesaplar
    r#"
    CREATE TABLE db_tool_sessions (
        id         INTEGER PRIMARY KEY,
        engine     TEXT NOT NULL,
        name       TEXT NOT NULL,
        host       TEXT NOT NULL DEFAULT '',
        database   TEXT NOT NULL,
        expires_at INTEGER NOT NULL
    );
    "#,
    // 8: Node.js ayarları ve yol bazlı vekil kuralları
    r#"
    ALTER TABLE sites ADD COLUMN node_settings TEXT NOT NULL DEFAULT '{}';
    ALTER TABLE sites ADD COLUMN proxy_rules TEXT NOT NULL DEFAULT '[]';
    "#,
    // 9: FTP hesapları
    r#"
    CREATE TABLE ftp_accounts (
        id         INTEGER PRIMARY KEY,
        site_id    INTEGER NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
        name       TEXT NOT NULL UNIQUE,
        subdir     TEXT NOT NULL DEFAULT '',
        created_at INTEGER NOT NULL
    );
    CREATE INDEX ftp_accounts_site ON ftp_accounts(site_id);
    "#,
    // 10: DNS bölgeleri ve Cloudflare anahtarları
    r#"
    CREATE TABLE dns_zones (
        id         INTEGER PRIMARY KEY,
        user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
        domain     TEXT NOT NULL UNIQUE,
        provider   TEXT NOT NULL CHECK (provider IN ('local', 'cloudflare')),
        cf_zone_id TEXT NOT NULL DEFAULT '',
        created_at INTEGER NOT NULL
    );
    CREATE INDEX dns_zones_user ON dns_zones(user_id);

    CREATE TABLE cloudflare_tokens (
        user_id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
        token   TEXT NOT NULL
    );
    "#,
    // 11: iki adımlı doğrulama
    r#"
    ALTER TABLE users ADD COLUMN totp_enabled INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE users ADD COLUMN totp_last_step INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE users ADD COLUMN recovery_codes TEXT NOT NULL DEFAULT '[]';
    "#,
    // 12: yedekleme hedefleri ve planları
    r#"
    CREATE TABLE backup_targets (
        id               INTEGER PRIMARY KEY,
        name             TEXT NOT NULL UNIQUE,
        kind             TEXT NOT NULL CHECK (kind IN ('local', 'sftp', 's3')),
        config           TEXT NOT NULL,
        secret           TEXT NOT NULL DEFAULT '',
        password         TEXT NOT NULL,
        public_key       TEXT NOT NULL DEFAULT '',
        last_check_at    INTEGER,
        last_check_ok    INTEGER NOT NULL DEFAULT 0,
        last_check_error TEXT NOT NULL DEFAULT '',
        created_at       INTEGER NOT NULL
    );
    CREATE TABLE backup_plans (
        id           INTEGER PRIMARY KEY,
        user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        name         TEXT NOT NULL,
        target_id    INTEGER NOT NULL REFERENCES backup_targets(id) ON DELETE RESTRICT,
        scope        TEXT NOT NULL,
        frequency    TEXT NOT NULL CHECK (frequency IN ('manual', 'daily', 'weekly', 'monthly')),
        at_minute    INTEGER NOT NULL DEFAULT 180,
        weekday      INTEGER NOT NULL DEFAULT 1,
        month_day    INTEGER NOT NULL DEFAULT 1,
        keep_daily   INTEGER NOT NULL DEFAULT 7,
        keep_weekly  INTEGER NOT NULL DEFAULT 4,
        keep_monthly INTEGER NOT NULL DEFAULT 6,
        enabled      INTEGER NOT NULL DEFAULT 1,
        next_run_at  INTEGER,
        last_run_at  INTEGER,
        last_status  TEXT NOT NULL DEFAULT '',
        last_job_id  INTEGER,
        created_at   INTEGER NOT NULL
    );
    CREATE INDEX backup_plans_target ON backup_plans(target_id);
    "#,
    // 13: kendi kendine bakım. Süren bulgular ve yapılan düzeltmelerin günlüğü.
    r#"
    CREATE TABLE health_findings (
        key          TEXT PRIMARY KEY,
        rule         TEXT NOT NULL,
        first_seen   INTEGER NOT NULL,
        fix_attempts INTEGER NOT NULL DEFAULT 0,
        last_fix_at  INTEGER
    );
    CREATE TABLE health_actions (
        id         INTEGER PRIMARY KEY,
        created_at INTEGER NOT NULL,
        rule       TEXT NOT NULL,
        target     TEXT NOT NULL DEFAULT '',
        summary    TEXT NOT NULL,
        automatic  INTEGER NOT NULL DEFAULT 0,
        success    INTEGER NOT NULL DEFAULT 1,
        detail     TEXT NOT NULL DEFAULT ''
    );
    "#,
    // 14: site erişilebilirlik geçmişi (dakikalık yoklamalar, yedi gün saklanır).
    r#"
    CREATE TABLE site_checks (
        site_id    INTEGER NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
        checked_at INTEGER NOT NULL,
        status     INTEGER NOT NULL,
        millis     INTEGER NOT NULL
    );
    CREATE INDEX site_checks_site ON site_checks(site_id, checked_at);
    CREATE INDEX site_checks_time ON site_checks(checked_at);
    "#,
    // 15: hizmet paketleri, hesaba paket atama ve ölçülen disk kullanımı.
    r#"
    CREATE TABLE packages (
        id         INTEGER PRIMARY KEY,
        owner_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        name       TEXT NOT NULL,
        limits     TEXT NOT NULL DEFAULT '{}',
        created_at INTEGER NOT NULL,
        UNIQUE (owner_id, name)
    );
    ALTER TABLE users ADD COLUMN package_id INTEGER REFERENCES packages(id) ON DELETE SET NULL;
    CREATE TABLE account_usage (
        user_id     INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
        disk_bytes  INTEGER NOT NULL,
        measured_at INTEGER NOT NULL
    );
    "#,
    // 16: hesapların SFTP/kabuk erişimi ve açık anahtarları.
    r#"
    ALTER TABLE users ADD COLUMN ssh_mode TEXT NOT NULL DEFAULT 'none' CHECK (ssh_mode IN ('none', 'sftp', 'shell'));
    ALTER TABLE users ADD COLUMN ssh_password_set INTEGER NOT NULL DEFAULT 0;
    CREATE TABLE ssh_keys (
        id          INTEGER PRIMARY KEY,
        user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        name        TEXT NOT NULL,
        key_line    TEXT NOT NULL,
        fingerprint TEXT NOT NULL,
        created_at  INTEGER NOT NULL,
        UNIQUE (user_id, fingerprint)
    );
    "#,
    // 17: site istatistikleri (erişim günlüklerinden çıkarılan günlük özetler).
    r#"
    CREATE TABLE site_log_state (
        site_id INTEGER PRIMARY KEY REFERENCES sites(id) ON DELETE CASCADE,
        inode   INTEGER NOT NULL,
        offset  INTEGER NOT NULL
    );
    CREATE TABLE site_stats_daily (
        site_id  INTEGER NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
        day      INTEGER NOT NULL,
        requests INTEGER NOT NULL DEFAULT 0,
        pages    INTEGER NOT NULL DEFAULT 0,
        bytes    INTEGER NOT NULL DEFAULT 0,
        visitors INTEGER NOT NULL DEFAULT 0,
        bots     INTEGER NOT NULL DEFAULT 0,
        s2xx     INTEGER NOT NULL DEFAULT 0,
        s3xx     INTEGER NOT NULL DEFAULT 0,
        s4xx     INTEGER NOT NULL DEFAULT 0,
        s5xx     INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (site_id, day)
    ) WITHOUT ROWID;
    CREATE TABLE site_stats_visitors (
        site_id INTEGER NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
        day     INTEGER NOT NULL,
        hash    INTEGER NOT NULL,
        PRIMARY KEY (site_id, day, hash)
    ) WITHOUT ROWID;
    CREATE TABLE site_stats_paths (
        site_id INTEGER NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
        day     INTEGER NOT NULL,
        path    TEXT NOT NULL,
        hits    INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (site_id, day, path)
    ) WITHOUT ROWID;
    "#,
    // 18: sunucu yükünün uzun dönem geçmişi (dakikalık ortalamalar ve saatlik özetler).
    r#"
    CREATE TABLE metrics_minutes (
        ts        INTEGER PRIMARY KEY,
        cpu       REAL NOT NULL,
        mem_used  INTEGER NOT NULL,
        mem_total INTEGER NOT NULL,
        load1     REAL NOT NULL,
        net_rx    INTEGER NOT NULL,
        net_tx    INTEGER NOT NULL
    ) WITHOUT ROWID;
    CREATE TABLE metrics_hours (
        ts        INTEGER PRIMARY KEY,
        cpu       REAL NOT NULL,
        mem_used  INTEGER NOT NULL,
        mem_total INTEGER NOT NULL,
        load1     REAL NOT NULL,
        net_rx    INTEGER NOT NULL,
        net_tx    INTEGER NOT NULL
    ) WITHOUT ROWID;
    "#,
    // 19: yedek hedefi türlerine "ftp" eklenir. SQLite denetim kısıtını yerinde değiştiremez;
    // tablo yeniden kurulur. Planlar hedefe bağlı olduğundan (ON DELETE RESTRICT) geçici
    // olarak kenara alınır.
    r#"
    CREATE TABLE backup_plans_keep AS SELECT * FROM backup_plans;
    DELETE FROM backup_plans;
    CREATE TABLE backup_targets_new (
        id               INTEGER PRIMARY KEY,
        name             TEXT NOT NULL UNIQUE,
        kind             TEXT NOT NULL CHECK (kind IN ('local', 'sftp', 's3', 'ftp')),
        config           TEXT NOT NULL,
        secret           TEXT NOT NULL DEFAULT '',
        password         TEXT NOT NULL,
        public_key       TEXT NOT NULL DEFAULT '',
        last_check_at    INTEGER,
        last_check_ok    INTEGER NOT NULL DEFAULT 0,
        last_check_error TEXT NOT NULL DEFAULT '',
        created_at       INTEGER NOT NULL
    );
    INSERT INTO backup_targets_new SELECT id, name, kind, config, secret, password, public_key, last_check_at, last_check_ok, last_check_error, created_at FROM backup_targets;
    DROP TABLE backup_targets;
    ALTER TABLE backup_targets_new RENAME TO backup_targets;
    INSERT INTO backup_plans SELECT * FROM backup_plans_keep;
    DROP TABLE backup_plans_keep;
    "#,
    // 20: site şablonu (uygulamaya göre hazır web sunucusu ayarı).
    r#"
    ALTER TABLE sites ADD COLUMN template TEXT NOT NULL DEFAULT 'default';
    "#,
    // 21: sunucunun IP adresleri hakkında panelin tuttuğu bilgiler ve sitenin bağlandığı adres
    //     (boş: bütün adresler).
    r#"
    CREATE TABLE server_ips (
        address        TEXT PRIMARY KEY,
        prefix         INTEGER NOT NULL DEFAULT 0,
        interface      TEXT NOT NULL DEFAULT '',
        managed        INTEGER NOT NULL DEFAULT 0,
        label          TEXT NOT NULL DEFAULT '',
        public_address TEXT NOT NULL DEFAULT '',
        user_id        INTEGER REFERENCES users(id) ON DELETE SET NULL,
        created_at     INTEGER NOT NULL
    );
    ALTER TABLE sites ADD COLUMN ip TEXT NOT NULL DEFAULT '';
    "#,
    // 22: hesap başına Redis örnekleri.
    r#"
    CREATE TABLE redis_instances (
        user_id    INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
        port       INTEGER NOT NULL UNIQUE,
        password   TEXT NOT NULL,
        memory_mb  INTEGER NOT NULL,
        created_at INTEGER NOT NULL
    );
    "#,
    // 23: hesap numaraları yeniden kullanılmaz. SQLite, en büyük numaralı hesap silinince o
    //     numarayı sonraki hesaba verir; görev ve denetim kayıtları numarayla tutulduğundan yeni
    //     hesap, silinmiş hesabın geçmişini görebiliyordu. Bugüne kadar verilmiş en büyük numara
    //     kaydedilir (bkz. `next_user_id`) ve sahipsiz kalmış kayıtların hesap bağı koparılır.
    r#"
    INSERT OR REPLACE INTO settings (key, value) VALUES ('users.id_floor', (
        SELECT MAX(
            COALESCE((SELECT MAX(id) FROM users), 0),
            COALESCE((SELECT MAX(user_id) FROM jobs), 0),
            COALESCE((SELECT MAX(user_id) FROM audit_log), 0)
        )
    ));
    UPDATE jobs SET user_id = NULL WHERE user_id IS NOT NULL AND user_id NOT IN (SELECT id FROM users);
    UPDATE audit_log SET user_id = NULL WHERE user_id IS NOT NULL AND user_id NOT IN (SELECT id FROM users);
    "#,
    // 24: başka hesap adına açılan oturumlar (yönetici ya da bayi, müşterisinin panelini görür).
    //     Oturumu asıl açan hesap silinirse oturum da kapanır.
    r#"
    ALTER TABLE sessions ADD COLUMN impersonator_id INTEGER REFERENCES users(id) ON DELETE CASCADE;
    "#,
    // 25: site kuralları (yönlendirme, parola koruması, adres engelleme, bakım kipi…).
    r#"
    ALTER TABLE sites ADD COLUMN rules TEXT NOT NULL DEFAULT '{}';
    "#,
    // 26: disk kotası uyarısının en son hangi düzeyde gönderildiği (0: yok, 1: %90, 2: doldu);
    //     aynı uyarı her ölçümde yinelenmesin diye.
    r#"
    ALTER TABLE account_usage ADD COLUMN warned_level INTEGER NOT NULL DEFAULT 0;
    "#,
    // 27: site türlerine "python" eklenir. SQLite denetim kısıtını yerinde değiştiremez; tabloyu
    //     yeniden kurmak ise ona bağlı dokuz tabloyu taşımayı gerektirirdi. Diskteki veriyi
    //     etkilemeyen bu tür değişiklikler için SQLite'ın belgelediği yol izlenir: tablonun
    //     kayıtlı tanımı düzeltilir (göç yürütücüsü ardından şema sürümünü artırır).
    r#"
    PRAGMA writable_schema = ON;
    UPDATE sqlite_master
       SET sql = replace(sql, 'CHECK (kind IN (''php'', ''static'', ''proxy'', ''node''))', 'CHECK (kind IN (''php'', ''static'', ''proxy'', ''node'', ''python''))')
     WHERE type = 'table' AND name = 'sites';
    PRAGMA writable_schema = OFF;
    "#,
    // 28: WordPress hazırlık ortamı: kopyanın hangi sitenin hazırlık ortamı olduğu. Asıl site
    //     silinince bağ kopar, kopya sıradan bir site olarak kalır.
    r#"
    ALTER TABLE sites ADD COLUMN staging_of INTEGER REFERENCES sites(id) ON DELETE SET NULL;
    "#,
    // 29: olay kancaları (hesabın tanımladığı adreslere HTTP POST) ve son gönderimlerin sonucu;
    //     Git web kancası için isteğe bağlı imza gizli anahtarı (boşsa yalnızca adresteki anahtar geçerli).
    r#"
    CREATE TABLE webhooks (
        id         INTEGER PRIMARY KEY,
        user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        url        TEXT NOT NULL,
        events     TEXT NOT NULL DEFAULT '["*"]',
        secret     TEXT NOT NULL,
        enabled    INTEGER NOT NULL DEFAULT 1,
        created_at INTEGER NOT NULL
    );
    CREATE INDEX webhooks_user ON webhooks(user_id);

    CREATE TABLE webhook_deliveries (
        id          INTEGER PRIMARY KEY,
        webhook_id  INTEGER NOT NULL REFERENCES webhooks(id) ON DELETE CASCADE,
        event       TEXT NOT NULL,
        attempt     INTEGER NOT NULL,
        status      INTEGER,
        duration_ms INTEGER NOT NULL,
        error       TEXT NOT NULL DEFAULT '',
        created_at  INTEGER NOT NULL
    );
    CREATE INDEX webhook_deliveries_hook ON webhook_deliveries(webhook_id, id);

    ALTER TABLE site_git ADD COLUMN signature_secret TEXT NOT NULL DEFAULT '';
    "#,
];

const USER_ID_FLOOR_KEY: &str = "users.id_floor";

/// Yeni hesabın numarası: bugüne kadar verilmiş en büyük numaranın bir fazlası. Silinmiş bir
/// hesabın numarası asla yeniden verilmez; verilen numara aynı işlemde kaydedilir.
pub fn next_user_id(conn: &Connection) -> rusqlite::Result<i64> {
    use rusqlite::OptionalExtension;
    let highest: i64 = conn.query_row("SELECT COALESCE(MAX(id), 0) FROM users", [], |row| row.get(0))?;
    let floor: i64 = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [USER_ID_FLOOR_KEY], |row| row.get::<_, String>(0))
        .optional()?
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let id = highest.max(floor) + 1;
    conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)", rusqlite::params![USER_ID_FLOOR_KEY, id.to_string()])?;
    Ok(id)
}

#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("veri klasörü oluşturulamadı: {}", parent.display()))?;
        }
        let mut conn = Connection::open(path)
            .with_context(|| format!("veritabanı açılamadı: {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        migrate(&mut conn)?;
        restrict_permissions(path);
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    /// Bağlantı üzerinde bir işlem çalıştırır.
    pub fn call<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
    ) -> AppResult<T> {
        let mut conn = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(f(&mut conn)?)
    }
}

impl Db {
    /// `settings` tablosundan bir değer okur.
    pub fn setting(&self, key: &str) -> AppResult<Option<String>> {
        use rusqlite::OptionalExtension;
        self.call(|conn| {
            conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| row.get(0)).optional()
        })
    }

    pub fn set_setting(&self, key: &str, value: &str) -> AppResult<()> {
        self.call(|conn| {
            conn.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, value],
            )
        })?;
        Ok(())
    }
}

fn migrate(conn: &mut Connection) -> anyhow::Result<()> {
    let current: usize = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)
            .with_context(|| format!("şema göçü {} uygulanamadı", index + 1))?;
        if sql.contains("writable_schema") {
            // Tablo tanımı elle düzeltildi: bağlantıların şemayı yeniden okuması için sürüm artırılır
            // ve düzeltmenin gerçekten uygulandığı, veritabanının sağlam kaldığı doğrulanır.
            let version: i64 = tx.pragma_query_value(None, "schema_version", |row| row.get(0))?;
            tx.pragma_update(None, "schema_version", version + 1)?;
            let definition: String = tx.query_row("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'sites'", [], |row| row.get(0))?;
            anyhow::ensure!(definition.contains("'python'"), "şema göçü {}: sites tablosunun tanımı beklenen biçimde değil", index + 1);
        }
        tx.pragma_update(None, "user_version", index + 1)?;
        tx.commit()?;
        if sql.contains("writable_schema") {
            let check: String = conn.pragma_query_value(None, "integrity_check", |row| row.get(0))?;
            anyhow::ensure!(check == "ok", "şema göçü {} sonrası bütünlük denetimi başarısız: {check}", index + 1);
        }
        tracing::info!(version = index + 1, "şema göçü uygulandı");
    }
    Ok(())
}

/// Veritabanı parola özetleri içerdiğinden yalnızca root okuyabilmelidir.
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Şu anki zaman (Unix saniyesi).
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
