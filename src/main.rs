#![windows_subsystem = "windows"]

mod device;
mod import;
mod journal;
mod worker;

use eframe::egui;
use egui::{Color32, FontFamily, RichText};
use import::{Progress, Report, Stop};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;
use worker::{Cmd, Msg, Phone};

const STORE_URL: &str = "ms-windows-store://pdp/?productid=9NP83LWLPZ9K";

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([520.0, 580.0]).with_min_inner_size([420.0, 460.0]),
        ..Default::default()
    };
    eframe::run_native("iPhone Importer", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

struct App {
    dest: PathBuf,
    cmds: mpsc::Sender<Cmd>,
    msgs: mpsc::Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    phone: Phone,
    importing: Option<(Progress, Instant)>,
    /// Текст «скорость · осталось» и когда он посчитан: обновляется раз в секунду, иначе цифры мельтешат.
    rate: (Instant, String),
    done: Option<(Report, PathBuf)>,
    error: Option<String>,
    confirm_all: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_style(&cc.egui_ctx);
        let dest = load_dest();
        let cancel = Arc::new(AtomicBool::new(false));
        let (cmds, msgs) = worker::spawn(dest.clone(), cc.egui_ctx.clone(), cancel.clone());
        App {
            dest,
            cmds,
            msgs,
            cancel,
            phone: Phone::NoDevice,
            importing: None,
            rate: (Instant::now(), String::new()),
            done: None,
            error: None,
            confirm_all: false,
        }
    }

    fn drain(&mut self) {
        while let Ok(msg) = self.msgs.try_recv() {
            match msg {
                // Во время импорта поток шлёт только Progress/Done; состояние телефона значит, что импорт не идёт.
                Msg::Phone(phone) => {
                    if matches!(phone, Phone::Ready { .. }) {
                        self.error = None;
                    }
                    self.importing = None;
                    self.phone = phone;
                }
                Msg::Progress(p) => {
                    let started = self.importing.as_ref().map_or_else(Instant::now, |(_, t)| *t);
                    self.importing = Some((p, started));
                }
                Msg::Done { report, day } => {
                    self.importing = None;
                    self.done = Some((report, day));
                }
                Msg::Error(e) => {
                    self.importing = None;
                    self.error = Some(e);
                }
            }
        }
    }

    fn start(&mut self, all: bool) {
        self.done = None;
        self.error = None;
        self.confirm_all = false;
        self.importing = Some((Progress::default(), Instant::now()));
        self.rate.1.clear();
        self.cancel.store(false, Ordering::Relaxed);
        let _ = self.cmds.send(Cmd::Import { all });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain();
        let panel = egui::Frame::new().fill(BG).inner_margin(24);
        egui::CentralPanel::default().frame(panel).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 12.0;
            let busy = self.importing.is_some();

            ui.label(semibold("iPhone Importer", 26.0));
            ui.add_space(4.0);

            card(ui, |ui| {
                let (dot, title, detail): (Color32, &str, String) = match &self.phone {
                    Phone::NoUsbmuxd => (RED, "Нет приложения Apple Devices", "Без него iPhone не виден.".into()),
                    Phone::NoDevice => (GRAY, "iPhone не подключён", "Подключите его кабелем USB.".into()),
                    Phone::NotTrusted => (
                        ORANGE,
                        "Нужно доверие",
                        "Разблокируйте iPhone и нажмите «Доверять». Если запроса нет — откройте Apple Devices.".into(),
                    ),
                    Phone::Scanning => (BLUE, "Поиск новых файлов…", String::new()),
                    Phone::Ready { udid, .. } => (GREEN, "iPhone подключён", udid.clone()),
                };
                ui.horizontal(|ui| {
                    if matches!(self.phone, Phone::Scanning) {
                        ui.spinner();
                    } else {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 22.0), egui::Sense::hover());
                        ui.painter().circle_filled(r.center(), 5.0, dot);
                    }
                    ui.label(semibold(title, 16.0));
                });
                if !detail.is_empty() {
                    ui.label(secondary(detail));
                }
                if matches!(self.phone, Phone::NoUsbmuxd) && ui.add(primary("Открыть в Microsoft Store")).clicked() {
                    let _ = std::process::Command::new("explorer").arg(STORE_URL).spawn();
                }
                if let Phone::Ready { new, .. } = &self.phone {
                    ui.separator();
                    if new.photos + new.videos == 0 {
                        ui.label(secondary("Всё уже импортировано."));
                    } else {
                        ui.columns(3, |c| {
                            stat(&mut c[0], new.photos.to_string(), "новых фото");
                            stat(&mut c[1], new.videos.to_string(), "новых видео");
                            stat(&mut c[2], size(new.bytes), "объём");
                        });
                    }
                }
            });

            if let Some((p, started)) = &self.importing {
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(semibold("Импорт", 16.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(secondary(format!("{} из {}", p.files_done, p.files_total)));
                        });
                    });
                    let frac = if p.bytes_total == 0 { 0.0 } else { p.bytes_done as f32 / p.bytes_total as f32 };
                    ui.add(egui::ProgressBar::new(frac).desired_height(6.0).fill(BLUE).corner_radius(3));
                    let (at, rate) = &mut self.rate;
                    if rate.is_empty() || at.elapsed().as_secs() >= 1 {
                        let secs = started.elapsed().as_secs_f64().max(0.001);
                        let speed = p.bytes_done as f64 / secs;
                        let eta = if speed > 0.0 { p.bytes_total.saturating_sub(p.bytes_done) as f64 / speed } else { 0.0 };
                        *rate = format!("{:.1} МБ/с · осталось ~{}:{:02}", speed / 1e6, eta as u64 / 60, eta as u64 % 60);
                        *at = Instant::now();
                    }
                    ui.label(secondary(format!("{:.0}% · {rate}", frac * 100.0)));
                    ui.add(egui::Label::new(secondary(&p.current)).truncate());
                    if ui.button("Отменить").clicked() {
                        self.cancel.store(true, Ordering::Relaxed);
                    }
                });
            }

            if let Some((r, day)) = &self.done {
                card(ui, |ui| {
                    let (color, head) = match &r.stopped {
                        None => (GREEN, format!("Импортировано {} файлов", r.imported)),
                        Some(Stop::Cancelled) => (ORANGE, format!("Отменено. Импортировано {} из {}.", r.imported, r.total)),
                        Some(Stop::ConnectionLost(_)) => (
                            ORANGE,
                            format!(
                                "Соединение потеряно. Импортировано {} из {} — подключите iPhone и нажмите «Импортировать», продолжится с места остановки.",
                                r.imported, r.total
                            ),
                        ),
                        Some(Stop::Disk(e)) => (RED, format!("Ошибка записи на диск: {e}. Импортировано {} из {}.", r.imported, r.total)),
                    };
                    ui.label(semibold(head, 15.0).color(color));
                    ui.label(secondary(day.display().to_string()));
                    if !r.failed.is_empty() {
                        ui.label(RichText::new(format!("Не удалось: {} файлов", r.failed.len())).color(ORANGE));
                        egui::ScrollArea::vertical().max_height(80.0).show(ui, |ui| {
                            for (path, e) in &r.failed {
                                ui.label(secondary(format!("{path}: {e}")));
                            }
                        });
                    }
                    if ui.button("Открыть папку").clicked() {
                        let _ = std::process::Command::new("explorer").arg(day).spawn();
                    }
                });
            }

            if let Some(e) = &self.error {
                card(ui, |ui| {
                    ui.label(RichText::new(e).color(RED));
                });
            }

            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(secondary("Сохранять в"));
                        ui.add(egui::Label::new(self.dest.display().to_string()).truncate());
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(!busy, egui::Button::new("Изменить…")).clicked()
                            && let Some(dir) = rfd::FileDialog::new().set_directory(&self.dest).pick_folder()
                        {
                            save_dest(&dir);
                            self.dest = dir.clone();
                            let _ = self.cmds.send(Cmd::SetDest(dir));
                        }
                    });
                });
            });

            if let Phone::Ready { new, all, .. } = &self.phone {
                let (n, all) = (new.photos + new.videos, *all);
                if self.confirm_all && !busy {
                    card(ui, |ui| {
                        ui.label(format!("Скопировать все {} файлов ({}) ещё раз?", all.photos + all.videos, size(all.bytes)));
                        ui.horizontal(|ui| {
                            if ui.add(primary("Скопировать")).clicked() {
                                self.start(true);
                            }
                            if ui.button("Отмена").clicked() {
                                self.confirm_all = false;
                            }
                        });
                    });
                } else {
                    let label = if n == 0 { "Импортировать".to_string() } else { format!("Импортировать {n} файлов") };
                    let button = primary(label).min_size(egui::vec2(ui.available_width(), 38.0));
                    if ui.add_enabled(!busy && n > 0, button).clicked() {
                        self.start(false);
                    }
                    ui.vertical_centered(|ui| {
                        let link = egui::Button::new(RichText::new("Скопировать всё заново…").color(BLUE)).frame(false);
                        if ui.add_enabled(!busy, link).clicked() {
                            self.confirm_all = true;
                        }
                    });
                }
            }
        });
    }
}

