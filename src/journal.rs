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
