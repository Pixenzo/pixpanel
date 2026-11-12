//! Posta listeleri: `liste@alan` adresine gelen ileti bütün üyelere dağıtılır.
//!
//! Dağıtım, var olan takma ad (alias) altyapısına eklemlenir: her liste, Postfix'in
//! `virtual_alias_maps` eşlemesine "adres → üyeler" satırı olarak yazılır (`mail::sync`). Yani ileti
//! yeniden gönderilmez, Postfix'in kendi genişletmesiyle teslim edilir: zarf gönderen değişmez
//! (geri dönen iletiler asıl gönderene gider), SPF/DKIM bozulmaz.
//!
//! Gönderim kipi (`mode`):
//! - `anyone`: herkes gönderebilir,
//! - `members`: yalnızca üyeler (zarf göndereni üye adresi olmalı),
//! - `admins`: yalnızca yönetici işaretli üyeler.
//!
//! Kısıt, Postfix'in kendi mekanizmasıyla uygulanır (panel kapalıyken de çalışır, kapalı kalmaz):
//! `smtpd_recipient_restrictions` sonuna eklenen `check_recipient_access texthash:list-access`
//! eşlemesi kısıtlı listeler için `pixlist_<kimlik>` adlı bir kısıt sınıfına yönlendirir; sınıf
//! `check_sender_access texthash:lists/<kimlik>.senders, reject` çalıştırır. Üyesiz liste hiçbir
//! iletiyi kabul etmez (`550 5.1.1`). Zarf göndereni 25. portta taklit edilebilir; bu yüzden kısıt,
//! kimlik doğrulamasız bağlantıda "göndereni üye gibi görünen herkesi" durdurmaz (kimlikli
//! gönderimde `reject_sender_login_mismatch` zaten adresi doğrular).
//!
//! `List-Id` / `List-Unsubscribe` başlıkları eklenmez: ileti yeniden yazılmadan genişletildiği için
//! Postfix `header_checks` alıcıya göre başlık ekleyemez.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rusqlite::params;
use serde::Serialize;

use super::{validate_address, MAIL_DIR};
use crate::db::{now, Db};
use crate::error::{AppError, AppResult};
use crate::hosting::atomic_write;
use crate::sys::exec;

pub const MODES: [&str; 3] = ["anyone", "members", "admins"];
/// Bir listenin üye sınırı. Postfix `virtual_alias_expansion_limit` varsayılanı (1000) aşılırsa
/// genişletme kesilir; bu yüzden sınır onunla aynı tutulur.
pub const MAX_MEMBERS: usize = 1000;
/// Toplu yapıştırmada kabul edilen en çok metin (bayt).
pub const MAX_TEXT_BYTES: usize = 256 * 1024;
const CLASS_PREFIX: &str = "pixlist_";
const REJECT_EMPTY: &str = "550 5.1.1 Liste henuz uye icermiyor / This list has no members";

fn access_map_path() -> PathBuf {
    Path::new(MAIL_DIR).join("list-access")
}

fn senders_dir() -> PathBuf {
    Path::new(MAIL_DIR).join("lists")
}

fn senders_path(id: i64) -> PathBuf {
    senders_dir().join(format!("{id}.senders"))
}

/// Postfix'in eşleme adı (`texthash:` + yol).
fn access_map_spec() -> String {
    format!("texthash:{}", access_map_path().display())
}

// ---- Kayıtlar ----

#[derive(Debug, Clone, Serialize)]
pub struct MailList {
    pub id: i64,
    pub domain_id: i64,
    pub local: String,
    pub address: String,
    pub mode: String,
    pub member_count: i64,
    pub admin_count: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ListMember {
    pub id: i64,
    pub address: String,
    pub is_admin: bool,
    pub created_at: i64,
}

/// Yapılandırma üretimi için bir listenin tam hâli.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListData {
    pub id: i64,
    pub address: String,
    pub mode: String,
    /// `(adres, yönetici mi)` — adrese göre sıralı.
    pub members: Vec<(String, bool)>,
}

