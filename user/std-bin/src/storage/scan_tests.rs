use super::*;
use std::sync::atomic::AtomicUsize;
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        // macOS /tmp itself is a symlink. The scanner intentionally rejects it.
        let base = fs::canonicalize(std::env::temp_dir()).unwrap();
        let path = base.join(format!(
            "storage-scan-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn scan(&self, limits: Limits) -> Snapshot {
        scan(&self.0, &AtomicBool::new(false), limits).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn ranks_recursive_children_and_keeps_exact_cached_totals() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("large")).unwrap();
    fs::create_dir(f.0.join("empty")).unwrap();
    fs::write(f.0.join("large/one"), [0; 900]).unwrap();
    fs::write(f.0.join("small"), [0; 100]).unwrap();
    let result = f.scan(Limits::default());
    let root = &result.nodes[0];
    assert_eq!((root.bytes, root.files), (1000, 2));
    assert!(root.complete);
    assert_eq!(result.nodes[root.children[0]].name(), "large");
    assert_eq!(result.nodes[root.children[1]].name(), "small");
    let child = &result.nodes[root.children[0]];
    assert_eq!(child.bytes, 900);
    assert_eq!(result.nodes[child.children[0]].bytes, 900);
    assert_eq!(child.parent, Some(0));
}
#[cfg(unix)]
#[test]
fn skips_cycle_external_and_broken_symlinks_and_rejects_link_roots() {
    let f = Fixture::new();
    fs::write(f.0.join("data"), b"four").unwrap();
    std::os::unix::fs::symlink(&f.0, f.0.join("cycle")).unwrap();
    std::os::unix::fs::symlink("/", f.0.join("external")).unwrap();
    std::os::unix::fs::symlink("missing", f.0.join("broken")).unwrap();
    let result = f.scan(Limits::default());
    assert_eq!((result.nodes[0].bytes, result.nodes[0].links), (4, 3));
    assert!(result.nodes[0].complete);
    assert!(
        scan(
            &f.0.join("cycle"),
            &AtomicBool::new(false),
            Limits::default()
        )
        .is_err()
    );
    assert!(
        scan(
            &f.0.join("cycle/cycle"),
            &AtomicBool::new(false),
            Limits::default()
        )
        .is_err()
    );
}
#[test]
fn empty_folder_is_complete_but_missing_folder_and_file_are_errors() {
    let f = Fixture::new();
    let result = f.scan(Limits::default());
    assert!(result.nodes[0].complete);
    assert_eq!(result.nodes[0].bytes, 0);
    assert!(result.nodes[0].children.is_empty());
    fs::write(f.0.join("file"), b"").unwrap();
    for path in [f.0.join("missing"), f.0.join("file")] {
        assert!(scan(&path, &AtomicBool::new(false), Limits::default()).is_err());
    }
}
#[test]
fn item_depth_memory_and_time_limits_are_explicit_partial_results() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("one")).unwrap();
    fs::create_dir(f.0.join("one/two")).unwrap();
    fs::write(f.0.join("one/two/data"), b"abc").unwrap();
    let result = f.scan(Limits {
        nodes: 2,
        ..Limits::default()
    });
    assert_eq!(result.nodes.len(), 2);
    assert!(!result.nodes[0].complete);
    assert!(result.stopped.unwrap().contains("Item limit"));
    let result = f.scan(Limits {
        depth: 1,
        ..Limits::default()
    });
    assert!(!result.nodes[0].complete);
    assert!(
        result.nodes[1]
            .issue
            .as_ref()
            .unwrap()
            .contains("Depth limit")
    );
    let result = f.scan(Limits {
        path_bytes: f.0.as_os_str().len(),
        ..Limits::default()
    });
    assert_eq!(result.nodes.len(), 1);
    assert!(!result.nodes[0].complete);
    assert!(result.stopped.unwrap().contains("memory limit"));
    let result = f.scan(Limits {
        time: Duration::ZERO,
        ..Limits::default()
    });
    assert!(!result.nodes[0].complete);
    assert!(result.stopped.unwrap().contains("Time limit"));
}

struct Faults<'a> {
    denied: &'a Path,
    cancel_on: Option<(&'a Path, &'a AtomicBool)>,
}
impl Source for Faults<'_> {
    fn metadata(&self, path: &Path) -> io::Result<(Kind, u64)> {
        if let Some((trigger, cancelled)) = self.cancel_on {
            if trigger == path {
                cancelled.store(true, Ordering::Relaxed);
            }
        }
        System.metadata(path)
    }
    fn entries(&self, path: &Path) -> io::Result<Box<dyn Iterator<Item = io::Result<PathBuf>>>> {
        if path == self.denied {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Access denied",
            ));
        }
        System.entries(path)
    }
}
#[test]
fn unreadable_child_is_not_reported_as_an_empty_complete_folder() {
    let f = Fixture::new();
    let denied = f.0.join("private");
    fs::create_dir(&denied).unwrap();
    fs::write(f.0.join("readable"), b"1234").unwrap();
    let result = scan_with(
        &Faults {
            denied: &denied,
            cancel_on: None,
        },
        &f.0,
        &AtomicBool::new(false),
        Limits::default(),
    )
    .unwrap();
    assert_eq!(result.nodes[0].bytes, 4);
    assert!(!result.nodes[0].complete);
    assert_eq!(result.errors, 1);
    let child = result.nodes.iter().find(|n| n.path == denied).unwrap();
    assert!(!child.complete);
    assert_eq!(child.issue.as_deref(), Some("Access denied"));
}
#[test]
fn cancellation_keeps_partial_results_and_marks_all_ancestors() {
    let f = Fixture::new();
    let trigger = f.0.join("nested");
    fs::create_dir(&trigger).unwrap();
    fs::write(trigger.join("file"), b"abc").unwrap();
    let cancelled = AtomicBool::new(false);
    let result = scan_with(
        &Faults {
            denied: Path::new("/not-used"),
            cancel_on: Some((&trigger, &cancelled)),
        },
        &f.0,
        &cancelled,
        Limits::default(),
    )
    .unwrap();
    assert_eq!(result.stopped.as_deref(), Some("Scan cancelled"));
    assert!(!result.nodes[0].complete);
    assert_eq!(result.nodes[0].bytes, 0);
    assert!(scan(&f.0, &AtomicBool::new(true), Limits::default()).is_err());
}
#[test]
fn hard_links_and_sparse_files_are_logical_bytes_per_path() {
    let f = Fixture::new();
    let file = fs::File::create(f.0.join("sparse")).unwrap();
    file.set_len(1024 * 1024).unwrap();
    fs::hard_link(f.0.join("sparse"), f.0.join("another-name")).unwrap();
    let result = f.scan(Limits::default());
    assert_eq!(result.nodes[0].bytes, 2 * 1024 * 1024);
    assert_eq!(size(result.nodes[0].bytes), "2.0 MiB");
}
