//! Экран галереи: плитки с миниатюрами iOS, выбор файлов, увеличение.
use crate::import::{Item, RemoteFile};
use eframe::egui;
use std::collections::{HashMap, HashSet};

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

/// Состояние экрана галереи.
pub struct Gallery {
    /// Показывать и импортированные плитки.
    pub all: bool,
    /// Ключи выбранных плиток; выбор сохраняется при смене фильтра.
    pub selected: HashSet<String>,
    /// Ключ плитки, открытой в увеличении.
    pub zoom: Option<String>,
    /// Текстура увеличения и чья она; `None` внутри — миниатюры нет.
    zoom_tex: Option<(String, Option<egui::TextureHandle>)>,
    /// Последний запрос миниатюр — чтобы не слать тот же каждый кадр.
    sent: Vec<String>,
}

impl Gallery {
    /// Выбраны все неимпортированные плитки.
    pub fn new(items: &[Item]) -> Self {
        Gallery {
            all: false,
            selected: items.iter().filter(|i| !i.imported).map(|i| i.key().to_string()).collect(),
            zoom: None,
            zoom_tex: None,
            sent: vec![],
        }
    }

    /// После пересканирования: выбор только среди плиток, которые ещё есть.
    pub fn retain(&mut self, items: &[Item]) {
        let keys: HashSet<&str> = items.iter().map(Item::key).collect();
        self.selected.retain(|k| keys.contains(k.as_str()));
    }

    /// Индексы плиток, видимых при текущем фильтре.
    pub fn shown(&self, items: &[Item]) -> Vec<usize> {
        (0..items.len()).filter(|&i| self.all || !items[i].imported).collect()
    }

    /// Файлы выбранных плиток в порядке плиток (у Live Photo — оба).
    pub fn files(&self, items: &[Item]) -> Vec<RemoteFile> {
        items.iter().filter(|i| self.selected.contains(i.key())).flat_map(|i| i.files.iter().cloned()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::{Item, RemoteFile};

    fn rf(path: &str, size: u64) -> RemoteFile {
        RemoteFile { path: path.into(), size }
    }

    fn items() -> Vec<Item> {
        vec![
            Item { files: vec![rf("/A.HEIC", 5), rf("/A.MOV", 2)], imported: false },
            Item { files: vec![rf("/B.JPG", 3)], imported: true },
            Item { files: vec![rf("/C.MOV", 9)], imported: false },
        ]
    }

    #[test]
    fn gallery_starts_with_new_items_selected_and_shown() {
        let items = items();
        let mut g = Gallery::new(&items);
        assert_eq!(g.files(&items), vec![rf("/A.HEIC", 5), rf("/A.MOV", 2), rf("/C.MOV", 9)]);
        assert_eq!(g.shown(&items), vec![0, 2]);
        g.all = true;
        assert_eq!(g.shown(&items), vec![0, 1, 2]);
    }

    #[test]
    fn gallery_files_follow_item_order_and_include_imported() {
        let items = items();
        let mut g = Gallery::new(&items);
        g.selected = ["/C.MOV", "/B.JPG"].map(String::from).into();
        assert_eq!(g.files(&items), vec![rf("/B.JPG", 3), rf("/C.MOV", 9)]);
    }

    #[test]
    fn gallery_retain_drops_vanished_items() {
        let mut items = items();
        let mut g = Gallery::new(&items);
        items.remove(0);
        g.retain(&items);
        assert_eq!(g.files(&items), vec![rf("/C.MOV", 9)]);
        assert!(!g.selected.contains("/A.HEIC"));
    }

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
