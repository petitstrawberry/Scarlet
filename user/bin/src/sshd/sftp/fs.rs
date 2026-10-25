//! Host validation and Scarlet Native adapters for the same SFTP request engine.
use super::{Attrs, Error, FAILURE, Result, UNSUPPORTED};
use std::{
    format, fs,
    io::{self, SeekFrom},
    string::String,
    vec,
    vec::Vec,
};

#[cfg(not(target_os = "scarlet"))]
use std::io::{Read, Seek, Write};

fn io_error(error: io::Error) -> Error {
    match error.kind() {
        io::ErrorKind::NotFound => Error(2, "No such file or directory"),
        io::ErrorKind::PermissionDenied => Error(3, "Permission denied"),
        io::ErrorKind::Unsupported => UNSUPPORTED,
        _ => FAILURE,
    }
}
#[cfg(target_os = "scarlet")]
fn checked(value: usize) -> Result<usize> {
    if (value as isize) >= 0 {
        return Ok(value);
    }
    Err(match -(value as isize) {
        2 | 20 => Error(2, "No such file or directory"),
        13 | 30 => Error(3, "Permission denied or read-only filesystem"),
        38 | 95 => UNSUPPORTED,
        _ => FAILURE,
    })
}
#[cfg(target_os = "scarlet")]
fn cpath(path: &str) -> Result<Vec<u8>> {
    if path.len() >= 1024 || path.as_bytes().contains(&0) {
        return Err(FAILURE);
    }
    let mut bytes = path.as_bytes().to_vec();
    bytes.push(0);
    Ok(bytes)
}
#[cfg(target_os = "scarlet")]
fn native_attrs(raw: scarlet_sys::RawFileMetadata) -> Attrs {
    let mode = match raw.file_type {
        scarlet_sys::FILE_TYPE_REGULAR => 0o100000,
        scarlet_sys::FILE_TYPE_DIRECTORY => 0o040000,
        scarlet_sys::FILE_TYPE_SYMLINK => 0o120000,
        scarlet_sys::FILE_TYPE_CHAR_DEVICE => 0o020000,
        scarlet_sys::FILE_TYPE_BLOCK_DEVICE => 0o060000,
        scarlet_sys::FILE_TYPE_PIPE => 0o010000,
        scarlet_sys::FILE_TYPE_SOCKET => 0o140000,
        _ => 0,
    } | if raw.permissions & 1 != 0 { 0o400 } else { 0 }
        | if raw.permissions & 2 != 0 { 0o200 } else { 0 }
        | if raw.permissions & 4 != 0 { 0o100 } else { 0 };
    Attrs {
        size: Some(raw.size),
        mode: Some(mode),
        times: Some((
            raw.accessed.min(u32::MAX as u64) as u32,
            raw.modified.min(u32::MAX as u64) as u32,
        )),
        ..Attrs::default()
    }
}
#[cfg(not(target_os = "scarlet"))]
fn host_attrs(metadata: fs::Metadata) -> Attrs {
    use std::os::unix::fs::MetadataExt;
    Attrs {
        size: Some(metadata.len()),
        mode: Some(metadata.mode()),
        times: Some((
            metadata.atime().clamp(0, u32::MAX as i64) as u32,
            metadata.mtime().clamp(0, u32::MAX as i64) as u32,
        )),
        ..Attrs::default()
    }
}

pub(super) fn metadata(path: &str, nofollow: bool) -> Result<Attrs> {
    #[cfg(target_os = "scarlet")]
    {
        let path = cpath(path)?;
        let mut raw = scarlet_sys::RawFileMetadata::default();
        // SAFETY: the NUL pathname and exclusively borrowed repr(C) output
        // remain valid for this synchronous call; the kernel retains neither.
        checked(unsafe {
            scarlet_sys::syscall3(
                scarlet_sys::Syscall::VfsMetadataWithStatus,
                path.as_ptr() as usize,
                &mut raw as *mut _ as usize,
                usize::from(nofollow),
            )
        })?;
        Ok(native_attrs(raw))
    }
    #[cfg(not(target_os = "scarlet"))]
    {
        (if nofollow {
            fs::symlink_metadata(path)
        } else {
            fs::metadata(path)
        })
        .map(host_attrs)
        .map_err(io_error)
    }
}

