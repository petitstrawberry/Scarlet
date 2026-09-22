//! Actual C ABI and Rust std checks, executed inside a private Scarlet guest.
use scarlet_abi::{Syscall, fs::*};
use std::ffi::CString;
use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

unsafe extern "C" {
    fn scarlet_libc_probe(file: i32, directory: i32, expected: *const std::ffi::c_char) -> i32;
}

fn symlink(target: &str, link: &Path) {
    let target = CString::new(target).unwrap();
    let link = CString::new(link.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: both C strings live until the syscall returns.
    assert_eq!(
        unsafe {
            scarlet_sys::syscall2(
                Syscall::VfsCreateSymlink,
                link.as_ptr() as usize,
                target.as_ptr() as usize,
            )
        },
        0
    );
}

fn check(root: &Path, ext2: bool) -> Result<(), Box<dyn std::error::Error>> {
    println!("NATIVE_RUSTC FILESYSTEM fixture={}", root.display());
    fs::create_dir_all(root.join("real/sub"))?;
    symlink("real/sub", &root.join("alias"));
    symlink("real/file", &root.join("file-link"));
    symlink("absent", &root.join("dangling"));
    symlink("loop", &root.join("loop"));
    symlink(".", &root.join("repeat"));
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(root.join("real/file"))?;
    file.write_all(b"timestamp fixture")?;
    let directory = File::open(root)?;
    std::env::set_current_dir(root)?;
    let expected = root.join("real/file");
    assert!(Path::new("/").is_absolute());
    assert_eq!(fs::canonicalize("alias/../file")?, expected);
    assert_eq!(fs::canonicalize("repeat/".repeat(40))?, root);
    assert_eq!(
        fs::canonicalize("repeat/".repeat(41))
            .unwrap_err()
            .raw_os_error(),
        Some(ERRNO_ELOOP)
    );
    assert_eq!(fs::canonicalize("..")?, root.parent().unwrap());
    let mut actual = String::new();
    File::open("alias/../file")?.read_to_string(&mut actual)?;
    assert_eq!(actual, "timestamp fixture");
    for invalid in ["real/file/.", "real/file/..", "file-link/"] {
        assert_eq!(
            fs::canonicalize(invalid).unwrap_err().kind(),
            std::io::ErrorKind::NotADirectory
        );
    }
    assert_eq!(
        fs::canonicalize("").unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    assert!(!Path::new("missing").try_exists()?);
    assert_eq!(
        Path::new("loop").try_exists().unwrap_err().raw_os_error(),
        Some(ERRNO_ELOOP)
    );
    let expected_c = CString::new(expected.as_os_str().as_encoded_bytes())?;
    // Reference the Rust crate as well as its C exports, so rustc links the rlib.
    assert!(!scarlet_c::__errno_location().is_null());
    let c_line =
        unsafe { scarlet_libc_probe(file.as_raw_fd(), directory.as_raw_fd(), expected_c.as_ptr()) };
    assert_eq!(c_line, 0, "C ABI check failed at native.c:{c_line}");
    let metadata = file.metadata()?;
    assert_eq!(
        metadata.accessed()?.duration_since(UNIX_EPOCH)?.as_secs(),
        1234
    );
    assert_eq!(
        metadata.modified()?.duration_since(UNIX_EPOCH)?.as_secs(),
        6789
    );
    let link = fs::symlink_metadata("file-link")?;
    assert_eq!(link.accessed()?.duration_since(UNIX_EPOCH)?.as_secs(), 4321);
    assert_eq!(link.modified()?.duration_since(UNIX_EPOCH)?.as_secs(), 8765);

    // The descriptor continues to identify this inode after a rename.
    fs::rename("real/file", "real/moved")?;
    file.set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(9876)))?;
    file.sync_all()?;
    drop(file);
    let metadata = fs::metadata("real/moved")?;
    assert_eq!(
        metadata.accessed()?.duration_since(UNIX_EPOCH)?.as_secs(),
        1234
    );
    assert_eq!(
        metadata.modified()?.duration_since(UNIX_EPOCH)?.as_secs(),
        9876
    );
    assert_eq!(fs::read("real/moved")?, b"timestamp fixture");
    if ext2 {
        let file = File::open("real/moved")?;
        let error = file
            .set_times(
                FileTimes::new()
                    .set_accessed(UNIX_EPOCH + Duration::from_secs(1111))
                    .set_modified(UNIX_EPOCH + Duration::from_secs(u32::MAX as u64 + 1)),
            )
            .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(ERRNO_EOVERFLOW));
        assert_eq!(
            file.metadata()?
                .accessed()?
                .duration_since(UNIX_EPOCH)?
                .as_secs(),
            1234
        );
    }
    // No truncated canonical output, and no writes to the undersized buffer.
    let mut tiny = [0xa5u8; 1];
    let ret = unsafe {
        scarlet_sys::syscall3(
            Syscall::VfsCanonicalize,
            c"real/moved".as_ptr() as usize,
            tiny.as_mut_ptr() as usize,
            tiny.len(),
        )
    };
    assert_eq!(ret as isize, -(ERRNO_ERANGE as isize));
    assert_eq!(tiny, [0xa5]);
    let ret = unsafe {
        scarlet_sys::syscall3(
            Syscall::VfsCanonicalize,
            0,
            tiny.as_mut_ptr() as usize,
            tiny.len(),
        )
    };
    assert_eq!(ret as isize, -(ERRNO_EFAULT as isize));
    let file = File::open("real/moved")?;
    let ret = unsafe { scarlet_sys::syscall2(Syscall::FileSetTimes, file.as_raw_fd() as usize, 0) };
    assert_eq!(ret as isize, -(ERRNO_EFAULT as isize));
    for times in [
        RawFileTimes {
            version: 2,
            flags: FILE_TIMES_MODIFIED,
            modified: 1111,
            accessed: 0,
        },
        RawFileTimes {
            version: FILE_TIMES_VERSION,
            flags: 0x80,
            modified: 1111,
            accessed: 0,
        },
    ] {
        let ret = unsafe {
            scarlet_sys::syscall2(
                Syscall::FileSetTimes,
                file.as_raw_fd() as usize,
                &times as *const _ as usize,
            )
        };
        assert_eq!(ret as isize, -(scarlet_abi::ERRNO_EINVAL as isize));
    }
    assert_eq!(
        file.metadata()?
            .modified()?
            .duration_since(UNIX_EPOCH)?
            .as_secs(),
        9876
    );
    Ok(())
}

