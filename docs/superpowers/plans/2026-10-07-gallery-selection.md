# Галерея с превью и выбор файлов — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** экран галереи с миниатюрами iOS, где можно отметить любые файлы телефона (новые и уже импортированные) и импортировать только их.

**Architecture:** `import::group` собирает плитки (Live Photo = фото + MOV), worker отдаёт их в `Phone::Ready`
и по запросу `Cmd::Thumbs` читает готовые JPEG-миниатюры iOS через тот же AFC. GUI декодирует JPEG
крейтом `image`, держит LRU-кеш текстур и шлёт `Cmd::Import(список файлов)`. Копирование (`import::run`)
и журнал не меняются.

**Tech Stack:** Rust 2024, eframe/egui 0.36, idevice (AFC), `image` 0.25 (только `jpeg`).

**Spec:** `docs/superpowers/specs/2026-10-07-gallery-selection-design.md`

## Global Constraints

- Тулчейн `stable-x86_64-pc-windows-gnu`; `cargo` запускать из **PowerShell**, не из Git Bash.
- Строки UI, комментарии и doc-комментарии — на русском.
- `.exe` линкуется только с системными DLL: `image = { version = "0.25", default-features = false, features = ["jpeg"] }`, ничего с C-библиотеками.
- Путь миниатюры: `/PhotoData/Thumbnails/V2<путь в DCIM>/5005.JPG`.
- Инварианты целостности из `CLAUDE.md` (`.part` → fsync → rename → журнал; не перезаписывать) не трогаем — `import::run` не меняется.
- Кеш миниатюр — не больше 1 500 записей; текстура плитки ≤160 px по длинной стороне; увеличение ≤480 px.
- Коммиты заканчиваются строкой `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

- Одинаковое имя в разных папках DCIM (`/DCIM/100APPLE/IMG_1.HEIC` и `/DCIM/101APPLE/IMG_1.MOV`) — **не** Live Photo, две плитки. Тест в Task 1.
- Расширения в разном регистре (`A.heic` + `A.MOV`) — всё равно пара. Тест в Task 1.
- Битая/не-JPEG миниатюра — заглушка, без паники, повторно не запрашивается. Тест в Task 6.
- Телефон отключили при открытой галерее — возврат на главный экран, кеш очищен. Тест в Task 6.
- Пересканирование убрало выбранный файл — он выпадает из выбора и не уходит в импорт. Тест в Task 5.

---

## Карта файлов

- `src/import.rs` — `Item`, `group`; удалить `select` (Task 1, Task 4).
- `src/gallery.rs` — **новый**: `Cache<V>`, `Gallery` (состояние выбора), отрисовка галереи и увеличения, декодирование JPEG (Task 2, 5, 6).
- `src/device.rs` — `Device::thumbnail` (Task 3).
- `src/worker.rs` — новый протокол и очередь миниатюр (Task 4).
- `src/main.rs` — счётчики из плиток, `start(files)`, ссылка «Выбрать файлы…», подключение галереи (Task 4, 6).
- `Cargo.toml` — `image` (Task 6).
- `CLAUDE.md`, `README.md` — документация (Task 7).

---

### Task 1: Плитки — `import::Item` и `import::group`

**Files:**
- Modify: `src/import.rs` (после `summarize`, тесты — в `mod tests`)

**Interfaces:**
- Produces:
  ```rust
  #[derive(Clone, Debug, PartialEq)]
  pub struct Item { pub files: Vec<RemoteFile>, pub imported: bool }
  impl Item { pub fn key(&self) -> &str }            // = files[0].path
  pub fn group(files: &[RemoteFile], journal: &Journal) -> Vec<Item>
  ```

- [ ] **Step 1: Написать падающие тесты** — в `mod tests` файла `src/import.rs`:

```rust
    fn paths(items: &[Item]) -> Vec<Vec<&str>> {
        items.iter().map(|i| i.files.iter().map(|f| f.path.as_str()).collect()).collect()
    }

    #[test]
    fn group_pairs_live_photos_and_sorts_by_path() {
        let dir = tempfile::tempdir().unwrap();
        let j = Journal::open(dir.path()).unwrap();
        let files = [
            rf("/DCIM/1/B.MOV", 9),
            rf("/DCIM/1/A.MOV", 2),
            rf("/DCIM/1/A.heic", 5),
            rf("/DCIM/1/C.JPG", 3),
            rf("/DCIM/1/A.AAE", 1),
        ];
        assert_eq!(
            paths(&group(&files, &j)),
            vec![vec!["/DCIM/1/A.heic", "/DCIM/1/A.MOV"], vec!["/DCIM/1/B.MOV"], vec!["/DCIM/1/C.JPG"]]
        );
    }

    #[test]
    fn group_does_not_pair_across_folders() {
        let dir = tempfile::tempdir().unwrap();
        let j = Journal::open(dir.path()).unwrap();
        let files = [rf("/DCIM/100APPLE/IMG_1.HEIC", 5), rf("/DCIM/101APPLE/IMG_1.MOV", 2)];
        assert_eq!(
            paths(&group(&files, &j)),
            vec![vec!["/DCIM/100APPLE/IMG_1.HEIC"], vec!["/DCIM/101APPLE/IMG_1.MOV"]]
        );
    }

    #[test]
    fn group_item_is_imported_only_when_all_files_journaled() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(dir.path()).unwrap();
        j.record("/1/A.HEIC", 5).unwrap();
        j.record("/1/C.JPG", 3).unwrap();
        let files = [rf("/1/A.HEIC", 5), rf("/1/A.MOV", 2), rf("/1/C.JPG", 3)];

        let g = group(&files, &j);
        assert_eq!(g.iter().map(|i| i.imported).collect::<Vec<_>>(), vec![false, true]);
        assert_eq!(g[0].key(), "/1/A.HEIC");

        j.record("/1/A.MOV", 2).unwrap();
        assert!(group(&files, &j)[0].imported);
    }
