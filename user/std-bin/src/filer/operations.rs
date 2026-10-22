//! Filesystem operations used by Files. No operation replaces an existing item.
//!
//! Copies reserve destinations with create_new/create_dir and synchronize bytes.
//! Moves and renames use atomic no-replace rename, retaining inode identity,
//! open handles and hard links. Cross-filesystem moves fail with the original
//! intact; no copy-and-delete fallback is permitted. Symbolic links are copied
//! as links and never traversed by recursive work.

use scarlet_fs as fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const MAX_ITEMS: usize = 100_000;
const MAX_DEPTH: usize = 64;

pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
        return Err("Enter a name without / or a NUL character".into());
    }
    if name.len() > 255 {
        return Err("The name is too long (maximum 255 bytes)".into());
    }
    Ok(())
}

pub fn location_path(current: &Path, input: &str, home: &Path) -> Result<PathBuf, String> {
    if input.is_empty() || input.contains('\0') {
        return Err("Enter a folder path".into());
    }
    let path = if input == "~" {
        home.to_path_buf()
    } else if let Some(rest) = input.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(input).is_absolute() {
        PathBuf::from(input)
    } else {
        current.join(input)
    };
    // Preserve `..` until the filesystem resolves it: a preceding symlink can
    // change its meaning, so lexical normalization would navigate incorrectly.
    Ok(path)
}

#[derive(Clone, Debug)]
pub enum Operation {
    NewFolder {
        directory: PathBuf,
        name: String,
    },
    Rename {
        source: PathBuf,
        name: String,
    },
    Transfer {
        source: PathBuf,
        directory: PathBuf,
        cut: bool,
    },
    Delete {
        source: PathBuf,
    },
}

pub fn execute(operation: &Operation) -> Result<String, String> {
    match operation {
        Operation::NewFolder { directory, name } => {
            validate_name(name)?;
            let path = directory.join(name);
            std::fs::create_dir(&path)
                .map_err(|error| format!("Cannot create {}: {error}", path.display()))?;
            Ok(format!("Created {name}"))
        }
        Operation::Rename { source, name } => {
            validate_name(name)?;
            let parent = source.parent().ok_or("Cannot rename the filesystem root")?;
            let destination = parent.join(name);
            if source == &destination {
                return Ok("Name unchanged".into());
            }
            transfer(source, &destination, true)?;
            Ok(format!("Renamed to {name}"))
        }
        Operation::Transfer {
            source,
            directory,
            cut,
        } => {
            let name = source
                .file_name()
                .ok_or("Cannot transfer the filesystem root")?;
            transfer(source, &directory.join(name), *cut)?;
            Ok(format!(
                "{} {}",
                if *cut { "Moved" } else { "Copied" },
                name.to_string_lossy()
            ))
        }
        Operation::Delete { source } => {
            if source.file_name().is_none() {
                return Err("Cannot delete the filesystem root".into());
            }
            // Check limits and readability before the first destructive step.
            inspect_tree(source, 0, &mut 0)
                .map_err(|error| format!("Cannot delete {}: {error}", source.display()))?;
            remove_tree(source).map_err(|error| {
                if error.kind() == io::ErrorKind::Unsupported {
                    format!("Cannot delete {}: {error}. No contents were removed.", source.display())
                } else {
                    format!("Deletion stopped at {}: {error}. Some items may already have been removed.", source.display())
                }
            })?;
            Ok(format!("Deleted {}", source.display()))
        }
    }
}

fn transfer(source: &Path, destination: &Path, cut: bool) -> Result<(), String> {
    if cut {
        fs::require_noreplace_move().map_err(|error| error.to_string())?;
        return fs::rename_noreplace(source, destination).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                format!("{} already exists. Nothing was replaced and the original was kept.", destination.display())
            } else if error.kind() == io::ErrorKind::CrossesDevices {
                "Moving between filesystems is unavailable. The original was kept; use Copy instead.".into()
            } else {
                format!("Cannot move {} to {}: {error}.", source.display(), destination.display())
            }
        });
    }
    match fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(format!(
                "{} already exists. Rename it or choose another folder; nothing was replaced.",
                destination.display()
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Cannot inspect {}: {error}", destination.display())),
    }
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| format!("Cannot read {}: {error}", source.display()))?;
    if metadata.is_dir() {
        let canonical_source = std::fs::canonicalize(source).map_err(|error| error.to_string())?;
        let parent = destination.parent().ok_or("Invalid destination")?;
        let canonical_parent = std::fs::canonicalize(parent).map_err(|error| error.to_string())?;
        if canonical_parent.starts_with(&canonical_source) {
            return Err("A folder cannot be copied or moved into itself".into());
        }
    }
    inspect_tree(source, 0, &mut 0)
        .map_err(|error| format!("Cannot transfer {}: {error}", source.display()))?;
    copy_tree(source, destination, 0, &mut 0)
        .map_err(|error| format!("Cannot copy to {}: {error}", destination.display()))?;
    Ok(())
}