pub(super) struct File(fs::File);
impl File {
    pub fn open(path: &str, flags: u32, mode: u32) -> Result<Self> {
        #[cfg(target_os = "scarlet")]
        let file = {
            use scarlet_abi::fs::*;
            let path = cpath(path)?;
            let access = match flags & 3 {
                1 => VFS_O_RDONLY,
                2 => VFS_O_WRONLY,
                _ => VFS_O_RDWR,
            };
            let native = access
                | if flags & 4 != 0 { VFS_O_APPEND } else { 0 }
                | if flags & 8 != 0 { VFS_O_CREAT } else { 0 }
                | if flags & 16 != 0 { VFS_O_TRUNC } else { 0 }
                | if flags & 32 != 0 { VFS_O_EXCL } else { 0 };
            // SAFETY: live NUL path, scalar ABI flags/mode, no retained pointers.
            // A successful open returns one fresh owned descriptor.
            let raw = checked(unsafe {
                scarlet_sys::syscall4(
                    scarlet_sys::Syscall::VfsOpenAt,
                    CURRENT_DIRECTORY,
                    path.as_ptr() as usize,
                    native as usize,
                    mode as usize,
                )
            })?;
            // SAFETY: the descriptor just returned has no other Rust owner.
            let handle =
                unsafe { std::handle::Handle::from_raw(i32::try_from(raw).map_err(|_| FAILURE)?) }
                    .map_err(|_| FAILURE)?;
            fs::File::from_handle(handle).map_err(io_error)?
        };
        #[cfg(not(target_os = "scarlet"))]
        let file = {
            use std::os::unix::fs::OpenOptionsExt;
            fs::OpenOptions::new()
                .read(flags & 1 != 0)
                .write(flags & 2 != 0)
                .append(flags & 4 != 0)
                .create(flags & 8 != 0)
                .truncate(flags & 16 != 0)
                .create_new(flags & 32 != 0)
                .mode(mode)
                .open(path)
                .map_err(io_error)?
        };
        let result = Self(file);
        if result.metadata()?.mode.unwrap_or(0) & 0o170000 != 0o100000 {
            return Err(Error(4, "Only regular files can be transferred"));
        }
        Ok(result)
    }
    pub fn metadata(&self) -> Result<Attrs> {
        #[cfg(target_os = "scarlet")]
        {
            let mut raw = scarlet_sys::RawFileMetadata::default();
            // SAFETY: the file owns the live descriptor and the repr(C) output
            // is exclusively borrowed until the synchronous metadata call ends.
            checked(unsafe {
                scarlet_sys::syscall2(
                    scarlet_sys::Syscall::FileMetadata,
                    self.0.as_raw() as usize,
                    &mut raw as *mut _ as usize,
                )
            })?;
            Ok(native_attrs(raw))
        }
        #[cfg(not(target_os = "scarlet"))]
        {
            self.0.metadata().map(host_attrs).map_err(io_error)
        }
    }
    pub fn read(&mut self, offset: u64, count: usize) -> Result<Vec<u8>> {
        if offset > i64::MAX as u64 {
            return Err(FAILURE);
        }
        self.0.seek(SeekFrom::Start(offset)).map_err(io_error)?;
        let mut bytes = vec![0; count];
        let n = loop {
            match self.0.read(&mut bytes) {
                Ok(n) => break n,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break 0,
                Err(error) => return Err(io_error(error)),
            }
        };
        bytes.truncate(n);
        Ok(bytes)
    }
    pub fn write(&mut self, offset: u64, bytes: &[u8], append: bool) -> Result<()> {
        if offset > i64::MAX as u64
            || offset
                .checked_add(bytes.len() as u64)
                .is_none_or(|end| end > i64::MAX as u64)
        {
            return Err(FAILURE);
        }
        if !append {
            self.0.seek(SeekFrom::Start(offset)).map_err(io_error)?;
        }
        let mut remaining = bytes;
        while !remaining.is_empty() {
            match self.0.write(remaining) {
                Ok(0) => return Err(FAILURE),
                Ok(n) => remaining = &remaining[n..],
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(io_error(error)),
            }
        }
        Ok(())
    }
    pub fn set_attrs(&mut self, attrs: Attrs) -> Result<()> {
        if attrs.uid_gid.is_some() || attrs.extended {
            return Err(UNSUPPORTED);
        }
        #[cfg(target_os = "scarlet")]
        if let Some(mode) = attrs.mode {
            // Native permissions are coarse owner capabilities; there is no
            // chmod syscall. Only an already-satisfied request can succeed.
            if self.metadata()?.mode.unwrap_or(0) & 0o7777 != mode & 0o7777 {
                return Err(UNSUPPORTED);
            }
        }
        if attrs.size.is_some_and(|size| size > i64::MAX as u64) {
            return Err(FAILURE);
        }
        if let Some(size) = attrs.size {
            self.0.set_len(size).map_err(io_error)?;
        }
        #[cfg(not(target_os = "scarlet"))]
        if let Some(mode) = attrs.mode {
            use std::os::unix::fs::PermissionsExt;
            self.0
                .set_permissions(fs::Permissions::from_mode(mode & 0o7777))
                .map_err(io_error)?;
        }
        if let Some((accessed, modified)) = attrs.times {
            #[cfg(target_os = "scarlet")]
            {
                let times = scarlet_abi::fs::RawFileTimes {
                    version: 1,
                    flags: 3,
                    accessed: accessed as u64,
                    modified: modified as u64,
                };
                // SAFETY: live owned file and read-only repr(C) timestamp record.
                checked(unsafe {
                    scarlet_sys::syscall2(
                        scarlet_sys::Syscall::FileSetTimes,
                        self.0.as_raw() as usize,
                        &times as *const _ as usize,
                    )
                })?;
            }
            #[cfg(not(target_os = "scarlet"))]
            {
                use std::time::{Duration, UNIX_EPOCH};
                self.0
                    .set_times(
                        fs::FileTimes::new()
                            .set_accessed(UNIX_EPOCH + Duration::from_secs(accessed as u64))
                            .set_modified(UNIX_EPOCH + Duration::from_secs(modified as u64)),
                    )
                    .map_err(io_error)?;
            }
        }
        Ok(())
    }
    pub fn sync(&self) -> Result<()> {
        #[cfg(target_os = "scarlet")]
        // SAFETY: the descriptor stays owned for the synchronous FileSync call.
        {
            checked(unsafe {
                scarlet_sys::syscall1(scarlet_sys::Syscall::FileSync, self.0.as_raw() as usize)
            })
            .map(|_| ())
        }
        #[cfg(not(target_os = "scarlet"))]
        {
            self.0.sync_all().map_err(io_error)
        }
    }
}
pub(super) fn set_attrs(path: &str, attrs: Attrs) -> Result<()> {
    // Open an existing regular file only; SETSTAT must never create one.
    let mut file = File::open(path, if attrs.size.is_some() { 2 } else { 1 }, 0)?;
    file.set_attrs(attrs)
}

