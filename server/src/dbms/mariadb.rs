//! MariaDB yönetimi. Panel root olarak çalıştığından yerel soketten
//! parolasız (unix_socket) bağlanır.
//!
//! Adlar (`validate_name`, `validate_host`) çağıran tarafından doğrulanmış
//! olmalıdır; burada ek olarak SQL'e gömülürken tırnaklanır.

use tokio::process::Command;

use crate::dbms::stats::{parse_rows, Stats};
use crate::dbms::{run_client, Access};
use crate::error::{AppError, AppResult};
use crate::sys::exec::base_command;

/// Oturumun SQL kipi sıfırlanır; böylece ters eğik çizgiyle kaçışlamanın
/// (aşağıdaki `quote`) her kurulumda aynı biçimde çalıştığı kesinleşir.
const PREAMBLE: &str = "SET SESSION sql_mode = '';\n";

fn client() -> Command {
    let mut command = base_command("mariadb");
    command.args(["--batch", "--skip-column-names", "--default-character-set=utf8mb4"]);
    command
}

async fn run(script: &str) -> AppResult<String> {
    run_client(client(), &format!("{PREAMBLE}{script}"), "MariaDB").await
}

/// Metni tek tırnaklı SQL sabitine çevirir.
pub(crate) fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            _ => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// `'kullanıcı'@'adres'` biçiminde hesap adı.
fn account(user: &str, host: &str) -> String {
    format!("{}@{}", quote(user), quote(host))
}

/// Yönetici kimliğiyle verilen SQL betiğini çalıştırıp çıktısını döner (yalnızca panelin kendi
/// sabit betikleri için; kullanıcı metni `quote` ile kaçışlanmadan girmemelidir).
pub async fn query(script: &str) -> AppResult<String> {
    run(script).await
}

/// Bütün veritabanlarının boyutu ve tablo sayısı; tek sorguyla. Görünümler tablo sayılmaz.
pub async fn stats() -> AppResult<Stats> {
    let output = run(
        "SELECT s.schema_name, COALESCE(SUM(t.data_length + t.index_length), 0),
                COUNT(CASE WHEN t.table_type IN ('BASE TABLE', 'SYSTEM VERSIONED') THEN 1 END)
         FROM information_schema.schemata s
         LEFT JOIN information_schema.tables t ON t.table_schema = s.schema_name
         GROUP BY s.schema_name;\n",
    )
    .await?;
    Ok(parse_rows(&output))
}

pub async fn create_database(name: &str) -> AppResult<()> {
    run(&format!("CREATE DATABASE `{name}` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;\n")).await?;
    Ok(())
}

pub async fn drop_database(name: &str) -> AppResult<()> {
    // Veritabanının içe aktarma hesabı da onunla birlikte kalkar.
    let import_account = account(&crate::dbms::import::mariadb_account(name), "localhost");
    run(&format!("DROP DATABASE IF EXISTS `{name}`;\nDROP USER IF EXISTS {import_account};\n")).await?;
    Ok(())
}

/// Veritabanının içe aktarma hesabını (yoksa oluşturup) açar: yeni parola verilir, kilidi kaldırılır
/// ve yalnızca o veritabanına tam yetki tanınır. Hesap genel yetki (FILE, SUPER, GRANT…) taşımaz.
pub async fn open_import_account(database: &str, role: &str, password: &str) -> AppResult<()> {
    let account = account(role, "localhost");
    run(&format!(
        "CREATE USER IF NOT EXISTS {account} IDENTIFIED BY {password};\n\
         ALTER USER {account} IDENTIFIED BY {password} ACCOUNT UNLOCK;\n\
         GRANT ALL PRIVILEGES ON {target} TO {account};\n",
        password = quote(password),
        target = grant_target(database),
    ))
    .await?;
    Ok(())
}

/// İçe aktarma hesabını kilitler. Hesap silinmez: yüklenen görünüm ve yordamların tanımlayanı
/// odur; kilitli olması onların çalışmasını engellemez, yalnızca kimse bu hesapla bağlanamaz.
pub async fn lock_import_account(role: &str) -> AppResult<()> {
    run(&format!("ALTER USER {} ACCOUNT LOCK;\n", account(role, "localhost"))).await?;
    Ok(())
}

