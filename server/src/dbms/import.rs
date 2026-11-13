//! SQL dökümlerinin veritabanına yüklenmesi (dosya yükleme, yedekten dönüş, kopyalama).
//!
//! Döküm güvenilmeyen girdidir: müşterinin yüklediği dosya da, müşterinin verisinden üretilmiş
//! yedek de keyfi SQL ve istemci komutu içerebilir. Bu yüzden döküm asla yönetici kimliğiyle
//! çalıştırılmaz:
//!
//! * **Veritabanı tarafında** yalnızca o veritabanına yetkili, sınırlı bir hesapla bağlanılır.
//!   Döküm başka veritabanına dokunamaz, kullanıcı açamaz, yetki veremez, dosya okuyup yazamaz.
//! * **İşletim sistemi tarafında** istemci yetkisiz `nobody` kullanıcısıyla çalışır. İstemcilerin
//!   kabuk komutu çalıştıran yönergeleri (`\!`, `system`) dökümün içinden çağrılsa bile panelin
//!   (root) ya da veritabanı sunucusunun kimliğiyle çalışmaz.
//!
//! MariaDB'de görünüm, tetikleyici ve yordamların "tanımlayan" hesabı kalıcı olmalıdır (silinirse
//! o nesneler çalışmaz). Bu yüzden veritabanı başına kalıcı ama kilitli bir içe aktarma hesabı
//! tutulur; yalnızca yükleme sırasında açılır. Dökümdeki `DEFINER=` ifadeleri akıştan çıkarılır,
//! böylece nesnelerin tanımlayanı bu hesap olur.

use std::process::Stdio;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::{mariadb, postgres, Engine};
use crate::auth;
use crate::error::{AppError, AppResult};
use crate::jobs::JobCtx;
use crate::sys::exec::base_command;

/// `nobody` kullanıcısının ve grubunun numarası (Debian ve Ubuntu'da sabittir).
const NOBODY: &str = "65534";
/// Günlüğe aktarılan en çok hata satırı; bozuk bir döküm binlerce satır üretebilir.
const MAX_LOG_LINES: usize = 200;

/// Veritabanının kalıcı içe aktarma hesabının adı (MariaDB). Ad veritabanından türetilir;
/// müşterinin açabileceği kullanıcı adlarıyla çakışmaması için özet kullanılır.
pub fn mariadb_account(database: &str) -> String {
    format!("pxi_{}", &hex::encode(Sha256::digest(database.as_bytes()))[..24])
}

/// Yetkisiz kullanıcıyla çalışan bir komut hazırlar. Ortam temizdir; istemci, kimsenin
/// ev klasöründeki ayar dosyasını okumaz.
fn unprivileged(program: &str, args: &[&str]) -> Command {
    let mut command = base_command(crate::sys::runas::SETPRIV);
    command.args([&format!("--reuid={NOBODY}"), &format!("--regid={NOBODY}"), "--clear-groups", "--", program]);
    command.args(args);
    command.env_clear().env("PATH", "/usr/bin:/bin").env("HOME", "/nonexistent").env("LC_ALL", "C.UTF-8").current_dir("/");
    command
}

/// Açık bir içe aktarma oturumu: sınırlı hesabın adı ve o yüklemeye özel parolası.
struct Session {
    engine: Engine,
    database: String,
    role: String,
    password: String,
    /// PostgreSQL istemcisi ilk hatada dursun mu (yoksa hataları yazıp devam eder).
    strict: bool,
}

impl Session {
    async fn open(engine: Engine, database: &str, strict: bool) -> AppResult<Self> {
        super::validate_name(database, "Veritabanı adı")?;
        let password = auth::random_token(24);
        let role = match engine {
            Engine::Mariadb => {
                let role = mariadb_account(database);
                mariadb::open_import_account(database, &role, &password).await?;
                role
            }
            Engine::Postgresql => {
                // Nesnelerin sahibi veritabanının ortak sahip rolüdür; bu rol yalnızca geçici bir giriştir.
                let role = format!("pxi_{}", auth::random_token(10));
                postgres::create_user(&role, &password).await?;
                if let Err(err) = postgres::grant(database, &role, super::Access::Full).await {
                    let _ = postgres::drop_user(&role).await;
                    return Err(err);
                }
                role
            }
        };
        Ok(Self { engine, database: database.to_string(), role, password, strict })
    }

    /// Dökümü standart girdisinden okuyan istemci.
    fn client(&self) -> Command {
        match self.engine {
            Engine::Mariadb => {
                let user = format!("--user={}", self.role);
                // `--sandbox`: dökümün içindeki istemci komutları (`\!`, `system`, `source`, `tee`) çalışmaz;
                // yoksa döküm yükleyen müşteri sunucuda `nobody` olarak komut çalıştırabilirdi.
                let mut command =
                    unprivileged("mariadb", &["--no-defaults", "--sandbox", "--default-character-set=utf8mb4", "--protocol=socket", &user, "--", &self.database]);
                command.env("MYSQL_PWD", &self.password);
                command
            }
            Engine::Postgresql => {
                // Döküm okunmadan önce istemci kısıtlı kipe alınır: `\restrict` sonrasında ters bölülü
                // komutların hiçbiri (`\!`, `\copy … program`, `\o |komut`, `\i`) çalışmaz; kipten çıkmak
                // için gereken anahtar rastgeledir ve dökümü yazan onu bilemez.
                let restrict = format!("\\restrict {}", auth::random_token(24));
                let mut args = vec!["-X", "-q"];
                if self.strict {
                    args.extend(["-v", "ON_ERROR_STOP=1"]);
                }
                args.extend(["-h", "127.0.0.1", "-U", &self.role, "-d", &self.database, "-c", &restrict, "-f", "-"]);
                let mut command = unprivileged("psql", &args);
                command.env("PGPASSWORD", &self.password);
                command
            }
        }
    }