fn check_budget(depth: usize, count: &mut usize) -> io::Result<()> {
    *count += 1;
    if depth > MAX_DEPTH || *count > MAX_ITEMS {
        return Err(io::Error::other(
            "Folder is too large or deeply nested for this operation",
        ));
    }
    Ok(())
}

fn inspect_tree(path: &Path, depth: usize, count: &mut usize) -> io::Result<()> {
    check_budget(depth, count)?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        for child in std::fs::read_dir(path)? {
            inspect_tree(&child?.path(), depth + 1, count)?;
        }
    } else if !metadata.is_file() && !metadata.file_type().is_symlink() {
        return Err(io::Error::other(
            "Device files and other special files are not supported",
        ));
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path, depth: usize, count: &mut usize) -> io::Result<()> {
    check_budget(depth, count)?;
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        copy_symlink(source, destination)
    } else if metadata.is_dir() {
        std::fs::create_dir(destination)?;
        let result = (|| {
            for child in std::fs::read_dir(source)? {
                let child = child?;
                copy_tree(
                    &child.path(),
                    &destination.join(child.file_name()),
                    depth + 1,
                    count,
                )?;
            }
            fs::set_permissions(destination, metadata.permissions())?;
            fs::open_read_nofollow(destination)?.sync_all()
        })();
        match result {
            Err(error) => match remove_tree(destination) {
                Ok(()) => Err(error),
                Err(cleanup) => Err(io::Error::other(format!(
                    "{error}. Partial copy remains at {}: {cleanup}",
                    destination.display()
                ))),
            },
            success => success,
        }
    } else if metadata.is_file() {
        let mut input = fs::open_read_nofollow(source)?;
        if !input.metadata()?.is_file() {
            return Err(io::Error::other("Source changed type before copying"));
        }
        let mut output =
            fs::create_new_with_permissions(destination, Some(&metadata.permissions()))?;
        let result = (|| {
            io::copy(&mut input, &mut output)?;
            output.flush()?;
            fs::set_permissions(destination, metadata.permissions())?;
            output.sync_all()
        })();
        drop(output);
        match result {
            Err(error) => match std::fs::remove_file(destination) {
                Ok(()) => Err(error),
                Err(cleanup) => Err(io::Error::other(format!(
                    "{error}. Partial copy remains at {}: {cleanup}",
                    destination.display()
                ))),
            },
            success => success,
        }
    } else {
        Err(io::Error::other(
            "Device files and other special files are not supported",
        ))
    }
}

#[cfg(unix)]
fn copy_symlink(source: &Path, destination: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(std::fs::read_link(source)?, destination)
}

#[cfg(target_os = "scarlet")]
fn copy_symlink(source: &Path, destination: &Path) -> io::Result<()> {
    use std::ffi::CString;
    let target = std::fs::read_link(source)?;
    let destination = CString::new(destination.as_os_str().as_encoded_bytes())
        .map_err(|_| io::Error::other("Invalid link path"))?;
    let target = CString::new(target.as_os_str().as_encoded_bytes())
        .map_err(|_| io::Error::other("Invalid link target"))?;
    // SAFETY: VfsCreateSymlink takes the link path then its target as two
    // NUL-terminated strings. Both buffers remain live for the synchronous
    // call; the two reserved scalar arguments are zero.
    let result = unsafe {
        scarlet_sys::syscall4(
            scarlet_sys::Syscall::VfsCreateSymlink,
            destination.as_ptr() as usize,
            target.as_ptr() as usize,
            0,
            0,
        )
    };
    if result == usize::MAX {
        Err(io::Error::other("Cannot create symbolic link"))
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, target_os = "scarlet")))]
fn copy_symlink(_source: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::Error::other(
        "Copying symbolic links is unavailable on this platform",
    ))
}

