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
