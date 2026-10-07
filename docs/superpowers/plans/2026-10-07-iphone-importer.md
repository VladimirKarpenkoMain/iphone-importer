# iPhone Importer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Windows-приложение на Rust/egui, которое по USB копирует новые фото и видео с iPhone (AFC через usbmuxd из Apple Devices) в `<цель>\ГГГГ-ММ-ДД\Фото|Видео`.

**Architecture:** Один крейт, пять файлов. `journal.rs` (журнал импортированного) и `import.rs` (классификация, отбор, копирование через `.part`) — чистый stdlib, тестируются без телефона: источник байтов передаётся замыканием. `device.rs` — блокирующая обёртка над `idevice` со своим tokio-runtime. `worker.rs` — фоновый поток (подключение, подсчёт, импорт), общается с окном каналами. `main.rs` — окно egui.

**Tech Stack:** Rust 2024 (тулчейн `stable-x86_64-pc-windows-gnu` + MinGW WinLibs), `idevice 0.1.68` (`usbmuxd`, `afc`, `rustcrypto`), `tokio`, `eframe 0.36`, `rfd 0.17`, `chrono`, dev: `tempfile`.

**Spec:** `docs/superpowers/specs/2026-10-07-iphone-importer-design.md`

## Global Constraints

- Сборка без Visual Studio Build Tools: тулчейн `stable-x86_64-pc-windows-gnu` (закреплён в `rust-toolchain.toml`), MinGW (WinLibs, `winget install BrechtSanders.WinLibs.POSIX.UCRT --scope user`) в PATH. Если `cargo` пишет `dlltool.exe: program not found` — откройте новый терминал.
- `cargo` запускать из PowerShell (в Git Bash `link` — это GNU coreutils).
- `idevice` только с `default-features = false, features = ["rustcrypto", "usbmuxd", "afc"]` — без C-криптографии.
- Только USB-подключение iPhone; Wi-Fi-запись того же устройства игнорируется.
- Оригиналы байт-в-байт, без конвертации.
- Фото: `HEIC HEIF JPG JPEG PNG DNG GIF TIFF WEBP`; видео: `MOV MP4 M4V`; регистр не важен; прочее (AAE и т. п.) не импортируется.
- Раскладка: `<цель>\ГГГГ-ММ-ДД\Фото\`, `<цель>\ГГГГ-ММ-ДД\Видео\`; дата — день старта импорта.
- Занятое имя → `IMG_0001 (1).HEIC`, `(2)` …; существующие файлы не перезаписываются.
- Журнал `<цель>\.import-log`, строка `путь_на_телефоне\tразмер`; запись — только после успешного rename `.part` → итог.
- Цель по умолчанию `%USERPROFILE%\Pictures\iPhone`, выбранная хранится в `%APPDATA%\iphone-importer\config.txt`.
- Нет Apple Devices → кнопка `ms-windows-store://pdp/?productid=9NP83LWLPZ9K`.
- Тексты интерфейса — на русском.

## Review Focus

- Два файла с одинаковым именем из разных папок DCIM (`100APPLE/IMG_0001.HEIC` и `105APPLE/IMG_0001.HEIC` — счётчик iPhone переходит через 9999) → оба сохранены, второй как `IMG_0001 (1).HEIC`. Тест: Task 3 `run_same_name_from_two_dcim_folders_keeps_both`.
- Журнал с оборванной последней строкой после аварии/выключения → следующая запись не склеивается, старые записи читаются. Тест: Task 1 `torn_last_line_does_not_glue_next_record`.
- Кабель выдернут посреди файла → нет `.part`, файл не в журнале, следующий импорт продолжает. Тест: Task 3 `run_connection_lost_stops_and_leaves_no_part` + ручная проверка Task 5.
- Папка дня не может быть создана (на её месте файл / нет прав) → остановка с ошибкой диска, без паники. Тест: Task 3 `run_unwritable_day_dir_is_disk_stop`.
- iPhone заблокирован / не доверяет компьютеру при запуске → понятное сообщение «Разблокируйте… Доверять», после разблокировки приложение подключается само. Ручная проверка Task 5.

