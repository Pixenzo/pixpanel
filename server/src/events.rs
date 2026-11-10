//! Canlı olay yolu: arayüze anlık bildirilen değişiklikler.
//!
//! Panelin herhangi bir yerinde bir şey değiştiğinde (kaynak oluşturuldu, görev bitti,
//! yeni ölçüm örneği alındı) buraya bir olay bırakılır; açık arayüzler bunu
//! `GET /api/v1/events` akışından alır ve ekranı yoklamadan günceller.
//!
//! Yol süreç genelinde tektir: olayı üreten kod (`auth::audit`, `JobManager`, ölçüm
//! örnekleyicisi) uygulama durumuna erişmeden yayın yapabilsin diye.

use std::sync::OnceLock;

use serde::Serialize;
use tokio::sync::broadcast;

use crate::auth::AuthUser;
use crate::jobs::JobInfo;
use crate::sys::metrics::Sample;

/// Yavaş kalan dinleyici bu kadar olaydan sonra atlar ve arayüze "yeniden eşitle" denir.
const CAPACITY: usize = 512;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Yeni sunucu yükü örneği (yalnızca yöneticilere).
    Metrics { sample: Sample },
    /// Bir görev başladı ya da bitti.
    Job { job: JobInfo },
    /// Bir kaynak değişti; `area` denetim eyleminin ilk parçasıdır (`site`, `mail`, `dns`...).
    Changed { area: String },
}

#[derive(Debug, Clone)]
pub struct Envelope {
    pub event: Event,
    /// Olayın ilgilendirdiği hesap; `None` ise yalnızca yöneticiler görür.
    pub account: Option<i64>,
}

impl Envelope {
    /// Olay bu kullanıcıya gösterilebilir mi? Yönetici her şeyi, diğerleri yalnızca
    /// kendi hesaplarını (alt hesapsa bağlı olduğu hesabı) ilgilendirenleri görür.
    pub fn visible_to(&self, user: &AuthUser) -> bool {
        if user.is_admin() {
            return true;
        }
        match self.account {
            Some(account) => account == user.id || account == user.owner_id(),
            None => false,
        }
    }
}

static BUS: OnceLock<broadcast::Sender<Envelope>> = OnceLock::new();

fn bus() -> &'static broadcast::Sender<Envelope> {
    BUS.get_or_init(|| broadcast::channel(CAPACITY).0)
}

/// Olayı yayınlar. Dinleyen yoksa olay sessizce düşer.
pub fn publish(event: Event, account: Option<i64>) {
    let _ = bus().send(Envelope { event, account });
}

/// Bir kaynağın değiştiğini bildirir; `action` denetim eylemidir (ör. `site.create`).
pub fn changed(action: &str, account: i64) {
    let area = action.split('.').next().unwrap_or(action).to_string();
    publish(Event::Changed { area }, Some(account));
}

pub fn subscribe() -> broadcast::Receiver<Envelope> {
    bus().subscribe()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Role;

    fn user(id: i64, role: Role, parent_id: Option<i64>) -> AuthUser {
        AuthUser {
            id,
            username: format!("u{id}"),
            email: String::new(),
            role,
            parent_id,
            permissions: Vec::new(),
            language: "tr".into(),
            ip: String::new(),
            via_token: false,
            session_id: None,
        }
    }

    fn envelope(account: Option<i64>) -> Envelope {
        Envelope { event: Event::Changed { area: "site".into() }, account }
    }

    #[test]
    fn admin_sees_everything() {
        let admin = user(1, Role::Admin, None);
        assert!(envelope(None).visible_to(&admin));
        assert!(envelope(Some(7)).visible_to(&admin));
    }

    #[test]
    fn customer_sees_only_own_account() {
        let customer = user(5, Role::User, None);
        assert!(envelope(Some(5)).visible_to(&customer));
        assert!(!envelope(Some(6)).visible_to(&customer));
        assert!(!envelope(None).visible_to(&customer));
    }

    #[test]
    fn member_sees_parent_account() {
        let member = user(9, Role::Member, Some(5));
        assert!(envelope(Some(5)).visible_to(&member));
        assert!(envelope(Some(9)).visible_to(&member));
        assert!(!envelope(Some(6)).visible_to(&member));
    }

    #[test]
    fn area_is_first_part_of_action() {
        let mut receiver = subscribe();
        changed("site.create", 3);
        let envelope = receiver.try_recv().expect("olay yayınlanmalı");
        assert_eq!(envelope.account, Some(3));
        match envelope.event {
            Event::Changed { area } => assert_eq!(area, "site"),
            other => panic!("beklenmeyen olay: {other:?}"),
        }
    }
}
