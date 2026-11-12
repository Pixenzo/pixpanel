//! Bayinin toplam kullanımı: bayinin kendi müşterilerinin disk, trafik, site, veritabanı ve
//! posta kutusu toplamları. Bu değerler yalnızca gösterilir; bayinin kendi paket sınırlarına
//! karşı çubuk olarak çizilir ama oluşturmayı engellemez.

use rusqlite::params;
use serde::Serialize;

use crate::db::Db;
use crate::error::AppResult;

/// Bir bayinin müşterilerinin (rolü `user`, alt hesaplar kaynak sahibi değildir) toplam kullanımı.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CustomerTotals {
    /// Müşteri hesabı sayısı.
    pub accounts: i64,
    pub sites: i64,
    pub databases: i64,
    pub mail_domains: i64,
    pub mailboxes: i64,
    /// Müşterilerin son ölçülen toplam disk kullanımı (bayt; veritabanları dahil).
    pub disk_bytes: i64,
    /// Müşterilerin bu ayki toplam trafiği (bayt).
    pub traffic_bytes: i64,
}

/// Hesap bayiyse müşterilerinin toplamı, değilse `None`.
pub fn for_reseller(db: &Db, account_id: i64) -> AppResult<Option<CustomerTotals>> {
    let is_reseller: bool = db.call(|conn| {
        conn.query_row("SELECT role = 'reseller' FROM users WHERE id = ?1", params![account_id], |row| row.get(0))
            .or_else(|err| if matches!(err, rusqlite::Error::QueryReturnedNoRows) { Ok(false) } else { Err(err) })
    })?;
    if !is_reseller {
        return Ok(None);
    }
    let mut totals = db.call(|conn| {
        let count = |sql: &str| conn.query_row(sql, params![account_id], |row| row.get::<_, i64>(0));
        Ok(CustomerTotals {
            accounts: count("SELECT COUNT(*) FROM users WHERE parent_id = ?1 AND role = 'user'")?,
            sites: count("SELECT COUNT(*) FROM sites s JOIN users u ON u.id = s.user_id WHERE u.parent_id = ?1 AND u.role = 'user'")?,
            databases: count("SELECT COUNT(*) FROM databases d JOIN users u ON u.id = d.user_id WHERE u.parent_id = ?1 AND u.role = 'user'")?,
            mail_domains: count("SELECT COUNT(*) FROM mail_domains d JOIN users u ON u.id = d.user_id WHERE u.parent_id = ?1 AND u.role = 'user'")?,
            mailboxes: count(
                "SELECT COUNT(*) FROM mailboxes m JOIN mail_domains d ON d.id = m.domain_id JOIN users u ON u.id = d.user_id WHERE u.parent_id = ?1 AND u.role = 'user'",
            )?,
            disk_bytes: count(
                "SELECT COALESCE(SUM(a.disk_bytes), 0) FROM account_usage a JOIN users u ON u.id = a.user_id WHERE u.parent_id = ?1 AND u.role = 'user'",
            )?,
            traffic_bytes: 0,
        })
    })?;
    totals.traffic_bytes = crate::stats::customers_month_bytes(db, account_id)?;
    Ok(Some(totals))
}