---

## File Structure

| Файл | Ответственность |
|---|---|
| `Cargo.toml`, `rust-toolchain.toml`, `.gitignore` | Сборка |
| `src/journal.rs` | Журнал импортированных файлов |
| `src/import.rs` | Классификация, отбор новых, имена, `.part`, свободное место, цикл копирования |
| `src/device.rs` | usbmuxd → AFC: подключение, список `/DCIM`, чтение файла, проверка подключения |
| `src/worker.rs` | Фоновый поток: состояние подключения, подсчёт, запуск импорта, сообщения в GUI |
| `src/main.rs` | Окно egui, настройка папки |

Отклонение от спецификации: спецификация называет четыре модуля; фоновый поток вынесен из `main.rs` в `worker.rs`, чтобы окно не смешивалось с логикой подключения.

---

### Task 1: Каркас крейта и журнал

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `.gitignore`, `src/main.rs`, `src/journal.rs`

**Interfaces:**
- Produces: `journal::Journal` с `open(dir: &Path) -> io::Result<Journal>` (создаёт `dir`), `contains(&self, path: &str, size: u64) -> bool`, `record(&mut self, path: &str, size: u64) -> io::Result<()>`; `journal::FILE_NAME = ".import-log"`.

- [ ] **Step 1: Каркас**

`Cargo.toml`:
```toml
[package]
name = "iphone-importer"
version = "0.1.0"
edition = "2024"

[dependencies]

[dev-dependencies]
tempfile = "3"

[profile.release]
strip = true
```

`rust-toolchain.toml`:
```toml
[toolchain]
channel = "stable-x86_64-pc-windows-gnu"
```

`.gitignore`:
```
/target
```

`src/main.rs`:
```rust
mod journal;

fn main() {}
```

- [ ] **Step 2: Написать падающие тесты** — `src/journal.rs`:

```rust
//! Журнал импортированных файлов: `<цель>\.import-log`, строка `путь_на_телефоне\tразмер`.
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

pub const FILE_NAME: &str = ".import-log";

pub struct Journal {
    seen: HashSet<(String, u64)>,
    file: File,
}

impl Journal {
    pub fn open(dir: &Path) -> io::Result<Journal> {
        todo!()
    }

    pub fn contains(&self, path: &str, size: u64) -> bool {
        todo!()
    }

    pub fn record(&mut self, path: &str, size: u64) -> io::Result<()> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(dir.path()).unwrap();
        j.record("/DCIM/100APPLE/IMG_0001.HEIC", 10).unwrap();
        assert!(j.contains("/DCIM/100APPLE/IMG_0001.HEIC", 10));

        let j = Journal::open(dir.path()).unwrap();
        assert!(j.contains("/DCIM/100APPLE/IMG_0001.HEIC", 10));
        assert!(!j.contains("/DCIM/100APPLE/IMG_0001.HEIC", 11));
        assert!(!j.contains("/DCIM/100APPLE/IMG_0002.HEIC", 10));
    }

    #[test]
    fn creates_missing_dir() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        Journal::open(&nested).unwrap();
        assert!(nested.is_dir());
    }

    #[test]
    fn torn_last_line_does_not_glue_next_record() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(FILE_NAME), "/a\t1\n/b\t").unwrap();
        let mut j = Journal::open(dir.path()).unwrap();
        j.record("/c", 3).unwrap();

        let j = Journal::open(dir.path()).unwrap();
        assert!(j.contains("/a", 1));
        assert!(j.contains("/c", 3));
    }
}
```

- [ ] **Step 3: Убедиться, что падают**

Run: `cargo test journal`
Expected: 3 теста FAIL с `not yet implemented`.

- [ ] **Step 4: Реализация** — заменить три `todo!()`:

```rust
    /// Создаёт `dir`, если её нет, и читает журнал из неё.
    pub fn open(dir: &Path) -> io::Result<Journal> {
        fs::create_dir_all(dir)?;
        let path = dir.join(FILE_NAME);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        let seen = text
            .lines()
            .filter_map(|line| {
                let (path, size) = line.rsplit_once('\t')?;
                Some((path.to_string(), size.parse().ok()?))
            })
            .collect();
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        // Обрывок строки после аварии: закрываем его, чтобы следующая запись не склеилась с ним.
        if !text.is_empty() && !text.ends_with('\n') {
            file.write_all(b"\n")?;
        }
        Ok(Journal { seen, file })
    }

    pub fn contains(&self, path: &str, size: u64) -> bool {
        self.seen.contains(&(path.to_string(), size))
    }

    pub fn record(&mut self, path: &str, size: u64) -> io::Result<()> {
        writeln!(self.file, "{path}\t{size}")?;
        self.file.sync_data()?;
        self.seen.insert((path.to_string(), size));
        Ok(())
    }
```

- [ ] **Step 5: Тесты проходят**

Run: `cargo test journal`
Expected: `3 passed`. Предупреждения о неиспользуемом коде допустимы до Task 5.

- [ ] **Step 6: Commit**

```powershell
git add Cargo.toml Cargo.lock rust-toolchain.toml .gitignore src
git commit -m "feat: crate skeleton and import journal"
```

---

### Task 2: Отбор и раскладка (`import.rs`, чистые функции)

**Files:**
- Create: `src/import.rs`
- Modify: `src/main.rs` (добавить `mod import;`)

**Interfaces:**
- Consumes: `journal::Journal::{open, contains, record}`.
- Produces:
  - `enum Kind { Photo, Video }`, `Kind::folder(self) -> &'static str` («Фото» / «Видео»)
  - `fn kind_of(path: &str) -> Option<Kind>`
  - `struct RemoteFile { pub path: String, pub size: u64 }` (`Clone, Debug, PartialEq`), `RemoteFile::name(&self) -> &str`
  - `struct Summary { pub photos: usize, pub videos: usize, pub bytes: u64 }` (`Clone, Copy, Debug, Default, PartialEq`)
  - `fn select(files: &[RemoteFile], journal: &Journal, all: bool) -> Vec<RemoteFile>`
  - `fn summarize(files: &[RemoteFile]) -> Summary`
  - `fn free_path(dir: &Path, name: &str) -> PathBuf`
  - `fn clean_parts(day: &Path) -> io::Result<()>`
  - `fn free_space(dir: &Path) -> Option<u64>`

- [ ] **Step 1: Падающие тесты** — `src/import.rs` (заглушки + тесты), в `src/main.rs` добавить `mod import;` над `mod journal;`:

```rust
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
    todo!()
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

pub fn select(files: &[RemoteFile], journal: &Journal, all: bool) -> Vec<RemoteFile> {
    todo!()
}

pub fn summarize(files: &[RemoteFile]) -> Summary {
    todo!()
}

pub fn free_path(dir: &Path, name: &str) -> PathBuf {
    todo!()
}

pub fn clean_parts(day: &Path) -> io::Result<()> {
    todo!()
}

pub fn free_space(dir: &Path) -> Option<u64> {
    todo!()
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
}
```

- [ ] **Step 2: Убедиться, что падают**

Run: `cargo test import`
Expected: 6 тестов FAIL с `not yet implemented`.

- [ ] **Step 3: Реализация** — заменить заглушки:

```rust
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
```

```rust
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
```

- [ ] **Step 4: Тесты проходят**

Run: `cargo test`
Expected: `9 passed` (3 journal + 6 import). Предупреждение про неиспользуемые `File`, `Write`, `AtomicBool`, `Ordering` уйдёт в Task 3.

- [ ] **Step 5: Commit**

