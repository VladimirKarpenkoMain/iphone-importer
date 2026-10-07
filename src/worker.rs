//! Фоновый поток: держит подключение к iPhone, считает новые, выполняет импорт.
use crate::device::{self, ConnectError, Device};
use crate::import::{self, Progress, RemoteFile, Report, Summary};
use crate::journal::Journal;
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

pub enum Cmd {
    SetDest(PathBuf),
    Import { all: bool },
}

pub enum Msg {
    NoUsbmuxd,
    NoDevice,
    NotTrusted,
    Scanning,
    Ready { udid: String, new: Summary, all: Summary },
    Progress(Progress),
    Done { report: Report, day: PathBuf },
    Error(String),
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
    loop {
        if dev.as_ref().is_some_and(|d| !d.still_connected()) {
            dev = None;
        }
        if dev.is_none() {
            files = None;
            match device::connect() {
                Ok(d) => dev = Some(d),
                Err(ConnectError::NoUsbmuxd) => send(Msg::NoUsbmuxd),
                Err(ConnectError::NoDevice) => send(Msg::NoDevice),
                Err(ConnectError::NotTrusted) => send(Msg::NotTrusted),
                Err(ConnectError::Other(e)) => send(Msg::Error(format!("Не удалось подключиться: {e}"))),
            }
        }
        if let (Some(d), None) = (dev.as_mut(), files.as_ref()) {
            send(Msg::Scanning);
            match d.list() {
                Ok(list) => {
                    send(ready(&d.udid, &list, &dest));
                    files = Some(list);
                }
                Err(e) => {
                    dev = None;
                    send(Msg::Error(format!("Не удалось прочитать список файлов: {e}")));
                }
            }
        }
        match cmds.recv_timeout(Duration::from_secs(2)) {
            Ok(Cmd::SetDest(p)) => {
                dest = p;
                if let (Some(d), Some(list)) = (dev.as_ref(), files.as_ref()) {
                    send(ready(&d.udid, list, &dest));
                }
            }
            Ok(Cmd::Import { all }) => {
                let (Some(d), Some(list)) = (dev.as_mut(), files.as_ref()) else { continue };
                match import_now(d, list, &dest, all, cancel, send) {
                    Ok((report, day)) => {
                        let lost = matches!(report.stopped, Some(import::Stop::ConnectionLost(_)));
                        send(Msg::Done { report, day });
                        if lost {
                            dev = None;
                        } else {
                            send(ready(&d.udid, list, &dest));
                        }
                    }
                    Err(e) => send(Msg::Error(e)),
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn ready(udid: &str, files: &[RemoteFile], dest: &Path) -> Msg {
    match Journal::open(dest) {
        Ok(j) => Msg::Ready {
            udid: udid.to_string(),
            new: import::summarize(&import::select(files, &j, false)),
            all: import::summarize(&import::select(files, &j, true)),
        },
        Err(e) => Msg::Error(format!("Папка {}: {e}", dest.display())),
    }
}

fn import_now(
    dev: &mut Device,
    files: &[RemoteFile],
    dest: &Path,
    all: bool,
    cancel: &AtomicBool,
    send: &dyn Fn(Msg),
) -> Result<(Report, PathBuf), String> {
    let mut journal = Journal::open(dest).map_err(|e| format!("Папка {}: {e}", dest.display()))?;
    let todo = import::select(files, &journal, all);
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
    cancel.store(false, Ordering::Relaxed);
    let report = import::run(&todo, &day, &mut journal, cancel, |f, out| dev.fetch(f, out), |p| {
        send(Msg::Progress(p.clone()))
    });
    Ok((report, day))
}