```

- [ ] **Step 2: Убедиться, что падают**

Run (PowerShell): `cargo test group`
Expected: ошибка компиляции `cannot find function group` / `cannot find type Item`.

- [ ] **Step 3: Реализация** — в `src/import.rs` добавить импорт и код после `summarize`:

```rust
use std::collections::{HashMap, HashSet};
```

```rust
/// Плитка галереи: один файл или пара Live Photo (фото первым).
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub files: Vec<RemoteFile>,
    /// Все файлы плитки есть в журнале.
    pub imported: bool,
}

impl Item {
    /// Ключ плитки; по нему же берётся миниатюра.
    pub fn key(&self) -> &str {
        &self.files[0].path
    }
}

/// Фото и видео из `files` плитками по порядку путей. Live Photo — фото и `.MOV` с тем же путём
/// без расширения — одна плитка.
pub fn group(files: &[RemoteFile], journal: &Journal) -> Vec<Item> {
    let stem = |p: &str| p.rsplit_once('.').map_or(p, |(s, _)| s).to_ascii_lowercase();
    let is_mov = |p: &str| p.rsplit_once('.').is_some_and(|(_, e)| e.eq_ignore_ascii_case("mov"));
    let mut media: Vec<&RemoteFile> = files.iter().filter(|f| kind_of(&f.path).is_some()).collect();
    media.sort_by(|a, b| a.path.cmp(&b.path));
    let photos: HashSet<String> =
        media.iter().filter(|f| kind_of(&f.path) == Some(Kind::Photo)).map(|f| stem(&f.path)).collect();

    let mut items = vec![];
    let mut photo_at = HashMap::new();
    let mut live = vec![];
    for f in media {
        if is_mov(&f.path) && photos.contains(&stem(&f.path)) {
            live.push(f);
            continue;
        }
        if kind_of(&f.path) == Some(Kind::Photo) {
            photo_at.insert(stem(&f.path), items.len());
        }
        items.push(Item { files: vec![f.clone()], imported: false });
    }
    for f in live {
        items[photo_at[&stem(&f.path)]].files.push(f.clone());
    }
    for item in &mut items {
        item.imported = item.files.iter().all(|f| journal.contains(&f.path, f.size));
    }
    items
}
```

- [ ] **Step 4: Тесты проходят**

Run: `cargo test group` → 3 passed. Затем `cargo test` → всё зелёное (предупреждение «`group` never used» допустимо до Task 4).

- [ ] **Step 5: Коммит**

```powershell
git add src/import.rs
git commit -m @'
feat: group media into gallery items, Live Photo as one item

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
'@
```

---

### Task 2: LRU-кеш миниатюр `gallery::Cache<V>`

**Files:**
- Create: `src/gallery.rs`
- Modify: `src/main.rs` (строка `mod device;` — добавить `mod gallery;` по алфавиту)

**Interfaces:**
- Produces:
  ```rust
  pub const CAP: usize = 1500;
  pub struct Cache<V> { .. }
  impl<V> Cache<V> {
      pub fn new(cap: usize) -> Self;
      pub fn contains(&self, key: &str) -> bool;              // в т.ч. «миниатюры нет»
      pub fn get(&mut self, key: &str) -> Option<&Option<V>>;  // отмечает запись показанной
      pub fn insert(&mut self, key: String, v: Option<V>);    // вытесняет давно не показанную
      pub fn clear(&mut self);
  }
  ```

- [ ] **Step 1: Падающий тест** — создать `src/gallery.rs`:

```rust
//! Экран галереи: плитки с миниатюрами iOS, выбор файлов, увеличение.
use std::collections::HashMap;

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
```

В `src/main.rs`: после `mod device;` добавить `mod gallery;`.

- [ ] **Step 2: Убедиться, что падает**

Run: `cargo test cache_evicts` → ошибка компиляции `cannot find type Cache`.

- [ ] **Step 3: Реализация** — в `src/gallery.rs` после `use`:

```rust
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
```

- [ ] **Step 4: Тест проходит**

Run: `cargo test cache_evicts` → 1 passed. `cargo test` — всё зелёное (dead_code-предупреждения допустимы до Task 6).

- [ ] **Step 5: Коммит**

```powershell
git add src/gallery.rs src/main.rs
git commit -m @'
feat: LRU cache for gallery thumbnails

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
'@
```

---

### Task 3: `Device::thumbnail`

**Files:**
- Modify: `src/device.rs` (метод в `impl Device` после `fetch`; тест в `mod tests`)

**Interfaces:**
- Produces: `pub fn thumbnail(&mut self, path: &str) -> Option<Vec<u8>>` — `path` — путь файла на телефоне (`/DCIM/…`). (Отличие от спеки: принимает путь, а не `&RemoteFile` — размер файла не нужен, а worker хранит в очереди только пути.)

- [ ] **Step 1: Аппаратный тест** — в `mod tests` файла `src/device.rs`:

```rust
    /// Нужен подключённый и доверенный iPhone: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn reads_thumbnail_from_real_iphone() {
        let mut dev = connect().expect("connect");
        let files = dev.list().expect("list");
        let photo = files.iter().find(|f| crate::import::kind_of(&f.path) == Some(crate::import::Kind::Photo)).expect("нет фото");
        let jpeg = dev.thumbnail(&photo.path).expect("нет миниатюры");
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        assert!(dev.thumbnail("/DCIM/нет/такого.HEIC").is_none());
        println!("{}: миниатюра {} байт", photo.path, jpeg.len());
    }