```powershell
git add src
git commit -m "feat: media classification, selection and naming helpers"
```

---

### Task 3: Цикл копирования `import::run`

**Files:**
- Modify: `src/import.rs` (добавить типы и `run` после `free_space`; тесты — в конец `mod tests`)

**Interfaces:**
- Consumes: всё из Task 2, `Journal::record`.
- Produces:
  - `struct Progress { pub files_done: usize, pub files_total: usize, pub bytes_done: u64, pub bytes_total: u64, pub current: String }` (`Clone, Debug, Default`)
  - `enum FetchError { File(String), Connection(String) }`
  - `enum Stop { Cancelled, ConnectionLost(String), Disk(String) }` (`Debug, PartialEq`)
  - `struct Report { pub imported: usize, pub total: usize, pub failed: Vec<(String, String)>, pub stopped: Option<Stop> }` (`Debug, Default`)
  - `fn run(files: &[RemoteFile], day: &Path, journal: &mut Journal, cancel: &AtomicBool, fetch: impl FnMut(&RemoteFile, &mut dyn Write) -> Result<(), FetchError>, on_progress: impl FnMut(&Progress)) -> Report`

Правила `run` (из спецификации): пишем в `<day>\<Фото|Видео>\<имя>.part`; после успешного чтения и совпадения размера — rename в `free_path(...)`, затем `journal.record`. Отмена / ошибка записи на диск / `FetchError::Connection` → удалить `.part`, остановиться. `FetchError::File` или несовпадение размера → удалить `.part`, записать в `failed`, продолжить.

- [ ] **Step 1: Типы и заглушка** — вставить после `free_space`:

```rust
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

/// Копирует `files` в `day\Фото` / `day\Видео` по одному: `.part` → rename → запись в журнал.
pub fn run(
    files: &[RemoteFile],
    day: &Path,
    journal: &mut Journal,
    cancel: &AtomicBool,
    mut fetch: impl FnMut(&RemoteFile, &mut dyn Write) -> Result<(), FetchError>,
    mut on_progress: impl FnMut(&Progress),
) -> Report {
    todo!()
}
```

- [ ] **Step 2: Падающие тесты** — добавить в конец `mod tests`:

```rust
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
```

- [ ] **Step 3: Убедиться, что падают**

Run: `cargo test run_`
Expected: 7 тестов FAIL с `not yet implemented`.

- [ ] **Step 4: Реализация** — вставить перед `pub fn run` и заменить его тело:

```rust
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
```

```rust
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
```

- [ ] **Step 5: Тесты проходят**

Run: `cargo test`
Expected: `16 passed`.

- [ ] **Step 6: Commit**

```powershell
git add src
git commit -m "feat: resumable copy loop with .part files and journal"
```

---

### Task 4: Доступ к iPhone (`device.rs`)

**Files:**
- Create: `src/device.rs`
- Modify: `Cargo.toml` (`[dependencies]`), `src/main.rs` (`mod device;`)

**Interfaces:**
- Consumes: `import::{RemoteFile, FetchError, kind_of}`.
- Produces:
  - `enum ConnectError { NoUsbmuxd, NoDevice, NotTrusted, Other(String) }`
  - `struct Device { pub udid: String, .. }`
  - `fn connect() -> Result<Device, ConnectError>`
  - `Device::list(&mut self) -> Result<Vec<RemoteFile>, idevice::IdeviceError>`
  - `Device::fetch(&mut self, f: &RemoteFile, out: &mut dyn Write) -> Result<(), FetchError>`
  - `Device::still_connected(&self) -> bool`

Факты из spike: usbmuxd на `127.0.0.1:27015`; телефон виден дважды (USB и Wi-Fi) — брать `Connection::Usb`; файл AFC надо закрывать `close().await`; устаревшее сопряжение → `IdeviceError::InvalidHostID`. В `idevice 0.1.68` с нашими фичами нет вариантов `PasswordProtected` / `PairingDialogResponsePending` / `UserDeniedPairing` — не использовать.