// Палитра в духе Apple (светлая тема macOS).
const BG: Color32 = Color32::from_rgb(0xF5, 0xF5, 0xF7);
const TEXT: Color32 = Color32::from_rgb(0x1D, 0x1D, 0x1F);
const GRAY: Color32 = Color32::from_rgb(0x8E, 0x8E, 0x93);
const BLUE: Color32 = Color32::from_rgb(0x00, 0x7A, 0xFF);
const GREEN: Color32 = Color32::from_rgb(0x34, 0xC7, 0x59);
const ORANGE: Color32 = Color32::from_rgb(0xFF, 0x95, 0x00);
const RED: Color32 = Color32::from_rgb(0xFF, 0x3B, 0x30);
const SEMIBOLD: &str = "semibold";

/// Шрифт Segoe UI из Windows (если есть) и светлая тема со скруглёнными элементами.
fn setup_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let dir = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts");
    if let Ok(bytes) = std::fs::read(dir.join("segoeui.ttf")) {
        fonts.font_data.insert("segoe".into(), Arc::new(egui::FontData::from_owned(bytes)));
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "segoe".into());
    }
    let mut semibold = fonts.families[&FontFamily::Proportional].clone();
    if let Ok(bytes) = std::fs::read(dir.join("seguisb.ttf")) {
        fonts.font_data.insert("segoe-sb".into(), Arc::new(egui::FontData::from_owned(bytes)));
        semibold.insert(0, "segoe-sb".into());
    }
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), semibold);
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::Theme::Light);
    ctx.style_mut_of(egui::Theme::Light, |s| {
        use egui::TextStyle::*;
        for (style, size) in [(Body, 14.0), (Button, 14.0), (Small, 12.0), (Heading, 22.0)] {
            s.text_styles.insert(style, egui::FontId::proportional(size));
        }
        s.spacing.button_padding = egui::vec2(14.0, 6.0);
        let v = &mut s.visuals;
        v.panel_fill = BG;
        v.extreme_bg_color = Color32::from_rgb(0xE5, 0xE5, 0xEA);
        v.selection.bg_fill = BLUE;
        v.hyperlink_color = BLUE;
        v.widgets.noninteractive.fg_stroke.color = TEXT;
        v.widgets.noninteractive.bg_stroke.color = Color32::from_rgb(0xE5, 0xE5, 0xEA);
        for (w, fill) in [
            (&mut v.widgets.inactive, Color32::from_rgb(0xE9, 0xE9, 0xEB)),
            (&mut v.widgets.hovered, Color32::from_rgb(0xDE, 0xDE, 0xE1)),
            (&mut v.widgets.active, Color32::from_rgb(0xD1, 0xD1, 0xD6)),
        ] {
            w.weak_bg_fill = fill;
            w.bg_fill = fill;
            w.bg_stroke = egui::Stroke::NONE;
            w.fg_stroke.color = TEXT;
            w.corner_radius = 8.into();
            w.expansion = 0.0;
        }
    });
}

