//! Filesystem capabilities not fully exposed by the current Scarlet std API.
//!
//! Ordinary metadata/canonicalization/sync fixes belong to the supported Rust
//! toolchain (Scarlet Rust 26d00a8 or newer), not application workarounds. This
//! adapter retains richer permission metadata, atomic exclusive creation and
//! explicit policies for operations the native ABI cannot safely provide.
//! Native metadata exposes only coarse R/W/X flags. File creation retains those
//! capabilities for the owner; it cannot reproduce POSIX ownership, group bits,
//! ACLs or extended attributes. Existing permissions cannot be changed.
//! Native recursive deletion and no-replace rename fail rather than simulate
//! primitives the ABI does not expose.
#![allow(dead_code, unused_imports)]
pub use std::fs::{
    File, ReadDir, canonicalize, create_dir, read_dir, read_link, remove_dir, remove_file, rename,
};
use std::io;
use std::path::{Path, PathBuf};

// The supported native toolchain maps filesystem errno to the same ErrorKind
// values as the host. Older environment toolchains must not run these apps.
fn errno(error: i32) -> io::Error {
    io::Error::from_raw_os_error(error)
}

fn checked(result: usize) -> io::Result<usize> {
    if (result as isize) < 0 {
        Err(errno(
            (result as isize)
                .checked_neg()
                .and_then(|n| i32::try_from(n).ok())
                .unwrap_or(5),
        ))
    } else {
        Ok(result)
    }
}

#[cfg(not(target_os = "scarlet"))]
pub use std::fs::{
    FileType, Metadata, Permissions, metadata, remove_dir_all, set_permissions, symlink_metadata,
};

#[cfg(not(target_os = "scarlet"))]
pub fn create_new(path: impl AsRef<Path>) -> io::Result<File> {
    create_new_with_permissions(path, None)
}
/// Exclusively create a file with owner-only permissions at creation time.
/// Group, world and special permission bits are always stripped.
#[cfg(not(target_os = "scarlet"))]
pub fn create_new_with_owner_mode(path: impl AsRef<Path>, mode: u32) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode & 0o700)
            .open(path)
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Owner permission creation is unavailable",
        ))
    }
}
#[cfg(not(target_os = "scarlet"))]
pub fn create_new_with_permissions(
    path: impl AsRef<Path>,
    permissions: Option<&Permissions>,
) -> io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    if let Some(permissions) = permissions {
        file.set_permissions(permissions.clone())?;
    }
    Ok(file)
}
#[cfg(not(target_os = "scarlet"))]
pub fn open_read_nofollow(path: impl AsRef<Path>) -> io::Result<File> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(0x20000)
            .open(path)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let _ = path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "No-follow file opening is unavailable on this host",
        ))
    }
}