- [ ] **Step 1: Зависимости** — в `Cargo.toml` `[dependencies]`:

```toml
idevice = { version = "0.1.68", default-features = false, features = ["rustcrypto", "usbmuxd", "afc"] }
tokio = { version = "1", features = ["rt", "net", "time"] }
```

В `src/main.rs` добавить `mod device;` первой строкой модулей.

- [ ] **Step 2: Тест на реальном телефоне** — `src/device.rs` начать с теста (он `#[ignore]`, т.к. нужен iPhone):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Нужен подключённый и доверенный iPhone: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn lists_and_reads_from_real_iphone() {
        let mut dev = connect().expect("connect");
        let files = dev.list().expect("list");
        assert!(!files.is_empty(), "в /DCIM нет фото/видео");
        let small = files.iter().min_by_key(|f| f.size).unwrap();
        let mut buf = Vec::new();
        dev.fetch(small, &mut buf).expect("fetch");
        assert_eq!(buf.len() as u64, small.size);
        assert!(dev.still_connected());
        println!("{} файлов, прочитан {} ({} байт)", files.len(), small.path, small.size);
    }
}
```

Run: `cargo test -- --ignored`
Expected: ошибка компиляции `cannot find function connect`.

- [ ] **Step 3: Реализация** — над `mod tests` в `src/device.rs`:

```rust
//! iPhone через usbmuxd (Apple Devices) → AFC. Блокирующий API поверх своего tokio-runtime.
use crate::import::{FetchError, RemoteFile, kind_of};
use idevice::afc::AfcClient;
use idevice::afc::opcode::AfcFopenMode;
use idevice::usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection};
use idevice::{IdeviceError, IdeviceService};
use std::io::Write;
use tokio::runtime::Runtime;

const CHUNK: u64 = 1 << 20;

#[derive(Debug)]
pub enum ConnectError {
    NoUsbmuxd,
    NoDevice,
    NotTrusted,
    Other(String),
}

pub struct Device {
    pub udid: String,
    rt: Runtime,
    afc: AfcClient,
}

/// Подключается к первому iPhone, подключённому по USB (Wi-Fi игнорируем).
pub fn connect() -> Result<Device, ConnectError> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| ConnectError::Other(e.to_string()))?;
    let (udid, afc) = rt.block_on(async {
        let mut mux = UsbmuxdConnection::default().await.map_err(|_| ConnectError::NoUsbmuxd)?;
        let devices = mux.get_devices().await.map_err(|e| ConnectError::Other(e.to_string()))?;
        let dev = devices
            .into_iter()
            .find(|d| matches!(d.connection_type, Connection::Usb))
            .ok_or(ConnectError::NoDevice)?;
        let provider = dev.to_provider(UsbmuxdAddr::default(), "iphone-importer");
        let afc = AfcClient::connect(&provider).await.map_err(|e| match e {
            IdeviceError::InvalidHostID | IdeviceError::DeviceLocked | IdeviceError::NotFound => {
                ConnectError::NotTrusted
            }
            e => ConnectError::Other(e.to_string()),
        })?;
        Ok((dev.udid, afc))
    })?;
    Ok(Device { udid, rt, afc })
}

impl Device {
    /// Все фото и видео из `/DCIM/<папка>/`.
    pub fn list(&mut self) -> Result<Vec<RemoteFile>, IdeviceError> {
        let Device { rt, afc, .. } = self;
        rt.block_on(async {
            let mut out = vec![];
            for d in afc.list_dir("/DCIM").await? {
                let dir = format!("/DCIM/{d}");
                if d.starts_with('.') || afc.get_file_info(&dir).await?.st_ifmt != "S_IFDIR" {
                    continue;
                }
                for name in afc.list_dir(&dir).await? {
                    if name.starts_with('.') || kind_of(&name).is_none() {
                        continue;
                    }
                    let path = format!("{dir}/{name}");
                    let info = afc.get_file_info(&path).await?;
                    if info.st_ifmt == "S_IFREG" {
                        out.push(RemoteFile { path, size: info.size as u64 });
                    }
                }
            }
            Ok(out)
        })
    }