pub fn run(output: &Path) -> Result<(), String> {
    let original = std::env::current_dir().map_err(|error| error.to_string())?;
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let ext2 = output.join("fs-ext2");
        check(&ext2, true)?;
        let tmpfs = output.join("fs-tmpfs");
        fs::create_dir(&tmpfs)?;
        let mount = CString::new(tmpfs.as_os_str().as_encoded_bytes())?;
        let mounted = unsafe {
            scarlet_sys::syscall5(
                Syscall::FsMount,
                c"none".as_ptr() as usize,
                mount.as_ptr() as usize,
                c"tmpfs".as_ptr() as usize,
                0,
                0,
            )
        };
        assert_eq!(mounted, 0, "mount private tmpfs");
        check(&tmpfs, false)?;
        std::env::set_current_dir(&original)?;
        let unmounted =
            unsafe { scarlet_sys::syscall2(Syscall::FsUmount, mount.as_ptr() as usize, 0) };
        assert_eq!(unmounted, 0);
        let marker = File::create(output.join("NATIVE_FS_PASS"))?;
        (&marker).write_all(b"C ABI + Rust std: canonical paths, errno, allocation, timestamps, fsync; ext2 + tmpfs\n")?;
        marker.sync_all()?;
        Ok(())
    })();
    let restored = std::env::set_current_dir(original);
    if let Err(error) = result {
        let error = error.to_string();
        let _ = fs::write(output.join("NATIVE_FS_FAIL"), &error);
        return Err(error);
    }
    restored.map_err(|error| error.to_string())
}