#[cfg(target_os = "scarlet")]
mod native {
    use super::*;
    use scarlet_abi::fs::*;
    use scarlet_sys::{RawFileMetadata, Syscall, syscall1, syscall2, syscall3, syscall4};
    use std::ffi::CString;
    use std::os::fd::FromRawFd;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn path_string(path: &Path) -> io::Result<CString> {
        let bytes = path.as_os_str().as_encoded_bytes();
        if bytes.is_empty() {
            return Err(errno(ERRNO_ENOENT));
        }
        if bytes.len() >= PATH_MAX {
            return Err(errno(ERRNO_ENAMETOOLONG));
        }
        CString::new(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Path contains NUL"))
    }
    #[derive(Clone, Debug)]
    pub struct Metadata {
        raw: RawFileMetadata,
    }
    impl Metadata {
        pub fn len(&self) -> u64 {
            self.raw.size
        }
        pub fn is_file(&self) -> bool {
            self.file_type().is_file()
        }
        pub fn is_dir(&self) -> bool {
            self.file_type().is_dir()
        }
        pub fn file_type(&self) -> FileType {
            FileType(self.raw.file_type)
        }
        pub fn permissions(&self) -> Permissions {
            Permissions(self.raw.permissions)
        }
        pub fn modified(&self) -> io::Result<SystemTime> {
            UNIX_EPOCH
                .checked_add(Duration::from_secs(self.raw.modified))
                .ok_or_else(|| io::Error::other("Timestamp overflow"))
        }
        pub fn accessed(&self) -> io::Result<SystemTime> {
            UNIX_EPOCH
                .checked_add(Duration::from_secs(self.raw.accessed))
                .ok_or_else(|| io::Error::other("Timestamp overflow"))
        }
        pub fn file_id(&self) -> u64 {
            self.raw.file_id
        }
        pub fn link_count(&self) -> u32 {
            self.raw.link_count
        }
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct FileType(u32);
    impl FileType {
        pub fn is_file(&self) -> bool {
            self.0 == scarlet_sys::FILE_TYPE_REGULAR
        }
        pub fn is_dir(&self) -> bool {
            self.0 == scarlet_sys::FILE_TYPE_DIRECTORY
        }
        pub fn is_symlink(&self) -> bool {
            self.0 == scarlet_sys::FILE_TYPE_SYMLINK
        }
    }
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Permissions(u32);
    impl Permissions {
        pub fn readonly(&self) -> bool {
            self.0 & scarlet_sys::FILE_PERMISSION_WRITE == 0
        }
        pub fn set_readonly(&mut self, readonly: bool) {
            if readonly {
                self.0 &= !scarlet_sys::FILE_PERMISSION_WRITE;
            } else {
                self.0 |= scarlet_sys::FILE_PERMISSION_WRITE;
            }
        }
        pub(super) fn owner_mode(&self) -> u32 {
            (if self.0 & scarlet_sys::FILE_PERMISSION_READ != 0 {
                0o400
            } else {
                0
            }) | (if self.0 & scarlet_sys::FILE_PERMISSION_WRITE != 0 {
                0o200
            } else {
                0
            }) | (if self.0 & scarlet_sys::FILE_PERMISSION_EXECUTE != 0 {
                0o100
            } else {
                0
            })
        }
    }
    fn query(path: &Path, nofollow: bool) -> io::Result<Metadata> {
        let path = path_string(path)?;
        let mut raw = RawFileMetadata::default();
        // SAFETY: NUL path and exclusively borrowed repr(C) output remain live
        // for the synchronous call. The kernel does not retain either pointer.
        checked(unsafe {
            syscall3(
                Syscall::VfsMetadataWithStatus,
                path.as_ptr() as usize,
                &mut raw as *mut RawFileMetadata as usize,
                usize::from(nofollow),
            )
        })?;
        Ok(Metadata { raw })
    }
    pub fn metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
        query(path.as_ref(), false)
    }
    pub fn symlink_metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
        query(path.as_ref(), true)
    }
    fn open(path: &Path, flags: u32, mode: u32) -> io::Result<File> {
        let path = path_string(path)?;
        // SAFETY: scalar flags are ABI constants; live path buffer is read only.
        // A nonnegative result transfers one exclusively owned descriptor.
        let descriptor = checked(unsafe {
            syscall4(
                Syscall::VfsOpenAt,
                CURRENT_DIRECTORY,
                path.as_ptr() as usize,
                flags as usize,
                mode as usize,
            )
        })?;
        let descriptor =
            i32::try_from(descriptor).map_err(|_| io::Error::other("Invalid file descriptor"))?;
        // SAFETY: the successful syscall returned a fresh descriptor which has
        // no other Rust owner. File closes it exactly once.
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
    pub fn create_new(path: impl AsRef<Path>) -> io::Result<File> {
        create_new_with_permissions(path, None)
    }
    pub fn create_new_with_owner_mode(path: impl AsRef<Path>, mode: u32) -> io::Result<File> {
        open(
            path.as_ref(),
            VFS_O_WRONLY | VFS_O_CREAT | VFS_O_EXCL | VFS_O_NOFOLLOW | VFS_O_CLOEXEC,
            mode & 0o700,
        )
    }
    pub fn create_new_with_permissions(
        path: impl AsRef<Path>,
        permissions: Option<&Permissions>,
    ) -> io::Result<File> {
        open(
            path.as_ref(),
            VFS_O_WRONLY | VFS_O_CREAT | VFS_O_EXCL | VFS_O_NOFOLLOW | VFS_O_CLOEXEC,
            permissions.map_or(0o600, Permissions::owner_mode),
        )
    }
    pub fn open_read_nofollow(path: impl AsRef<Path>) -> io::Result<File> {
        open(
            path.as_ref(),
            VFS_O_RDONLY | VFS_O_NOFOLLOW | VFS_O_CLOEXEC,
            0,
        )
    }
    pub fn remove_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
        // Never emulate recursive removal through path-based ancestor traversal.
        // The ABI lacks anchored unlinkat, so only an empty real folder is safe.
        let path = path.as_ref();
        let metadata = symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return remove_file(path);
        }
        if !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "Expected a folder",
            ));
        }
        match remove_dir(path) {
            Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Deleting nonempty folders is unavailable until Scarlet supports safe directory-relative removal; no contents were removed",
            )),
            result => result,
        }
    }
    pub fn set_permissions(path: impl AsRef<Path>, permissions: Permissions) -> io::Result<()> {
        if symlink_metadata(path)?.permissions() == permissions {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Scarlet cannot change existing file permissions; create the destination with the required permissions",
            ))
        }
    }
}
#[cfg(target_os = "scarlet")]
pub use native::*;

/// Stable signature for capability/permission change detection. Native metadata
/// can describe owner R/W/X capabilities only, not the original POSIX mode.
pub fn permissions_mode(permissions: &Permissions) -> u32 {
    #[cfg(target_os = "scarlet")]
    {
        permissions.owner_mode()
    }
    #[cfg(all(not(target_os = "scarlet"), unix))]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.mode()
    }
    #[cfg(all(not(target_os = "scarlet"), not(unix)))]
    {
        if permissions.readonly() { 0o400 } else { 0o600 }
    }
}
pub fn is_executable(permissions: &Permissions) -> bool {
    permissions_mode(permissions) & 0o111 != 0
}

/// The native syscall only exposes replacing rename, so Files cannot use it
/// for a move that promises to preserve an existing destination.
pub fn require_noreplace_move() -> io::Result<()> {
    #[cfg(target_os = "scarlet")]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Move and rename are unavailable until Scarlet exposes atomic no-replace rename. The original was kept; use Copy instead.",
        ))
    }
    #[cfg(not(target_os = "scarlet"))]
    {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filesystem_errors_have_actionable_kinds() {
        for (code, kind) in [
            (2, io::ErrorKind::NotFound),
            (17, io::ErrorKind::AlreadyExists),
            (13, io::ErrorKind::PermissionDenied),
            (39, io::ErrorKind::DirectoryNotEmpty),
        ] {
            assert_eq!(
                checked((-(code as isize)) as usize).unwrap_err().kind(),
                kind
            );
        }
        assert_eq!(checked(0).unwrap(), 0);
        assert_eq!(checked(42).unwrap(), 42);
    }
    #[test]
    fn exclusive_creation_never_overwrites_existing_file() {
        let path =
            std::env::temp_dir().join(format!("scarlet-exclusive-test-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, b"preserved").unwrap();
        assert_eq!(
            create_new(&path).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"preserved");
        std::fs::remove_file(&path).unwrap();
    }
}