    /// Читает файл блоками по 1 МБ в `out`.
    pub fn fetch(&mut self, f: &RemoteFile, out: &mut dyn Write) -> Result<(), FetchError> {
        let Device { rt, afc, .. } = self;
        rt.block_on(async {
            let mut fd = afc.open(f.path.as_str(), AfcFopenMode::RdOnly).await.map_err(classify)?;
            let copied = async {
                let mut left = f.size;
                while left > 0 {
                    let chunk = fd.read_n(left.min(CHUNK) as usize).await.map_err(classify)?;
                    if chunk.is_empty() {
                        break;
                    }
                    left = left.saturating_sub(chunk.len() as u64);
                    out.write_all(&chunk).map_err(|e| FetchError::File(e.to_string()))?;
                }
                Ok(())
            }
            .await;
            let closed = fd.close().await.map_err(classify);
            copied.and(closed)
        })
    }

    /// Телефон всё ещё виден usbmuxd по USB.
    pub fn still_connected(&self) -> bool {
        self.rt.block_on(async {
            let Ok(mut mux) = UsbmuxdConnection::default().await else { return false };
            mux.get_devices()
                .await
                .map(|ds| ds.iter().any(|d| d.udid == self.udid && matches!(d.connection_type, Connection::Usb)))
                .unwrap_or(false)
        })
    }
}

/// Ошибка AFC про конкретный файл — пропускаем файл; всё остальное считаем обрывом связи.
fn classify(e: IdeviceError) -> FetchError {
    match e {
        IdeviceError::Afc(_) | IdeviceError::NotFound => FetchError::File(e.to_string()),
        e => FetchError::Connection(e.to_string()),
    }
}
```

- [ ] **Step 4: Проверка**

Run: `cargo test` → Expected: `16 passed; 1 ignored`.
С подключённым разблокированным iPhone: `cargo test -- --ignored --nocapture` → Expected: `1 passed`, строка вида `5432 файлов, прочитан /DCIM/103APPLE/IMG_3240.JPG (55891 байт)`.

- [ ] **Step 5: Commit**

```powershell
git add Cargo.toml Cargo.lock src
git commit -m "feat: iPhone access over usbmuxd/AFC"
```

---

### Task 5: Фоновый поток и окно

**Files:**
- Create: `src/worker.rs`
- Modify: `Cargo.toml` (`[dependencies]`), `src/main.rs` (полностью)

**Interfaces:**
- Consumes: `device::{connect, ConnectError, Device}`, `import::{select, summarize, clean_parts, free_space, run, Progress, Report, Stop, Summary, RemoteFile}`, `journal::Journal`.
- Produces: `worker::spawn(dest: PathBuf, ctx: egui::Context, cancel: Arc<AtomicBool>) -> (mpsc::Sender<Cmd>, mpsc::Receiver<Msg>)`; `enum Cmd { SetDest(PathBuf), Import { all: bool } }`; `enum Msg { NoUsbmuxd, NoDevice, NotTrusted, Scanning, Ready { udid: String, new: Summary, all: Summary }, Progress(Progress), Done { report: Report, day: PathBuf }, Error(String) }`.

Поведение (спецификация, раздел «Экран»): опрос каждые 2 с; при появлении телефона — подсчёт; «Импортировать» активна, только если новых > 0; «Всё заново…» — подтверждение строкой «Скопировать все N файлов (X) ещё раз? [Да] [Нет]»; нехватка места → ошибка до старта; после обрыва связи — повторное подключение.

- [ ] **Step 1: Зависимости** — в `Cargo.toml` `[dependencies]`:

```toml
chrono = { version = "0.4", default-features = false, features = ["clock"] }
eframe = "0.36"
rfd = "0.17"
```

Внимание: в eframe 0.36 у `App` обязательный метод `fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame)` (не `update`), панель — `egui::CentralPanel::default().show(ui, |ui| …)`.

- [ ] **Step 2: `src/worker.rs`**

```rust
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
```

- [ ] **Step 3: `src/main.rs`** (заменить целиком)

```rust
#![windows_subsystem = "windows"]

