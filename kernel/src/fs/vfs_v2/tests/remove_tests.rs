//! Typed final-entry removal must reject every invalid operation before mutation.

use crate::fs::{FileSystemErrorKind as Error, FileType, vfs_v2::manager::VfsManager};

#[test_case]
fn typed_remove_unlinks_symlinks_and_dangling_links_without_following() {
    let vfs = VfsManager::new();
    vfs.create_file("/target", FileType::RegularFile).unwrap();
    vfs.create_symlink("/link", "/target").unwrap();
    vfs.create_symlink("/dangling", "/missing").unwrap();
    for path in ["/link", "/dangling"] {
        vfs.remove_with_kind(path, false).unwrap();
        assert_eq!(
            vfs.symlink_metadata(path).unwrap_err().kind,
            Error::NotFound
        );
    }
    assert!(vfs.metadata("/target").is_ok());
    assert_eq!(vfs.metadata("/missing").unwrap_err().kind, Error::NotFound);
}

#[test_case]
fn typed_remove_rejects_wrong_kind_and_nonempty_directories() {
    let vfs = VfsManager::new();
    vfs.create_dir("/dir").unwrap();
    vfs.create_file("/dir/file", FileType::RegularFile).unwrap();
    vfs.create_symlink("/alias", "/dir").unwrap();
    assert_eq!(
        vfs.remove_with_kind("/dir", false).unwrap_err().kind,
        Error::IsADirectory
    );
    assert_eq!(
        vfs.remove_with_kind("/dir/file", true).unwrap_err().kind,
        Error::NotADirectory
    );
    assert_eq!(
        vfs.remove_with_kind("/alias", true).unwrap_err().kind,
        Error::NotADirectory
    );
    assert_eq!(
        vfs.remove_with_kind("/dir", true).unwrap_err().kind,
        Error::DirectoryNotEmpty
    );
    assert!(vfs.metadata("/dir/file").is_ok());
    vfs.remove_with_kind("/dir/file", false).unwrap();
    vfs.remove_with_kind("/dir///", true).unwrap();
    assert_eq!(vfs.metadata("/dir").unwrap_err().kind, Error::NotFound);
    vfs.remove_with_kind("/alias", false).unwrap();
}

#[test_case]
fn typed_remove_trailing_slashes_never_delete_links_or_files() {
    let vfs = VfsManager::new();
    vfs.create_dir("/dir").unwrap();
    vfs.create_file("/file", FileType::RegularFile).unwrap();
    vfs.create_symlink("/alias", "/dir").unwrap();
    vfs.create_symlink("/dangling", "/missing").unwrap();
    for path in ["/file/", "/alias///", "/dangling/"] {
        for directory in [false, true] {
            assert_eq!(
                vfs.remove_with_kind(path, directory).unwrap_err().kind,
                Error::NotADirectory
            );
        }
        assert!(vfs.symlink_metadata(path.trim_end_matches('/')).is_ok());
    }
    assert!(vfs.metadata("/dir").is_ok());
}

#[test_case]
fn typed_remove_preserves_intermediate_symlink_and_dotdot_resolution() {
    let vfs = VfsManager::new();
    vfs.create_dir("/dir").unwrap();
    vfs.create_dir("/dir/sub").unwrap();
    vfs.create_file("/dir/file", FileType::RegularFile).unwrap();
    vfs.create_file("/file", FileType::RegularFile).unwrap();
    vfs.create_symlink("/alias", "/dir/sub").unwrap();
    vfs.remove_with_kind("/alias/../file", false).unwrap();
    assert_eq!(vfs.metadata("/dir/file").unwrap_err().kind, Error::NotFound);
    assert!(vfs.metadata("/file").is_ok());
    assert!(vfs.symlink_metadata("/alias").is_ok());
}

#[test_case]
fn typed_remove_rejects_empty_missing_dot_and_root_entries() {
    let vfs = VfsManager::new();
    vfs.create_dir("/dir").unwrap();
    vfs.create_dir("/dir/sub").unwrap();
    for path in ["", "/missing", "/missing/child"] {
        assert_eq!(
            vfs.remove_with_kind(path, true).unwrap_err().kind,
            Error::NotFound
        );
    }
    for path in ["/dir/.", "/dir/sub/.."] {
        assert_eq!(
            vfs.remove_with_kind(path, true).unwrap_err().kind,
            Error::InvalidPath
        );
    }
    for path in ["/", "///"] {
        assert_eq!(
            vfs.remove_with_kind(path, true).unwrap_err().kind,
            Error::Busy
        );
    }
    assert!(vfs.metadata("/dir/sub").is_ok());
}

#[test_case]
fn typed_remove_rejects_mount_roots() {
    let vfs = VfsManager::new();
    vfs.create_dir("/mount").unwrap();
    vfs.mount(
        crate::fs::vfs_v2::drivers::tmpfs::TmpFS::new(0),
        "/mount",
        0,
    )
    .unwrap();
    assert_eq!(
        vfs.remove_with_kind("/mount", true).unwrap_err().kind,
        Error::Busy
    );
    assert!(vfs.metadata("/mount").is_ok());
}

#[test_case]
fn readonly_open_in_atomic_context_succeeds_when_namespace_uncontended() {
    let vfs = VfsManager::new();
    vfs.create_file("/file", FileType::RegularFile).unwrap();
    let _preempt_guard = crate::sync::PreemptGuard::new();
    let file = vfs.open("/file", 0).unwrap();
    assert_eq!(file.as_file().unwrap().metadata().unwrap().size, 0);
}
