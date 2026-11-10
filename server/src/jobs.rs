//! Uzun süren işler (paket kurulumu, yedekleme vb.) için görev sistemi.
//!
//! Her görev arka planda çalışır; çıktısı satır satır biriktirilir ve canlı
//! olarak izlenebilir. Bittiğinde günlüğüyle birlikte veritabanına yazılır.

use std::collections::HashMap;
use std::future::Future;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use rusqlite::params;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::broadcast;

use crate::db::{now, Db};
use crate::error::{AppError, AppResult};
use crate::sys::exec::base_command;

/// Bellekte tutulan günlüğün üst sınırı; aşılırsa baştan kırpılır.
const MAX_LOG_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum JobEvent {
    Log { line: String },
    Done { status: String },
}

struct LiveJob {
    log: Mutex<String>,
    tx: broadcast::Sender<JobEvent>,
}

#[derive(Clone)]
pub struct JobManager {
    db: Db,
    live: Arc<Mutex<HashMap<i64, Arc<LiveJob>>>>,
}

/// Çalışan görevin günlüğe yazmak ve komut çalıştırmak için kullandığı tutamak.
#[derive(Clone)]
pub struct JobCtx {
    job: Arc<LiveJob>,
}

impl JobCtx {
    pub fn log(&self, line: impl Into<String>) {
        let line = line.into();
        {
            let mut log = self.job.log.lock().unwrap_or_else(|p| p.into_inner());
            log.push_str(&line);
            log.push('\n');
            if log.len() > MAX_LOG_BYTES {
                let mut cut = log.len() - MAX_LOG_BYTES / 2;
                while !log.is_char_boundary(cut) {
                    cut += 1;
                }
                log.drain(..cut);
            }
        }
        let _ = self.job.tx.send(JobEvent::Log { line });
    }

    /// Komutu çalıştırır ve çıktısını canlı olarak görev günlüğüne aktarır.
    pub async fn exec(&self, program: &str, args: &[&str]) -> AppResult<()> {
        self.log(format!("$ {program} {}", args.join(" ")));
        let mut child = base_command(program)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| AppError::internal(format!("komut başlatılamadı ({program}): {err}")))?;

        let stdout = child.stdout.take().expect("stdout borusu");
        let stderr = child.stderr.take().expect("stderr borusu");
        let out_ctx = self.clone();
        let err_ctx = self.clone();
        let out_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                out_ctx.log(line);
            }
        });
        let err_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                err_ctx.log(line);
            }
        });

        let status = child.wait().await?;
        let _ = tokio::join!(out_task, err_task);
        if status.success() {
            Ok(())
        } else {
            Err(AppError::internal(format!(
                "{program} başarısız oldu (kod {})",
                status.code().unwrap_or(-1)
            )))
        }
    }
}

#[derive(Debug, Serialize)]
pub struct JobInfo {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub status: String,
    pub user_id: Option<i64>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

impl JobManager {
    pub fn new(db: Db) -> AppResult<Self> {
        // Panel yeniden başladıysa yarım kalan görevler artık çalışmıyordur.
        db.call(|conn| {
            conn.execute(
                "UPDATE jobs SET status = 'failed', finished_at = ?1,
                        log = log || char(10) || '[panel yeniden başlatıldı, görev yarıda kesildi]'
                 WHERE status = 'running'",
                params![now()],
            )
        })?;
        Ok(Self { db, live: Arc::default() })
    }

    /// Görevi arka planda başlatır ve kimliğini döner.
    pub fn start<F, Fut>(&self, kind: &str, title: &str, user_id: Option<i64>, work: F) -> AppResult<i64>
    where
        F: FnOnce(JobCtx) -> Fut + Send + 'static,
        Fut: Future<Output = AppResult<()>> + Send + 'static,
    {
        let id = self.db.call(|conn| {
            conn.execute(
                "INSERT INTO jobs (kind, title, status, user_id, created_at)
                 VALUES (?1, ?2, 'running', ?3, ?4)",
                params![kind, title, user_id, now()],
            )?;
            Ok(conn.last_insert_rowid())
        })?;

        let (tx, _) = broadcast::channel(1024);
        let job = Arc::new(LiveJob { log: Mutex::new(String::new()), tx });
        self.live.lock().unwrap_or_else(|p| p.into_inner()).insert(id, job.clone());

        let manager = self.clone();
        tokio::spawn(async move {
            let ctx = JobCtx { job: job.clone() };
            // Görev paniklese bile durumunun "çalışıyor" olarak kalmaması için ayrı görevde koşturulur.
            let outcome = tokio::spawn(work(ctx.clone())).await;
            let status = match outcome {
                Ok(Ok(())) => "success",
                Ok(Err(err)) => {
                    ctx.log(format!("HATA: {err}"));
                    "failed"
                }
                Err(err) => {
                    ctx.log(format!("HATA: görev beklenmedik biçimde sonlandı: {err}"));
                    "failed"
                }
            };
            let log = job.log.lock().unwrap_or_else(|p| p.into_inner()).clone();
            let saved = manager.db.call(|conn| {
                conn.execute(
                    "UPDATE jobs SET status = ?1, log = ?2, finished_at = ?3 WHERE id = ?4",
                    params![status, log, now(), id],
                )
            });
            if let Err(err) = saved {
                tracing::error!(error = %err, job = id, "görev sonucu kaydedilemedi");
            }
            manager.live.lock().unwrap_or_else(|p| p.into_inner()).remove(&id);
            let _ = job.tx.send(JobEvent::Done { status: status.to_string() });
        });
        Ok(id)
    }

    pub fn list(&self, limit: i64) -> AppResult<Vec<JobInfo>> {
        self.db.call(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, kind, title, status, user_id, created_at, finished_at
                 FROM jobs ORDER BY id DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit], read_info)?;
            rows.collect()
        })
    }

    /// Görev bilgisi ve o ana kadarki günlüğü.
    pub fn get(&self, id: i64) -> AppResult<(JobInfo, String)> {
        let (info, saved_log) = self.db.call(|conn| {
            conn.query_row(
                "SELECT id, kind, title, status, user_id, created_at, finished_at, log
                 FROM jobs WHERE id = ?1",
                params![id],
                |row| Ok((read_info(row)?, row.get::<_, String>(7)?)),
            )
        })?;
        let live = self.live.lock().unwrap_or_else(|p| p.into_inner()).get(&id).cloned();
        let log = match live {
            Some(job) => job.log.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            None => saved_log,
        };
        Ok((info, log))
    }

    /// Canlı izleme: o ana kadarki günlük ve sonraki olaylar için alıcı.
    /// Görev bitmişse alıcı `None` döner.
    pub fn subscribe(&self, id: i64) -> AppResult<(String, Option<broadcast::Receiver<JobEvent>>)> {
        let live = self.live.lock().unwrap_or_else(|p| p.into_inner()).get(&id).cloned();
        match live {
            Some(job) => {
                // Günlük kilidi tutulurken abone olunur; arada satır kaçmaz.
                let log = job.log.lock().unwrap_or_else(|p| p.into_inner());
                let rx = job.tx.subscribe();
                Ok((log.clone(), Some(rx)))
            }
            None => Ok((self.get(id)?.1, None)),
        }
    }
}

fn read_info(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobInfo> {
    Ok(JobInfo {
        id: row.get(0)?,
        kind: row.get(1)?,
        title: row.get(2)?,
        status: row.get(3)?,
        user_id: row.get(4)?,
        created_at: row.get(5)?,
        finished_at: row.get(6)?,
    })
}