const LIST_COLUMNS: &str = "l.id, l.domain_id, l.local, d.domain, l.mode, l.created_at,
     (SELECT COUNT(*) FROM mail_list_members m WHERE m.list_id = l.id),
     (SELECT COUNT(*) FROM mail_list_members m WHERE m.list_id = l.id AND m.is_admin = 1)";

fn read_list(row: &rusqlite::Row<'_>) -> rusqlite::Result<MailList> {
    let (local, domain): (String, String) = (row.get(2)?, row.get(3)?);
    Ok(MailList {
        id: row.get(0)?,
        domain_id: row.get(1)?,
        address: format!("{local}@{domain}"),
        local,
        mode: row.get(4)?,
        created_at: row.get(5)?,
        member_count: row.get(6)?,
        admin_count: row.get(7)?,
    })
}

pub fn list_lists(db: &Db, domain_id: i64) -> AppResult<Vec<MailList>> {
    db.call(|conn| {
        let mut stmt = conn.prepare(&format!("SELECT {LIST_COLUMNS} FROM mail_lists l JOIN mail_domains d ON d.id = l.domain_id WHERE l.domain_id = ?1 ORDER BY l.local"))?;
        stmt.query_map(params![domain_id], read_list)?.collect()
    })
}

pub fn load_list(db: &Db, id: i64) -> AppResult<MailList> {
    use rusqlite::OptionalExtension;
    db.call(|conn| {
        conn.query_row(&format!("SELECT {LIST_COLUMNS} FROM mail_lists l JOIN mail_domains d ON d.id = l.domain_id WHERE l.id = ?1"), params![id], read_list).optional()
    })?
    .ok_or(AppError::NotFound)
}

pub fn list_members(db: &Db, list_id: i64) -> AppResult<Vec<ListMember>> {
    db.call(|conn| {
        let mut stmt = conn.prepare("SELECT id, address, is_admin, created_at FROM mail_list_members WHERE list_id = ?1 ORDER BY address")?;
        stmt.query_map(params![list_id], |row| {
            Ok(ListMember { id: row.get(0)?, address: row.get(1)?, is_admin: row.get(2)?, created_at: row.get(3)? })
        })?
        .collect()
    })
}

/// Bütün listeler (yapılandırma üretimi için).
pub fn all_list_data(db: &Db) -> AppResult<Vec<ListData>> {
    let lists: Vec<(i64, String, String)> = db.call(|conn| {
        let mut stmt = conn.prepare("SELECT l.id, l.local || '@' || d.domain, l.mode FROM mail_lists l JOIN mail_domains d ON d.id = l.domain_id ORDER BY d.domain, l.local")?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect()
    })?;
    let members: Vec<(i64, String, bool)> = db.call(|conn| {
        let mut stmt = conn.prepare("SELECT list_id, address, is_admin FROM mail_list_members ORDER BY list_id, address")?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect()
    })?;
    Ok(lists
        .into_iter()
        .map(|(id, address, mode)| ListData {
            id,
            address,
            mode,
            members: members.iter().filter(|(list_id, _, _)| *list_id == id).map(|(_, member, admin)| (member.clone(), *admin)).collect(),
        })
        .collect())
}

// ---- Doğrulama ----

pub fn validate_mode(value: &str) -> AppResult<String> {
    let mode = value.trim();
    if MODES.contains(&mode) { Ok(mode.to_string()) } else { Err(AppError::bad("Gönderim kipi anyone, members ya da admins olmalı")) }
}