```

- [ ] **Step 2: Убедиться, что не компилируется**

Run: `cargo test reads_thumbnail -- --ignored` → `no method named thumbnail`.

- [ ] **Step 3: Реализация** — в `impl Device` после `fetch`:

```rust
    /// Готовая JPEG-миниатюра iOS (~360×480) для файла `path` или `None`, если её нет или не прочиталась.
    /// Пропажу телефона здесь не различаем — её ловит `still_connected`.
    pub fn thumbnail(&mut self, path: &str) -> Option<Vec<u8>> {
        let Device { rt, afc, .. } = self;
        rt.block_on(async {
            let thumb = format!("/PhotoData/Thumbnails/V2{path}/5005.JPG");
            let size = afc.get_file_info(&thumb).await.ok()?.size;
            let mut fd = afc.open(thumb.as_str(), AfcFopenMode::RdOnly).await.ok()?;
            let data = fd.read_n(size as usize).await;
            let _ = fd.close().await;
            data.ok()
        })
    }
```

- [ ] **Step 4: Проверка на телефоне**

Run (iPhone подключён, разблокирован): `cargo test reads_thumbnail -- --ignored --nocapture`
Expected: `1 passed`, строка `…: миниатюра ~45000 байт`. Если телефона нет — `cargo build` должен проходить; отметить в отчёте, что аппаратный тест не запускался.

- [ ] **Step 5: Коммит**

```powershell
git add src/device.rs
git commit -m @'
feat: read iOS thumbnail JPEG for a phone file

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
'@
```

---

### Task 4: Протокол worker ↔ GUI: плитки, импорт списка, очередь миниатюр

**Files:**
- Modify: `src/worker.rs` (весь файл, кроме `spawn`)
- Modify: `src/import.rs` (удалить `select` и тест `select_skips_journaled_and_non_media`)
- Modify: `src/main.rs` (поля `App`, `drain`, `start`, главный экран, тесты)

**Interfaces:**
- Consumes: `import::{Item, group}` (Task 1), `Device::thumbnail(&mut self, &str)` (Task 3).
- Produces:
  ```rust
  pub enum Cmd { SetDest(PathBuf), Import(Vec<RemoteFile>), Thumbs(Vec<String>) }
  pub enum Phone { NoUsbmuxd, NoDevice, NotTrusted, Scanning, Ready { udid: String, items: Vec<Item> } }
  pub enum Msg { Phone(Phone), Progress(Progress), Done { report: Report, day: PathBuf }, Error(String),
                 Thumb { path: String, jpeg: Option<Vec<u8>> } }
  // main.rs
  struct App { .., new: Vec<RemoteFile>, new_sum: Summary, .. }   // файлы неимпортированных плиток и их сводка
  fn start(&mut self, files: Vec<RemoteFile>)
  ```

- [ ] **Step 1: Падающие тесты GUI** — в `mod tests` файла `src/main.rs` заменить тесты целиком:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use import::{Item, RemoteFile, Summary};

    fn rf(path: &str, size: u64) -> RemoteFile {
        RemoteFile { path: path.into(), size }
    }

    fn app() -> (App, mpsc::Sender<Msg>, mpsc::Receiver<Cmd>) {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (msg_tx, msg_rx) = mpsc::channel();
        let app = App {
            dest: PathBuf::new(),
            cmds: cmd_tx,
            msgs: msg_rx,
            cancel: Arc::new(AtomicBool::new(false)),
            phone: Phone::NoDevice,
            new: vec![],
            new_sum: Summary::default(),
            importing: None,
            rate: (Instant::now(), String::new()),
            done: None,
            error: None,
        };
        (app, msg_tx, cmd_rx)
    }

    #[test]
    fn import_click_while_phone_vanishes_does_not_stick_busy() {
        let (mut app, msgs, _cmds) = app();
        app.start(vec![rf("/A.HEIC", 1)]);
        msgs.send(Msg::Phone(Phone::NoDevice)).unwrap();
        app.drain();
        assert!(app.importing.is_none());
    }

    #[test]
    fn start_resets_cancel_before_sending_import() {
        let (mut app, _msgs, cmds) = app();
        app.cancel.store(true, Ordering::Relaxed);
        app.start(vec![rf("/A.HEIC", 1)]);
        assert!(!app.cancel.load(Ordering::Relaxed));
        assert!(matches!(cmds.try_recv(), Ok(Cmd::Import(f)) if f == vec![rf("/A.HEIC", 1)]));
    }

    #[test]
    fn ready_collects_files_of_new_items() {
        let (mut app, msgs, _cmds) = app();
        let items = vec![
            Item { files: vec![rf("/A.HEIC", 5), rf("/A.MOV", 2)], imported: false },
            Item { files: vec![rf("/B.JPG", 3)], imported: true },
        ];
        msgs.send(Msg::Phone(Phone::Ready { udid: "u".into(), items })).unwrap();
        app.drain();
        assert_eq!(app.new, vec![rf("/A.HEIC", 5), rf("/A.MOV", 2)]);
        assert_eq!(app.new_sum, Summary { photos: 1, videos: 1, bytes: 7 });
    }
}
```

