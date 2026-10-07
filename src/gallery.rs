//! Экран галереи: плитки с миниатюрами iOS, выбор файлов, увеличение.
use std::collections::HashMap;

/// Сколько миниатюр держать в памяти (~150 КБ каждая).
pub const CAP: usize = 1500;

/// Миниатюры по ключу плитки; `None` — миниатюры нет. При переполнении вытесняется давно не показанная.
pub struct Cache<V> {
    cap: usize,
    tick: u64,
    map: HashMap<String, (u64, Option<V>)>,
}

impl<V> Cache<V> {
    pub fn new(cap: usize) -> Self {
        Cache { cap, tick: 0, map: HashMap::new() }
    }

    pub fn contains(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    pub fn get(&mut self, key: &str) -> Option<&Option<V>> {
        self.tick += 1;
        let (at, v) = self.map.get_mut(key)?;
        *at = self.tick;
        Some(v)
    }

    pub fn insert(&mut self, key: String, v: Option<V>) {
        self.tick += 1;
        self.map.insert(key, (self.tick, v));
        if self.map.len() > self.cap {
            // ponytail: линейный поиск по ≤1 500 записям на вставку; куча/связный список, если лимит вырастет.
            let oldest = self.map.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                self.map.remove(&k);
            }
        }
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_evicts_least_recently_shown() {
        let mut c = Cache::new(2);
        c.insert("a".into(), Some(()));
        c.insert("b".into(), None);
        assert_eq!(c.get("a"), Some(&Some(())));
        c.insert("c".into(), Some(()));
        assert!(c.contains("a") && !c.contains("b") && c.contains("c"));
        assert_eq!(c.get("x"), None);
        c.clear();
        assert!(!c.contains("a"));
    }
}