/// Toplu yapıştırılan metni adreslere ayırır: satır, boşluk, virgül ya da noktalı virgülle ayrılmış
/// adresler; `Ad Soyad <adres@alan>` biçiminde yalnızca köşeli parantez içi alınır. Biri bile
/// geçersizse hiçbiri kabul edilmez (hata, ilk geçersiz değerleri listeler).
pub fn parse_member_text(text: &str) -> AppResult<Vec<String>> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(AppError::bad("Metin çok büyük"));
    }
    let mut valid: Vec<String> = Vec::new();
    let mut invalid: Vec<String> = Vec::new();
    for line in text.lines() {
        let candidates: Vec<&str> = match (line.rfind('<'), line.rfind('>')) {
            (Some(open), Some(close)) if open < close => vec![&line[open + 1..close]],
            _ => line.split(|c: char| c.is_whitespace() || c == ',' || c == ';').collect(),
        };
        for candidate in candidates {
            let candidate = candidate.trim().trim_matches(|c| matches!(c, '"' | '\'' | '<' | '>'));
            if candidate.is_empty() {
                continue;
            }
            match validate_address(candidate) {
                Ok(address) => {
                    if !valid.contains(&address) {
                        valid.push(address);
                    }
                }
                Err(_) => invalid.push(candidate.chars().take(60).collect()),
            }
        }
    }
    if !invalid.is_empty() {
        let shown: Vec<String> = invalid.iter().take(5).cloned().collect();
        let more = if invalid.len() > 5 { format!(" (+{} adres daha)", invalid.len() - 5) } else { String::new() };
        return Err(AppError::bad(format!("Geçersiz e-posta adresi: {}{more}. Hiçbir üye eklenmedi.", shown.join(", "))));
    }
    if valid.is_empty() {
        return Err(AppError::bad("En az bir e-posta adresi girin"));
    }
    Ok(valid)
}

/// Üyelerin listenin kendi adresi olmaması (döngü) denetlenir.
pub fn ensure_no_loop(list_address: &str, members: &[String]) -> AppResult<()> {
    if members.iter().any(|member| member == list_address) {
        return Err(AppError::bad("Liste kendi adresini üye olarak alamaz"));
    }
    Ok(())
}

// ---- Üye işlemleri ----

/// Üyeleri ekler; var olan üye yönetici işaretini korur (yükseltilebilir, düşürülmez).
/// `(yeni eklenen, zaten üye olan)` döner.
pub fn add_members(db: &Db, list_id: i64, addresses: &[String], is_admin: bool) -> AppResult<(usize, usize)> {
    let existing: BTreeSet<String> = list_members(db, list_id)?.into_iter().map(|member| member.address).collect();
    let fresh = addresses.iter().filter(|address| !existing.contains(*address)).count();
    if existing.len() + fresh > MAX_MEMBERS {
        return Err(AppError::bad(format!("Bir listede en fazla {MAX_MEMBERS} üye olabilir")));
    }
    let created = now();
    db.call(|conn| {
        let tx = conn.transaction()?;
        for address in addresses {
            tx.execute(
                "INSERT INTO mail_list_members (list_id, address, is_admin, created_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(list_id, address) DO UPDATE SET is_admin = MAX(is_admin, excluded.is_admin)",
                params![list_id, address, is_admin, created],
            )?;
        }
        tx.commit()
    })?;
    Ok((fresh, addresses.len() - fresh))
}

pub fn remove_members(db: &Db, list_id: i64, addresses: &[String]) -> AppResult<usize> {
    db.call(|conn| {
        let tx = conn.transaction()?;
        let mut removed = 0;
        for address in addresses {
            removed += tx.execute("DELETE FROM mail_list_members WHERE list_id = ?1 AND address = ?2", params![list_id, address])?;
        }
        tx.commit()?;
        Ok(removed)
    })
}

pub fn set_admin(db: &Db, list_id: i64, addresses: &[String], is_admin: bool) -> AppResult<usize> {
    db.call(|conn| {
        let tx = conn.transaction()?;
        let mut changed = 0;
        for address in addresses {
            changed += tx.execute("UPDATE mail_list_members SET is_admin = ?3 WHERE list_id = ?1 AND address = ?2", params![list_id, address, is_admin])?;
        }
        tx.commit()?;
        Ok(changed)
    })
}