- [ ] **Step 2: Убедиться, что не компилируется**

Run: `cargo test` → ошибки `no field new`, `Cmd::Import` ожидает `{ all }`, `Phone::Ready` не имеет `items`.

- [ ] **Step 3: worker.rs** — заменить `use`, типы, `run`, `ready`, `import_now` (функция `spawn` без изменений):

```rust
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
```

```rust
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
        let cmd = if thumbs.is_empty() {
            cmds.recv_timeout(POLL)
        } else {
            cmds.try_recv().map_err(|e| match e {
                mpsc::TryRecvError::Empty => mpsc::RecvTimeoutError::Timeout,
                mpsc::TryRecvError::Disconnected => mpsc::RecvTimeoutError::Disconnected,
            })
        };
        match cmd {
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
            let jpeg = d.thumbnail(&path);
            send(Msg::Thumb { path, jpeg });
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
```

- [ ] **Step 4: import.rs** — удалить функцию `select` (с doc-комментарием «Фото и видео из `files`; без `all`…») и тест `select_skips_journaled_and_non_media`.

- [ ] **Step 5: main.rs** — поля и логика `App`:

  1. `use`: `use import::{Progress, RemoteFile, Report, Stop, Summary};`
  2. В `struct App` удалить `confirm_all: bool`, добавить после `phone: Phone,`:
     ```rust
         /// Файлы неимпортированных плиток (главная кнопка) и их сводка.
         new: Vec<RemoteFile>,
         new_sum: Summary,
     ```
  3. В `App::new` убрать `confirm_all: false,`, добавить `new: vec![], new_sum: Summary::default(),`.
  4. В `drain` ветку `Msg::Phone` заменить на:
     ```rust
                 Msg::Phone(phone) => {
                     if let Phone::Ready { items, .. } = &phone {
                         self.error = None;
                         self.new = items.iter().filter(|i| !i.imported).flat_map(|i| i.files.iter().cloned()).collect();
                         self.new_sum = import::summarize(&self.new);
                     }
                     self.importing = None;
                     self.phone = phone;
                 }
     ```
     и добавить ветку (до Task 6 миниатюры не нужны): `Msg::Thumb { .. } => {}`.
  5. `start`:
     ```rust
         fn start(&mut self, files: Vec<RemoteFile>) {
             self.done = None;
             self.error = None;
             self.importing = Some((Progress::default(), Instant::now()));
             self.rate.1.clear();
             self.cancel.store(false, Ordering::Relaxed);
             let _ = self.cmds.send(Cmd::Import(files));
         }
     ```
  6. Карточка телефона: `if let Phone::Ready { new, .. } = &self.phone {` → `if matches!(self.phone, Phone::Ready { .. }) {` и в теле `let new = self.new_sum;` первой строкой (остальное без изменений).
  7. Нижний блок `if let Phone::Ready { new, all, .. } = &self.phone { … }` целиком заменить на:
     ```rust
                 if matches!(self.phone, Phone::Ready { .. }) {
                     let n = self.new.len();
                     let label = if n == 0 { "Импортировать".to_string() } else { format!("Импортировать {n} файлов") };
                     let button = primary(label).min_size(egui::vec2(ui.available_width(), 38.0));
                     if ui.add_enabled(!busy && n > 0, button).clicked() {
                         self.start(self.new.clone());
                     }
                 }
     ```
     (Ссылка «Скопировать всё заново…» исчезает; её заменит «Выбрать файлы…» в Task 6.)

