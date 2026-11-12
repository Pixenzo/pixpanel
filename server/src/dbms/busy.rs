//! Aynı veritabanında aynı anda yalnızca tek bir uzun işlem (kopyalama, bakım, içe aktarma)
//! yürüsün diye bellek içi kilit. Onarım ve iyileştirme tabloları kilitleyebildiğinden,
//! iki işlemin üst üste binmesi hem sunucuyu yorar hem de sonucu belirsizleştirir.

use std::collections::HashSet;
use std::sync::Mutex;

use super::Engine;

static ACTIVE: Mutex<Option<HashSet<(Engine, String)>>> = Mutex::new(None);

/// Kilidin sahibi; düşünce (iş bitince ya da hata olunca) kilit kalkar.
pub struct Guard {
    key: (Engine, String),
}

/// Veritabanını işgal eder. Başka bir işlem sürüyorsa `None` döner.
pub fn try_lock(engine: Engine, name: &str) -> Option<Guard> {
    let mut active = ACTIVE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let set = active.get_or_insert_with(HashSet::new);
    let key = (engine, name.to_string());
    if set.insert(key.clone()) { Some(Guard { key }) } else { None }
}

/// Veritabanında şu an bir işlem sürüyor mu?
pub fn is_busy(engine: Engine, name: &str) -> bool {
    let active = ACTIVE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    active.as_ref().is_some_and(|set| set.contains(&(engine, name.to_string())))
}

impl Drop for Guard {
    fn drop(&mut self) {
        let mut active = ACTIVE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(set) = active.as_mut() {
            set.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_holder_per_database() {
        let first = try_lock(Engine::Mariadb, "busy_test_db").expect("ilk kilit alınmalı");
        assert!(try_lock(Engine::Mariadb, "busy_test_db").is_none());
        assert!(is_busy(Engine::Mariadb, "busy_test_db"));
        // Aynı ad başka motorda ayrı bir veritabanıdır.
        assert!(try_lock(Engine::Postgresql, "busy_test_db").is_some());
        drop(first);
        assert!(!is_busy(Engine::Mariadb, "busy_test_db"));
        assert!(try_lock(Engine::Mariadb, "busy_test_db").is_some());
    }
}