// ---- Yapılandırma üretimi (saf işlevler) ----

/// `virtual_alias_maps` satırı: `liste@alan üye1, üye2`. Üyesiz liste satır üretmez.
pub fn alias_line(list: &ListData) -> Option<String> {
    if list.members.is_empty() {
        return None;
    }
    let members: Vec<&str> = list.members.iter().map(|(address, _)| address.as_str()).collect();
    Some(format!("{} {}", list.address, members.join(", ")))
}

fn class_name(id: i64) -> String {
    format!("{CLASS_PREFIX}{id}")
}

fn is_restricted(list: &ListData) -> bool {
    list.mode != "anyone" && !list.members.is_empty()
}

/// `check_recipient_access` eşlemesi: üyesiz liste reddedilir, kısıtlı liste kendi sınıfına gider.
pub fn access_map(lists: &[ListData]) -> String {
    let mut out = String::from("# PixPanel tarafından üretildi.\n");
    for list in lists {
        if list.members.is_empty() {
            out.push_str(&format!("{} {REJECT_EMPTY}\n", list.address));
        } else if list.mode != "anyone" {
            out.push_str(&format!("{} {}\n", list.address, class_name(list.id)));
        }
    }
    out
}

/// Kısıt sınıfının izin verilen gönderenleri: üyeler (`members`) ya da yöneticiler (`admins`).
pub fn senders_file(list: &ListData) -> String {
    let mut out = String::from("# PixPanel tarafından üretildi.\n");
    for (address, admin) in &list.members {
        if list.mode == "members" || (list.mode == "admins" && *admin) {
            out.push_str(&format!("{address} OK\n"));
        }
    }
    out
}

/// `(sınıf adı, tanım)` çiftleri. Tanım virgülle ayrılır (boşluk, `postconf -P` ve kısıt listesi
/// biçimiyle uyumlu): önce gönderen eşlemesi, eşleşmezse ret.
pub fn restriction_classes(lists: &[ListData]) -> Vec<(String, String)> {
    lists
        .iter()
        .filter(|list| is_restricted(list))
        .map(|list| (class_name(list.id), format!("check_sender_access,texthash:{},reject", senders_path(list.id).display())))
        .collect()
}

/// Kısıt listesine liste eşlemesini ekler (sondaki `permit` varsa onun önüne). Zaten ekliyse aynen döner.
pub fn with_list_check(current: &str, map_spec: &str) -> String {
    if current.contains(map_spec) {
        return current.trim().to_string();
    }
    let mut items: Vec<&str> = current.split(|c: char| c == ',' || c.is_whitespace()).filter(|item| !item.is_empty()).collect();
    let tail = if items.last() == Some(&"permit") { items.pop() } else { None };
    items.extend(["check_recipient_access", map_spec]);
    items.extend(tail);
    items.join(",")
}

// ---- Dosyalar ve Postfix ----

/// Eşleme ve gönderen dosyalarını yazar; artık listesi olmayan dosyalar silinir.
pub fn write_files(lists: &[ListData]) -> AppResult<()> {
    atomic_write(&access_map_path(), access_map(lists).as_bytes(), 0o644)?;
    std::fs::create_dir_all(senders_dir())?;
    let mut keep = BTreeSet::new();
    for list in lists.iter().filter(|list| is_restricted(list)) {
        let path = senders_path(list.id);
        atomic_write(&path, senders_file(list).as_bytes(), 0o644)?;
        keep.insert(path);
    }
    if let Ok(entries) = std::fs::read_dir(senders_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "senders") && !keep.contains(&path) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    Ok(())
}

async fn postconf_value(args: &[&str]) -> String {
    let mut full = vec!["-h"];
    full.extend_from_slice(args);
    exec::run("postconf", &full).await.map(|out| out.stdout.trim().to_string()).unwrap_or_default()
}