/// Белая карточка со скруглением и мягкой тенью.
fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(Color32::WHITE)
        .corner_radius(14)
        .inner_margin(16)
        .shadow(egui::Shadow { offset: [0, 1], blur: 8, spread: 0, color: Color32::from_black_alpha(20) })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            add(ui);
        });
}

fn semibold(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).family(FontFamily::Name(SEMIBOLD.into())).size(size).color(TEXT)
}

fn secondary(text: impl Into<String>) -> RichText {
    RichText::new(text).color(GRAY)
}

/// Синяя кнопка основного действия.
fn primary<'a>(text: impl Into<String>) -> egui::Button<'a> {
    egui::Button::new(RichText::new(text).family(FontFamily::Name(SEMIBOLD.into())).color(Color32::WHITE)).fill(BLUE)
}

fn stat(ui: &mut egui::Ui, value: String, caption: &str) {
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.label(semibold(value, 24.0));
        ui.label(secondary(caption).small());
    });
}

fn size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} ГБ", bytes as f64 / 1e9)
    } else {
        format!("{:.0} МБ", bytes as f64 / 1e6)
    }
}

fn config_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("iphone-importer").join("config.txt"))
}

fn load_dest() -> PathBuf {
    config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
            home.join("Pictures").join("iPhone")
        })
}

fn save_dest(dest: &std::path::Path) {
    if let Some(p) = config_path() {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, dest.to_string_lossy().as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> (App, mpsc::Sender<Msg>, mpsc::Receiver<Cmd>) {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (msg_tx, msg_rx) = mpsc::channel();
        let app = App {
            dest: PathBuf::new(),
            cmds: cmd_tx,
            msgs: msg_rx,
            cancel: Arc::new(AtomicBool::new(false)),
            phone: Phone::NoDevice,
            importing: None,
            rate: (Instant::now(), String::new()),
            done: None,
            error: None,
            confirm_all: false,
        };
        (app, msg_tx, cmd_rx)
    }

    #[test]
    fn import_click_while_phone_vanishes_does_not_stick_busy() {
        let (mut app, msgs, _cmds) = app();
        app.start(false);
        msgs.send(Msg::Phone(Phone::NoDevice)).unwrap();
        app.drain();
        assert!(app.importing.is_none());
    }

    #[test]
    fn start_resets_cancel_before_sending_import() {
        let (mut app, _msgs, cmds) = app();
        app.cancel.store(true, Ordering::Relaxed);
        app.start(false);
        assert!(!app.cancel.load(Ordering::Relaxed));
        assert!(matches!(cmds.try_recv(), Ok(Cmd::Import { all: false })));
    }
}
