//! PostgreSQL yönetimi. Komutlar `postgres` sistem kullanıcısı olarak,
//! yerel soket üzerinden (peer kimlik doğrulaması) çalıştırılır.
//!
//! Değerler SQL metnine doğrudan yazılmaz; psql değişkeni olarak verilir ve
//! `:"ad"` (tanımlayıcı) / `:'değer'` (metin) biçimleriyle psql tarafından
//! tırnaklanır.
//!
//! Sahiplik modeli: her veritabanının `pixpanel_own_<ad>` adında, giriş
//! yapamayan bir sahip rolü vardır. "Tam yetkili" kullanıcılar bu rolün
//! üyesidir ve o veritabanına bağlandıklarında kendiliğinden bu role
//! bürünürler; böylece hangisi tablo oluşturursa oluştursun nesnelerin sahibi
//! ortak roldür ve tam yetkili kullanıcıların hepsi onları yönetebilir.

use tokio::process::Command;

use crate::dbms::stats::{parse_rows, Stats};
use crate::dbms::{run_client, Access};
use crate::error::AppResult;
use crate::sys::exec::base_command;

fn owner_role(database: &str) -> String {
    format!("pixpanel_own_{database}")
}

/// `postgres` kullanıcısı olarak çalışan bir komut hazırlar.
fn as_postgres(program: &str) -> Command {
    let mut command = base_command("runuser");
    command.args(["-u", "postgres", "--", program]);
    // Çalışma dizini postgres kullanıcısının okuyamadığı bir yer olmasın.
    command.current_dir("/");
    command
}

/// Betiği verilen veritabanında çalıştırır.
///
/// Değişkenler komut satırı argümanı olarak DEĞİL ortam değişkeni olarak
/// geçirilir ve betiğin başında `\getenv` ile okunur: argümanlar süreç
/// listesinde herkese görünür, ortam değişkenleri yalnızca sürecin sahibine.
async fn run(database: &str, script: &str, vars: &[(&str, &str)]) -> AppResult<String> {
    let mut command = as_postgres("psql");
    command.args(["-X", "-q", "-At", "-F", "\t", "-v", "ON_ERROR_STOP=1", "-d", database, "-f", "-"]);
    let mut preamble = String::new();
    for (name, value) in vars {
        let env_name = format!("PIXPANEL_PSQL_{}", name.to_ascii_uppercase());
        preamble.push_str(&format!("\\getenv {name} {env_name}\n"));
        command.env(env_name, value);
    }
    run_client(command, &format!("{preamble}{script}"), "PostgreSQL").await
}

/// Yönetici kimliğiyle `postgres` veritabanında sabit bir betik çalıştırıp çıktısını döner
/// (yalnızca panelin kendi betikleri için; kullanıcı metni betiğe girmez).
pub async fn query(script: &str) -> AppResult<String> {
    run("postgres", script, &[]).await
}

/// Veritabanındaki tablo sayısını veren sorgu. Müşterinin veritabanına yönetici rolüyle
/// bağlanıldığından arama yolu `pg_catalog`a sabitlenir ve her nesne adı nitelenir; müşterinin
/// açtığı bir şema, işlev ya da geçici tablo sorgunun anlamını değiştiremez.
const TABLE_COUNT_SCRIPT: &str = "SET search_path = pg_catalog;\n\
    SELECT pg_catalog.count(*) FROM pg_catalog.pg_class c\n\
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace\n\
    WHERE c.relkind IN ('r', 'p') AND NOT c.relispartition\n\
      AND n.nspname <> 'information_schema' AND n.nspname !~ '^pg_';\n";

/// Bütün veritabanlarının boyutu (tek sorgu) ve tablo sayısı (veritabanı başına bir bağlantı).
pub async fn stats() -> AppResult<Stats> {
    let output = run(
        "postgres",
        "SELECT datname, pg_database_size(datname), 0 FROM pg_database WHERE NOT datistemplate AND datallowconn;\n",
        &[],
    )
    .await?;
    let mut stats = parse_rows(&output);
    for (name, stat) in stats.iter_mut() {
        // `-d` değeri `=` içeriyorsa bağlantı dizesi sayılır; yalnızca panelin açabileceği türden adlara bağlanılır.
        if crate::dbms::validate_name(name, "Veritabanı adı").is_err() && name != "postgres" {
            continue;
        }
        if let Ok(count) = run(name, TABLE_COUNT_SCRIPT, &[]).await {
            stat.tables = count.lines().rev().find_map(|line| line.trim().parse().ok()).unwrap_or(0);
        }
    }
    Ok(stats)
}

/// PostgreSQL varsayılan olarak her role `postgres` ve `template1`
/// veritabanlarına bağlanma izni verir. Hosting ortamında bir hesabın
/// kullanıcısı kendi veritabanı dışında hiçbir yere bağlanamamalıdır.
const HARDEN_SCRIPT: &str = "REVOKE CONNECT ON DATABASE postgres FROM PUBLIC;\n\
    REVOKE CONNECT ON DATABASE template1 FROM PUBLIC;\n";

pub async fn create_database(name: &str) -> AppResult<()> {
    let owner = owner_role(name);
    run("postgres", HARDEN_SCRIPT, &[]).await?;
    run(
        "postgres",
        "CREATE ROLE :\"owner\" NOLOGIN;\n\
         CREATE DATABASE :\"db\" OWNER :\"owner\" ENCODING 'UTF8' TEMPLATE template0;\n\
         REVOKE ALL ON DATABASE :\"db\" FROM PUBLIC;\n",
        &[("db", name), ("owner", &owner)],
    )
    .await?;
    Ok(())
}

