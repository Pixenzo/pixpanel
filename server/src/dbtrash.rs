//! Veritabanı çöp kutusu: silme onayında "önce yedek al" seçilirse veritabanının dökümü sıkıştırılıp
//! panelin kendi klasörüne (`<veri klasörü>/trash/databases`, yalnızca root okuyabilir) yazılır ve
//! 7 gün saklanır. Bu sürede veritabanı aynı adla geri getirilebilir; süresi dolan döküm kendiliğinden silinir.
//!
//! Döküm veritabanı silinmeden ÖNCE alınır ve başarısız olursa silme yapılmaz.

use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::auth;
use crate::db::now;
use crate::dbms::Engine;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::sys::exec::base_command;

/// Çöp kutusundaki dökümün saklanma süresi.
pub const RETENTION_SECS: i64 = 7 * 86400;
/// Dökümün en uzun süreceği zaman.
const DUMP_TIMEOUT: Duration = Duration::from_secs(3600);

/// Veritabanındaki bir kullanıcı yetkisi; geri getirilirken yeniden verilir.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Grant {
    pub name: String,
    pub host: String,
    pub access: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub id: i64,
    pub user_id: i64,
    pub engine: String,
    pub name: String,
    pub size: i64,
    pub deleted_at: i64,
    pub expires_at: i64,
    #[serde(skip)]
    pub file: String,
    #[serde(skip)]
    pub grants: Vec<Grant>,
}

fn directory(state: &AppState) -> PathBuf {
    state.cfg.data_dir.join("trash").join("databases")
}

/// Çöp klasörü ve içindeki dosya adı: dosya adı rastgeledir, veritabanı adından türemez.
fn file_path(state: &AppState, file: &str) -> AppResult<PathBuf> {
    let valid = !file.is_empty() && file.len() <= 64 && file.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')) && !file.contains("..");
    if !valid {
        return Err(AppError::internal("çöp kutusu dosya adı geçersiz"));
    }
    Ok(directory(state).join(file))
}

fn prepare_directory(state: &AppState) -> AppResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = directory(state);
    std::fs::create_dir_all(&dir)?;
    // Üst klasörler de yalnızca root'a açık olsun (döküm müşteri verisidir).
    for path in [dir.parent().map(PathBuf::from), Some(dir.clone())].into_iter().flatten() {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Veritabanının dökümünü sıkıştırarak çöp klasörüne yazar; dosya adı ve boyutunu döner.
/// Döküm ya da sıkıştırma başarısız olursa yarım dosya silinir ve hata döner.
pub async fn dump(state: &AppState, engine: Engine, database: &str) -> AppResult<(String, i64)> {
    prepare_directory(state)?;
    let file = format!("{}.sql.gz", auth::random_token(16));
    let path = file_path(state, &file)?;
    let output = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;

    let outcome = async {
        let mut dump = crate::dbms::dump_command(engine, database)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| AppError::internal(format!("döküm başlatılamadı: {err}")))?;
        let pipe: Stdio = dump.stdout.take().expect("stdout borusu").try_into().map_err(|err: std::io::Error| AppError::internal(err.to_string()))?;
        let mut gzip = base_command("gzip")
            .arg("-c")
            .stdin(pipe)
            .stdout(Stdio::from(output))
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| AppError::internal(format!("gzip başlatılamadı: {err}")))?;
        let waited = tokio::time::timeout(DUMP_TIMEOUT, async { (gzip.wait().await, dump.wait().await) }).await;
        match waited {
            Err(_) => {
                let _ = dump.kill().await;
                let _ = gzip.kill().await;
                Err(AppError::bad("Veritabanı dökümü bir saatte bitmedi"))
            }
            Ok((Ok(gz), Ok(dumped))) if gz.success() && dumped.success() => Ok(()),
            Ok(_) => Err(AppError::bad("Veritabanının yedeği alınamadı; silme yapılmadı")),
        }
    }
    .await;
    if let Err(err) = outcome {
        let _ = std::fs::remove_file(&path);
        return Err(err);
    }
    let size = std::fs::metadata(&path).map(|meta| meta.len() as i64).unwrap_or(0);
    Ok((file, size))
}

/// Dökümü çöp kutusuna kaydeder (veritabanı silinmeden önce); kaydın kimliğini döner.
pub fn record(state: &AppState, user_id: i64, engine: Engine, name: &str, file: &str, size: i64, grants: &[Grant]) -> AppResult<i64> {
    let encoded = serde_json::to_string(grants).map_err(|err| AppError::internal(err.to_string()))?;
    let deleted_at = now();
    state.db.call(|conn| {
        conn.execute(
            "INSERT INTO database_trash (user_id, engine, name, file, size, grants, deleted_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![user_id, engine.as_str(), name, file, size, encoded, deleted_at, deleted_at + RETENTION_SECS],
        )?;
        Ok(conn.last_insert_rowid())
    })
}

