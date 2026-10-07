//! Экран галереи: плитки с миниатюрами iOS, выбор файлов, увеличение.
use crate::import::{Item, Kind, RemoteFile, kind_of};
use crate::{BLUE, GRAY, primary, secondary, size};
use eframe::egui::{self, Color32};
use std::collections::{HashMap, HashSet};

/// Сторона плитки и зазор, px.
const TILE: f32 = 120.0;
const GAP: f32 = 6.0;

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

/// Миниатюра плитки: исходный JPEG (для увеличения) и уменьшенная текстура.
pub struct Thumb {
    pub jpeg: Vec<u8>,
    pub tex: egui::TextureHandle,
}

/// JPEG → текстура не больше `max` px по длинной стороне; `None`, если JPEG не декодируется.
pub fn texture(ctx: &egui::Context, name: &str, jpeg: &[u8], max: u32) -> Option<egui::TextureHandle> {
    let img = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg).ok()?;
    let img = if img.width() > max || img.height() > max { img.thumbnail(max, max) } else { img };
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(ctx.load_texture(name, egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()), egui::TextureOptions::LINEAR))
}

/// Что пользователь сделал на экране галереи за кадр.
pub enum Action {
    None,
    Back,
    Import,
}

/// Экран галереи. `request` получает ключи видимых плиток без миниатюры (только когда набор изменился).
pub fn show(
    ui: &mut egui::Ui,
    g: &mut Gallery,
    items: &[Item],
    thumbs: &mut Cache<Thumb>,
    request: &mut dyn FnMut(Vec<String>),
) -> Action {
    let mut action = Action::None;
    let shown = g.shown(items);
    ui.horizontal(|ui| {
        if ui.button("← Назад").clicked() {
            action = Action::Back;
        }
        ui.selectable_value(&mut g.all, false, "Новые");
        ui.selectable_value(&mut g.all, true, "Все");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Снять все").clicked() {
                for &i in &shown {
                    g.selected.remove(items[i].key());
                }
            }
            if ui.button("Выбрать все").clicked() {
                for &i in &shown {
                    g.selected.insert(items[i].key().to_string());
                }
            }
        });
    });
    let (mut count, mut bytes) = (0, 0u64);
    for f in items.iter().filter(|i| g.selected.contains(i.key())).flat_map(|i| &i.files) {
        count += 1;
        bytes += f.size;
    }
    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        let button = primary(format!("Импортировать {count}")).min_size(egui::vec2(ui.available_width(), 38.0));
        if ui.add_enabled(count > 0, button).clicked() {
            action = Action::Import;
        }
        ui.label(secondary(format!("Выбрано {count} · {}", size(bytes))));
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| grid(ui, g, items, &shown, thumbs, request));
    });
    zoom(ui, g, items, &shown, thumbs);
    action
}

fn grid(
    ui: &mut egui::Ui,
    g: &mut Gallery,
    items: &[Item],
    shown: &[usize],
    thumbs: &mut Cache<Thumb>,
    request: &mut dyn FnMut(Vec<String>),
) {
    let cols = (((ui.available_width() + GAP) / (TILE + GAP)).floor() as usize).max(1);
    let rows = shown.len().div_ceil(cols);
    let mut want = vec![];
    ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);
    egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, TILE, rows, |ui, range| {
        for row in range {
            ui.horizontal(|ui| {
                for &i in shown.iter().skip(row * cols).take(cols) {
                    let item = &items[i];
                    let key = item.key();
                    if !thumbs.contains(key) {
                        want.push(key.to_string());
                    }
                    // Двойной клик даёт и два обычных — выбор переключится дважды и не изменится.
                    let resp = tile(ui, item, g.selected.contains(key), thumbs.get(key));
                    if resp.clicked() {
                        toggle(&mut g.selected, key);
                    }
                    if resp.double_clicked() {
                        g.zoom = Some(key.to_string());
                    }
                }
            });
        }
    });
    if !want.is_empty() && want != g.sent {
        request(want.clone());
        g.sent = want;
    }
}

