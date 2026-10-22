//! Bounded directory enumeration and sorting, independent of the UI renderer.

use std::cmp::Reverse;
use std::fs;
use std::io;

pub const MAX_DIRECTORY_ENTRIES: usize = 20_000;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size: u64,
}

pub fn read_directory(
    path: &str,
    query: &str,
    show_hidden: bool,
    sort: usize,
) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let query = query.to_lowercase();
    for (scanned, entry) in fs::read_dir(path)?.enumerate() {
        if scanned >= MAX_DIRECTORY_ENTRIES {
            return Err(io::Error::other(
                "This folder exceeds the 20,000 item browsing limit",
            ));
        }
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|_| {
            io::Error::other("This folder contains a file name that is not valid UTF-8")
        })?;
        if (!show_hidden && name.starts_with('.'))
            || (!query.is_empty() && !name.to_lowercase().contains(&query))
        {
            continue;
        }
        let file_type = entry.file_type()?;
        let is_directory = file_type.is_dir() || (file_type.is_symlink() && entry.path().is_dir());
        let size = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        let path = entry
            .path()
            .into_os_string()
            .into_string()
            .map_err(|_| io::Error::other("The folder path is not valid UTF-8"))?;
        entries.push(Entry {
            name,
            path,
            is_directory,
            size,
        });
    }
    sort_entries(&mut entries, sort);
    Ok(entries)
}

fn sort_entries(entries: &mut [Entry], sort: usize) {
    // Cache case-folded names once, rather than allocating at every comparison.
    match sort {
        1 => entries.sort_by_cached_key(|entry| {
            (
                !entry.is_directory,
                Reverse(entry.name.to_lowercase()),
                entry.name.clone(),
            )
        }),
        2 => entries.sort_by_cached_key(|entry| {
            (
                !entry.is_directory,
                Reverse(entry.size),
                entry.name.to_lowercase(),
                entry.name.clone(),
            )
        }),
        _ => entries.sort_by_cached_key(|entry| {
            (
                !entry.is_directory,
                entry.name.to_lowercase(),
                entry.name.clone(),
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "scarlet-files-list-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::create_dir(path.join("Zoo")).unwrap();
            fs::write(path.join("beta.txt"), b"123456").unwrap();
            fs::write(path.join("Alpha.txt"), b"1").unwrap();
            fs::write(path.join(".hidden"), b"secret").unwrap();
            Self(path)
        }
        fn list(&self, query: &str, hidden: bool, sort: usize) -> Vec<String> {
            read_directory(self.0.to_str().unwrap(), query, hidden, sort)
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    #[ignore = "manual release-mode directory sorting benchmark"]
    fn benchmark_large_directory_sorting() {
        use std::time::{Duration, Instant};
        let entries: Vec<_> = (0..20_000usize)
            .map(|number| {
                let shuffled = (number * 7919) % 20_000;
                Entry {
                    name: format!("Document-メモ-{:05}-long mixed CASE.txt", shuffled),
                    path: String::new(),
                    is_directory: shuffled % 11 == 0,
                    size: shuffled as u64,
                }
            })
            .collect();
        let mut before = Vec::new();
        let mut after = Vec::new();
        for _ in 0..7 {
            let mut baseline = entries.clone();
            let mut improved = entries.clone();
            let start = Instant::now();
            baseline.sort_by(|a, b| {
                b.is_directory
                    .cmp(&a.is_directory)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                    .then_with(|| a.name.cmp(&b.name))
            });
            before.push(start.elapsed());
            let start = Instant::now();
            sort_entries(&mut improved, 0);
            after.push(start.elapsed());
            assert!(
                baseline
                    .iter()
                    .zip(&improved)
                    .all(|(a, b)| a.name == b.name)
            );
            std::hint::black_box(improved);
        }
        before.sort();
        after.sort();
        let median = |times: &[Duration]| times[times.len() / 2].as_secs_f64() * 1000.;
        println!(
            "20,000 entries, seven runs, median sort: baseline {:.3} ms, cached {:.3} ms ({:.2}x); output identical",
            median(&before),
            median(&after),
            median(&before) / median(&after)
        );
    }

    #[test]
    fn sorts_folders_first_and_names_case_insensitively() {
        let fixture = Fixture::new();
        assert_eq!(fixture.list("", false, 0), ["Zoo", "Alpha.txt", "beta.txt"]);
        assert_eq!(fixture.list("", false, 1), ["Zoo", "beta.txt", "Alpha.txt"]);
        assert_eq!(fixture.list("", false, 2), ["Zoo", "beta.txt", "Alpha.txt"]);
    }

    #[test]
    fn filters_names_and_can_show_hidden_files() {
        let fixture = Fixture::new();
        assert_eq!(fixture.list("ALPHA", false, 0), ["Alpha.txt"]);
        assert!(fixture.list("hidden", false, 0).is_empty());
        assert_eq!(fixture.list("hidden", true, 0), [".hidden"]);
        assert!(fixture.list("missing", true, 0).is_empty());
    }

    #[test]
    fn reports_unreadable_locations_without_fabricating_empty_success() {
        let fixture = Fixture::new();
        assert!(read_directory(fixture.0.join("missing").to_str().unwrap(), "", false, 0).is_err());
        assert!(
            read_directory(fixture.0.join("Alpha.txt").to_str().unwrap(), "", false, 0).is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn folder_links_are_browsable_and_broken_links_stay_visible() {
        let fixture = Fixture::new();
        std::os::unix::fs::symlink("Zoo", fixture.0.join("Folder Link")).unwrap();
        std::os::unix::fs::symlink("missing", fixture.0.join("Broken Link")).unwrap();
        let entries = read_directory(fixture.0.to_str().unwrap(), "link", false, 0).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "Folder Link");
        assert!(entries[0].is_directory);
        assert!(!entries[1].is_directory);
    }
}