- [ ] **Step 6: Тесты проходят**

Run: `cargo test` → всё зелёное, включая 3 теста `main::tests`.

- [ ] **Step 7: Проверка вручную (если телефон есть)**

Run: `cargo run` → статус «iPhone подключён», счётчики новых те же, что до изменений; «Импортировать N» импортирует.

- [ ] **Step 8: Коммит**

```powershell
git add src/worker.rs src/import.rs src/main.rs
git commit -m @'
refactor: worker sends gallery items, imports an explicit file list, serves thumbnails

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
'@
```

---

### Task 5: Состояние галереи `gallery::Gallery`

**Files:**
- Modify: `src/gallery.rs`

**Interfaces:**
- Consumes: `import::{Item, RemoteFile}` (Task 1).
- Produces:
  ```rust
  pub struct Gallery {
      pub all: bool,                       // фильтр: false — только новые
      pub selected: HashSet<String>,       // ключи плиток
      pub zoom: Option<String>,            // ключ плитки в увеличении
      zoom_tex: Option<(String, Option<egui::TextureHandle>)>,
      sent: Vec<String>,                   // последний отправленный Cmd::Thumbs
  }
  impl Gallery {
      pub fn new(items: &[Item]) -> Self;                     // выбраны все неимпортированные
      pub fn retain(&mut self, items: &[Item]);               // выбор ∩ существующие ключи
      pub fn shown(&self, items: &[Item]) -> Vec<usize>;      // индексы с учётом фильтра
      pub fn files(&self, items: &[Item]) -> Vec<RemoteFile>; // файлы выбранных, в порядке плиток
  }
  ```

- [ ] **Step 1: Падающие тесты** — в `mod tests` файла `src/gallery.rs`:

```rust
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
```

- [ ] **Step 2: Убедиться, что не компилируется**

Run: `cargo test gallery_` → `cannot find type Gallery`.

- [ ] **Step 3: Реализация** — в `src/gallery.rs`, `use` заменить на:

```rust
use crate::import::{Item, RemoteFile};
use eframe::egui;
use std::collections::{HashMap, HashSet};
```

и добавить после `Cache`:

```rust
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
```

- [ ] **Step 4: Тесты проходят**

Run: `cargo test gallery_` → 3 passed; `cargo test` — всё зелёное.

- [ ] **Step 5: Коммит**

```powershell
git add src/gallery.rs
git commit -m @'
feat: gallery selection state

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
'@
```

---

### Task 6: Экран галереи, миниатюры и подключение к `App`

