//! Document transitions and filesystem operations, independent of the UI.
#[path = "../desktop_utilities/fs_sync.rs"]
mod fs_sync;
#[path = "../desktop_utilities/native_fs.rs"]
mod native_fs;
use std::fs;
use std::io::{self, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingAction {
    New,
    Open,
    Close,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    OpenPicker,
    SavePicker,
    Read,
    Save,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub id: u64,
    pub document: u64,
    pub revision: u64,
    pub version: u64,
}
#[derive(Clone, Debug, Default)]
pub struct Session {
    pub path: Option<PathBuf>,
    pub revision: u64,
    pub saved_revision: u64,
    pub content_version: u64,
    saved_version: u64,
    pub pending: Option<PendingAction>,
    pub operation: Option<(Ticket, Operation)>,
    document: u64,
    next_request: u64,
}
impl Session {
    pub fn identity(&self) -> (u64, u64) {
        (self.document, self.revision)
    }
    pub fn dirty(&self) -> bool {
        self.content_version != self.saved_version
    }
    pub fn edited(&mut self) {
        self.revision += 1;
        self.content_version = self.revision;
    }
    pub fn restored(&mut self, version: u64) {
        self.revision += 1;
        self.content_version = version;
    }
    pub fn begin(&mut self, operation: Operation) -> Option<Ticket> {
        if self.operation.is_some() {
            return None;
        }
        self.next_request += 1;
        let ticket = Ticket {
            id: self.next_request,
            document: self.document,
            revision: self.revision,
            version: self.content_version,
        };
        self.operation = Some((ticket, operation));
        Some(ticket)
    }
    pub fn finish(&mut self, ticket: Ticket) -> bool {
        if !self.operation.is_some_and(|(active, _)| active == ticket) {
            return false;
        }
        self.operation = None;
        ticket.document == self.document
    }
    pub fn unchanged(&self, ticket: Ticket) -> bool {
        ticket.document == self.document && ticket.revision == self.revision
    }
    pub fn loaded(&mut self, path: Option<PathBuf>) {
        self.document += 1;
        self.path = path;
        self.revision = 0;
        self.saved_revision = 0;
        self.content_version = 0;
        self.saved_version = 0;
        self.operation = None;
        self.pending = None;
    }
    pub fn saved(&mut self, ticket: Ticket, path: PathBuf) {
        if ticket.document == self.document {
            self.path = Some(path);
            self.saved_revision = ticket.revision;
            self.saved_version = ticket.version;
        }
    }
    pub fn request(&mut self, action: PendingAction) -> bool {
        if self.operation.is_some() || self.pending.is_some() {
            return false;
        }
        self.pending = Some(action);
        true
    }
    pub fn cancel(&mut self) {
        self.pending = None;
    }
    pub fn take_ready_action(&mut self) -> Option<PendingAction> {
        if self.dirty() || self.operation.is_some() {
            None
        } else {
            self.pending.take()
        }
    }
}

/// Stage replacements beside the destination; reserve new files exclusively.
/// New files are visible while writing because native no-replace rename is unavailable.
/// Existing symlinks keep their identity; save their resolved target instead.
pub fn save_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let target = match native_fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => native_fs::canonicalize(path)?,
        Ok(_) => path.to_path_buf(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => path.to_path_buf(),
        Err(e) => return Err(e),
    };
    save_staged(&target, |file| {
        file.write_all(contents)?;
        fs_sync::sync_file(file)
    })
}

fn save_staged(path: &Path, write: impl FnOnce(&mut fs::File) -> io::Result<()>) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let metadata = match native_fs::metadata(path) {
        Ok(meta) if meta.is_file() => Some(meta),
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination is not a regular file",
            ));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    if metadata.is_none() {
        let mut file = native_fs::create_new(path)?;
        let result = write(&mut file);
        drop(file);
        return finish_or_cleanup(result, path);
    }
    let mut temporary = None;
    for _ in 0..128 {
        let candidate = parent.join(format!(
            ".notepad-save-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let permissions = metadata.as_ref().map(|meta| meta.permissions());
        match native_fs::create_new_with_permissions(&candidate, permissions.as_ref()) {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    let (temporary, mut file) = temporary.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "cannot allocate a temporary save file",
        )
    })?;
    let result = (|| {
        write(&mut file)?;
        drop(file);
        native_fs::rename(&temporary, path)
    })();
    finish_or_cleanup(result, &temporary)
}
fn finish_or_cleanup(result: io::Result<()>, path: &Path) -> io::Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(error) => match native_fs::remove_file(path) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(io::Error::other(format!(
                "{error}; partial output remains at {}: {cleanup}",
                path.display()
            ))),
        },
    }
}

fn boundary(text: &str, byte: usize) -> usize {
    let mut byte = byte.min(text.len());
    while !text.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}