/// Postfix'te kısıt sınıflarını ve `check_recipient_access` satırını günceller (`postfix check` ile
/// sınar, hata olursa `main.cf`/`master.cf` eski hâline döner). Postfix'i yeniden yüklemez:
/// çağıran (`mail::sync`) yükler. Hiç liste yoksa ve ek hiç kurulmamışsa dokunmaz.
pub async fn apply_postfix(lists: &[ListData]) -> AppResult<()> {
    let spec = access_map_spec();
    let main_current = postconf_value(&["smtpd_recipient_restrictions"]).await;
    let configured = main_current.contains(&spec);
    if lists.is_empty() && !configured {
        return Ok(());
    }
    let snapshot = super::PostfixSnapshot::take();
    match apply_postfix_inner(lists, &spec, &main_current).await {
        Ok(()) => Ok(()),
        Err(err) => {
            snapshot.restore();
            Err(err)
        }
    }
}

async fn apply_postfix_inner(lists: &[ListData], spec: &str, main_current: &str) -> AppResult<()> {
    let classes = restriction_classes(lists);
    let wanted_names: Vec<&str> = classes.iter().map(|(name, _)| name.as_str()).collect();

    // Sınıf tanımları ve listesi.
    for (name, definition) in &classes {
        let current = postconf_value(&[name]).await;
        if current != *definition {
            exec::run_ok("postconf", &["-e", &format!("{name}={definition}")]).await?;
        }
    }
    let current_classes = postconf_value(&["smtpd_restriction_classes"]).await;
    let mut current_names: Vec<&str> = current_classes.split(|c: char| c == ',' || c.is_whitespace()).filter(|name| !name.is_empty()).collect();
    // Panele ait olmayan sınıflar korunur; yalnızca `pixlist_` önekliler yeniden kurulur.
    let foreign: Vec<&str> = current_names.iter().copied().filter(|name| !name.starts_with(CLASS_PREFIX)).collect();
    let stale: Vec<String> = current_names.iter().copied().filter(|name| name.starts_with(CLASS_PREFIX) && !wanted_names.contains(name)).map(str::to_string).collect();
    let mut all: Vec<&str> = foreign;
    all.extend(wanted_names.iter().copied());
    current_names.sort_unstable();
    let mut sorted_all = all.clone();
    sorted_all.sort_unstable();
    if sorted_all != current_names {
        if all.is_empty() {
            exec::run_ok("postconf", &["-X", "smtpd_restriction_classes"]).await?;
        } else {
            exec::run_ok("postconf", &["-e", &format!("smtpd_restriction_classes={}", all.join(","))]).await?;
        }
    }
    for name in stale {
        exec::run_ok("postconf", &["-X", &name]).await?;
    }

    // Kısıt listelerine liste eşlemesi: (postconf bayrağı, parametre adı, şimdiki değer).
    let mut targets: Vec<(&str, String, String)> = vec![("-e", "smtpd_recipient_restrictions".to_string(), main_current.to_string())];
    for service in ["submission", "submissions"] {
        let defined = exec::run("postconf", &["-M", &format!("{service}/inet")]).await?;
        if defined.success() && !defined.stdout.trim().is_empty() {
            let key = format!("{service}/inet/smtpd_recipient_restrictions");
            let current = postconf_value(&["-P", &key]).await;
            targets.push(("-P", key, current));
        }
    }
    for (flag, key, current) in targets {
        if !current.contains(spec) {
            exec::run_ok("postconf", &[flag, &format!("{key}={}", with_list_check(&current, spec))]).await?;
        }
    }
    exec::run_ok("postfix", &["check"]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(id: i64, address: &str, mode: &str, members: &[(&str, bool)]) -> ListData {
        ListData { id, address: address.into(), mode: mode.into(), members: members.iter().map(|(a, admin)| (a.to_string(), *admin)).collect() }
    }

    #[test]
    fn alias_line_expands_members() {
        let l = list(1, "duyuru@ornek.test", "members", &[("a@x.test", false), ("b@y.test", true)]);
        assert_eq!(alias_line(&l).unwrap(), "duyuru@ornek.test a@x.test, b@y.test");
        assert!(alias_line(&list(2, "bos@ornek.test", "members", &[])).is_none());
    }

    #[test]
    fn access_map_routes_restricted_and_rejects_empty() {
        let lists = [
            list(1, "acik@o.test", "anyone", &[("a@x.test", false)]),
            list(2, "uye@o.test", "members", &[("a@x.test", false)]),
            list(3, "bos@o.test", "members", &[]),
        ];
        let map = access_map(&lists);
        assert!(!map.contains("acik@o.test"));
        assert!(map.contains("uye@o.test pixlist_2\n"));
        assert!(map.contains("bos@o.test 550 5.1.1"));
    }

    #[test]
    fn senders_follow_mode() {
        let members = [("a@x.test", false), ("b@x.test", true)];
        let all = senders_file(&list(1, "l@o.test", "members", &members));
        assert!(all.contains("a@x.test OK") && all.contains("b@x.test OK"));
        let admins = senders_file(&list(1, "l@o.test", "admins", &members));
        assert!(!admins.contains("a@x.test") && admins.contains("b@x.test OK"));
    }

    #[test]
    fn classes_only_for_restricted_nonempty_lists() {
        let lists = [list(5, "a@o.test", "members", &[("m@x.test", false)]), list(6, "b@o.test", "anyone", &[("m@x.test", false)]), list(7, "c@o.test", "admins", &[])];
        let classes = restriction_classes(&lists);
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].0, "pixlist_5");
        assert_eq!(classes[0].1, "check_sender_access,texthash:/etc/pixpanel/mail/lists/5.senders,reject");
    }

    #[test]
    fn list_check_goes_before_final_permit() {
        let spec = "texthash:/etc/pixpanel/mail/list-access";
        assert_eq!(with_list_check("", spec), "check_recipient_access,texthash:/etc/pixpanel/mail/list-access");
        assert_eq!(
            with_list_check("check_policy_service,unix:private/pixpanel-policy,permit", spec),
            "check_policy_service,unix:private/pixpanel-policy,check_recipient_access,texthash:/etc/pixpanel/mail/list-access,permit"
        );
        // Zaten ekliyse değişmez.
        let done = "check_recipient_access,texthash:/etc/pixpanel/mail/list-access";
        assert_eq!(with_list_check(done, spec), done);
        // Boşlukla ayrılmış mevcut liste de virgüllü biçime çevrilir.
        assert_eq!(with_list_check("reject_rbl_client x.test permit", spec), "reject_rbl_client,x.test,check_recipient_access,texthash:/etc/pixpanel/mail/list-access,permit");
    }

    #[test]
    fn member_text_is_parsed_and_validated() {
        let text = "Ali Veli <Ali@Ornek.test>\nb@x.test, c@x.test; b@x.test\n\n  \"d@y.test\"  ";
        assert_eq!(parse_member_text(text).unwrap(), vec!["ali@ornek.test", "b@x.test", "c@x.test", "d@y.test"]);
        let bad = parse_member_text("a@x.test\nsaçma\nb@@x.test").unwrap_err().to_string();
        assert!(bad.contains("saçma") && bad.contains("Hiçbir üye eklenmedi"));
        assert!(parse_member_text("a@x.test\r\nb@x.test\r\n").is_ok());
        assert!(parse_member_text("  \n ").is_err());
        assert!(parse_member_text("a@x.test\"; DROP").is_err());
    }

    #[test]
    fn loop_and_mode_checks() {
        assert!(ensure_no_loop("l@o.test", &["l@o.test".to_string()]).is_err());
        assert!(ensure_no_loop("l@o.test", &["a@o.test".to_string()]).is_ok());
        for mode in MODES {
            assert!(validate_mode(mode).is_ok());
        }
        assert!(validate_mode("everyone").is_err());
    }
}
