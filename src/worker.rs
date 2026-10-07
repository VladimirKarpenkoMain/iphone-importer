//! Фоновый поток: держит подключение к iPhone, собирает плитки, читает миниатюры, выполняет импорт.
use crate::device::{self, ConnectError, Device};
use crate::import::{self, Item, Progress, RemoteFile, Report};
use crate::journal::Journal;
use eframe::egui;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// Как часто проверять, на месте ли телефон.
const POLL: Duration = Duration::from_secs(2);

pub enum Cmd {
    SetDest(PathBuf),
    /// Импортировать эти файлы.
    Import(Vec<RemoteFile>),
    /// Прочитать миниатюры этих плиток (ключи); заменяет прежнюю очередь.
    Thumbs(Vec<String>),
}

/// Что показывать про телефон.
pub enum Phone {
    NoUsbmuxd,
    NoDevice,
    NotTrusted,
    Scanning,
    Ready { udid: String, items: Vec<Item> },
}

pub enum Msg {
    Phone(Phone),
    Progress(Progress),
    Done { report: Report, day: PathBuf },
    Error(String),
    /// Миниатюра плитки `path`; `None` — её нет.
    Thumb { path: String, jpeg: Option<Vec<u8>> },
}

pub fn spawn(dest: PathBuf, ctx: egui::Context, cancel: Arc<AtomicBool>) -> (mpsc::Sender<Cmd>, mpsc::Receiver<Msg>) {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (msg_tx, msg_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let send = |m: Msg| {
            let _ = msg_tx.send(m);
            ctx.request_repaint();
        };
        run(dest, &cmd_rx, &send, &cancel);
    });
    (cmd_tx, msg_rx)
}

fn run(mut dest: PathBuf, cmds: &mpsc::Receiver<Cmd>, send: &dyn Fn(Msg), cancel: &AtomicBool) {
    let mut dev: Option<Device> = None;
    let mut files: Option<Vec<RemoteFile>> = None;
    // Миниатюры видимых плиток; каждый `Cmd::Thumbs` заменяет очередь целиком.
    let mut thumbs: VecDeque<String> = VecDeque::new();
    let mut polled: Option<Instant> = None;
    loop {
        if polled.is_none_or(|t| t.elapsed() >= POLL) {
            polled = Some(Instant::now());
            if dev.as_ref().is_some_and(|d| !d.still_connected()) {
                dev = None;
            }
            if dev.is_none() {
                files = None;
                match device::connect() {
                    Ok(d) => dev = Some(d),
                    Err(ConnectError::NoUsbmuxd) => send(Msg::Phone(Phone::NoUsbmuxd)),
                    Err(ConnectError::NoDevice) => send(Msg::Phone(Phone::NoDevice)),
                    Err(ConnectError::NotTrusted) => send(Msg::Phone(Phone::NotTrusted)),
                    Err(ConnectError::Other(e)) => {
                        send(Msg::Phone(Phone::NoDevice));
                        send(Msg::Error(format!("Не удалось подключиться: {e}")));
                    }
                }
            }
        }
        if let (Some(d), None) = (dev.as_mut(), files.as_ref()) {
            send(Msg::Phone(Phone::Scanning));
            match d.list() {
                Ok(list) => {
                    send(ready(&d.udid, &list, &dest));
                    files = Some(list);
                }
                Err(e) => {
                    dev = None;
                    send(Msg::Phone(Phone::NoDevice));
                    send(Msg::Error(format!("Не удалось прочитать список файлов: {e}")));
                }
            }
        }
        if dev.is_none() {
            thumbs.clear();
        }
        // Пока есть миниатюры в очереди, команды не ждём.
        match cmds.recv_timeout(if thumbs.is_empty() { POLL } else { Duration::ZERO }) {
            Ok(Cmd::SetDest(p)) => {
                dest = p;
                if let (Some(d), Some(list)) = (dev.as_ref(), files.as_ref()) {
                    send(ready(&d.udid, list, &dest));
                }
            }
            Ok(Cmd::Import(todo)) => {
                let (Some(d), Some(_)) = (dev.as_mut(), files.as_ref()) else { continue };
                thumbs.clear();
                match import_now(d, &todo, &dest, cancel, send) {
                    Ok((report, day)) => {
                        let lost = matches!(report.stopped, Some(import::Stop::ConnectionLost(_)));
                        send(Msg::Done { report, day });
                        if lost {
                            dev = None;
                            polled = None;
                        }
                        // Пересчитать заново: на телефоне могли появиться новые снимки.
                        files = None;
                    }
                    Err(e) => send(Msg::Error(e)),
                }
            }
            Ok(Cmd::Thumbs(paths)) => thumbs = paths.into(),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        if let (Some(d), Some(path)) = (dev.as_mut(), thumbs.pop_front()) {
            match d.thumbnail(&path) {
                Ok(jpeg) => send(Msg::Thumb { path, jpeg }),
                // Связь потеряна — переподключиться сразу; после пересканирования GUI запросит миниатюры заново.
                Err(_) => {
                    dev = None;
                    polled = None;
                }
            }
        }
    }
}

fn ready(udid: &str, files: &[RemoteFile], dest: &Path) -> Msg {
    match Journal::open(dest) {
        Ok(j) => Msg::Phone(Phone::Ready { udid: udid.to_string(), items: import::group(files, &j) }),
        Err(e) => Msg::Error(format!("Папка {}: {e}", dest.display())),
    }
}

fn import_now(
    dev: &mut Device,
    todo: &[RemoteFile],
    dest: &Path,
    cancel: &AtomicBool,
    send: &dyn Fn(Msg),
) -> Result<(Report, PathBuf), String> {
    let mut journal = Journal::open(dest).map_err(|e| format!("Папка {}: {e}", dest.display()))?;
    let need: u64 = todo.iter().map(|f| f.size).sum();
    if let Some(free) = import::free_space(dest)
        && free < need
    {
        return Err(format!(
            "Недостаточно места: нужно {:.1} ГБ, свободно {:.1} ГБ",
            need as f64 / 1e9,
            free as f64 / 1e9
        ));
    }
    let day = dest.join(chrono::Local::now().format("%Y-%m-%d").to_string());
    import::clean_parts(&day).map_err(|e| format!("Папка {}: {e}", day.display()))?;
    let report = import::run(todo, &day, &mut journal, cancel, |f, out| dev.fetch(f, out), |p| {
        send(Msg::Progress(p.clone()))
    });
    Ok((report, day))
}
