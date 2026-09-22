//! Native openat semantics, kept separate from the legacy open contract.

use crate::fs::{FileSystemErrorKind, FileType, SeekFrom, vfs_v2::manager::VfsManager};
use alloc::format;

const RDWR: u32 = 2;
const CREATE: u32 = 0x40;
const EXCLUSIVE: u32 = 0x80;
const TRUNCATE: u32 = 0x200;
const DIRECTORY: u32 = 0x10000;
const NOFOLLOW: u32 = 0x20000;

fn error(vfs: &VfsManager, path: &str, flags: u32) -> FileSystemErrorKind {
    vfs.open_at(None, path, flags, 0o600)
        .err()
        .expect("open must fail")
        .kind
}

#[test_case]
fn native_openat_creates_and_reopens_without_truncating() {
    let vfs = VfsManager::new();
    let opened = vfs.open_at(None, "/new", CREATE | RDWR, 0o600).unwrap();
    let first = opened.as_file().unwrap();
    first.write(b"preserve").unwrap();
    let reopened = vfs.open_at(None, "/new", CREATE | RDWR, 0o777).unwrap();
    let second = reopened.as_file().unwrap();
    assert_eq!(
        first.metadata().unwrap().file_id,
        second.metadata().unwrap().file_id
    );
    let mut bytes = [0; 8];
    assert_eq!(second.read(&mut bytes).unwrap(), 8);
    assert_eq!(&bytes, b"preserve");
    assert_eq!(
        error(&vfs, "/missing/child", CREATE | RDWR),
        FileSystemErrorKind::NotFound
    );
    assert!(vfs.metadata("/missing").is_err());
}

#[test_case]
fn native_openat_truncates_existing_inode_only_when_requested() {
    let vfs = VfsManager::new();
    let opened = vfs.open_at(None, "/file", CREATE | RDWR, 0o600).unwrap();
    let file = opened.as_file().unwrap();
    file.write(b"remove").unwrap();
    let inode = file.metadata().unwrap().file_id;
    assert_eq!(
        error(&vfs, "/file", TRUNCATE),
        FileSystemErrorKind::InvalidOperation
    );
    assert_eq!(file.metadata().unwrap().size, 6);
    let truncated = vfs.open_at(None, "/file", RDWR | TRUNCATE, 0).unwrap();
    assert_eq!(
        truncated.as_file().unwrap().metadata().unwrap().file_id,
        inode
    );
    assert_eq!(file.metadata().unwrap().size, 0);
    assert_eq!(
        error(&vfs, "/missing", TRUNCATE),
        FileSystemErrorKind::InvalidOperation
    );
    assert!(vfs.metadata("/missing").is_err());
}

#[test_case]
fn native_openat_exclusive_rejects_existing_and_dangling_symlinks() {
    let vfs = VfsManager::new();
    let opened = vfs.open_at(None, "/file", CREATE | RDWR, 0o600).unwrap();
    opened.as_file().unwrap().write(b"keep").unwrap();
    vfs.create_dir("/dir").unwrap();
    vfs.create_symlink("/link", "/file").unwrap();
    vfs.create_symlink("/dangling", "/missing").unwrap();
    for path in ["/file", "/dir", "/link", "/dangling", "/"] {
        assert_eq!(
            error(&vfs, path, CREATE | EXCLUSIVE | RDWR | TRUNCATE),
            FileSystemErrorKind::AlreadyExists
        );
    }
    assert_eq!(vfs.metadata("/file").unwrap().size, 4);
    assert!(vfs.metadata("/missing").is_err());
}

#[test_case]
fn native_openat_creates_relative_and_absolute_symlink_targets() {
    let vfs = VfsManager::new();
    vfs.create_dir("/dir").unwrap();
    vfs.create_symlink("/relative", "dir/file").unwrap();
    vfs.create_symlink("/dir/absolute", "/other").unwrap();
    vfs.open_at(None, "/relative", CREATE | RDWR, 0o600)
        .unwrap();
    vfs.open_at(None, "/dir/absolute", CREATE | RDWR, 0o600)
        .unwrap();
    assert_eq!(
        vfs.metadata("/dir/file").unwrap().file_type,
        FileType::RegularFile
    );
    assert_eq!(
        vfs.metadata("/other").unwrap().file_type,
        FileType::RegularFile
    );
    assert!(matches!(
        vfs.symlink_metadata("/relative").unwrap().file_type,
        FileType::SymbolicLink(_)
    ));
}

#[test_case]
fn native_openat_retains_renamed_directory_base() {
    let vfs = VfsManager::new();
    vfs.create_dir("/before").unwrap();
    vfs.create_dir("/destination").unwrap();
    let (base, mount) = vfs.resolve_path("/before").unwrap();
    vfs.rename("/before", "/destination/after").unwrap();
    vfs.open_at(Some((&base, &mount)), "file", CREATE | RDWR, 0o600)
        .unwrap();
    vfs.open_at(Some((&base, &mount)), "../sibling", CREATE | RDWR, 0o600)
        .unwrap();
    assert!(vfs.metadata("/destination/after/file").is_ok());
    assert!(vfs.metadata("/destination/sibling").is_ok());
    assert!(vfs.metadata("/before").is_err());
    assert!(vfs.metadata("/file").is_err());
}