pub(super) struct Directory {
    path: String,
    #[cfg(target_os = "scarlet")]
    inner: fs::File,
    #[cfg(not(target_os = "scarlet"))]
    inner: fs::ReadDir,
}
impl Directory {
    pub fn open(path: &str) -> Result<Self> {
        if metadata(path, false)?.mode.unwrap_or(0) & 0o170000 != 0o040000 {
            return Err(Error(2, "Not a directory"));
        }
        #[cfg(target_os = "scarlet")]
        let inner = fs::File::open(path).map_err(io_error)?;
        #[cfg(not(target_os = "scarlet"))]
        let inner = fs::read_dir(path).map_err(io_error)?;
        Ok(Self {
            path: String::from(path),
            inner,
        })
    }
    pub fn metadata(&self) -> Result<Attrs> {
        metadata(&self.path, false)
    }
    pub fn next(&mut self) -> Result<Option<(String, Attrs)>> {
        loop {
            #[cfg(target_os = "scarlet")]
            let name = match self.inner.read_dir().map_err(io_error)? {
                Some(entry) => entry.name,
                None => return Ok(None),
            };
            #[cfg(not(target_os = "scarlet"))]
            let name = match self.inner.next() {
                Some(entry) => entry
                    .map_err(io_error)?
                    .file_name()
                    .into_string()
                    .map_err(|_| FAILURE)?,
                None => return Ok(None),
            };
            if name == "." || name == ".." {
                continue;
            }
            match metadata(&format!("{}/{}", self.path, name), true) {
                Ok(attrs) => return Ok(Some((name, attrs))),
                Err(error) if error.0 == 2 => continue, // Concurrent unlink.
                Err(error) => return Err(error),
            }
        }
    }
}
pub(super) fn mkdir(path: &str, mode: u32) -> Result<()> {
    #[cfg(target_os = "scarlet")]
    {
        if mode & 0o700 != 0o700 {
            return Err(UNSUPPORTED);
        }
        let path = cpath(path)?;
        // SAFETY: the NUL path is readable for this synchronous call only.
        checked(unsafe {
            scarlet_sys::syscall1(
                scarlet_sys::Syscall::VfsCreateDirectoryWithStatus,
                path.as_ptr() as usize,
            )
        })
        .map(|_| ())
    }
    #[cfg(not(target_os = "scarlet"))]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .mode(mode)
            .create(path)
            .map_err(io_error)
    }
}
pub(super) fn remove(path: &str, directory: bool) -> Result<()> {
    #[cfg(target_os = "scarlet")]
    {
        let path = cpath(path)?;
        // SAFETY: live NUL path and scalar kind flag; no retained pointers.
        checked(unsafe {
            scarlet_sys::syscall2(
                scarlet_sys::Syscall::VfsRemoveWithStatus,
                path.as_ptr() as usize,
                usize::from(directory),
            )
        })
        .map(|_| ())
    }
    #[cfg(not(target_os = "scarlet"))]
    {
        (if directory {
            fs::remove_dir(path)
        } else {
            fs::remove_file(path)
        })
        .map_err(io_error)
    }
}
pub(super) fn rename(from: &str, to: &str, replace: bool) -> Result<()> {
    #[cfg(target_os = "scarlet")]
    {
        let from = cpath(from)?;
        let to = cpath(to)?;
        // SAFETY: both NUL path buffers outlive the synchronous operation.
        checked(unsafe {
            scarlet_sys::syscall3(
                scarlet_sys::Syscall::VfsRenameWithStatus,
                from.as_ptr() as usize,
                to.as_ptr() as usize,
                if replace { 0 } else { 1 },
            )
        })
        .map(|_| ())
    }
    #[cfg(not(target_os = "scarlet"))]
    {
        if replace {
            return fs::rename(from, to).map_err(io_error);
        }
        use std::ffi::CString;
        let from = CString::new(from).map_err(|_| FAILURE)?;
        let to = CString::new(to).map_err(|_| FAILURE)?;
        #[cfg(target_os = "macos")]
        unsafe extern "C" {
            fn renamex_np(
                from: *const core::ffi::c_char,
                to: *const core::ffi::c_char,
                flags: u32,
            ) -> i32;
        }
        #[cfg(target_os = "linux")]
        unsafe extern "C" {
            fn renameat2(
                a: i32,
                from: *const core::ffi::c_char,
                b: i32,
                to: *const core::ffi::c_char,
                flags: u32,
            ) -> i32;
        }
        // SAFETY: C strings are read-only and live for the synchronous call.
        #[cfg(target_os = "macos")]
        let rc = unsafe { renamex_np(from.as_ptr(), to.as_ptr(), 4) };
        #[cfg(target_os = "linux")]
        let rc = unsafe { renameat2(-100, from.as_ptr(), -100, to.as_ptr(), 1) };
        if rc == 0 {
            Ok(())
        } else {
            Err(io_error(io::Error::last_os_error()))
        }
    }
}
pub(super) fn canonicalize(path: &str) -> Result<String> {
    #[cfg(target_os = "scarlet")]
    {
        let path = cpath(path)?;
        let mut out = [0u8; 1024];
        // SAFETY: live NUL input and an exclusively borrowed output buffer of
        // the advertised length; neither pointer is retained by the kernel.
        let n = checked(unsafe {
            scarlet_sys::syscall3(
                scarlet_sys::Syscall::VfsCanonicalize,
                path.as_ptr() as usize,
                out.as_mut_ptr() as usize,
                out.len(),
            )
        })?;
        core::str::from_utf8(out.get(..n).ok_or(FAILURE)?)
            .map(String::from)
            .map_err(|_| FAILURE)
    }
    #[cfg(not(target_os = "scarlet"))]
    {
        fs::canonicalize(path)
            .map_err(io_error)?
            .into_os_string()
            .into_string()
            .map_err(|_| FAILURE)
    }
}
pub(super) fn readlink(path: &str) -> Result<String> {
    #[cfg(target_os = "scarlet")]
    {
        fs::read_link(path).map_err(io_error)
    }
    #[cfg(not(target_os = "scarlet"))]
    {
        fs::read_link(path)
            .map_err(io_error)?
            .into_os_string()
            .into_string()
            .map_err(|_| FAILURE)
    }
}
pub(super) fn symlink(target: &str, path: &str) -> Result<()> {
    #[cfg(target_os = "scarlet")]
    {
        fs::create_symlink(path, target).map_err(io_error)
    }
    #[cfg(not(target_os = "scarlet"))]
    {
        std::os::unix::fs::symlink(target, path).map_err(io_error)
    }
}