pub async fn drop_database(name: &str) -> AppResult<()> {
    let owner = owner_role(name);
    run(
        "postgres",
        "DROP DATABASE IF EXISTS :\"db\" WITH (FORCE);\n\
         DROP ROLE IF EXISTS :\"owner\";\n",
        &[("db", name), ("owner", &owner)],
    )
    .await?;
    Ok(())
}

pub async fn create_user(name: &str, password: &str) -> AppResult<()> {
    run("postgres", HARDEN_SCRIPT, &[]).await?;
    run(
        "postgres",
        "CREATE ROLE :\"name\" LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE PASSWORD :'password';\n",
        &[("name", name), ("password", password)],
    )
    .await?;
    Ok(())
}

pub async fn set_password(name: &str, password: &str) -> AppResult<()> {
    run("postgres", "ALTER ROLE :\"name\" PASSWORD :'password';\n", &[("name", name), ("password", password)]).await?;
    Ok(())
}

/// Rolü siler. Çağıran, rolün bütün veritabanı yetkilerini önceden `revoke`
/// ile almış olmalıdır; aksi hâlde PostgreSQL silmeyi reddeder.
pub async fn drop_user(name: &str) -> AppResult<()> {
    run("postgres", "DROP ROLE IF EXISTS :\"name\";\n", &[("name", name)]).await?;
    Ok(())
}

const REVOKE_SCRIPT: &str = "ALTER ROLE :\"user\" IN DATABASE :\"db\" RESET ALL;\n\
    REVOKE :\"owner\" FROM :\"user\";\n\
    ALTER DEFAULT PRIVILEGES FOR ROLE :\"owner\" IN SCHEMA public REVOKE ALL ON TABLES FROM :\"user\";\n\
    ALTER DEFAULT PRIVILEGES FOR ROLE :\"owner\" IN SCHEMA public REVOKE ALL ON SEQUENCES FROM :\"user\";\n\
    REVOKE ALL ON ALL TABLES IN SCHEMA public FROM :\"user\";\n\
    REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM :\"user\";\n\
    REVOKE ALL ON SCHEMA public FROM :\"user\";\n\
    REVOKE ALL ON DATABASE :\"db\" FROM :\"user\";\n";

pub async fn revoke(database: &str, user: &str) -> AppResult<()> {
    let owner = owner_role(database);
    run(database, REVOKE_SCRIPT, &[("db", database), ("user", user), ("owner", &owner)]).await?;
    Ok(())
}

pub async fn grant(database: &str, user: &str, access: Access) -> AppResult<()> {
    let owner = owner_role(database);
    // Düzey değişirken eski yetkiler kalmasın diye önce hepsi alınır.
    let mut script = String::from(REVOKE_SCRIPT);
    script.push_str("GRANT CONNECT ON DATABASE :\"db\" TO :\"user\";\n");
    match access {
        Access::Full => script.push_str(
            "GRANT :\"owner\" TO :\"user\";\n\
             ALTER ROLE :\"user\" IN DATABASE :\"db\" SET role = :'owner';\n",
        ),
        Access::Readwrite | Access::Readonly => {
            let (tables, sequences) = if access == Access::Readwrite {
                ("SELECT, INSERT, UPDATE, DELETE", "USAGE, SELECT, UPDATE")
            } else {
                ("SELECT", "SELECT")
            };
            script.push_str(&format!(
                "GRANT USAGE ON SCHEMA public TO :\"user\";\n\
                 GRANT {tables} ON ALL TABLES IN SCHEMA public TO :\"user\";\n\
                 GRANT {sequences} ON ALL SEQUENCES IN SCHEMA public TO :\"user\";\n\
                 ALTER DEFAULT PRIVILEGES FOR ROLE :\"owner\" IN SCHEMA public GRANT {tables} ON TABLES TO :\"user\";\n\
                 ALTER DEFAULT PRIVILEGES FOR ROLE :\"owner\" IN SCHEMA public GRANT {sequences} ON SEQUENCES TO :\"user\";\n"
            ));
        }
    }
    run(database, &script, &[("db", database), ("user", user), ("owner", &owner)]).await?;
    Ok(())
}

pub fn dump_command(database: &str) -> Command {
    let mut command = as_postgres("pg_dump");
    command.args(["--no-owner", "--no-acl", "--clean", "--if-exists", "--", database]);
    command
}

/// Kopyalama için döküm: hedef veritabanı boş olduğundan `DROP` ifadelerine gerek yoktur.
pub fn copy_dump_command(database: &str) -> Command {
    let mut command = as_postgres("pg_dump");
    command.args(["--no-owner", "--no-acl", "--", database]);
    command
}

/// `VACUUM ANALYZE`: ölü satırları temizler, planlayıcı istatistiklerini yeniler. Ad çağıran
/// tarafından doğrulanmıştır. Tablolar bu sırada sahip rolün yetkisiyle işlenir (PostgreSQL,
/// VACUUM/ANALYZE sırasında dizin ifadelerini tablo sahibi olarak çalıştırır).
pub fn vacuum_command(database: &str) -> Command {
    let mut command = as_postgres("psql");
    command.args(["-X", "-q", "-d", database, "-c", "VACUUM (VERBOSE, ANALYZE)"]);
    command
}