mod device;
mod import;
mod journal;
mod worker;

use eframe::egui;
use import::{Progress, Report, Stop, Summary};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;
use worker::{Cmd, Msg};

const STORE_URL: &str = "ms-windows-store://pdp/?productid=9NP83LWLPZ9K";

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([560.0, 380.0]),
        ..Default::default()
    };
    eframe::run_native("iPhone Importer", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

enum Phone {
    NoUsbmuxd,
    NoDevice,
    NotTrusted,
    Scanning,
    Ready { udid: String, new: Summary, all: Summary },
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
                Msg::NoUsbmuxd => self.phone = Phone::NoUsbmuxd,
                Msg::NoDevice => self.phone = Phone::NoDevice,
                Msg::NotTrusted => self.phone = Phone::NotTrusted,
                Msg::Scanning => self.phone = Phone::Scanning,
                Msg::Ready { udid, new, all } => {
                    self.phone = Phone::Ready { udid, new, all };
                    self.error = None;
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
                if new.photos + new.videos == 0 {
                    ui.label("Всё уже импортировано.");
                } else {
                    ui.label(format!(
                        "Новых: {} файлов — {} фото, {} видео, {}",
                        new.photos + new.videos,
                        new.photos,
                        new.videos,
                        size(new.bytes)
                    ));
                }
                ui.horizontal(|ui| {
                    let has_new = new.photos + new.videos > 0;
                    if ui.add_enabled(!busy && has_new, egui::Button::new("Импортировать")).clicked() {
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
                let eta = if speed > 0.0 { (p.bytes_total - p.bytes_done) as f64 / speed } else { 0.0 };
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
```

- [ ] **Step 4: Сборка и тесты**

Run: `cargo test` → Expected: `16 passed; 1 ignored`, без предупреждений.
Run: `cargo build --release` → Expected: `Finished`.
Run: `objdump -p target\release\iphone-importer.exe | Select-String 'DLL Name'` → Expected: только системные DLL Windows (`KERNEL32`, `USER32`, `api-ms-win-*`, `opengl32` …), без `libgcc*`, `libwinpthread*`, `libstdc++*`.

- [ ] **Step 5: Ручная проверка на iPhone** — запустить `target\release\iphone-importer.exe`, папку выбрать временную (например `D:\iphone-test`):

1. Телефон отключён → «Подключите iPhone кабелем.»
2. Подключить заблокированным → «Разблокируйте iPhone и нажмите «Доверять»…»; разблокировать → в течение ~2 с «Считаю…», затем «iPhone подключён» и «Новых: N файлов — …».
3. «Импортировать» → прогресс, скорость ~30–35 МБ/с на крупных видео; через ~20 с выдернуть кабель → «Соединение потеряно. Импортировано X из N…»; в папке дня нет `*.part`.
4. Подключить снова → «Новых» уменьшилось на X; «Импортировать» → докачивает остальное; «Импортировано … в D:\iphone-test\2026-…».
5. «Открыть папку» → Проводник; HEIC/JPG в `Фото`, MOV/MP4 в `Видео`.
6. Перезапуск приложения → папка та же, «Всё уже импортировано.»
7. «Всё заново…» → строка подтверждения с общим числом и объёмом; «Да», затем через пару секунд «Отмена» → «Отменено…», нет `*.part`; копии получили суффиксы ` (1)`.

- [ ] **Step 6: Commit**

```powershell
git add Cargo.toml Cargo.lock src
git commit -m "feat: egui window and background import worker"
```
