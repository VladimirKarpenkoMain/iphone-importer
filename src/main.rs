#![windows_subsystem = "windows"]

mod device;
mod import;
mod journal;
mod worker;

use eframe::egui;
use import::{Progress, Report, Stop};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;
use worker::{Cmd, Msg, Phone};

const STORE_URL: &str = "ms-windows-store://pdp/?productid=9NP83LWLPZ9K";

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([560.0, 380.0]),
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
    done: Option<(Report, PathBuf)>,
    error: Option<String>,
    confirm_all: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
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
        self.cancel.store(false, Ordering::Relaxed);
        let _ = self.cmds.send(Cmd::Import { all });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain();
        egui::CentralPanel::default().show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            let busy = self.importing.is_some();

            match &self.phone {
                Phone::NoUsbmuxd => {
                    ui.colored_label(egui::Color32::RED, "Не найдено приложение Apple Devices — без него iPhone не виден.");
                    if ui.button("Открыть в Microsoft Store").clicked() {
                        let _ = std::process::Command::new("explorer").arg(STORE_URL).spawn();
                    }
                }
                Phone::NoDevice => {
                    ui.label("Подключите iPhone кабелем.");
                }
                Phone::NotTrusted => {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "Разблокируйте iPhone и нажмите «Доверять». Если запроса нет — откройте Apple Devices.",
                    );
                }
                Phone::Scanning => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Считаю новые файлы…");
                    });
                }
                Phone::Ready { udid, .. } => {
                    ui.colored_label(egui::Color32::GREEN, format!("iPhone подключён ({udid})"));
                }
            }

            ui.horizontal(|ui| {
                ui.label(format!("Папка: {}", self.dest.display()));
                if ui.add_enabled(!busy, egui::Button::new("Изменить")).clicked()
                    && let Some(dir) = rfd::FileDialog::new().set_directory(&self.dest).pick_folder()
                {
                    save_dest(&dir);
                    self.dest = dir.clone();
                    let _ = self.cmds.send(Cmd::SetDest(dir));
                }
            });

            if let Phone::Ready { new, all, .. } = &self.phone {
                let (new, all) = (*new, *all);
                let n = new.photos + new.videos;
                if n == 0 {
                    ui.label("Всё уже импортировано.");
                } else {
                    ui.label(format!(
                        "Новых: {} файлов — {} фото, {} видео, {}",
                        n,
                        new.photos,
                        new.videos,
                        size(new.bytes)
                    ));
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(!busy && n > 0, egui::Button::new("Импортировать")).clicked() {
                        self.start(false);
                    }
                    if ui.add_enabled(!busy, egui::Button::new("Всё заново…")).clicked() {
                        self.confirm_all = true;
                    }
                });
                if self.confirm_all && !busy {
                    ui.horizontal(|ui| {
                        ui.label(format!("Скопировать все {} файлов ({}) ещё раз?", all.photos + all.videos, size(all.bytes)));
                        if ui.button("Да").clicked() {
                            self.start(true);
                        }
                        if ui.button("Нет").clicked() {
                            self.confirm_all = false;
                        }
                    });
                }
            }

            if let Some((p, started)) = &self.importing {
                let frac = if p.bytes_total == 0 { 0.0 } else { p.bytes_done as f32 / p.bytes_total as f32 };
                ui.add(egui::ProgressBar::new(frac).show_percentage());
                let secs = started.elapsed().as_secs_f64().max(0.001);
                let speed = p.bytes_done as f64 / secs;
                let eta = if speed > 0.0 { p.bytes_total.saturating_sub(p.bytes_done) as f64 / speed } else { 0.0 };
                ui.label(format!(
                    "{} / {} · {:.1} МБ/с · осталось ~{}:{:02} · {}",
                    p.files_done,
                    p.files_total,
                    speed / 1e6,
                    eta as u64 / 60,
                    eta as u64 % 60,
                    p.current
                ));
                if ui.button("Отмена").clicked() {
                    self.cancel.store(true, Ordering::Relaxed);
                }
            }

            if let Some((r, day)) = &self.done {
                let head = match &r.stopped {
                    None => format!("Импортировано {} файлов в {}", r.imported, day.display()),
                    Some(Stop::Cancelled) => format!("Отменено. Импортировано {} из {}.", r.imported, r.total),
                    Some(Stop::ConnectionLost(_)) => format!(
                        "Соединение потеряно. Импортировано {} из {} — подключите iPhone и нажмите «Импортировать», продолжится с места остановки.",
                        r.imported, r.total
                    ),
                    Some(Stop::Disk(e)) => format!("Ошибка записи на диск: {e}. Импортировано {} из {}.", r.imported, r.total),
                };
                ui.label(head);
                if ui.button("Открыть папку").clicked() {
                    let _ = std::process::Command::new("explorer").arg(day).spawn();
                }
                if !r.failed.is_empty() {
                    ui.colored_label(egui::Color32::YELLOW, format!("Не удалось: {} файлов", r.failed.len()));
                    egui::ScrollArea::vertical().max_height(80.0).show(ui, |ui| {
                        for (path, e) in &r.failed {
                            ui.label(format!("{path}: {e}"));
                        }
                    });
                }
            }

            if let Some(e) = &self.error {
                ui.colored_label(egui::Color32::RED, e);
            }
        });
    }
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
