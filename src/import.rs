//! Раскладка по папкам, отбор новых и копирование. Про iPhone ничего не знает:
//! источник байтов передаётся замыканием `fetch`.
use crate::journal::Journal;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const PHOTO: &[&str] = &["heic", "heif", "jpg", "jpeg", "png", "dng", "gif", "tiff", "webp"];
const VIDEO: &[&str] = &["mov", "mp4", "m4v"];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Photo,
    Video,
}

impl Kind {
    pub fn folder(self) -> &'static str {
        match self {
            Kind::Photo => "Фото",
            Kind::Video => "Видео",
        }
    }
}

pub fn kind_of(path: &str) -> Option<Kind> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    if PHOTO.contains(&ext.as_str()) {
        Some(Kind::Photo)
    } else if VIDEO.contains(&ext.as_str()) {
        Some(Kind::Video)
    } else {
        None
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteFile {
    pub path: String,
    pub size: u64,
}

impl RemoteFile {
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Summary {
    pub photos: usize,
    pub videos: usize,
    pub bytes: u64,
}

/// Фото и видео из `files`; без `all` — только те, которых нет в журнале.
pub fn select(files: &[RemoteFile], journal: &Journal, all: bool) -> Vec<RemoteFile> {
    files
        .iter()
        .filter(|f| kind_of(&f.path).is_some() && (all || !journal.contains(&f.path, f.size)))
        .cloned()
        .collect()
}

pub fn summarize(files: &[RemoteFile]) -> Summary {
    let mut s = Summary::default();
    for f in files {
        match kind_of(&f.path) {
            Some(Kind::Photo) => s.photos += 1,
            Some(Kind::Video) => s.videos += 1,
            None => continue,
        }
        s.bytes += f.size;
    }
    s
}

/// `dir\name`, а если занято — `dir\stem (n).ext` с наименьшим свободным n.
pub fn free_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) => (stem, format!(".{ext}")),
        None => (name, String::new()),
    };
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap()
}

/// Удаляет недокачанные `*.part` из папок дня.
pub fn clean_parts(day: &Path) -> io::Result<()> {
    for kind in [Kind::Photo, Kind::Video] {
        let entries = match fs::read_dir(day.join(kind.folder())) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "part") {
                fs::remove_file(path)?;
            }
        }
    }
    Ok(())
}

/// Свободное место на томе с `dir` (Win32 GetDiskFreeSpaceExW).
pub fn free_space(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(dir: *const u16, avail: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    }
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    let mut avail = 0u64;
    // SAFETY: `wide` оканчивается нулём; null для ненужных выходов API допускает.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(avail)
}

#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub files_done: usize,
    pub files_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub current: String,
}

#[derive(Debug)]
pub enum FetchError {
    /// Не удалось прочитать этот файл — пропускаем, идём дальше.
    File(String),
    /// Связь с телефоном потеряна — останавливаемся.
    Connection(String),
}

#[derive(Debug, PartialEq)]
pub enum Stop {
    Cancelled,
    ConnectionLost(String),
    Disk(String),
}

#[derive(Debug, Default)]
pub struct Report {
    pub imported: usize,
    pub total: usize,
    pub failed: Vec<(String, String)>,
    pub stopped: Option<Stop>,
}

/// Куда `fetch` пишет байты: файл `.part` + счётчик + прогресс + отмена.
struct Sink<'a, F: FnMut(&Progress)> {
    file: File,
    progress: &'a mut Progress,
    on_progress: &'a mut F,
    cancel: &'a AtomicBool,
    written: u64,
    write_err: Option<String>,
}

impl<F: FnMut(&Progress)> Write for Sink<'_, F> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::other("отменено"));
        }
        let n = self.file.write(buf).inspect_err(|e| self.write_err = Some(e.to_string()))?;
        self.written += n as u64;
        self.progress.bytes_done += n as u64;
        (self.on_progress)(self.progress);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

enum Outcome {
    Done,
    FileFailed(String),
    Stop(Stop),
}