**Files:**
- Modify: `Cargo.toml` (`[dependencies]`)
- Modify: `src/gallery.rs` (декодирование, отрисовка, увеличение)
- Modify: `src/main.rs` (поля `ctx`, `gallery`, `thumbs`; `drain`; `ui`; ссылка «Выбрать файлы…»; тесты)

**Interfaces:**
- Consumes: `Cache`, `CAP`, `Gallery` (Task 2, 5); `Cmd::Thumbs`, `Msg::Thumb`, `Phone::Ready { items }`, `App::start(Vec<RemoteFile>)` (Task 4).
- Produces:
  ```rust
  pub struct Thumb { pub jpeg: Vec<u8>, pub tex: egui::TextureHandle }
  pub fn texture(ctx: &egui::Context, name: &str, jpeg: &[u8], max: u32) -> Option<egui::TextureHandle>;
  pub enum Action { None, Back, Import }
  pub fn show(ui: &mut egui::Ui, g: &mut Gallery, items: &[Item], thumbs: &mut Cache<Thumb>,
              request: &mut dyn FnMut(Vec<String>)) -> Action;
  // main.rs
  struct App { .., ctx: egui::Context, gallery: Option<Gallery>, thumbs: Cache<Thumb>, .. }
  fn import_selected(&mut self);
  fn open_gallery(&mut self);
  ```

- [ ] **Step 1: Зависимость** — в `Cargo.toml` в `[dependencies]`:

```toml
image = { version = "0.25", default-features = false, features = ["jpeg"] }
```

- [ ] **Step 2: Падающие тесты** — в `mod tests` файла `src/main.rs`:

  1. `use`: `use gallery::{Cache, Gallery};`
  2. В `app()` добавить поля: `ctx: egui::Context::default(), gallery: None, thumbs: Cache::new(gallery::CAP),`
  3. Тесты:

```rust
    fn ready(items: Vec<Item>) -> Msg {
        Msg::Phone(Phone::Ready { udid: "u".into(), items })
    }

    #[test]
    fn thumb_msg_decodes_jpeg_and_remembers_missing() {
        let (mut app, msgs, _cmds) = app();
        let mut jpeg = vec![];
        image::RgbImage::new(4, 2).write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg).unwrap();
        msgs.send(Msg::Thumb { path: "/A.HEIC".into(), jpeg: Some(jpeg) }).unwrap();
        msgs.send(Msg::Thumb { path: "/B.HEIC".into(), jpeg: Some(b"not a jpeg".to_vec()) }).unwrap();
        msgs.send(Msg::Thumb { path: "/C.HEIC".into(), jpeg: None }).unwrap();
        app.drain();
        assert!(matches!(app.thumbs.get("/A.HEIC"), Some(Some(t)) if t.tex.size() == [4, 2]));
        assert!(matches!(app.thumbs.get("/B.HEIC"), Some(None)));
        assert!(matches!(app.thumbs.get("/C.HEIC"), Some(None)));
    }

    #[test]
    fn phone_loss_closes_gallery_and_clears_thumbs() {
        let (mut app, msgs, _cmds) = app();
        let items = vec![Item { files: vec![rf("/A.HEIC", 5)], imported: false }];
        msgs.send(ready(items)).unwrap();
        app.drain();
        app.open_gallery();
        app.thumbs.insert("/A.HEIC".into(), None);
        msgs.send(Msg::Phone(Phone::Scanning)).unwrap();
        app.drain();
        assert!(app.gallery.is_some(), "пересканирование не закрывает галерею");
        msgs.send(Msg::Phone(Phone::NoDevice)).unwrap();
        app.drain();
        assert!(app.gallery.is_none());
        assert!(!app.thumbs.contains("/A.HEIC"));
    }

    #[test]
    fn import_selected_sends_both_live_halves_and_closes_gallery() {
        let (mut app, msgs, cmds) = app();
        let items = vec![
            Item { files: vec![rf("/A.HEIC", 5), rf("/A.MOV", 2)], imported: true },
            Item { files: vec![rf("/B.JPG", 3)], imported: false },
        ];
        msgs.send(ready(items)).unwrap();
        app.drain();
        app.open_gallery();
        let g = app.gallery.as_mut().unwrap();
        g.selected.clear();
        g.selected.insert("/A.HEIC".into());
        app.import_selected();
        assert!(app.gallery.is_none());
        assert!(matches!(cmds.try_recv(), Ok(Cmd::Import(f)) if f == vec![rf("/A.HEIC", 5), rf("/A.MOV", 2)]));
    }
```