/// Literal, case-sensitive UTF-8 search with one wrap; empty queries never match.
pub fn find(text: &str, query: &str, start: usize, backwards: bool) -> Option<Range<usize>> {
    if query.is_empty() {
        return None;
    }
    let start = boundary(text, start);
    let offset = if backwards {
        text[..start]
            .rfind(query)
            .or_else(|| text[start..].rfind(query).map(|n| start + n))
    } else {
        text[start..]
            .find(query)
            .map(|n| start + n)
            .or_else(|| text[..start].find(query))
    }?;
    Some(offset..offset + query.len())
}
pub fn replace_all(text: &str, query: &str, replacement: &str) -> Option<(String, usize)> {
    if query.is_empty() {
        return None;
    }
    let count = text.matches(query).count();
    if count == 0 {
        None
    } else {
        Some((text.replace(query, replacement), count))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "scarlet-notepad-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn save_and_cancel_preserve_pending_document() {
        for action in [
            PendingAction::New,
            PendingAction::Open,
            PendingAction::Close,
        ] {
            let mut s = Session::default();
            s.edited();
            assert!(s.request(action));
            assert_eq!(s.take_ready_action(), None);
            s.cancel();
            assert!(s.dirty());
            assert_eq!(s.pending, None);
            assert!(s.request(action));
            let t = s.begin(Operation::Save).unwrap();
            assert!(!s.request(PendingAction::New));
            assert!(s.finish(t));
            s.saved(t, "a.txt".into());
            assert_eq!(s.take_ready_action(), Some(action));
        }
    }
    #[test]
    fn newer_edits_are_not_marked_saved_or_discarded() {
        let mut s = Session::default();
        s.edited();
        s.request(PendingAction::Close);
        let t = s.begin(Operation::Save).unwrap();
        s.edited();
        assert!(s.finish(t));
        s.saved(t, "a.txt".into());
        assert!(s.dirty());
        assert_eq!(s.take_ready_action(), None);
    }
    #[test]
    fn failed_save_and_cancelled_save_as_keep_dirty() {
        let mut s = Session::default();
        s.edited();
        s.request(PendingAction::Open);
        let t = s.begin(Operation::SavePicker).unwrap();
        assert!(s.finish(t));
        s.cancel();
        assert!(s.dirty());
        assert_eq!(s.path, None);
        s.request(PendingAction::Open);
        let t = s.begin(Operation::Save).unwrap();
        assert!(s.finish(t));
        assert!(s.dirty());
        assert_eq!(s.take_ready_action(), None);
    }
    #[test]
    fn stale_and_duplicate_results_do_not_finish_new_operation() {
        let mut s = Session::default();
        let old = s.begin(Operation::OpenPicker).unwrap();
        assert!(s.finish(old));
        let new = s.begin(Operation::Read).unwrap();
        assert!(!s.finish(old));
        assert_eq!(s.operation.unwrap().0, new);
        s.edited();
        assert!(!s.unchanged(new));
        s.loaded(None);
        assert!(!s.finish(new));
    }
    #[test]
    fn duplicate_picker_requests_are_rejected() {
        let mut s = Session::default();
        let t = s.begin(Operation::OpenPicker).unwrap();
        assert_eq!(s.begin(Operation::SavePicker), None);
        assert_eq!(s.operation.unwrap().0, t);
    }
    #[test]
    fn unicode_search_wrap_and_replacement() {
        let text = "猫🐈\n猫 café";
        assert_eq!(find(text, "猫", 3, false), Some(8..11));
        assert_eq!(find(text, "猫", text.len(), false), Some(0..3));
        assert_eq!(find(text, "猫", 0, true), Some(8..11));
        assert_eq!(find(text, "🐈", 4, false), Some(3..7));
        assert_eq!(find(text, "", 0, false), None);
        assert_eq!(
            replace_all(text, "猫", "犬").unwrap(),
            ("犬🐈\n犬 café".into(), 2)
        );
        assert_eq!(replace_all(text, "", "x"), None);
        assert_eq!(replace_all(text, "absent", "x"), None);
    }
    #[test]
    fn staged_save_replaces_utf8_and_cleans_temporary() {
        let f = Fixture::new();
        let p = f.0.join("note.txt");
        fs::write(&p, "old").unwrap();
        save_atomic(&p, "猫🐈".as_bytes()).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "猫🐈");
        assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
        save_atomic(&p, b"").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"");
    }
    #[test]
    fn write_failure_preserves_original_and_cleans_temporary() {
        let f = Fixture::new();
        let p = f.0.join("note.txt");
        fs::write(&p, "original").unwrap();
        assert!(
            save_staged(&p, |file| {
                file.write_all(b"partial")?;
                Err(io::Error::other("injected full disk"))
            })
            .is_err()
        );
        assert_eq!(fs::read_to_string(&p).unwrap(), "original");
        assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
    }
    #[test]
    fn rename_failure_preserves_original_and_cleans_temporary() {
        let f = Fixture::new();
        let p = f.0.join("note.txt");
        fs::write(&p, b"old").unwrap();
        assert!(
            save_staged(&p, |file| {
                file.write_all(b"new")?;
                fs::remove_file(&p)?;
                fs::create_dir(&p)
            })
            .is_err()
        );
        assert!(p.is_dir());
        assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
    }
    #[test]
    fn new_save_reserves_destination_and_cleans_failed_output() {
        let f = Fixture::new();
        let path = f.0.join("new.txt");
        assert!(
            save_staged(&path, |file| {
                assert_eq!(
                    native_fs::create_new(&path).unwrap_err().kind(),
                    io::ErrorKind::AlreadyExists
                );
                file.write_all(b"partial")?;
                Err(io::Error::other("injected disk full"))
            })
            .is_err()
        );
        assert!(!path.exists());
        save_atomic(&path, b"complete").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"complete");
    }
    #[test]
    fn missing_directory_and_non_file_destinations_fail_safely() {
        let f = Fixture::new();
        assert!(save_atomic(&f.0.join("missing/note.txt"), b"new").is_err());
        assert!(save_atomic(&f.0, b"new").is_err());
        assert!(f.0.is_dir());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_and_permissions_are_preserved() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let f = Fixture::new();
        let p = f.0.join("target");
        let link = f.0.join("link");
        fs::write(&p, "old").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
        symlink(&p, &link).unwrap();
        save_atomic(&link, b"new").unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&p).unwrap(), "new");
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
}
