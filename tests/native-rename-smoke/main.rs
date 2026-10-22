//! Native-only smoke fixture. Run in a booted Scarlet environment with writable
//! /tmp and the ordinary /dev mount. All successful mutations use a unique /tmp
//! directory; the cross-mount /dev destination is never created or removed.
#[path = "../../user/std-bin/src/desktop_utilities/native_fs.rs"]
mod native_fs;

use scarlet_abi::fs::*;
use scarlet_sys::{Syscall, syscall2, syscall3};
use std::ffi::CString;
use std::io::Write;
use std::path::Path;

fn c_path(path: &Path) -> CString {
    CString::new(path.as_os_str().as_encoded_bytes()).unwrap()
}

fn raw_rename(old: &str, new: &str, flags: usize) -> usize {
    let old = CString::new(old).unwrap();
    let new = CString::new(new).unwrap();
    // SAFETY: immutable NUL path buffers remain live until return.
    unsafe {
        syscall3(
            Syscall::VfsRenameWithStatus,
            old.as_ptr() as usize,
            new.as_ptr() as usize,
            flags,
        )
    }
}

fn error(number: i32) -> usize {
    (-(number as isize)) as usize
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    native_fs::require_noreplace_move()?;
    // SAFETY: reserved flags must be rejected before null pointer validation.
    assert_eq!(
        unsafe { syscall3(Syscall::VfsRenameWithStatus, 0, 0, 2) },
        error(22)
    );
    // SAFETY: deliberately invalid userspace pointers exercise kernel usercopy;
    // no userspace reference or allocation is passed for the kernel to retain.
    assert_eq!(
        unsafe { syscall3(Syscall::VfsRenameWithStatus, 0, 0, 1) },
        error(ERRNO_EFAULT)
    );
    assert_eq!(raw_rename("", "", 1), error(ERRNO_ENOENT));
    assert_eq!(
        raw_rename(&"x".repeat(PATH_MAX), "", 1),
        error(ERRNO_ENAMETOOLONG)
    );

    let root = std::env::temp_dir().join(format!("rename419-smoke-{}", std::process::id()));
    std::fs::create_dir(&root)?; // exclusive fixture directory; never reuse one
    let source = root.join("source");
    let target = root.join("target");
    let moved = root.join("moved");
    let alias = root.join("alias");
    let link = root.join("dangling");
    let moved_link = root.join("moved-link");
    let mut open = native_fs::create_new(&source)?;
    open.write_all(b"before")?;
    native_fs::create_new(&target)?.write_all(b"keep")?;
    let source_id = native_fs::metadata(&source)?.file_id();
    let target_id = native_fs::metadata(&target)?.file_id();
    let source_c = c_path(&source);
    let alias_c = c_path(&alias);
    // SAFETY: both NUL paths are live and immutable for the synchronous call.
    assert_eq!(
        unsafe {
            syscall2(
                Syscall::VfsCreateHardlink,
                source_c.as_ptr() as usize,
                alias_c.as_ptr() as usize,
            )
        },
        0
    );
    let link_c = c_path(&link);
    let missing_c = CString::new("missing").unwrap();
    // SAFETY: same pathname lifetime contract; the link target need not exist.
    assert_eq!(
        unsafe {
            syscall2(
                Syscall::VfsCreateSymlink,
                link_c.as_ptr() as usize,
                missing_c.as_ptr() as usize,
            )
        },
        0
    );
    let link_id = native_fs::symlink_metadata(&link)?.file_id();
    for destination in [&target, &alias, &link, &source] {
        assert_eq!(
            native_fs::rename_noreplace(&source, destination)
                .unwrap_err()
                .raw_os_error(),
            Some(ERRNO_EEXIST)
        );
        assert_eq!(native_fs::metadata(&source)?.file_id(), source_id);
        assert_eq!(native_fs::metadata(&target)?.file_id(), target_id);
        assert_eq!(std::fs::read(&target)?, b"keep");
    }
    assert_eq!(native_fs::symlink_metadata(&link)?.file_id(), link_id);
    native_fs::rename_noreplace(&source, &moved)?;
    assert_eq!(
        native_fs::metadata(&source).unwrap_err().raw_os_error(),
        Some(ERRNO_ENOENT)
    );
    open.write_all(b" after")?;
    for path in [&moved, &alias] {
        assert_eq!(native_fs::metadata(path)?.file_id(), source_id);
        assert_eq!(std::fs::read(path)?, b"before after");
    }
    native_fs::rename_noreplace(&link, &moved_link)?;
    assert_eq!(native_fs::symlink_metadata(&moved_link)?.file_id(), link_id);
    assert_eq!(std::fs::read_link(&moved_link)?, Path::new("missing"));

    let cross_mount = Path::new("/dev").join(format!("rename419-smoke-{}", std::process::id()));
    assert_eq!(
        native_fs::rename_noreplace(&moved, &cross_mount)
            .unwrap_err()
            .raw_os_error(),
        Some(ERRNO_EXDEV)
    );
    assert_eq!(native_fs::metadata(&moved)?.file_id(), source_id);
    assert_eq!(
        native_fs::symlink_metadata(&cross_mount)
            .unwrap_err()
            .raw_os_error(),
        Some(ERRNO_ENOENT)
    );
    let directory = root.join("directory");
    std::fs::create_dir(&directory)?;
    std::fs::create_dir(directory.join("child"))?;
    assert!(native_fs::rename_noreplace(&directory, &directory.join("child/cycle")).is_err());
    assert!(native_fs::rename_noreplace(&directory.join("."), &root.join("invalid")).is_err());
    assert!(native_fs::rename_noreplace(Path::new("/"), &root.join("invalid")).is_err());
    assert!(native_fs::metadata(&directory.join("child"))?.is_dir());

    drop(open);
    for path in [&moved, &alias, &target, &moved_link] {
        std::fs::remove_file(path)?;
    }
    std::fs::remove_dir(directory.join("child"))?;
    std::fs::remove_dir(directory)?;
    std::fs::remove_dir(root)?;
    Ok(())
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        println!("SCARLET_RENAME419_SMOKE_FAIL: {info}");
    }));
    match std::panic::catch_unwind(run) {
        Ok(Ok(())) => println!("SCARLET_RENAME419_SMOKE_OK"),
        Ok(Err(error)) => {
            println!("SCARLET_RENAME419_SMOKE_FAIL: {error}");
            std::process::exit(1);
        }
        Err(_) => {
            println!("SCARLET_RENAME419_SMOKE_FAIL: assertion failed");
            std::process::exit(1);
        }
    }
}