- [ ] **Step 3: Убедиться, что не компилируется**

Run: `cargo test` → `no field ctx/gallery/thumbs`, `no method open_gallery/import_selected`.

- [ ] **Step 4: gallery.rs — декодирование и отрисовка.** `use` заменить на:

```rust
use crate::import::{Item, Kind, RemoteFile, kind_of};
use crate::{BLUE, GRAY, primary, secondary, size};
use eframe::egui::{self, Color32};
use std::collections::{HashMap, HashSet};

/// Сторона плитки и зазор, px.
const TILE: f32 = 120.0;
const GAP: f32 = 6.0;
```

Добавить в конец файла (до `#[cfg(test)]`):

```rust
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
```

Тест `square_uv` — в `mod tests` файла `src/gallery.rs`:

```rust
    #[test]
    fn square_uv_crops_center() {
        let r = square_uv([360, 480]);
        assert_eq!((r.min.x, r.max.x), (0.0, 1.0));
        assert!((r.min.y - 0.125).abs() < 1e-6 && (r.max.y - 0.875).abs() < 1e-6);
        assert_eq!(square_uv([4, 4]), egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)));
    }
```

Если какой-то вызов egui 0.36 называется иначе (`Painter::circle`, `Align2::anchor_size`, `Modal::should_close`), сверить по `~/.cargo/registry/src/*/egui-0.36*/src` и использовать ближайший эквивалент — поведение должно остаться тем же.

- [ ] **Step 5: main.rs — подключение.**

  1. `use`: добавить `use gallery::{Action, Cache, Gallery, Thumb};`
  2. В `struct App` добавить:
     ```rust
         ctx: egui::Context,
         /// Открыт экран галереи.
         gallery: Option<Gallery>,
         /// Миниатюры по ключу плитки; живут, пока подключён тот же телефон.
         thumbs: Cache<Thumb>,
     ```
     и в `App::new`: `ctx: cc.egui_ctx.clone(), gallery: None, thumbs: Cache::new(gallery::CAP),`.
  3. В `drain` ветку `Msg::Phone` заменить на:
     ```rust
                 Msg::Phone(phone) => {
                     match &phone {
                         Phone::Ready { items, .. } => {
                             self.error = None;
                             self.new = items.iter().filter(|i| !i.imported).flat_map(|i| i.files.iter().cloned()).collect();
                             self.new_sum = import::summarize(&self.new);
                             if let Some(g) = &mut self.gallery {
                                 g.retain(items);
                             }
                         }
                         Phone::Scanning => {}
                         // Телефон пропал или сменился.
                         _ => {
                             self.gallery = None;
                             self.thumbs.clear();
                         }
                     }
                     self.importing = None;
                     self.phone = phone;
                 }
     ```
     ветку `Msg::Thumb { .. } => {}` заменить на:
     ```rust
                 Msg::Thumb { path, jpeg } => {
                     let thumb = jpeg.and_then(|jpeg| {
                         let tex = gallery::texture(&self.ctx, &path, &jpeg, 160)?;
                         Some(Thumb { jpeg, tex })
                     });
                     self.thumbs.insert(path, thumb);
                 }
     ```
  4. Методы `App` (после `start`):
     ```rust
         fn open_gallery(&mut self) {
             if let Phone::Ready { items, .. } = &self.phone {
                 self.gallery = Some(Gallery::new(items));
                 self.done = None;
                 // В узком окне помещается три плитки — расширяем.
                 if self.ctx.input(|i| i.viewport().inner_rect).is_some_and(|r| r.width() < 800.0) {
                     self.ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(880.0, 720.0)));
                 }
             }
         }

         fn import_selected(&mut self) {
             let (Some(g), Phone::Ready { items, .. }) = (self.gallery.take(), &self.phone) else { return };
             let files = g.files(items);
             self.start(files);
         }
     ```
  5. В `ui` — первой строкой внутри замыкания `CentralPanel…show(ui, |ui| {`:
     ```rust
                 if let (Some(g), Phone::Ready { items, .. }) = (self.gallery.as_mut(), &self.phone) {
                     let cmds = &self.cmds;
                     let action = gallery::show(ui, g, items, &mut self.thumbs, &mut |keys| {
                         let _ = cmds.send(Cmd::Thumbs(keys));
                     });
                     match action {
                         Action::Back => self.gallery = None,
                         Action::Import => self.import_selected(),
                         Action::None => {}
                     }
                     return;
                 }
     ```
  6. Под главной кнопкой (блок `if matches!(self.phone, Phone::Ready { .. }) { … }` из Task 4) после `if ui.add_enabled(...) { self.start(...) }`:
     ```rust
                     ui.vertical_centered(|ui| {
                         let link = egui::Button::new(RichText::new("Выбрать файлы…").color(BLUE)).frame(false);
                         if ui.add_enabled(!busy, link).clicked() {
                             self.open_gallery();
                         }
                     });
     ```