fn remove_tree(path: &Path) -> io::Result<()> {
    // remove_dir_all in std does not follow symlinks, including links inside
    // the tree. A link selected directly must be unlinked with remove_file.
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

#[derive(Clone, Debug, Default)]
pub struct History {
    back: Vec<String>,
    forward: Vec<String>,
}

impl History {
    pub fn visit(&mut self, current: &str, next: &str) {
        if current == next {
            return;
        }
        self.back.push(current.into());
        if self.back.len() > 128 {
            self.back.remove(0);
        }
        self.forward.clear();
    }
    pub fn back(&mut self, current: &str) -> Option<String> {
        let next = self.back.pop()?;
        self.forward.push(current.into());
        Some(next)
    }
    pub fn forward(&mut self, current: &str) -> Option<String> {
        let next = self.forward.pop()?;
        self.back.push(current.into());
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "scarlet-files-{}-{}",
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
    fn names_and_locations_cannot_escape_via_names() {
        for name in ["", ".", "..", "a/b", "a\0b"] {
            assert!(validate_name(name).is_err());
        }
        assert!(validate_name("メモ with spaces.txt").is_ok());
        assert_eq!(
            location_path(Path::new("/a"), " folder ", Path::new("/home/u")).unwrap(),
            Path::new("/a/ folder ")
        );
        assert_eq!(
            location_path(Path::new("/a/b"), "../c", Path::new("/home/u")).unwrap(),
            Path::new("/a/b/../c")
        );
        assert_eq!(
            location_path(Path::new("/"), "~/notes", Path::new("/home/u")).unwrap(),
            Path::new("/home/u/notes")
        );
    }

    #[test]
    fn history_branches_and_does_not_record_refresh() {
        let mut history = History::default();
        history.visit("/a", "/b");
        history.visit("/b", "/b");
        history.visit("/b", "/c");
        assert_eq!(history.back("/c").as_deref(), Some("/b"));
        assert_eq!(history.forward("/b").as_deref(), Some("/c"));
        assert_eq!(history.back("/c").as_deref(), Some("/b"));
        history.visit("/b", "/d");
        assert!(history.forward("/d").is_none());
    }

    #[test]
    fn collisions_preserve_both_files_and_folders() {
        let fixture = Fixture::new();
        let source = fixture.0.join("source");
        let target = fixture.0.join("target");
        fs::write(&source, b"source").unwrap();
        fs::write(&target, b"target").unwrap();
        assert!(transfer(&source, &target, true).is_err());
        assert_eq!(fs::read(&source).unwrap(), b"source");
        assert_eq!(fs::read(&target).unwrap(), b"target");
        assert!(
            execute(&Operation::NewFolder {
                directory: fixture.0.clone(),
                name: "target".into()
            })
            .is_err()
        );
    }

    #[test]
    fn nested_copy_move_and_delete_preserve_contents() {
        let fixture = Fixture::new();
        let source = fixture.0.join("source");
        fs::create_dir(&source).unwrap();
        fs::create_dir(source.join("nested")).unwrap();
        fs::write(source.join("nested/メモ.txt"), "こんにちは").unwrap();
        let copy = fixture.0.join("copy");
        transfer(&source, &copy, false).unwrap();
        assert_eq!(
            fs::read_to_string(copy.join("nested/メモ.txt")).unwrap(),
            "こんにちは"
        );
        let moved = fixture.0.join("moved");
        transfer(&copy, &moved, true).unwrap();
        assert!(!copy.exists());
        assert!(source.exists());
        execute(&Operation::Delete { source: moved }).unwrap();
        assert!(source.join("nested/メモ.txt").exists());
    }

    #[test]
    fn refuses_folder_self_copy_before_creating_destination() {
        let fixture = Fixture::new();
        let destination = fixture.0.join("inside");
        assert!(transfer(&fixture.0, &destination, false).is_err());
        assert!(!destination.exists());
    }

    #[test]
    fn rename_preserves_bytes_and_refuses_existing_destination() {
        let fixture = Fixture::new();
        let source = fixture.0.join("old.txt");
        fs::write(&source, b"complete content").unwrap();
        execute(&Operation::Rename {
            source: source.clone(),
            name: "new.txt".into(),
        })
        .unwrap();
        assert!(!source.exists());
        let renamed = fixture.0.join("new.txt");
        assert_eq!(fs::read(&renamed).unwrap(), b"complete content");
        fs::write(&source, b"keep me").unwrap();
        assert!(
            execute(&Operation::Rename {
                source: renamed.clone(),
                name: "old.txt".into()
            })
            .is_err()
        );
        assert_eq!(fs::read(&source).unwrap(), b"keep me");
        assert_eq!(fs::read(&renamed).unwrap(), b"complete content");
    }

    #[test]
    fn excessive_depth_is_rejected_before_copy_or_delete() {
        let fixture = Fixture::new();
        let source = fixture.0.join("source");
        fs::create_dir(&source).unwrap();
        let mut nested = source.clone();
        for _ in 0..=MAX_DEPTH {
            nested.push("d");
            fs::create_dir(&nested).unwrap();
        }
        fs::write(nested.join("keep"), b"keep").unwrap();
        let destination = fixture.0.join("copy");
        assert!(transfer(&source, &destination, false).is_err());
        assert!(!destination.exists());
        assert!(execute(&Operation::Delete { source }).is_err());
        assert_eq!(fs::read(nested.join("keep")).unwrap(), b"keep");
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn atomic_move_preserves_inode_hard_links_and_live_open_handles() {
        use std::os::unix::fs::MetadataExt;
        let fixture = Fixture::new();
        let source = fixture.0.join("source");
        let destination = fixture.0.join("destination");
        let alias = fixture.0.join("hard-link");
        let mut open = fs::File::create(&source).unwrap();
        open.write_all(b"before").unwrap();
        fs::hard_link(&source, &alias).unwrap();
        let identity = fs::metadata(&source).unwrap().ino();
        transfer(&source, &destination, true).unwrap();
        assert!(!source.exists());
        assert_eq!(fs::metadata(&destination).unwrap().ino(), identity);
        assert_eq!(fs::metadata(&alias).unwrap().ino(), identity);
        open.write_all(b" after").unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"before after");
        assert_eq!(fs::read(&alias).unwrap(), b"before after");
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn atomic_move_rejects_dangling_targets_and_directory_cycles() {
        let fixture = Fixture::new();
        let source = fixture.0.join("source");
        let destination = fixture.0.join("dangling");
        fs::write(&source, b"keep").unwrap();
        std::os::unix::fs::symlink("missing", &destination).unwrap();
        assert!(transfer(&source, &destination, true).is_err());
        assert_eq!(fs::read(&source).unwrap(), b"keep");
        assert_eq!(fs::read_link(&destination).unwrap(), Path::new("missing"));
        let directory = fixture.0.join("directory");
        fs::create_dir(&directory).unwrap();
        fs::create_dir(directory.join("child")).unwrap();
        assert!(transfer(&directory, &directory.join("child/cycle"), true).is_err());
        assert!(directory.join("child").is_dir());
        assert!(!directory.join("child/cycle").exists());
    }

    #[cfg(unix)]
    #[test]
    fn selected_folder_link_is_unlinked_without_removing_target_contents() {
        let fixture = Fixture::new();
        let target = fixture.0.join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep"), b"keep").unwrap();
        let link = fixture.0.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        execute(&Operation::Delete {
            source: link.clone(),
        })
        .unwrap();
        assert!(fs::symlink_metadata(&link).is_err());
        assert_eq!(fs::read(target.join("keep")).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn location_parent_is_resolved_after_symbolic_links() {
        let fixture = Fixture::new();
        let external = Fixture::new();
        fs::create_dir(external.0.join("child")).unwrap();
        std::os::unix::fs::symlink(external.0.join("child"), fixture.0.join("link")).unwrap();
        let path = location_path(&fixture.0, "link/..", &fixture.0).unwrap();
        assert_eq!(fs::canonicalize(path).unwrap(), external.0);
    }

    #[cfg(unix)]
    #[test]
    fn copies_broken_and_external_symlinks_without_following_them() {
        let fixture = Fixture::new();
        let external = Fixture::new();
        fs::write(external.0.join("keep"), b"keep").unwrap();
        let source = fixture.0.join("source");
        fs::create_dir(&source).unwrap();
        std::os::unix::fs::symlink(&external.0, source.join("external")).unwrap();
        std::os::unix::fs::symlink("missing", source.join("broken")).unwrap();
        let copy = fixture.0.join("copy");
        transfer(&source, &copy, false).unwrap();
        assert_eq!(
            fs::read_link(copy.join("broken")).unwrap(),
            Path::new("missing")
        );
        execute(&Operation::Delete { source: copy }).unwrap();
        assert_eq!(fs::read(external.0.join("keep")).unwrap(), b"keep");
        assert!(transfer(&source, &source.join("external/inside"), false).is_ok());
        assert!(
            external
                .0
                .join("inside/broken")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