fn tile(ui: &mut egui::Ui, item: &Item, selected: bool, thumb: Option<&Option<Thumb>>) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(TILE, TILE), egui::Sense::click());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 8.0, Color32::from_rgb(0xE5, 0xE5, 0xEA));
    let first = &item.files[0];
    match thumb {
        Some(Some(t)) => {
            let tint = if item.imported { Color32::from_gray(150) } else { Color32::WHITE };
            p.image(t.tex.id(), rect, square_uv(t.tex.size()), tint);
        }
        Some(None) => {
            let ext = first.path.rsplit_once('.').map_or("", |(_, e)| e).to_ascii_uppercase();
            let text = format!("{ext}\n{}", size(first.size));
            p.text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(13.0), GRAY);
        }
        None => {}
    }
    if item.files.len() > 1 {
        badge(&p, rect.left_top() + egui::vec2(6.0, 6.0), egui::Align2::LEFT_TOP, "LIVE");
    } else if kind_of(&first.path) == Some(Kind::Video) {
        badge(&p, rect.left_top() + egui::vec2(6.0, 6.0), egui::Align2::LEFT_TOP, "▶");
    }
    if item.imported {
        badge(&p, rect.left_bottom() + egui::vec2(6.0, -6.0), egui::Align2::LEFT_BOTTOM, "✔ импортировано");
    }
    let c = rect.right_top() + egui::vec2(-14.0, 14.0);
    if selected {
        p.circle_filled(c, 9.0, BLUE);
        p.text(c, egui::Align2::CENTER_CENTER, "✔", egui::FontId::proportional(11.0), Color32::WHITE);
    } else {
        p.circle(c, 9.0, Color32::from_black_alpha(60), egui::Stroke::new(1.5, Color32::WHITE));
    }
    resp
}

/// Надпись на полупрозрачной тёмной подложке.
fn badge(p: &egui::Painter, pos: egui::Pos2, anchor: egui::Align2, text: &str) {
    let galley = p.layout_no_wrap(text.to_string(), egui::FontId::proportional(11.0), Color32::WHITE);
    let r = anchor.anchor_size(pos, galley.size());
    p.rect_filled(r.expand(3.0), 4.0, Color32::from_black_alpha(140));
    p.galley(r.min, galley, Color32::WHITE);
}

/// UV центрального квадрата текстуры `[w, h]`.
fn square_uv([w, h]: [usize; 2]) -> egui::Rect {
    let (w, h) = (w as f32, h as f32);
    let (du, dv) = if w > h { ((1.0 - h / w) / 2.0, 0.0) } else { (0.0, (1.0 - w / h) / 2.0) };
    egui::Rect::from_min_max(egui::pos2(du, dv), egui::pos2(1.0 - du, 1.0 - dv))
}

fn toggle(set: &mut HashSet<String>, key: &str) {
    if !set.remove(key) {
        set.insert(key.to_string());
    }
}

/// Увеличение плитки `g.zoom`: ←/→ — соседняя показанная, Пробел — выбор, Esc/клик вне — закрыть.
fn zoom(ui: &mut egui::Ui, g: &mut Gallery, items: &[Item], shown: &[usize], thumbs: &mut Cache<Thumb>) {
    let Some(key) = g.zoom.clone() else { return };
    let Some(pos) = shown.iter().position(|&i| items[i].key() == key) else {
        g.zoom = None;
        return;
    };
    let item = &items[shown[pos]];
    // Пока миниатюра не пришла — не запоминаем «нет текстуры», пробуем в следующем кадре.
    if g.zoom_tex.as_ref().is_none_or(|(k, _)| *k != key) && thumbs.contains(&key) {
        let tex = match thumbs.get(&key) {
            Some(Some(t)) => texture(ui.ctx(), &format!("zoom:{key}"), &t.jpeg, 480),
            _ => None,
        };
        g.zoom_tex = Some((key.clone(), tex));
    }
    let tex = g.zoom_tex.as_ref().filter(|(k, _)| *k == key).and_then(|(_, t)| t.clone());
    let selected = g.selected.contains(&key);
    let modal = egui::Modal::new(egui::Id::new("zoom")).show(ui.ctx(), |ui| {
        match &tex {
            Some(t) => {
                ui.add(egui::Image::new(t).max_size(egui::vec2(480.0, 480.0)));
            }
            None => {
                ui.add_sized([360.0, 480.0], egui::Label::new(secondary("Нет миниатюры")));
            }
        }
        for f in &item.files {
            ui.label(format!("{} · {}", f.name(), size(f.size)));
        }
        if item.imported {
            ui.label(secondary("Уже импортировано"));
        }
        let state = if selected { "Выбрано" } else { "Не выбрано" };
        ui.label(secondary(format!("{state} — Пробел, ←/→ — соседние, Esc — закрыть")));
    });
    let (left, right, space) = ui.input(|i| {
        (i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight), i.key_pressed(egui::Key::Space))
    });
    if space {
        toggle(&mut g.selected, &key);
    }
    if left && pos > 0 {
        g.zoom = Some(items[shown[pos - 1]].key().to_string());
    } else if right && pos + 1 < shown.len() {
        g.zoom = Some(items[shown[pos + 1]].key().to_string());
    }
    if modal.should_close() {
        g.zoom = None;
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
    fn square_uv_crops_center() {
        let r = square_uv([360, 480]);
        assert_eq!((r.min.x, r.max.x), (0.0, 1.0));
        assert!((r.min.y - 0.125).abs() < 1e-6 && (r.max.y - 0.875).abs() < 1e-6);
        assert_eq!(square_uv([4, 4]), egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)));
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
