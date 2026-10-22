//! The native errno-returning rename uses the same locked VFS transaction as
//! legacy rename. Each test owns an isolated temporary in-memory namespace.
use crate::fs::FileType;
use crate::fs::vfs_v2::{
    drivers::tmpfs::TmpFS, manager::VfsManager, syscall::rename_paths_with_status,
};
use scarlet_abi::fs::{ERRNO_EEXIST, ERRNO_ENOENT, ERRNO_EXDEV, VFS_RENAME_NOREPLACE};

fn failure(errno: i32) -> usize {
    (-(errno as isize)) as usize
}

#[test_case]
fn native_rename_flags_and_errno_preserve_both_entries() {
    let vfs = VfsManager::new();
    vfs.create_file("/source", FileType::RegularFile).unwrap();
    vfs.create_file("/target", FileType::RegularFile).unwrap();
    let source_id = vfs.metadata("/source").unwrap().file_id;
    let target_id = vfs.metadata("/target").unwrap().file_id;
    assert_eq!(crate::syscall::Syscall::VfsRename as usize, 409);
    assert_eq!(crate::syscall::Syscall::VfsRenameWithStatus as usize, 419);
    assert_eq!(scarlet_abi::Syscall::VfsRenameWithStatus as usize, 419);
    assert_eq!(
        rename_paths_with_status(&vfs, "", "", 2),
        failure(scarlet_abi::ERRNO_EINVAL)
    );
    for (old, new, expected) in [
        ("/source", "/target", ERRNO_EEXIST),
        ("/source", "/source", ERRNO_EEXIST),
        ("/missing", "/new", ERRNO_ENOENT),
        ("", "/new", ERRNO_ENOENT),
        ("/source", "", ERRNO_ENOENT),
        ("/source", "/missing/new", ERRNO_ENOENT),
    ] {
        assert_eq!(
            rename_paths_with_status(&vfs, old, new, VFS_RENAME_NOREPLACE),
            failure(expected)
        );
        assert_eq!(vfs.metadata("/source").unwrap().file_id, source_id);
        assert_eq!(vfs.metadata("/target").unwrap().file_id, target_id);
    }
    vfs.create_symlink("/dangling", "/missing").unwrap();
    let link_id = vfs.symlink_metadata("/dangling").unwrap().file_id;
    assert_eq!(
        rename_paths_with_status(&vfs, "/source", "/dangling", VFS_RENAME_NOREPLACE),
        failure(ERRNO_EEXIST)
    );
    assert_eq!(vfs.metadata("/source").unwrap().file_id, source_id);
    assert_eq!(vfs.symlink_metadata("/dangling").unwrap().file_id, link_id);
    // Flags zero explicitly retains replacing rename semantics for clients
    // needing errno detail, without changing the existing operation 409.
    assert_eq!(rename_paths_with_status(&vfs, "/source", "/target", 0), 0);
    assert_eq!(vfs.metadata("/target").unwrap().file_id, source_id);
}

#[test_case]
fn native_rename_retains_open_file_and_hard_link_identity() {
    let vfs = VfsManager::new();
    vfs.create_file("/source", FileType::RegularFile).unwrap();
    vfs.create_hardlink("/source", "/alias").unwrap();
    let identity = vfs.metadata("/source").unwrap().file_id;
    let open = vfs.open("/source", 2).unwrap();
    let file = open.as_file().unwrap();
    file.write(b"before").unwrap();
    assert_eq!(
        rename_paths_with_status(&vfs, "/source", "/moved", VFS_RENAME_NOREPLACE),
        0
    );
    assert!(vfs.metadata("/source").is_err());
    file.write(b" after").unwrap();
    for path in ["/moved", "/alias"] {
        assert_eq!(vfs.metadata(path).unwrap().file_id, identity);
        let reopened = vfs.open(path, 0).unwrap();
        let mut bytes = [0u8; 12];
        assert_eq!(reopened.as_file().unwrap().read(&mut bytes).unwrap(), 12);
        assert_eq!(&bytes, b"before after");
    }
}

#[test_case]
fn native_rename_rejects_cross_mount_and_invalid_directory_moves() {
    let vfs = VfsManager::new();
    vfs.create_dir("/dir").unwrap();
    vfs.create_dir("/dir/child").unwrap();
    vfs.create_dir("/other").unwrap();
    vfs.mount(TmpFS::new(1024 * 1024), "/other", 0).unwrap();
    vfs.create_file("/dir/source", FileType::RegularFile)
        .unwrap();
    let identity = vfs.metadata("/dir/source").unwrap().file_id;
    assert_eq!(
        rename_paths_with_status(&vfs, "/dir/source", "/other/new", VFS_RENAME_NOREPLACE),
        failure(ERRNO_EXDEV)
    );
    assert_eq!(vfs.metadata("/dir/source").unwrap().file_id, identity);
    assert!(vfs.metadata("/other/new").is_err());
    for (old, new) in [
        ("/dir", "/dir/child/cycle"),
        ("/dir/.", "/new"),
        ("/dir/..", "/new"),
        ("/", "/new"),
        ("/other", "/new"),
    ] {
        assert_ne!(
            rename_paths_with_status(&vfs, old, new, VFS_RENAME_NOREPLACE),
            0
        );
        assert_eq!(vfs.metadata("/dir/source").unwrap().file_id, identity);
        assert!(vfs.metadata("/dir/child").is_ok());
        assert!(vfs.metadata("/new").is_err());
    }
}