    /// Dökümün istemciye gitmeden önce geçtiği süzgeç.
    fn filter(&self) -> Command {
        match self.engine {
            // Yalnızca döküm aracının ürettiği tanım satırlarına dokunulur (`/*!50013 DEFINER=…`,
            // `CREATE DEFINER=…`); veri satırları `INSERT` ile başlar ve olduğu gibi geçer.
            Engine::Mariadb => unprivileged(
                "sed",
                &[
                    "-E",
                    "-e",
                    r"/^(\/\*![0-9]{5} |CREATE DEFINER=)/ s/ ?DEFINER=`[^`]*`@`[^`]*`//",
                    // MySQL 8 dökümlerindeki varsayılan sıralama MariaDB'de yoktur; yalnızca tablo ve
                    // sütun tanım satırlarında (veri satırları `INSERT` ile başlar) karşılığına çevrilir.
                    "-e",
                    r"/^(\)| ) .*utf8mb4_0900_ai_ci/ s/utf8mb4_0900_ai_ci/utf8mb4_unicode_520_ci/g",
                ],
            ),
            // pg_dump'ın kendi yazdığı `\restrict` / `\unrestrict` satırları atılır: istemci zaten kısıtlı
            // kiptedir ve bu satırlar orada hata verir (katı kipte yüklemeyi durdururdu).
            Engine::Postgresql => unprivileged("sed", &["-E", "-e", r"/^\\(un)?restrict /d"]),
        }
    }

    async fn close(self) {
        let outcome = match self.engine {
            Engine::Mariadb => mariadb::lock_import_account(&self.role).await,
            Engine::Postgresql => match postgres::revoke(&self.database, &self.role).await {
                Ok(()) => postgres::drop_user(&self.role).await,
                Err(err) => Err(err),
            },
        };
        if let Err(err) = outcome {
            tracing::warn!(database = %self.database, error = %err, "içe aktarma hesabı kapatılamadı");
        }
    }
}

/// `source`tan okunan dökümü veritabanına yükler. İstemcinin hata satırları görev günlüğüne yazılır.
pub async fn run(ctx: &JobCtx, engine: Engine, database: &str, source: Stdio) -> AppResult<()> {
    load(ctx, engine, database, source, false).await
}

/// `run` gibi, ama PostgreSQL'de ilk hatada durur ve hatayla sonlanır. Kopyalama gibi "ya hep ya
/// hiç" işlerde yarım yüklenmiş veritabanı sessizce kalmasın diye kullanılır (MariaDB istemcisi zaten
/// ilk hatada durur).
pub async fn run_strict(ctx: &JobCtx, engine: Engine, database: &str, source: Stdio) -> AppResult<()> {
    load(ctx, engine, database, source, true).await
}

async fn load(ctx: &JobCtx, engine: Engine, database: &str, source: Stdio, strict: bool) -> AppResult<()> {
    let session = Session::open(engine, database, strict).await?;
    let outcome = pipe(ctx, &session, source).await;
    session.close().await;
    outcome
}

async fn pipe(ctx: &JobCtx, session: &Session, source: Stdio) -> AppResult<()> {
    let mut filter = session
        .filter()
        .stdin(source)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| AppError::internal(format!("döküm süzgeci başlatılamadı: {err}")))?;
    let filtered: Stdio = filter.stdout.take().expect("stdout borusu").try_into().map_err(|err: std::io::Error| AppError::internal(err.to_string()))?;
    let mut client = session
        .client()
        .stdin(filtered)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| AppError::internal(format!("veritabanı istemcisi başlatılamadı: {err}")))?;

    let stderr = client.stderr.take().expect("stderr borusu");
    let log = ctx.clone();
    let secret = session.password.clone();
    let reader = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut count = 0;
        while let Ok(Some(line)) = lines.next_line().await {
            if count < MAX_LOG_LINES {
                // İstemci bağlantı hatasında parolayı yazmaz; yine de günlüğe geçmediğinden emin olunur.
                log.log(line.replace(&secret, "••••"));
            }
            count += 1;
        }
    });
    let status = client.wait().await?;
    let _ = filter.wait().await;
    let _ = reader.await;
    if status.success() { Ok(()) } else { Err(AppError::bad("İçe aktarma hatayla sonlandı")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_name_is_stable_and_safe() {
        let name = mariadb_account("musteri_blog");
        assert_eq!(name, mariadb_account("musteri_blog"));
        assert_ne!(name, mariadb_account("musteri_blog2"));
        assert_eq!(name.len(), 28);
        assert!(name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'));
    }
}