pub async fn create_user(name: &str, host: &str, password: &str) -> AppResult<()> {
    run(&format!("CREATE USER {} IDENTIFIED BY {};\n", account(name, host), quote(password))).await?;
    Ok(())
}

pub async fn set_password(name: &str, host: &str, password: &str) -> AppResult<()> {
    run(&format!("ALTER USER {} IDENTIFIED BY {};\n", account(name, host), quote(password))).await?;
    Ok(())
}

pub async fn drop_user(name: &str, host: &str) -> AppResult<()> {
    run(&format!("DROP USER IF EXISTS {};\n", account(name, host))).await?;
    Ok(())
}

fn privileges(access: Access) -> &'static str {
    match access {
        Access::Full => "ALL PRIVILEGES",
        Access::Readwrite => "SELECT, INSERT, UPDATE, DELETE, EXECUTE, CREATE TEMPORARY TABLES, LOCK TABLES, SHOW VIEW",
        Access::Readonly => "SELECT, SHOW VIEW",
    }
}

/// `GRANT ... ON` içinde kullanılacak veritabanı kalıbı.
///
/// MariaDB bu konumdaki adı LIKE kalıbı olarak yorumlar: `_` tek bir
/// karakterin yerini tutar. Kaçışlanmazsa `ali_blog` için verilen yetki
/// `alixblog` adlı (başka bir hesaba ait olabilecek) veritabanını da kapsar.
fn grant_target(database: &str) -> String {
    format!("`{}`.*", database.replace('_', r"\_"))
}

/// Kullanıcının veritabanındaki bütün yetkilerini alır; hiç yetkisi yoksa sessizce geçer.
async fn revoke_all(database: &str, account: &str) -> AppResult<()> {
    let script = format!("REVOKE ALL PRIVILEGES ON {} FROM {account};\n", grant_target(database));
    match run(&script).await {
        // 1141: "There is no such grant defined" — alınacak yetki yok.
        Err(AppError::BadRequest(detail)) if detail.contains("ERROR 1141") => Ok(()),
        other => other.map(|_| ()),
    }
}

pub async fn grant(database: &str, user: &str, host: &str, access: Access) -> AppResult<()> {
    let account = account(user, host);
    // Düzey değişirken eski yetkiler kalmasın diye önce hepsi alınır.
    revoke_all(database, &account).await?;
    run(&format!("GRANT {} ON {} TO {account};\n", privileges(access), grant_target(database))).await?;
    Ok(())
}

pub async fn revoke(database: &str, user: &str, host: &str) -> AppResult<()> {
    revoke_all(database, &account(user, host)).await
}

pub fn dump_command(database: &str) -> Command {
    let mut command = base_command("mariadb-dump");
    command.args(["--single-transaction", "--routines", "--triggers", "--events", "--default-character-set=utf8mb4", "--", database]);
    command
}

/// Veritabanının tablolarını denetleyen/onaran/iyileştiren program. Ad çağıran tarafından
/// `validate_name` ile doğrulanmıştır (küçük harfle başlar, seçenek gibi görünemez).
pub fn maintenance_command(database: &str, flag: &str) -> Command {
    let mut command = base_command("mariadb-check");
    command.args([flag, database]);
    command
}


#[cfg(test)]
mod tests {
    use super::quote;

    #[test]
    fn quote_neutralizes_injection() {
        assert_eq!(quote("basit"), "'basit'");
        assert_eq!(quote("a'b"), "'a\\'b'");
        assert_eq!(quote("a\\"), "'a\\\\'");
        // Tırnağı kapatıp komut eklemeye çalışan parola tek bir sabit olarak kalır.
        assert_eq!(quote("x'; DROP DATABASE mysql; -- "), "'x\\'; DROP DATABASE mysql; -- '");
        assert_eq!(quote("\\'"), "'\\\\\\''");
    }
}
