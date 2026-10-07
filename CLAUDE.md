# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Windows-only desktop app (Rust + egui) that copies original photos/videos from an iPhone over USB into `<dest>\YYYY-MM-DD\Фото|Видео\` (date = when the import started, not EXIF date), by default importing items not yet fully recorded in a journal (a half-imported Live Photo is re-imported whole; the already-copied half gets a `(n)` name — accepted by the spec). A gallery screen shows iOS thumbnails and lets the user pick any files, including already imported ones. Design and rationale: `docs/superpowers/specs/2026-10-07-iphone-importer-design.md`, `docs/superpowers/specs/2026-10-07-gallery-selection-design.md`.

## Build & test

- Toolchain is pinned to `stable-x86_64-pc-windows-gnu` (`rust-toolchain.toml`) — no Visual Studio Build Tools. Needs MinGW (WinLibs) on PATH: `winget install BrechtSanders.WinLibs.POSIX.UCRT --scope user`. `dlltool.exe: program not found` → open a new terminal.
- Run `cargo` from **PowerShell**, not Git Bash (there `link` resolves to GNU coreutils).
- `cargo test` — all unit tests (no phone needed). Single test: `cargo test run_cancel_mid_file`; by module: `cargo test journal`.
- `cargo test -- --ignored --nocapture` — hardware test in `device.rs`; needs a connected, unlocked, trusted iPhone.
- `cargo build --release` — single `.exe`. Check it links only system DLLs: `objdump -p target\release\iphone-importer.exe | Select-String 'DLL Name'` (no `libgcc*`, `libwinpthread*`, `libstdc++*`).
- Runtime dependency for users: Apple Devices app (Microsoft Store), which provides usbmuxd on `127.0.0.1:27015`.

## Architecture

Five modules, one crate:

- `device.rs` — usbmuxd → first **USB** device (the phone also appears via Wi-Fi; ignored) → AFC. Wraps async `idevice` in a blocking API, each `Device` owning its own current-thread tokio `Runtime`. Lists `/DCIM/*/*`, reads files in 1 MB chunks, must explicitly `close()` AFC handles. `classify` maps fetch errors: AFC/NotFound → skip this file (`FetchError::File`), anything else → connection lost (`FetchError::Connection`). At connect time, a failed pairing-file lookup or InvalidHostID/DeviceLocked/NotFound from AFC means `NotTrusted` (so NotFound means different things at connect and at fetch). `thumbnail(path)` reads the ready-made iOS JPEG `/PhotoData/Thumbnails/V2<path>/5005.JPG` (~360×480; Live Photo videos have none); errors go through the same `classify`: a per-file error → `Ok(None)` (cached as "no thumbnail"), anything else → `Err`, and the worker drops the device and reconnects.
- `import.rs` — knows nothing about the iPhone; the byte source is the `fetch` closure, so the copy loop `run` is fully tested with fake closures. The pre-flight steps (free-space check, `clean_parts`) are called from `worker::import_now`, not from `run`. `group` turns the file list into gallery `Item`s: a Live Photo (photo + `.MOV` with the same path stem) is one item; `imported` = every file is journaled.
- `journal.rs` — `<dest>\.import-log`, one `phone_path\tsize` line per imported file, `sync_data` after each record. Identity of a file = (path on phone, size).
- `worker.rs` — background thread: polls for the device every 2 s, rescans after each import, runs imports. Talks to GUI over `mpsc` (`Cmd` in, `Msg` out) and calls `ctx.request_repaint()` on every message. Cancel is a shared `AtomicBool`. Serves `Cmd::Thumbs` from a queue that each new request replaces; while the queue is non-empty it polls commands with a zero timeout instead of waiting 2 s. `Gallery::retain` (on every rescan) resets the last request, so thumbnails lost with a dropped queue are re-requested. `Cmd::Import` carries the exact file list.
- `gallery.rs` — gallery screen state (`Gallery`: filter, selected item keys, zoom), LRU `Cache` of thumbnail textures (≤1 500), drawing. Decodes JPEG with `image` (jpeg-only, pure Rust).
- `main.rs` — egui `App`; `drain()` folds `Msg`s into UI state. Any `Msg::Phone` or `Msg::Error` clears `importing` (the worker only sends Phone states when not importing, and `import_now` can fail before the copy starts — this keeps the GUI from sticking in "busy"). `App::start` resets `cancel` to false *before* sending `Cmd::Import`; reversing that order loses a cancel. Chosen destination persists in `%APPDATA%\iphone-importer\config.txt`. A non-`Ready`/`Scanning` `Msg::Phone`, or a `Ready` with a different udid (phone swapped between polls), also closes the gallery and clears the thumbnail cache.

### Integrity invariants (don't break these)

- Copy goes to `name.part`, then `fsync` → rename to a free name → journal record. A journal line exists **only** after a successful rename: a file without a journal entry is acceptable (re-copied next time), a journal entry without a file is not.
- Existing files are never overwritten. This relies solely on `free_path` checking `exists()` — `fs::rename` on Windows replaces the target.
- Connection loss / cancel / disk error stop the whole run and delete the current `.part`; a per-file read error or size mismatch skips that file (not journaled) and continues. Resume is implicit via the journal; stale `.part` files in the day folder are removed before each import.
- Copying is deliberately sequential — USB is the bottleneck.

## Conventions

- UI strings, comments, and doc comments are in Russian; keep it that way.
- Out of scope by design: deleting from phone, HEIC/HEVC conversion, decoding originals for preview, video playback, Wi-Fi, WPD fallback, macOS/Linux.