fn read_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    let grants: String = row.get(7)?;
    Ok(Entry {
        id: row.get(0)?,
        user_id: row.get(1)?,
        engine: row.get(2)?,
        name: row.get(3)?,
        file: row.get(4)?,
        size: row.get(5)?,
        deleted_at: row.get(6)?,
        grants: serde_json::from_str(&grants).unwrap_or_default(),
        expires_at: row.get(8)?,
    })
}

const COLUMNS: &str = "id, user_id, engine, name, file, size, deleted_at, grants, expires_at";

/// Geri getirilebilecek (süresi dolmamış, geri getirilmemiş) kayıtlar; `owner` verilirse yalnız o hesabınkiler.
pub fn list(state: &AppState, owner: Option<i64>) -> AppResult<Vec<Entry>> {
    let current = now();
    state.db.call(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM database_trash WHERE restored_at IS NULL AND expires_at > ?1 AND (?2 IS NULL OR user_id = ?2) ORDER BY deleted_at DESC"
        ))?;
        let rows = stmt.query_map(params![current, owner], read_entry)?;
        rows.collect()
    })
}

/// Tek kayıt; hesap sınırı `owner` ile uygulanır (başkasının kaydı "yok" sayılır).
pub fn load(state: &AppState, id: i64, owner: Option<i64>) -> AppResult<Entry> {
    let entry = state
        .db
        .call(|conn| conn.query_row(&format!("SELECT {COLUMNS} FROM database_trash WHERE id = ?1 AND restored_at IS NULL"), params![id], read_entry).optional())?
        .ok_or(AppError::NotFound)?;
    if owner.is_some_and(|owner| owner != entry.user_id) {
        return Err(AppError::NotFound);
    }
    if entry.expires_at <= now() {
        return Err(AppError::bad("Bu dökümün 7 günlük saklama süresi dolmuş"));
    }
    Ok(entry)
}

/// Dökümün dosya yolu.
pub fn path_of(state: &AppState, entry: &Entry) -> AppResult<PathBuf> {
    file_path(state, &entry.file)
}

/// Kaydı olmayan (silme yarıda kalmış) bir döküm dosyasını siler.
pub fn discard_file(state: &AppState, file: &str) {
    if let Ok(path) = file_path(state, file) {
        let _ = std::fs::remove_file(path);
    }
}

/// Kaydı ve dosyasını siler (geri getirme sonrası, elle silme ya da süre dolunca).
pub fn remove(state: &AppState, id: i64) -> AppResult<()> {
    let file: Option<String> = state.db.call(|conn| conn.query_row("SELECT file FROM database_trash WHERE id = ?1", params![id], |row| row.get(0)).optional())?;
    if let Some(file) = file {
        if let Ok(path) = file_path(state, &file) {
            let _ = std::fs::remove_file(path);
        }
    }
    state.db.call(|conn| conn.execute("DELETE FROM database_trash WHERE id = ?1", params![id]))?;
    Ok(())
}

pub fn mark_restored(state: &AppState, id: i64) -> AppResult<()> {
    state.db.call(|conn| conn.execute("UPDATE database_trash SET restored_at = ?1 WHERE id = ?2", params![now(), id]))?;
    Ok(())
}

/// Süresi dolan dökümleri (ve geri getirilmiş kayıtların kalıntısını) siler; silinen sayıyı döner.
pub fn purge_expired(state: &AppState) -> AppResult<usize> {
    let ids: Vec<i64> = state.db.call(|conn| {
        let mut stmt = conn.prepare("SELECT id FROM database_trash WHERE expires_at <= ?1 OR restored_at IS NOT NULL")?;
        let rows = stmt.query_map(params![now()], |row| row.get(0))?;
        rows.collect()
    })?;
    for id in &ids {
        remove(state, *id)?;
    }
    Ok(ids.len())
}

/// Saatte bir süresi dolanları temizler.
pub fn spawn_cleanup(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(150)).await;
        loop {
            match purge_expired(&state) {
                Ok(0) => {}
                Ok(count) => tracing::info!(count, "süresi dolan veritabanı dökümleri silindi"),
                Err(err) => tracing::warn!(error = %err, "veritabanı çöp kutusu temizlenemedi"),
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_is_seven_days() {
        assert_eq!(RETENTION_SECS, 7 * 86400);
    }

    #[test]
    fn grants_round_trip_through_json() {
        let grants = vec![Grant { name: "app".into(), host: "localhost".into(), access: "readwrite".into() }];
        let encoded = serde_json::to_string(&grants).unwrap();
        assert_eq!(serde_json::from_str::<Vec<Grant>>(&encoded).unwrap(), grants);
    }
}