- [ ] **Step 6: Тесты проходят**

Run: `cargo test` → всё зелёное, без предупреждений `dead_code` про `gallery`.

- [ ] **Step 7: Проверка с телефоном**

Run: `cargo run`. Проверить:
1. «Выбрать файлы…» → окно расширяется, сетка, новые отмечены, миниатюры появляются за доли секунды.
2. Быстрая прокрутка в конец 5 000+ файлов — миниатюры конца появляются без ожидания начала.
3. Live Photo — одна плитка с «LIVE»; видео — «▶»; «Все» показывает импортированные приглушёнными.
4. Двойной клик → увеличение; ←/→, Пробел, Esc работают.
5. Отметить 2 импортированных + 1 новый → «Импортировать 3» → прогресс на главном экране; файлы в папке дня.
6. Выдернуть кабель при открытой галерее → главный экран «iPhone не подключён».

- [ ] **Step 8: Коммит**

```powershell
git add Cargo.toml Cargo.lock src/gallery.rs src/main.rs
git commit -m @'
feat: gallery screen with iOS thumbnails, zoom and per-file selection

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
'@
```

---

### Task 7: Документация и проверка релизной сборки

**Files:**
- Modify: `CLAUDE.md`, `README.md`

- [ ] **Step 1: CLAUDE.md**

  1. В абзаце про проект после «importing only files not already recorded in a journal.» добавить: «A gallery screen shows iOS thumbnails and lets the user pick any files, including already imported ones.» и в ссылку на дизайн добавить `docs/superpowers/specs/2026-10-07-gallery-selection-design.md`.
  2. «Four modules» → «Five modules»; в `device.rs` дописать: «`thumbnail(path)` reads the ready-made iOS JPEG `/PhotoData/Thumbnails/V2<path>/5005.JPG` (~360×480; Live Photo videos have none).»
  3. В `import.rs` дописать: «`group` turns the file list into gallery `Item`s: a Live Photo (photo + `.MOV` with the same path stem) is one item; `imported` = every file is journaled.»
  4. В `worker.rs` дописать: «Serves `Cmd::Thumbs` from a queue that each new request replaces; while the queue is non-empty it polls commands with `try_recv` instead of waiting 2 s. `Cmd::Import` carries the exact file list.»
  5. Новый пункт: «`gallery.rs` — gallery screen state (`Gallery`: filter, selected item keys, zoom), LRU `Cache` of thumbnail textures (≤1 500), drawing. Decodes JPEG with `image` (jpeg-only, pure Rust).»
  6. В `main.rs` дописать: «A non-`Ready`/`Scanning` `Msg::Phone` also closes the gallery and clears the thumbnail cache.»
  7. Строку `Out of scope` заменить на: «Out of scope by design: deleting from phone, HEIC/HEVC conversion, decoding originals for preview, video playback, Wi-Fi, WPD fallback, macOS/Linux.»

- [ ] **Step 2: README.md**

  - Строку 29 («Чтобы скопировать всё ещё раз… «Скопировать всё заново…»…») заменить на: «Чтобы выбрать отдельные файлы — в том числе уже импортированные — нажмите «Выбрать файлы…»: откроется галерея с миниатюрами. Щелчок отмечает файл, двойной щелчок — увеличивает; Live Photo — одна плитка (фото и видео вместе).»
  - В строке 109 убрать «превью и выбор отдельных файлов, ».

- [ ] **Step 3: Релизная сборка и DLL**

Run (PowerShell):
```powershell
cargo test
cargo build --release
objdump -p target\release\iphone-importer.exe | Select-String 'DLL Name'
```
Expected: тесты зелёные; в списке DLL нет `libgcc*`, `libwinpthread*`, `libstdc++*`.

- [ ] **Step 4: Аппаратные тесты**

Run: `cargo test -- --ignored --nocapture` → 2 passed (`lists_and_reads_from_real_iphone`, `reads_thumbnail_from_real_iphone`).

- [ ] **Step 5: Коммит**

```powershell
git add CLAUDE.md README.md
git commit -m @'
docs: gallery and per-file selection

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
'@
```
