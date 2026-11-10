//! MariaDB yönetimi. Panel root olarak çalıştığından yerel soketten
//! parolasız (unix_socket) bağlanır.
//!
//! Adlar (`validate_name`, `validate_host`) çağıran tarafından doğrulanmış
//! olmalıdır; burada ek olarak SQL'e gömülürken tırnaklanır.

use std::collections::HashMap;

use tokio::process::Command;

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
fn quote(value: &str) -> String {
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

pub async fn sizes() -> AppResult<HashMap<String, u64>> {
    let output = run(
        "SELECT s.schema_name, COALESCE(SUM(t.data_length + t.index_length), 0)
         FROM information_schema.schemata s
         LEFT JOIN information_schema.tables t ON t.table_schema = s.schema_name
         GROUP BY s.schema_name;\n",
    )
    .await?;
    Ok(output
        .lines()
        .filter_map(|line| {
            let (name, size) = line.split_once('\t')?;
            Some((name.to_string(), size.trim().parse::<f64>().ok()? as u64))
        })
        .collect())
}

pub async fn create_database(name: &str) -> AppResult<()> {
    run(&format!("CREATE DATABASE `{name}` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;\n")).await?;
    Ok(())
}

pub async fn drop_database(name: &str) -> AppResult<()> {
    run(&format!("DROP DATABASE IF EXISTS `{name}`;\n")).await?;
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

pub fn import_command(database: &str) -> Command {
    let mut command = base_command("mariadb");
    command.args(["--default-character-set=utf8mb4", "--", database]);
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