#[test_case]
fn native_openat_absolute_ignores_file_base_relative_rejects_it() {
    let vfs = VfsManager::new();
    vfs.create_file("/base", FileType::RegularFile).unwrap();
    let (base, mount) = vfs.resolve_path("/base").unwrap();
    assert_eq!(
        vfs.open_at(Some((&base, &mount)), "child", CREATE | RDWR, 0o600)
            .err()
            .unwrap()
            .kind,
        FileSystemErrorKind::NotADirectory
    );
    vfs.open_at(Some((&base, &mount)), "/absolute", CREATE | RDWR, 0o600)
        .unwrap();
    assert!(vfs.metadata("/absolute").is_ok());
    vfs.set_cwd_by_path("/").unwrap();
    vfs.open_at(None, "cwd-file", CREATE | RDWR, 0o600).unwrap();
    assert!(vfs.metadata("/cwd-file").is_ok());
}

#[test_case]
fn native_openat_nofollow_and_directory_constraints() {
    let vfs = VfsManager::new();
    vfs.create_dir("/dir").unwrap();
    vfs.create_file("/file", FileType::RegularFile).unwrap();
    vfs.create_symlink("/link", "/file").unwrap();
    vfs.create_symlink("/dangling", "/missing").unwrap();
    for path in ["/link", "/dangling"] {
        assert_eq!(
            error(&vfs, path, CREATE | RDWR | NOFOLLOW),
            FileSystemErrorKind::TooManySymlinks
        );
    }
    assert!(vfs.metadata("/missing").is_err());
    assert_eq!(
        error(&vfs, "/file", DIRECTORY),
        FileSystemErrorKind::NotADirectory
    );
    assert_eq!(error(&vfs, "/dir", RDWR), FileSystemErrorKind::IsADirectory);
    assert_eq!(
        error(&vfs, "/new", CREATE | DIRECTORY),
        FileSystemErrorKind::NotADirectory
    );
    assert!(vfs.metadata("/new").is_err());
    assert!(vfs.open_at(None, "/dir", DIRECTORY, 0).is_ok());
    assert!(vfs.open_at(None, "/", DIRECTORY, 0).is_ok());
}

#[test_case]
fn native_openat_keeps_dotdot_and_trailing_slash_semantics() {
    let vfs = VfsManager::new();
    vfs.create_dir("/dir").unwrap();
    vfs.create_dir("/dir/child").unwrap();
    vfs.create_symlink("/link", "/dir/child").unwrap();
    vfs.open_at(None, "/link/../file", CREATE | RDWR, 0o600)
        .unwrap();
    assert!(vfs.metadata("/dir/file").is_ok());
    assert!(vfs.metadata("/file").is_err());
    for path in ["/dir/file/.", "/dir/file/..", "/dir/file/", "/new/"] {
        assert_eq!(
            error(&vfs, path, CREATE | RDWR),
            FileSystemErrorKind::NotADirectory
        );
    }
    assert!(vfs.metadata("/new").is_err());
    vfs.create_symlink("/trailing-target", "/dir/file/")
        .unwrap();
    assert_eq!(
        error(&vfs, "/trailing-target", 0),
        FileSystemErrorKind::NotADirectory
    );
    assert!(vfs.open_at(None, "/link/", DIRECTORY | NOFOLLOW, 0).is_ok());
}

#[test_case]
fn native_openat_limits_all_symlink_expansions() {
    let vfs = VfsManager::new();
    vfs.create_symlink("/loop", "loop").unwrap();
    assert_eq!(
        error(&vfs, "/loop", CREATE | RDWR),
        FileSystemErrorKind::TooManySymlinks
    );
    vfs.create_file("/end", FileType::RegularFile).unwrap();
    for index in (0..40).rev() {
        let target = if index == 39 {
            alloc::string::String::from("end")
        } else {
            format!("link{}", index + 1)
        };
        vfs.create_symlink(&format!("/link{index}"), &target)
            .unwrap();
    }
    vfs.open_at(None, "/link0", 0, 0).unwrap();
    vfs.create_symlink("/too-many", "link0").unwrap();
    assert_eq!(
        error(&vfs, "/too-many", 0),
        FileSystemErrorKind::TooManySymlinks
    );
}

#[test_case]
fn native_openat_validates_flags_before_mutating() {
    let vfs = VfsManager::new();
    for flags in [3, CREATE | 3, CREATE | 0x80000000, CREATE | TRUNCATE] {
        assert_eq!(
            error(&vfs, "/new", flags),
            FileSystemErrorKind::InvalidOperation
        );
        assert!(vfs.metadata("/new").is_err());
    }
    assert_eq!(
        error(&vfs, "", CREATE | RDWR),
        FileSystemErrorKind::NotFound
    );
    let opened = vfs.open_at(None, "/new", CREATE | RDWR, 0o600).unwrap();
    let file = opened.as_file().unwrap();
    file.write(b"abc").unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut byte = [0];
    file.read(&mut byte).unwrap();
    assert_eq!(byte[0], b'a');
}
