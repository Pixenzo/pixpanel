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
];

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
        tx.pragma_update(None, "user_version", index + 1)?;
        tx.commit()?;
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
