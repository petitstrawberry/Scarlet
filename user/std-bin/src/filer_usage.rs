//! Bounded, non-following folder size inspection for Files.
use crate::filer_operations::native_fs as fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const MAX_ITEMS: usize = 100_000;
const MAX_DEPTH: usize = 64;
const MAX_TIME: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Default)]
pub struct Usage {
    /// Logical bytes of regular files; links are never followed.
    pub bytes: u64,
    pub files: usize,
    pub folders: usize,
    pub links: usize,
    pub skipped: usize,
    pub incomplete: Option<String>,
    /// Largest immediate children, measured recursively.
    pub largest: Vec<(String, u64)>,
}

pub fn measure(path: &Path, cancelled: &AtomicBool) -> Result<Usage, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    let mut walker = Walker {
        usage: Usage::default(),
        visited: 0,
        start: Instant::now(),
        cancelled,
    };
    if metadata.is_dir() {
        walker.usage.folders = 1;
        for entry in fs::read_dir(path).map_err(|error| error.to_string())? {
            if !walker.available(0) {
                break;
            }
            match entry {
                Ok(entry) => {
                    let before = walker.usage.bytes;
                    walker.visit(&entry.path(), 1);
                    walker.usage.largest.push((
                        entry.file_name().to_string_lossy().into_owned(),
                        walker.usage.bytes.saturating_sub(before),
                    ));
                    walker
                        .usage
                        .largest
                        .sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                    walker.usage.largest.truncate(20);
                }
                Err(error) => walker.skip(error.to_string()),
            }
        }
    } else {
        walker.visit(path, 0);
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err("Size calculation cancelled".into());
    }
    Ok(walker.usage)
}

struct Walker<'a> {
    usage: Usage,
    visited: usize,
    start: Instant,
    cancelled: &'a AtomicBool,
}
impl Walker<'_> {
    fn available(&mut self, depth: usize) -> bool {
        if self.cancelled.load(Ordering::Relaxed) {
            return false;
        }
        let reason = if self.visited >= MAX_ITEMS {
            Some("Stopped at 100,000 items")
        } else if self.start.elapsed() >= MAX_TIME {
            Some("Stopped after 10 seconds")
        } else if depth > MAX_DEPTH {
            Some("Skipped folders deeper than 64 levels")
        } else {
            None
        };
        if let Some(reason) = reason {
            self.usage.incomplete.get_or_insert_with(|| reason.into());
            false
        } else {
            true
        }
    }
    fn skip(&mut self, error: String) {
        self.usage.skipped += 1;
        self.usage.incomplete.get_or_insert(error);
    }
    fn visit(&mut self, path: &Path, depth: usize) {
        if !self.available(depth) {
            return;
        }
        self.visited += 1;
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                self.skip(error.to_string());
                return;
            }
        };
        if metadata.file_type().is_symlink() {
            self.usage.links += 1;
        } else if metadata.is_file() {
            self.usage.files += 1;
            match self.usage.bytes.checked_add(metadata.len()) {
                Some(bytes) => self.usage.bytes = bytes,
                None => self.skip("Size exceeds supported range".into()),
            }
        } else if metadata.is_dir() {
            self.usage.folders += 1;
            match fs::read_dir(path) {
                Ok(entries) => {
                    for entry in entries {
                        if !self.available(depth + 1) {
                            break;
                        }
                        match entry {
                            Ok(entry) => self.visit(&entry.path(), depth + 1),
                            Err(error) => self.skip(error.to_string()),
                        }
                    }
                }
                Err(error) => self.skip(error.to_string()),
            }
        } else {
            self.skip("Special files are excluded".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::AtomicUsize;
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "scarlet-usage-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn measures_nested_logical_sizes_and_largest_children() {
        let f = Fixture::new();
        fs::create_dir(f.0.join("nested")).unwrap();
        fs::write(f.0.join("small"), b"123").unwrap();
        fs::write(f.0.join("nested/large"), b"1234567").unwrap();
        let result = measure(&f.0, &AtomicBool::new(false)).unwrap();
        assert_eq!(result.bytes, 10);
        assert_eq!(result.files, 2);
        assert_eq!(result.folders, 2);
        assert_eq!(result.largest[0], ("nested".into(), 7));
        assert!(result.incomplete.is_none());
    }
    #[cfg(unix)]
    #[test]
    fn does_not_follow_cycles_or_external_links() {
        let f = Fixture::new();
        fs::write(f.0.join("data"), b"safe").unwrap();
        std::os::unix::fs::symlink(&f.0, f.0.join("cycle")).unwrap();
        std::os::unix::fs::symlink("missing", f.0.join("broken")).unwrap();
        let result = measure(&f.0, &AtomicBool::new(false)).unwrap();
        assert_eq!(result.bytes, 4);
        assert_eq!(result.links, 2);
        assert!(result.incomplete.is_none());
    }
    #[test]
    fn cancellation_and_invalid_paths_are_reported() {
        let f = Fixture::new();
        assert!(measure(&f.0, &AtomicBool::new(true)).is_err());
        assert!(measure(&f.0.join("missing"), &AtomicBool::new(false)).is_err());
    }
}