/// Копирует `files` в `day\Фото` / `day\Видео` по одному: `.part` → rename → запись в журнал.
pub fn run(
    files: &[RemoteFile],
    day: &Path,
    journal: &mut Journal,
    cancel: &AtomicBool,
    mut fetch: impl FnMut(&RemoteFile, &mut dyn Write) -> Result<(), FetchError>,
    mut on_progress: impl FnMut(&Progress),
) -> Report {
    let mut report = Report { total: files.len(), ..Default::default() };
    let mut progress = Progress {
        files_total: files.len(),
        bytes_total: files.iter().map(|f| f.size).sum(),
        ..Default::default()
    };
    for f in files {
        let Some(kind) = kind_of(&f.path) else { continue };
        progress.current = f.name().to_string();
        on_progress(&progress);
        let dir = day.join(kind.folder());
        let part = dir.join(format!("{}.part", f.name()));
        let file = match fs::create_dir_all(&dir).and_then(|_| File::create(&part)) {
            Ok(file) => file,
            Err(e) => {
                report.stopped = Some(Stop::Disk(e.to_string()));
                break;
            }
        };
        let bytes_before = progress.bytes_done;
        let mut sink = Sink {
            file,
            progress: &mut progress,
            on_progress: &mut on_progress,
            cancel,
            written: 0,
            write_err: None,
        };
        let fetched = fetch(f, &mut sink);
        let Sink { file, written, write_err, .. } = sink;
        let synced = file.sync_all();
        drop(file);

        let outcome = if cancel.load(Ordering::Relaxed) {
            Outcome::Stop(Stop::Cancelled)
        } else if let Some(e) = write_err {
            Outcome::Stop(Stop::Disk(e))
        } else if let Err(e) = synced {
            Outcome::Stop(Stop::Disk(e.to_string()))
        } else {
            match fetched {
                Err(FetchError::Connection(e)) => Outcome::Stop(Stop::ConnectionLost(e)),
                Err(FetchError::File(e)) => Outcome::FileFailed(e),
                Ok(()) if written != f.size => {
                    Outcome::FileFailed(format!("получено {written} байт из {}", f.size))
                }
                Ok(()) => Outcome::Done,
            }
        };
        match outcome {
            Outcome::Done => {
                let target = free_path(&dir, f.name());
                if let Err(e) = fs::rename(&part, &target).and_then(|_| journal.record(&f.path, f.size)) {
                    let _ = fs::remove_file(&part);
                    report.stopped = Some(Stop::Disk(e.to_string()));
                    break;
                }
                report.imported += 1;
            }
            Outcome::FileFailed(e) => {
                let _ = fs::remove_file(&part);
                report.failed.push((f.path.clone(), e));
                progress.bytes_done = bytes_before + f.size;
            }
            Outcome::Stop(stop) => {
                let _ = fs::remove_file(&part);
                report.stopped = Some(stop);
                break;
            }
        }
        progress.files_done += 1;
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rf(path: &str, size: u64) -> RemoteFile {
        RemoteFile { path: path.to_string(), size }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = match fs::read_dir(dir) {
            Ok(rd) => rd.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect(),
            Err(_) => vec![],
        };
        v.sort();
        v
    }

    #[test]
    fn kind_by_extension_case_insensitive() {
        assert_eq!(kind_of("/DCIM/100APPLE/IMG_1.HEIC"), Some(Kind::Photo));
        assert_eq!(kind_of("/DCIM/100APPLE/IMG_1.jpeg"), Some(Kind::Photo));
        assert_eq!(kind_of("/DCIM/100APPLE/IMG_1.mov"), Some(Kind::Video));
        assert_eq!(kind_of("/DCIM/100APPLE/IMG_1.MP4"), Some(Kind::Video));
        assert_eq!(kind_of("/DCIM/100APPLE/IMG_1.AAE"), None);
        assert_eq!(kind_of("/DCIM/100APPLE/README"), None);
    }

    #[test]
    fn select_skips_journaled_and_non_media() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(dir.path()).unwrap();
        j.record("/DCIM/1/A.HEIC", 5).unwrap();
        let files = [rf("/DCIM/1/A.HEIC", 5), rf("/DCIM/1/B.MOV", 9), rf("/DCIM/1/A.AAE", 1)];

        assert_eq!(select(&files, &j, false), vec![rf("/DCIM/1/B.MOV", 9)]);
        assert_eq!(select(&files, &j, true), vec![rf("/DCIM/1/A.HEIC", 5), rf("/DCIM/1/B.MOV", 9)]);
    }

    #[test]
    fn summarize_counts_kinds_and_bytes() {
        let files = [rf("/a.HEIC", 5), rf("/b.JPG", 6), rf("/c.MOV", 100)];
        assert_eq!(summarize(&files), Summary { photos: 2, videos: 1, bytes: 111 });
    }

    #[test]
    fn free_path_adds_lowest_free_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert_eq!(free_path(d, "IMG.HEIC"), d.join("IMG.HEIC"));
        fs::write(d.join("IMG.HEIC"), "").unwrap();
        assert_eq!(free_path(d, "IMG.HEIC"), d.join("IMG (1).HEIC"));
        fs::write(d.join("IMG (1).HEIC"), "").unwrap();
        assert_eq!(free_path(d, "IMG.HEIC"), d.join("IMG (2).HEIC"));
        fs::write(d.join("NOEXT"), "").unwrap();
        assert_eq!(free_path(d, "NOEXT"), d.join("NOEXT (1)"));
    }

    #[test]
    fn clean_parts_removes_only_part_files() {
        let dir = tempfile::tempdir().unwrap();
        let photos = dir.path().join("Фото");
        fs::create_dir_all(&photos).unwrap();
        fs::write(photos.join("A.HEIC.part"), "x").unwrap();
        fs::write(photos.join("B.HEIC"), "x").unwrap();
        clean_parts(dir.path()).unwrap();
        assert_eq!(names(&photos), vec!["B.HEIC"]);
        clean_parts(&dir.path().join("нет такой")).unwrap();
    }

    #[test]
    fn free_space_reports_something_for_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(free_space(dir.path()).unwrap() > 0);
    }

    fn ok_fetch(f: &RemoteFile, out: &mut dyn Write) -> Result<(), FetchError> {
        out.write_all(&vec![7u8; f.size as usize]).map_err(|e| FetchError::File(e.to_string()))
    }

    #[test]
    fn run_sorts_into_kind_folders_and_journals() {
        let dest = tempfile::tempdir().unwrap();
        let day = dest.path().join("2026-10-07");
        let mut j = Journal::open(dest.path()).unwrap();
        let files = [rf("/DCIM/100APPLE/A.HEIC", 3), rf("/DCIM/100APPLE/B.MOV", 4)];
        let mut last = Progress::default();

        let r = run(&files, &day, &mut j, &AtomicBool::new(false), ok_fetch, |p| last = p.clone());

        assert_eq!((r.imported, r.total, r.stopped), (2, 2, None));
        assert_eq!(names(&day.join("Фото")), vec!["A.HEIC"]);
        assert_eq!(names(&day.join("Видео")), vec!["B.MOV"]);
        assert_eq!(fs::read(day.join("Фото").join("A.HEIC")).unwrap(), vec![7u8; 3]);
        assert_eq!(last.bytes_done, 7);
        let j = Journal::open(dest.path()).unwrap();
        assert!(j.contains("/DCIM/100APPLE/A.HEIC", 3) && j.contains("/DCIM/100APPLE/B.MOV", 4));
    }

    #[test]
    fn run_same_name_from_two_dcim_folders_keeps_both() {
        let dest = tempfile::tempdir().unwrap();
        let day = dest.path().join("d");
        let mut j = Journal::open(dest.path()).unwrap();
        let files = [rf("/DCIM/100APPLE/IMG_0001.HEIC", 1), rf("/DCIM/105APPLE/IMG_0001.HEIC", 2)];

        let r = run(&files, &day, &mut j, &AtomicBool::new(false), ok_fetch, |_| {});

        assert_eq!(r.imported, 2);
        assert_eq!(names(&day.join("Фото")), vec!["IMG_0001 (1).HEIC", "IMG_0001.HEIC"]);
    }

    #[test]
    fn run_file_error_skips_file_and_continues() {
        let dest = tempfile::tempdir().unwrap();
        let day = dest.path().join("d");
        let mut j = Journal::open(dest.path()).unwrap();
        let files = [rf("/1/A.HEIC", 3), rf("/1/B.HEIC", 3)];

        let r = run(&files, &day, &mut j, &AtomicBool::new(false), |f, out| {
            if f.path == "/1/A.HEIC" {
                out.write_all(b"x").unwrap();
                return Err(FetchError::File("нет доступа".into()));
            }
            ok_fetch(f, out)
        }, |_| {});

        assert_eq!(r.imported, 1);
        assert_eq!(r.failed, vec![("/1/A.HEIC".to_string(), "нет доступа".to_string())]);
        assert_eq!(names(&day.join("Фото")), vec!["B.HEIC"]);
        assert!(!j.contains("/1/A.HEIC", 3));
    }

    #[test]
    fn run_size_mismatch_is_file_failure() {
        let dest = tempfile::tempdir().unwrap();
        let day = dest.path().join("d");
        let mut j = Journal::open(dest.path()).unwrap();
        let files = [rf("/1/A.HEIC", 10)];

        let r = run(&files, &day, &mut j, &AtomicBool::new(false), |_, out| {
            out.write_all(b"short").map_err(|e| FetchError::File(e.to_string()))
        }, |_| {});

        assert_eq!((r.imported, r.failed.len()), (0, 1));
        assert!(names(&day.join("Фото")).is_empty());
        assert!(!j.contains("/1/A.HEIC", 10));
    }

    #[test]
    fn run_connection_lost_stops_and_leaves_no_part() {
        let dest = tempfile::tempdir().unwrap();
        let day = dest.path().join("d");
        let mut j = Journal::open(dest.path()).unwrap();
        let files = [rf("/1/A.HEIC", 3), rf("/1/B.HEIC", 3), rf("/1/C.HEIC", 3)];
        let mut calls = 0;

        let r = run(&files, &day, &mut j, &AtomicBool::new(false), |f, out| {
            calls += 1;
            if f.path == "/1/B.HEIC" {
                out.write_all(b"x").unwrap();
                return Err(FetchError::Connection("кабель".into()));
            }
            ok_fetch(f, out)
        }, |_| {});

        assert_eq!(r.imported, 1);
        assert_eq!(r.stopped, Some(Stop::ConnectionLost("кабель".into())));
        assert_eq!(calls, 2);
        assert_eq!(names(&day.join("Фото")), vec!["A.HEIC"]);
        let j = Journal::open(dest.path()).unwrap();
        assert!(j.contains("/1/A.HEIC", 3) && !j.contains("/1/B.HEIC", 3));
    }

    #[test]
    fn run_cancel_mid_file_stops_and_leaves_no_part() {
        let dest = tempfile::tempdir().unwrap();
        let day = dest.path().join("d");
        let mut j = Journal::open(dest.path()).unwrap();
        let cancel = AtomicBool::new(false);
        let files = [rf("/1/A.MOV", 3), rf("/1/B.MOV", 3)];

        let r = run(&files, &day, &mut j, &cancel, |_, out| {
            out.write_all(b"x").unwrap();
            cancel.store(true, Ordering::Relaxed);
            out.write_all(b"yz").map_err(|e| FetchError::File(e.to_string()))
        }, |_| {});

        assert_eq!((r.imported, r.stopped), (0, Some(Stop::Cancelled)));
        assert!(names(&day.join("Видео")).is_empty());
    }

    #[test]
    fn run_unwritable_day_dir_is_disk_stop() {
        let dest = tempfile::tempdir().unwrap();
        let day = dest.path().join("d");
        fs::write(&day, "это файл, а не папка").unwrap();
        let mut j = Journal::open(dest.path()).unwrap();

        let r = run(&[rf("/1/A.HEIC", 1)], &day, &mut j, &AtomicBool::new(false), ok_fetch, |_| {});

        assert!(matches!(r.stopped, Some(Stop::Disk(_))));
        assert_eq!(r.imported, 0);
    }
}
