//! C directory streams backed by the Native fixed-size directory records.

use std::ffi::{CStr, c_char, c_int, c_void};

use scarlet_abi::{
    ERRNO_EBADF, ERRNO_EIO, FILE_TYPE_BLOCK_DEVICE, FILE_TYPE_CHAR_DEVICE, FILE_TYPE_DIRECTORY,
    FILE_TYPE_PIPE, FILE_TYPE_REGULAR, FILE_TYPE_SOCKET, FILE_TYPE_SYMLINK,
};

use crate::descriptor;

const DT_UNKNOWN: u8 = 0;
const DT_FIFO: u8 = 1;
const DT_CHR: u8 = 2;
const DT_DIR: u8 = 4;
const DT_BLK: u8 = 6;
const DT_REG: u8 = 8;
const DT_LNK: u8 = 10;
const DT_SOCK: u8 = 12;

#[repr(C)]
struct RawDirEntry {
    file_id: u64,
    size: u64,
    file_type: u8,
    name_len: u8,
    reserved: [u8; 6],
    name: [u8; 256],
}

impl Default for RawDirEntry {
    fn default() -> Self {
        Self {
            file_id: 0,
            size: 0,
            file_type: 0,
            name_len: 0,
            reserved: [0; 6],
            name: [0; 256],
        }
    }
}

#[repr(C)]
pub struct Dirent {
    inode: u64,
    offset: i64,
    record_len: u16,
    file_type: u8,
    name: [c_char; 256],
}

impl Default for Dirent {
    fn default() -> Self {
        Self {
            inode: 0,
            offset: 0,
            record_len: 0,
            file_type: 0,
            name: [0; 256],
        }
    }
}

pub struct DirStream {
    fd: c_int,
    path: Vec<u8>,
    current: Dirent,
    position: i64,
}

fn dirent_type(file_type: u8) -> u8 {
    match u32::from(file_type) {
        FILE_TYPE_PIPE => DT_FIFO,
        FILE_TYPE_CHAR_DEVICE => DT_CHR,
        FILE_TYPE_DIRECTORY => DT_DIR,
        FILE_TYPE_BLOCK_DEVICE => DT_BLK,
        FILE_TYPE_REGULAR => DT_REG,
        FILE_TYPE_SYMLINK => DT_LNK,
        FILE_TYPE_SOCKET => DT_SOCK,
        _ => DT_UNKNOWN,
    }
}

/// # Safety
/// `path` points to a NUL-terminated pathname.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn opendir(path: *const c_char) -> *mut DirStream {
    if path.is_null() {
        crate::fail(scarlet_abi::fs::ERRNO_EFAULT);
        return std::ptr::null_mut();
    }
    // SAFETY: the caller provides a NUL-terminated path.
    let owned_path = unsafe { CStr::from_ptr(path) }.to_bytes_with_nul().to_vec();
    // SAFETY: the owned vector contains the original NUL terminator.
    let fd = unsafe {
        descriptor::open_impl(
            owned_path.as_ptr().cast(),
            descriptor::O_RDONLY | descriptor::O_DIRECTORY,
            0,
        )
    };
    if fd < 0 {
        return std::ptr::null_mut();
    }
    Box::into_raw(Box::new(DirStream {
        fd,
        path: owned_path,
        current: Dirent::default(),
        position: 0,
    }))
}

/// # Safety
/// `stream` is a live pointer returned by opendir, used without concurrent
/// mutation. The returned pointer remains valid until the next readdir or
/// closedir on that stream.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn readdir(stream: *mut DirStream) -> *mut Dirent {
    if stream.is_null() {
        crate::fail(ERRNO_EBADF);
        return std::ptr::null_mut();
    }
    // SAFETY: the caller owns a live, exclusively accessible stream.
    let stream = unsafe { &mut *stream };
    let mut raw = RawDirEntry::default();
    // SAFETY: raw has the Native fixed-size directory record layout.
    let bytes = unsafe {
        descriptor::read(
            stream.fd,
            (&raw mut raw as *mut RawDirEntry).cast::<c_void>(),
            size_of::<RawDirEntry>(),
        )
    };
    if bytes == 0 {
        return std::ptr::null_mut();
    }
    if bytes < 0 {
        return std::ptr::null_mut();
    }
    if bytes as usize != size_of::<RawDirEntry>() || raw.name_len as usize >= raw.name.len() {
        crate::fail(ERRNO_EIO);
        return std::ptr::null_mut();
    }
    let Some(position) = stream.position.checked_add(1) else {
        crate::fail(scarlet_abi::fs::ERRNO_EOVERFLOW);
        return std::ptr::null_mut();
    };
    let mut entry = Dirent {
        inode: raw.file_id,
        offset: position,
        record_len: size_of::<Dirent>() as u16,
        file_type: dirent_type(raw.file_type),
        ..Dirent::default()
    };
    for (output, byte) in entry
        .name
        .iter_mut()
        .zip(raw.name.iter())
        .take(raw.name_len as usize)
    {
        *output = *byte as c_char;
    }
    stream.current = entry;
    stream.position = position;
    &raw mut stream.current
}

/// # Safety
/// `stream` is a live pointer returned by opendir and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn closedir(stream: *mut DirStream) -> c_int {
    if stream.is_null() {
        return crate::fail(ERRNO_EBADF);
    }
    // SAFETY: ownership transfers back exactly once from the C caller.
    let stream = unsafe { Box::from_raw(stream) };
    descriptor::close(stream.fd)
}

/// # Safety
/// `stream` is a live pointer returned by opendir, used without concurrent
/// mutation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rewinddir(stream: *mut DirStream) {
    if stream.is_null() {
        crate::fail(ERRNO_EBADF);
        return;
    }
    // SAFETY: the caller owns a live, exclusively accessible stream.
    let stream = unsafe { &mut *stream };
    // SAFETY: the saved path retains its NUL terminator.
    let replacement = unsafe {
        descriptor::open_impl(
            stream.path.as_ptr().cast(),
            descriptor::O_RDONLY | descriptor::O_DIRECTORY,
            0,
        )
    };
    if replacement >= 0 {
        let old = std::mem::replace(&mut stream.fd, replacement);
        stream.position = 0;
        descriptor::close(old);
    }
}
