use super::scan::{Kind, Snapshot};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Default)]
pub struct Browser {
    pub root: Option<PathBuf>,
    pub snapshot: Option<Arc<Snapshot>>,
    pub current: usize,
    pub generation: u64,
    pub busy: bool,
    pub cancelling: bool,
    pub error: Option<String>,
}
impl Browser {
    pub fn begin(&mut self, root: PathBuf, generation: u64) {
        if self.root.as_ref() != Some(&root) {
            self.snapshot = None;
            self.current = 0;
        }
        self.root = Some(root);
        self.generation = generation;
        self.busy = true;
        self.cancelling = false;
        self.error = None;
    }
    pub fn accept(&mut self, generation: u64, result: Result<Snapshot, String>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.busy = false;
        self.cancelling = false;
        match result {
            Ok(snapshot) => {
                // Preserve the inspected directory after Refresh when it remains present.
                let previous = self.snapshot.as_ref().map(|s| &s.nodes[self.current].path);
                self.current = previous
                    .and_then(|path| {
                        snapshot
                            .nodes
                            .iter()
                            .position(|n| n.kind == Kind::Directory && &n.path == path)
                    })
                    .unwrap_or(0);
                self.snapshot = Some(Arc::new(snapshot));
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        true
    }
    pub fn enter(&mut self, id: usize) -> bool {
        let Some(snapshot) = &self.snapshot else {
            return false;
        };
        if snapshot
            .nodes
            .get(id)
            .is_none_or(|n| n.kind != Kind::Directory || n.parent != Some(self.current))
        {
            return false;
        }
        self.current = id;
        true
    }
    pub fn back(&mut self) -> bool {
        let Some(parent) = self
            .snapshot
            .as_ref()
            .and_then(|s| s.nodes[self.current].parent)
        else {
            return false;
        };
        self.current = parent;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{Limits, scan};
    use std::sync::atomic::AtomicBool;
    #[test]
    fn stale_results_do_not_replace_the_latest_folder() {
        let mut browser = Browser::default();
        browser.begin(PathBuf::from("/first"), 1);
        browser.begin(PathBuf::from("/second"), 2);
        assert!(!browser.accept(1, Err("stale failure".into())));
        assert!(browser.busy);
        assert!(browser.error.is_none());
        assert!(browser.accept(2, Err("current failure".into())));
        assert!(!browser.busy);
        assert_eq!(browser.root.unwrap(), PathBuf::from("/second"));
    }
    #[test]
    fn drill_down_and_back_use_snapshot_after_folder_is_removed() {
        let path = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("storage-cache-{}", std::process::id()));
        std::fs::create_dir_all(path.join("nested")).unwrap();
        std::fs::write(path.join("nested/data"), "123456").unwrap();
        let result = scan(&path, &AtomicBool::new(false), Limits::default()).unwrap();
        std::fs::remove_dir_all(&path).unwrap();
        let mut browser = Browser::default();
        browser.begin(path, 1);
        browser.accept(1, Ok(result));
        assert!(browser.enter(1));
        assert_eq!(
            browser.snapshot.as_ref().unwrap().nodes[browser.current].bytes,
            6
        );
        assert!(!browser.enter(2)); // regular file
        assert!(browser.back());
        assert_eq!(browser.current, 0);
        assert!(!browser.back());
    }
}
