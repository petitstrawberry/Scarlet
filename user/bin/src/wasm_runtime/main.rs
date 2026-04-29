#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![allow(unsafe_op_in_unsafe_fn, dead_code)]

extern crate alloc;
#[cfg(not(test))]
use scarlet_std as std;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use core::cell::UnsafeCell;
use std::fs::{
    File, OpenOptions, create_directory, create_symlink, list_directory, read_link,
    remove_directory, remove_file,
};
use std::io::SeekFrom;
use std::{format, println, vec::Vec};
use wasm_jit::runtime::HostOps;

const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];

const ESUCCESS: u32 = 0;
const EBADF: u32 = 8;
const EEXIST: u32 = 20;
const EINVAL: u32 = 28;
const EISDIR: u32 = 31;
const ENOENT: u32 = 44;
const ENOSYS: u32 = 52;
const ENOTDIR: u32 = 54;
const ENOTEMPTY: u32 = 55;

const FILETYPE_CHARACTER_DEVICE: u8 = 2;
const FILETYPE_DIRECTORY: u8 = 3;
const FILETYPE_REGULAR_FILE: u8 = 4;
const FILETYPE_SYMBOLIC_LINK: u8 = 7;

const PREOPEN_FD: u32 = 3;

struct FdEntry {
    kind: FdKind,
}

enum FdKind {
    Stdin,
    Stdout,
    Stderr,
    File(UnsafeCell<File>),
    PreopenDir { path: String },
}

struct WasiRuntime {
    fds: BTreeMap<u32, FdEntry>,
    next_fd: u32,
    preopen_path: String,
}

impl WasiRuntime {
    fn new(preopen_dirs: &[String]) -> Self {
        let mut fds = BTreeMap::new();
        fds.insert(
            0,
            FdEntry {
                kind: FdKind::Stdin,
            },
        );
        fds.insert(
            1,
            FdEntry {
                kind: FdKind::Stdout,
            },
        );
        fds.insert(
            2,
            FdEntry {
                kind: FdKind::Stderr,
            },
        );
        let mut next_fd = PREOPEN_FD;
        for dir in preopen_dirs {
            fds.insert(
                next_fd,
                FdEntry {
                    kind: FdKind::PreopenDir { path: dir.clone() },
                },
            );
            next_fd += 1;
        }
        let preopen_path = preopen_dirs.first().cloned().unwrap_or_default();
        Self {
            fds,
            next_fd,
            preopen_path,
        }
    }

    fn alloc_fd(&mut self) -> u32 {
        while self.fds.contains_key(&self.next_fd) {
            self.next_fd += 1;
        }
        let fd = self.next_fd;
        self.next_fd += 1;
        fd
    }
}

static mut WASI_RUNTIME: *mut WasiRuntime = core::ptr::null_mut();
static mut RANDOM_STATE: u64 = 0x1234_5678_9ABC_DEF0;
static mut FAKE_TIME_NS: u64 = 1_700_000_000_000_000_000;
static mut WASM_MEMORY_BASE: *mut u8 = core::ptr::null_mut();
static mut WASM_MEMORY_CAP: usize = 0;
static mut WASI_ARGS_STORE: *const Vec<Vec<u8>> = core::ptr::null();
static mut WASI_ENV_STORE: *const Vec<Vec<u8>> = core::ptr::null();

unsafe extern "C" fn wasm_memory_realloc(
    _old_base: *mut u8,
    _old_cap: usize,
    new_cap: usize,
) -> *mut u8 {
    use std::handle::capability::memory_mapping::{flags, mmap_anonymous, prot};
    let new_base = match mmap_anonymous(0, new_cap, prot::READ | prot::WRITE, flags::PRIVATE) {
        Ok(addr) => addr as *mut u8,
        Err(_) => return core::ptr::null_mut(),
    };
    let old_base = unsafe { core::ptr::addr_of!(WASM_MEMORY_BASE).read() };
    let old_cap = unsafe { core::ptr::addr_of!(WASM_MEMORY_CAP).read() };
    if !old_base.is_null() && old_cap > 0 {
        core::ptr::copy_nonoverlapping(old_base, new_base, old_cap);
    }
    unsafe {
        core::ptr::addr_of_mut!(WASM_MEMORY_BASE).write(new_base);
        core::ptr::addr_of_mut!(WASM_MEMORY_CAP).write(new_cap);
    }
    new_base
}

unsafe extern "C" fn host_fd_write(fd: u32, data: *const u8, data_len: usize) -> i64 {
    let rt = wasi_runtime();
    let entry = match rt.fds.get(&fd) {
        Some(entry) => entry,
        None => return neg_errno(EBADF),
    };
    let buf = core::slice::from_raw_parts(data, data_len);

    match &entry.kind {
        FdKind::Stdout => match std::io::stdout().write_all(buf) {
            Ok(()) => data_len as i64,
            Err(_) => neg_errno(EBADF),
        },
        FdKind::Stderr => match std::io::stderr().write_all(buf) {
            Ok(()) => data_len as i64,
            Err(_) => neg_errno(EBADF),
        },
        FdKind::File(cell) => {
            let file = &mut *cell.get();
            match file.write(buf) {
                Ok(n) => n as i64,
                Err(_) => neg_errno(EBADF),
            }
        }
        _ => neg_errno(EBADF),
    }
}

unsafe extern "C" fn host_fd_read(fd: u32, buf: *mut u8, buf_len: usize) -> i64 {
    let rt = wasi_runtime();
    let entry = match rt.fds.get(&fd) {
        Some(entry) => entry,
        None => return neg_errno(EBADF),
    };
    let dst = core::slice::from_raw_parts_mut(buf, buf_len);

    match &entry.kind {
        FdKind::Stdin => match std::io::stdin().read(dst) {
            Ok(n) => n as i64,
            Err(_) => neg_errno(EBADF),
        },
        FdKind::File(cell) => {
            let file = &mut *cell.get();
            match file.read(dst) {
                Ok(n) => n as i64,
                Err(_) => neg_errno(EBADF),
            }
        }
        _ => neg_errno(EBADF),
    }
}

unsafe extern "C" fn host_clock_time_get(_clock_id: u32, time: *mut u64) {
    *time = FAKE_TIME_NS;
    FAKE_TIME_NS = FAKE_TIME_NS.saturating_add(1_000_000);
}

unsafe extern "C" fn host_random_get(buf: *mut u8, buf_len: usize) {
    for i in 0..buf_len {
        RANDOM_STATE ^= RANDOM_STATE << 13;
        RANDOM_STATE ^= RANDOM_STATE >> 7;
        RANDOM_STATE ^= RANDOM_STATE << 17;
        *buf.add(i) = RANDOM_STATE as u8;
    }
}

unsafe extern "C" fn host_path_open(
    dirfd: u32,
    path: *const u8,
    path_len: u32,
    oflags: u32,
    fdflags: u32,
) -> i32 {
    let rt = wasi_runtime();
    let base = match rt.fds.get(&dirfd) {
        Some(FdEntry {
            kind: FdKind::PreopenDir { path },
        }) => path.as_str(),
        Some(_) => return -(ENOTDIR as i32),
        None => return -(ENOTDIR as i32),
    };

    let path_bytes = core::slice::from_raw_parts(path, path_len as usize);
    let path_str = match core::str::from_utf8(path_bytes) {
        Ok(path) => path,
        Err(_) => return -(EINVAL as i32),
    };

    let full_path = resolve_path(base, path_str);
    let is_dir = (oflags & 0x2) != 0;

    if is_dir {
        if list_directory(full_path.as_str()).is_err() {
            return -(ENOENT as i32);
        }
        let fd = rt.alloc_fd();
        rt.fds.insert(fd, FdEntry { kind: FdKind::PreopenDir { path: full_path } });
        return fd as i32;
    }

    let mut options = OpenOptions::new();
    let wants_write = (oflags & 0x1) != 0 || (oflags & 0x8) != 0 || (fdflags & 0x1) != 0;
    options.read(true);
    if wants_write {
        options.write(true);
    }
    if (fdflags & 0x1) != 0 {
        options.append(true);
    }
    if (oflags & 0x8) != 0 {
        options.truncate(true);
    }
    if (oflags & 0x1) != 0 {
        options.create(true);
    }

    let mut file = match options.open(full_path.as_str()) {
        Ok(file) => file,
        Err(_) => return -(ENOENT as i32),
    };

    if (oflags & 0x8) != 0 {
        let _ = file.set_len(0);
    }

    let fd = rt.alloc_fd();
    rt.fds.insert(fd, FdEntry { kind: FdKind::File(UnsafeCell::new(file)) });
    fd as i32
}

unsafe extern "C" fn host_args_sizes_get(argc_out: *mut u32, buf_size_out: *mut u32) {
    let store = &*WASI_ARGS_STORE;
    *argc_out = store.len() as u32;
    let mut total: u32 = 0;
    for arg in store.iter() {
        total += arg.len() as u32 + 1;
    }
    *buf_size_out = total;
}

unsafe extern "C" fn host_args_get_arg(index: u32, dst: *mut u8, dst_cap: usize) -> usize {
    let store = &*WASI_ARGS_STORE;
    if (index as usize) >= store.len() || dst_cap == 0 {
        return 0;
    }
    let arg = &store[index as usize];
    let copy_len = arg.len().min(dst_cap.saturating_sub(1));
    core::ptr::copy_nonoverlapping(arg.as_ptr(), dst, copy_len);
    *dst.add(copy_len) = 0;
    copy_len + 1
}

unsafe extern "C" fn host_environ_sizes_get(count_out: *mut u32, buf_size_out: *mut u32) {
    if WASI_ENV_STORE.is_null() {
        *count_out = 0;
        *buf_size_out = 0;
        return;
    }
    let store = &*WASI_ENV_STORE;
    *count_out = store.len() as u32;
    let mut total: u32 = 0;
    for env in store.iter() {
        total += env.len() as u32 + 1;
    }
    *buf_size_out = total;
}

unsafe extern "C" fn host_environ_get_env(index: u32, dst: *mut u8, dst_cap: usize) -> usize {
    if WASI_ENV_STORE.is_null() {
        return 0;
    }
    let store = &*WASI_ENV_STORE;
    if (index as usize) >= store.len() || dst_cap == 0 {
        return 0;
    }
    let env = &store[index as usize];
    let copy_len = core::cmp::min(env.len(), dst_cap - 1);
    core::ptr::copy_nonoverlapping(env.as_ptr(), dst, copy_len);
    *dst.add(copy_len) = 0;
    copy_len + 1
}

unsafe extern "C" fn host_path_unlink_file(dirfd: u32, path: *const u8, path_len: u32) -> i32 {
    let rt = wasi_runtime();
    let base = match rt.fds.get(&dirfd) {
        Some(FdEntry {
            kind: FdKind::PreopenDir { path },
        }) => path.as_str(),
        _ => return -(EBADF as i32),
    };
    let path_bytes = core::slice::from_raw_parts(path, path_len as usize);
    let path_str = match core::str::from_utf8(path_bytes) {
        Ok(p) => p,
        Err(_) => return -(EINVAL as i32),
    };
    let full_path = resolve_path(base, path_str);
    if path_str.ends_with('/') {
        if path_is_directory(full_path.as_str()) {
            return -(EISDIR as i32);
        }
        return -(ENOTDIR as i32);
    }
    if path_is_directory(full_path.as_str()) {
        return -(EISDIR as i32);
    }
    match remove_file(full_path.as_str()) {
        Ok(()) => ESUCCESS as i32,
        Err(_) => -(ENOENT as i32),
    }
}

unsafe extern "C" fn host_path_remove_dir(dirfd: u32, path: *const u8, path_len: u32) -> i32 {
    let rt = wasi_runtime();
    let base = match rt.fds.get(&dirfd) {
        Some(FdEntry {
            kind: FdKind::PreopenDir { path },
        }) => path.as_str(),
        _ => return -(EBADF as i32),
    };
    let path_bytes = core::slice::from_raw_parts(path, path_len as usize);
    let path_str = match core::str::from_utf8(path_bytes) {
        Ok(p) => p,
        Err(_) => return -(EINVAL as i32),
    };
    let full_path = resolve_path(base, path_str);
    if !path_is_directory(full_path.as_str()) {
        return -(ENOTDIR as i32);
    }
    if let Ok(entries) = list_directory(full_path.as_str()) {
        let real_entries: usize = entries.iter().filter(|e| e.name != "." && e.name != "..").count();
        if real_entries > 0 {
            return -(ENOTEMPTY as i32);
        }
    }
    match remove_directory(full_path.as_str()) {
        Ok(()) => ESUCCESS as i32,
        Err(_) => -(ENOENT as i32),
    }
}

unsafe extern "C" fn host_debug_print(msg: *const u8, msg_len: usize) {
    let s = core::str::from_utf8(core::slice::from_raw_parts(msg, msg_len)).unwrap_or("");
    use std::print;
    print!("{}", s);
}

unsafe extern "C" fn host_path_filestat_get(
    dirfd: u32,
    path: *const u8,
    path_len: u32,
    buf: *mut u8,
) -> i32 {
    let rt = wasi_runtime();
    let base = match rt.fds.get(&dirfd) {
        Some(FdEntry {
            kind: FdKind::PreopenDir { path },
        }) => path.as_str(),
        _ => return -(EBADF as i32),
    };

    let path_bytes = core::slice::from_raw_parts(path, path_len as usize);
    let path_str = match core::str::from_utf8(path_bytes) {
        Ok(p) => p,
        Err(_) => return -(EINVAL as i32),
    };

    let full_path = resolve_path(base, path_str);

    let out = core::slice::from_raw_parts_mut(buf, 64);
    out.fill(0);

    let is_dir = path_is_directory(full_path.as_str());
    if is_dir {
        out[16] = FILETYPE_DIRECTORY;
        write_le64(&mut out[24..32], 1);
        write_le64(&mut out[32..40], 0);
        return ESUCCESS as i32;
    }

    match File::open(full_path.as_str()) {
        Ok(mut file) => {
            out[16] = FILETYPE_REGULAR_FILE;
            write_le64(&mut out[24..32], 1);
            let size = match file.seek(SeekFrom::End(0)) {
                Ok(pos) => pos,
                Err(_) => 0,
            };
            write_le64(&mut out[32..40], size);
            ESUCCESS as i32
        }
        Err(_) => -(ENOENT as i32),
    }
}

unsafe extern "C" fn host_path_filestat_get_flags(
    dirfd: u32,
    flags: u32,
    path: *const u8,
    path_len: u32,
    buf: *mut u8,
) -> i32 {
    let rt = wasi_runtime();
    let base = match rt.fds.get(&dirfd) {
        Some(FdEntry {
            kind: FdKind::PreopenDir { path },
        }) => path.as_str(),
        _ => return -(EBADF as i32),
    };
    let path_bytes = core::slice::from_raw_parts(path, path_len as usize);
    let path_str = match core::str::from_utf8(path_bytes) {
        Ok(p) => p,
        Err(_) => return -(EINVAL as i32),
    };
    let full_path = resolve_path(base, path_str);
    let out = core::slice::from_raw_parts_mut(buf, 64);
    out.fill(0);

    let symlink_follow = (flags & 0x1) != 0;

    match path_entry_file_type(full_path.as_str()) {
        Some(1) => {
            out[16] = FILETYPE_DIRECTORY;
            write_le64(&mut out[24..32], 1);
            write_le64(&mut out[32..40], 0);
            ESUCCESS as i32
        }
        Some(2) if !symlink_follow => {
            out[16] = FILETYPE_SYMBOLIC_LINK;
            write_le64(&mut out[24..32], 1);
            write_le64(&mut out[32..40], 0);
            ESUCCESS as i32
        }
        Some(0) | Some(_) => {
            out[16] = FILETYPE_REGULAR_FILE;
            write_le64(&mut out[24..32], 1);
            if let Ok(mut file) = File::open(full_path.as_str()) {
                if let Ok(pos) = file.seek(SeekFrom::End(0)) {
                    write_le64(&mut out[32..40], pos);
                }
            }
            ESUCCESS as i32
        }
        None => {
            if is_open_path_directory(full_path.as_str()) {
                out[16] = FILETYPE_DIRECTORY;
                write_le64(&mut out[24..32], 1);
                write_le64(&mut out[32..40], 0);
                return ESUCCESS as i32;
            }
            match File::open(full_path.as_str()) {
                Ok(mut file) => {
                    out[16] = FILETYPE_REGULAR_FILE;
                    write_le64(&mut out[24..32], 1);
                    if let Ok(pos) = file.seek(SeekFrom::End(0)) {
                        write_le64(&mut out[32..40], pos);
                    }
                    ESUCCESS as i32
                }
                Err(_) => -(ENOENT as i32),
            }
        }
    }
}

unsafe extern "C" fn host_path_create_directory(dirfd: u32, path: *const u8, path_len: u32) -> i32 {
    let rt = wasi_runtime();
    let base = match rt.fds.get(&dirfd) {
        Some(FdEntry {
            kind: FdKind::PreopenDir { path },
        }) => path.as_str(),
        Some(_) => return -(EBADF as i32),
        None => return -(EBADF as i32),
    };

    let path_bytes = core::slice::from_raw_parts(path, path_len as usize);
    let path_str = match core::str::from_utf8(path_bytes) {
        Ok(path) => path,
        Err(_) => return -(EINVAL as i32),
    };

    let full_path = resolve_path(base, path_str);
    if path_is_directory(full_path.as_str()) {
        return ESUCCESS as i32;
    }
    match create_directory(full_path.as_str()) {
        Ok(()) => ESUCCESS as i32,
        Err(_) => -(EEXIST as i32),
    }
}

unsafe extern "C" fn host_fd_filestat_set_size(fd: u32, size: u64) -> i32 {
    let rt = wasi_runtime();
    let entry = match rt.fds.get(&fd) {
        Some(entry) => entry,
        None => return -(EBADF as i32),
    };

    match &entry.kind {
        FdKind::File(cell) => {
            let file = &mut *cell.get();
            let current_size = match file.seek(SeekFrom::End(0)) {
                Ok(pos) => pos,
                Err(_) => return -(EBADF as i32),
            };
            if size > current_size {
                let pos = file.seek(SeekFrom::Start(size - 1)).unwrap_or(0);
                let zero: [u8; 1] = [0];
                let _ = file.write(&zero);
            }
            match file.set_len(size) {
                Ok(()) => ESUCCESS as i32,
                Err(_) => -(EBADF as i32),
            }
        }
        _ => -(EBADF as i32),
    }
}

unsafe extern "C" fn host_fd_pread(
    fd: u32,
    buf: *mut u8,
    buf_len: usize,
    offset: u64,
    nread: *mut u32,
) -> i32 {
    let rt = wasi_runtime();
    let entry = match rt.fds.get(&fd) {
        Some(entry) => entry,
        None => return -(EBADF as i32),
    };

    match &entry.kind {
        FdKind::File(cell) => {
            let file = &mut *cell.get();
            let current = match file.stream_position() {
                Ok(pos) => pos,
                Err(_) => return -(EBADF as i32),
            };
            if file.seek(SeekFrom::Start(offset)).is_err() {
                return -(EBADF as i32);
            }

            let result = file.read(core::slice::from_raw_parts_mut(buf, buf_len));
            let restore = file.seek(SeekFrom::Start(current));
            match (result, restore) {
                (Ok(bytes), Ok(_)) => {
                    *nread = bytes as u32;
                    ESUCCESS as i32
                }
                _ => -(EBADF as i32),
            }
        }
        _ => -(EBADF as i32),
    }
}

unsafe extern "C" fn host_fd_pwrite(
    fd: u32,
    data: *const u8,
    data_len: usize,
    offset: u64,
    nwritten: *mut u32,
) -> i32 {
    let rt = wasi_runtime();
    let entry = match rt.fds.get(&fd) {
        Some(entry) => entry,
        None => return -(EBADF as i32),
    };

    match &entry.kind {
        FdKind::File(cell) => {
            let file = &mut *cell.get();
            let current = match file.stream_position() {
                Ok(pos) => pos,
                Err(_) => return -(EBADF as i32),
            };
            if file.seek(SeekFrom::Start(offset)).is_err() {
                return -(EBADF as i32);
            }

            let result = file.write(core::slice::from_raw_parts(data, data_len));
            let restore = file.seek(SeekFrom::Start(current));
            match (result, restore) {
                (Ok(bytes), Ok(_)) => {
                    *nwritten = bytes as u32;
                    ESUCCESS as i32
                }
                _ => -(EBADF as i32),
            }
        }
        _ => -(EBADF as i32),
    }
}

unsafe extern "C" fn host_fd_readdir(
    fd: u32,
    buf: *mut u8,
    buf_len: u32,
    cookie: u64,
    bufused: *mut u32,
) -> i32 {
    let rt = wasi_runtime();
    let base = match rt.fds.get(&fd) {
        Some(FdEntry {
            kind: FdKind::PreopenDir { path },
        }) => path.as_str(),
        Some(_) => return -(ENOTDIR as i32),
        None => return -(EBADF as i32),
    };

    let entries = match list_directory(base) {
        Ok(entries) => entries,
        Err(_) => return -(ENOENT as i32),
    };

    let out = core::slice::from_raw_parts_mut(buf, buf_len as usize);
    let mut used = 0usize;

    for (index, entry) in entries.iter().enumerate().skip(cookie as usize) {
        let name = entry.name.as_bytes();
        let mut header = [0u8; 24];
        write_le64(&mut header[0..8], (index + 1) as u64);
        write_le64(&mut header[8..16], entry.file_id);
        write_le32(&mut header[16..20], name.len() as u32);
        header[20] = match entry.file_type {
            0 => FILETYPE_REGULAR_FILE,
            1 => FILETYPE_DIRECTORY,
            2 => FILETYPE_SYMBOLIC_LINK,
            _ => FILETYPE_REGULAR_FILE,
        };

        let record_len = 24 + name.len();
        let remaining = out.len().saturating_sub(used);
        if remaining == 0 {
            break;
        }

        let copied = remaining.min(record_len);
        let header_copy = copied.min(24);
        out[used..used + header_copy].copy_from_slice(&header[..header_copy]);
        if copied > 24 {
            let name_copy = copied - 24;
            out[used + 24..used + 24 + name_copy].copy_from_slice(&name[..name_copy]);
        }
        used += copied;

        if copied < record_len {
            break;
        }
    }

    *bufused = used as u32;
    ESUCCESS as i32
}

unsafe extern "C" fn host_path_symlink(
    old_path: *const u8,
    old_path_len: u32,
    dirfd: u32,
    new_path: *const u8,
    new_path_len: u32,
) -> i32 {
    let rt = wasi_runtime();
    let base = match rt.fds.get(&dirfd) {
        Some(FdEntry {
            kind: FdKind::PreopenDir { path },
        }) => path.as_str(),
        Some(_) => return -(ENOTDIR as i32),
        None => return -(EBADF as i32),
    };

    let old_bytes = core::slice::from_raw_parts(old_path, old_path_len as usize);
    let old_str = match core::str::from_utf8(old_bytes) {
        Ok(path) => path,
        Err(_) => return -(EINVAL as i32),
    };
    let new_bytes = core::slice::from_raw_parts(new_path, new_path_len as usize);
    let new_str = match core::str::from_utf8(new_bytes) {
        Ok(path) => path,
        Err(_) => return -(EINVAL as i32),
    };

    let full_new_path = resolve_path(base, new_str);
    match create_symlink(full_new_path.as_str(), old_str) {
        Ok(()) => ESUCCESS as i32,
        Err(_) => -(ENOENT as i32),
    }
}

unsafe extern "C" fn host_path_readlink(
    fd: u32,
    path: *const u8,
    path_len: u32,
    buf: *mut u8,
    buf_len: u32,
    nread: *mut u32,
) -> i32 {
    let rt = wasi_runtime();
    let base = match rt.fds.get(&fd) {
        Some(FdEntry {
            kind: FdKind::PreopenDir { path },
        }) => path.as_str(),
        Some(_) => return -(ENOTDIR as i32),
        None => return -(EBADF as i32),
    };

    let path_bytes = core::slice::from_raw_parts(path, path_len as usize);
    let path_str = match core::str::from_utf8(path_bytes) {
        Ok(path) => path,
        Err(_) => return -(EINVAL as i32),
    };

    let full_path = resolve_path(base, path_str);
    let target = match read_link(full_path.as_str()) {
        Ok(target) => target,
        Err(_) => return -(ENOENT as i32),
    };

    let bytes = target.as_bytes();
    let copy_len = bytes.len().min(buf_len as usize);
    core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, copy_len);
    *nread = copy_len as u32;
    ESUCCESS as i32
}

unsafe extern "C" fn host_path_rename(
    _old_fd: u32,
    _old_path: *const u8,
    _old_path_len: u32,
    _new_fd: u32,
    _new_path: *const u8,
    _new_path_len: u32,
) -> i32 {
    -(ENOSYS as i32)
}

unsafe extern "C" fn host_fd_renumber(fd: u32, to: u32) -> i32 {
    let rt = wasi_runtime();
    let entry = match rt.fds.remove(&fd) {
        Some(e) => e,
        None => return -(EBADF as i32),
    };
    rt.fds.insert(to, entry);
    ESUCCESS as i32
}

unsafe extern "C" fn host_fd_close(fd: u32) -> i32 {
    let rt = wasi_runtime();
    match rt.fds.remove(&fd) {
        Some(_) => ESUCCESS as i32,
        None => -(EBADF as i32),
    }
}

unsafe extern "C" fn host_fd_seek(fd: u32, offset: i64, whence: u32, new_offset: *mut i64) -> i32 {
    let rt = wasi_runtime();
    let entry = match rt.fds.get(&fd) {
        Some(entry) => entry,
        None => return -(EBADF as i32),
    };

    match &entry.kind {
        FdKind::File(cell) => {
            let file = &mut *cell.get();
            match whence {
                0 => {
                    if offset < 0 {
                        return -(EINVAL as i32);
                    }
                    match file.seek(SeekFrom::Start(offset as u64)) {
                        Ok(pos) => {
                            *new_offset = pos as i64;
                            ESUCCESS as i32
                        }
                        Err(_) => -(EBADF as i32),
                    }
                }
                1 => {
                    // SEEK_CUR: validate resulting position
                    let cur = match file.stream_position() {
                        Ok(p) => p as i64,
                        Err(_) => return -(EBADF as i32),
                    };
                    let result_pos = cur.wrapping_add(offset);
                    if result_pos < 0 {
                        return -(EINVAL as i32);
                    }
                    match file.seek(SeekFrom::Current(offset)) {
                        Ok(pos) => {
                            *new_offset = pos as i64;
                            ESUCCESS as i32
                        }
                        Err(_) => -(EBADF as i32),
                    }
                }
                2 => {
                    // SEEK_END: allow negative offsets (relative to end)
                    match file.seek(SeekFrom::End(offset)) {
                        Ok(pos) => {
                            *new_offset = pos as i64;
                            ESUCCESS as i32
                        }
                        Err(_) => -(EINVAL as i32),
                    }
                }
                _ => -(EINVAL as i32),
            }
        }
        _ => -(EBADF as i32),
    }
}

unsafe extern "C" fn host_fd_tell(fd: u32, offset: *mut i64) -> i32 {
    let rt = wasi_runtime();
    let entry = match rt.fds.get(&fd) {
        Some(entry) => entry,
        None => return -(EBADF as i32),
    };

    match &entry.kind {
        FdKind::File(cell) => {
            let file = &mut *cell.get();
            match file.stream_position() {
                Ok(pos) => {
                    *offset = pos as i64;
                    ESUCCESS as i32
                }
                Err(_) => -(EBADF as i32),
            }
        }
        _ => -(EBADF as i32),
    }
}

unsafe extern "C" fn host_fd_fdstat_get(fd: u32, buf: *mut u8) -> i32 {
    let rt = wasi_runtime();
    let entry = match rt.fds.get(&fd) {
        Some(entry) => entry,
        None => return -(EBADF as i32),
    };

    let filetype = match &entry.kind {
        FdKind::Stdin | FdKind::Stdout | FdKind::Stderr => FILETYPE_CHARACTER_DEVICE,
        FdKind::File(_) => FILETYPE_REGULAR_FILE,
        FdKind::PreopenDir { .. } => FILETYPE_DIRECTORY,
    };

    let out = core::slice::from_raw_parts_mut(buf, 24);
    out.fill(0);
    out[0] = filetype;
    write_le16(&mut out[2..4], 0);
    write_le64(&mut out[8..16], u64::MAX);
    write_le64(&mut out[16..24], u64::MAX);
    ESUCCESS as i32
}

unsafe extern "C" fn host_fd_prestat_get(fd: u32, buf: *mut u8) -> i32 {
    let rt = wasi_runtime();
    let entry = match rt.fds.get(&fd) {
        Some(entry) => entry,
        None => return -(EBADF as i32),
    };

    match &entry.kind {
        FdKind::PreopenDir { path } => {
            let out = core::slice::from_raw_parts_mut(buf, 8);
            out.fill(0);
            out[0] = 0;
            write_le32(&mut out[4..8], path.len() as u32);
            ESUCCESS as i32
        }
        _ => -(EBADF as i32),
    }
}

unsafe extern "C" fn host_fd_prestat_dir_name(fd: u32, buf: *mut u8, buf_len: u32) -> i32 {
    let rt = wasi_runtime();
    let entry = match rt.fds.get(&fd) {
        Some(entry) => entry,
        None => return -(EBADF as i32),
    };

    match &entry.kind {
        FdKind::PreopenDir { path } => {
            let path_bytes = path.as_bytes();
            if path_bytes.len() > buf_len as usize {
                return -(EINVAL as i32);
            }
            core::ptr::copy_nonoverlapping(path_bytes.as_ptr(), buf, path_bytes.len());
            ESUCCESS as i32
        }
        _ => -(EBADF as i32),
    }
}

unsafe extern "C" fn host_fd_filestat_get(fd: u32, buf: *mut u8) -> i32 {
    let rt = wasi_runtime();
    let entry = match rt.fds.get(&fd) {
        Some(entry) => entry,
        None => return -(EBADF as i32),
    };

    let mut size = 0u64;
    let filetype = match &entry.kind {
        FdKind::Stdin | FdKind::Stdout | FdKind::Stderr => FILETYPE_CHARACTER_DEVICE,
        FdKind::PreopenDir { .. } => FILETYPE_DIRECTORY,
        FdKind::File(cell) => {
            let file = &mut *cell.get();
            let current = match file.stream_position() {
                Ok(pos) => pos,
                Err(_) => return -(EBADF as i32),
            };
            size = match file.seek(SeekFrom::End(0)) {
                Ok(pos) => pos,
                Err(_) => return -(EBADF as i32),
            };
            if file.seek(SeekFrom::Start(current)).is_err() {
                return -(EBADF as i32);
            }
            FILETYPE_REGULAR_FILE
        }
    };

    let out = core::slice::from_raw_parts_mut(buf, 64);
    out.fill(0);
    write_le64(&mut out[0..8], 0);
    write_le64(&mut out[8..16], fd as u64);
    out[16] = filetype;
    write_le64(&mut out[24..32], 1);
    write_le64(&mut out[32..40], size);
    write_le64(&mut out[40..48], 0);
    write_le64(&mut out[48..56], 0);
    write_le64(&mut out[56..64], 0);
    ESUCCESS as i32
}

fn resolve_path(base: &str, path: &str) -> String {
    if path.starts_with('/') {
        let p = path.trim_end_matches('/');
        if p.is_empty() {
            return "/".to_string();
        }
        return p.to_string();
    }
    let path = path.trim_end_matches('/');
    if path == "." || path.is_empty() {
        return base.trim_end_matches('/').to_string();
    }
    if base == "/" {
        return format!("/{}", path);
    }
    format!("{}/{}", base.trim_end_matches('/'), path)
}

fn path_entry_file_type(path: &str) -> Option<u8> {
    if path == "/" {
        return Some(1);
    }
    let (parent, name) = match path.rfind('/') {
        Some(0) => (alloc::string::String::from("/"), &path[1..]),
        Some(pos) => (path[..pos].to_string(), &path[pos + 1..]),
        None => return None,
    };
    if name.is_empty() {
        return Some(1);
    }
    if let Ok(entries) = list_directory(parent.as_str()) {
        for entry in &entries {
            if entry.name == name {
                return Some(entry.file_type);
            }
        }
    }
    None
}

fn path_is_directory(path: &str) -> bool {
    match path_entry_file_type(path) {
        Some(1) => true,
        Some(2) => is_open_path_directory(path),
        _ => false,
    }
}

fn is_open_path_directory(path: &str) -> bool {
    if let Ok(mut file) = File::open(path) {
        let mut buf = [0u8; 1];
        match file.read(&mut buf) {
            Ok(0) | Err(_) => true,
            Ok(_) => false,
        }
    } else {
        false
    }
}

unsafe fn wasi_runtime<'a>() -> &'a mut WasiRuntime {
    &mut *WASI_RUNTIME
}

const fn neg_errno(errno: u32) -> i64 {
    -(errno as i64)
}

fn write_le16(dst: &mut [u8], value: u16) {
    dst.copy_from_slice(&value.to_le_bytes());
}

fn write_le32(dst: &mut [u8], value: u32) {
    dst.copy_from_slice(&value.to_le_bytes());
}

fn write_le64(dst: &mut [u8], value: u64) {
    dst.copy_from_slice(&value.to_le_bytes());
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args: Vec<std::string::String> = std::env::args().collect();

    if args.len() < 2 {
        println!("wasm-runtime: missing wasm file operand");
        println!("usage: wasm-runtime [--dir <path>]... <file.wasm> [args...]");
        return 1;
    }

    let mut preopen_dirs: Vec<String> = Vec::new();
    let mut env_vars: Vec<String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--dir" && i + 1 < args.len() {
            preopen_dirs.push(args[i + 1].clone());
            i += 2;
        } else if args[i] == "--env" && i + 1 < args.len() {
            env_vars.push(args[i + 1].clone());
            i += 2;
        } else {
            break;
        }
    }

    if i >= args.len() {
        println!("wasm-runtime: missing wasm file operand");
        return 1;
    }

    let wasm_path = &args[i];
    let wasm_args = &args[i..];

    match run_wasm(wasm_path, wasm_args, &preopen_dirs, &env_vars) {
        Ok(code) => {
            println!("wasm-runtime: exited with code {}", code);
            code
        }
        Err(e) => {
            println!("wasm-runtime: {}: {}", wasm_path, e);
            1
        }
    }
}

fn run_wasm(
    wasm_path: &str,
    args: &[std::string::String],
    preopen_dirs: &[String],
    env_vars: &[String],
) -> Result<i32, std::string::String> {
    let args_bytes: Vec<Vec<u8>> = args.iter().map(|s| s.as_bytes().to_vec()).collect();
    let args_box = Box::new(args_bytes);
    let args_ref: &'static Vec<Vec<u8>> = Box::leak(args_box);
    unsafe {
        WASI_ARGS_STORE = args_ref;
    }

    let env_bytes: Vec<Vec<u8>> = env_vars.iter().map(|s| s.as_bytes().to_vec()).collect();
    let env_box = Box::new(env_bytes);
    let env_ref: &'static Vec<Vec<u8>> = Box::leak(env_box);
    unsafe {
        WASI_ENV_STORE = env_ref;
    }

    let mut file = std::fs::File::open(wasm_path).map_err(|_| format!("cannot open file"))?;

    let mut header = [0u8; 8];
    file.read(&mut header)
        .map_err(|_| format!("cannot read file header"))?;

    if header[..4] != WASM_MAGIC {
        return Err(format!("not a valid wasm file"));
    }

    let mut wasm_bytes = Vec::new();
    file.seek(std::io::SeekFrom::Start(0))
        .map_err(|_| format!("seek failed"))?;

    let mut buf = [0u8; 4096];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => wasm_bytes.extend_from_slice(&buf[..n]),
            Err(_) => return Err(format!("read failed")),
        }
    }

    execute_wasm(&wasm_bytes, preopen_dirs)
}

fn execute_wasm(wasm_bytes: &[u8], preopen_dirs: &[String]) -> Result<i32, std::string::String> {
    use std::handle::capability::memory_mapping::{flags, mmap_anonymous, prot};
    use wasm_jit::engine;

    fn scarlet_exec_alloc(size: usize) -> *mut u8 {
        let page_size = 4096;
        let pages = size.div_ceil(page_size);
        match mmap_anonymous(
            0,
            pages * page_size,
            prot::READ | prot::WRITE | prot::EXEC,
            flags::PRIVATE,
        ) {
            Ok(addr) => addr as *mut u8,
            Err(()) => core::ptr::null_mut(),
        }
    }

    let mut wasi_rt = Box::new(WasiRuntime::new(preopen_dirs));
    let host_ops = HostOps {
        fd_write: host_fd_write,
        fd_read: host_fd_read,
        clock_time_get: host_clock_time_get,
        random_get: host_random_get,
        path_open: host_path_open,
        path_create_directory: host_path_create_directory,
        fd_close: host_fd_close,
        fd_seek: host_fd_seek,
        fd_tell: host_fd_tell,
        fd_fdstat_get: host_fd_fdstat_get,
        fd_prestat_get: host_fd_prestat_get,
        fd_prestat_dir_name: host_fd_prestat_dir_name,
        fd_filestat_get: host_fd_filestat_get,
        args_sizes_get: host_args_sizes_get,
        args_get_arg: host_args_get_arg,
        path_filestat_get: host_path_filestat_get,
        debug_print: host_debug_print,
        path_unlink_file: host_path_unlink_file,
        path_remove_directory: host_path_remove_dir,
        path_filestat_get_flags: host_path_filestat_get_flags,
        fd_filestat_set_size: host_fd_filestat_set_size,
        fd_pread: host_fd_pread,
        fd_pwrite: host_fd_pwrite,
        fd_readdir: host_fd_readdir,
        path_symlink: host_path_symlink,
        path_readlink: host_path_readlink,
        path_rename: host_path_rename,
        fd_renumber: host_fd_renumber,
        environ_sizes_get: host_environ_sizes_get,
        environ_get_env: host_environ_get_env,
    };

    unsafe {
        WASI_RUNTIME = wasi_rt.as_mut() as *mut WasiRuntime;
    }

    engine::set_exec_allocator(scarlet_exec_alloc);

    let result = (|| {
        let module =
            engine::compile_module(wasm_bytes).map_err(|e| format!("compile error: {:?}", e))?;

        let data_pages = module
            .data_segments
            .iter()
            .map(|s| (s.offset as usize + s.data.len() + 65535) / 65536)
            .max()
            .unwrap_or(1);
        let memory_pages = data_pages.max(module.min_memory_pages as usize);
        let memory_pages_initial = memory_pages;
        let memory_pages_max: usize = 65536; // wasm32 max: 4GB
        let cap_bytes = memory_pages_max * 65536;
        use std::handle::capability::memory_mapping::{flags, mmap_anonymous, prot};
        let memory_base =
            match mmap_anonymous(0, cap_bytes, prot::READ | prot::WRITE, flags::PRIVATE) {
                Ok(addr) => addr as *mut u8,
                Err(_) => {
                    return Err(format!(
                        "failed to mmap {} bytes for wasm memory",
                        cap_bytes
                    ));
                }
            };
        let memory_slice =
            unsafe { core::slice::from_raw_parts_mut(memory_base, memory_pages_initial * 65536) };
        module.init_memory(memory_slice);

        let mut ctx_box = alloc::boxed::Box::new(wasm_jit::runtime::VmContext::new(
            memory_base,
            memory_pages_initial * 65536,
            cap_bytes,
            core::ptr::null(),
            0,
        ));
        ctx_box.memory_realloc = None;
        ctx_box.host_ops = &host_ops;

        let imported_names: alloc::vec::Vec<wasm_jit::runtime::ImportedFuncName> = module
            .imported_funcs
            .iter()
            .map(|f| wasm_jit::runtime::ImportedFuncName {
                module: f.module.as_ptr(),
                module_len: f.module.len(),
                name: f.name.as_ptr(),
                name_len: f.name.len(),
            })
            .collect();
        let imported_names_box = imported_names.into_boxed_slice();
        ctx_box.imported_names = imported_names_box.as_ptr();
        ctx_box.imported_count = imported_names_box.len();
        core::mem::forget(imported_names_box);

        let globals_box = module.globals.clone();
        let table_box = module.table.clone();
        wasm_jit::runtime::register_module_defaults(
            core::ptr::null(),
            0,
            globals_box.as_ptr() as *mut _,
            globals_box.len(),
            module.imported_global_count as usize,
            table_box.as_ptr(),
            table_box.len(),
        );
        core::mem::forget(globals_box);
        core::mem::forget(table_box);

        unsafe {
            #[cfg(target_arch = "riscv64")]
            core::arch::asm!("fence.i");
            #[cfg(target_arch = "aarch64")]
            core::arch::asm!(
                "dc cvau, {0}",
                "ic ivau, {0}",
                "dsb ish",
                "isb",
                in(reg) &&module as *const _ as u64,
            );
            match engine::invoke_export(&module, &mut *ctx_box, "_start", &[]) {
                Ok(r) => {
                    println!("ok val={} trap={}", r, ctx_box.trap as u32);
                    if ctx_box.exited {
                        Ok(ctx_box.exit_code as i32)
                    } else {
                        Ok(0)
                    }
                }
                Err(trap) => {
                    println!(
                        "trap:{:?} mem_len={} mem_pages={}",
                        trap,
                        ctx_box.memory_len,
                        ctx_box.memory_len / 65536
                    );
                    if ctx_box.exited {
                        Ok(ctx_box.exit_code as i32)
                    } else {
                        Err(format!("trap: {:?}", trap))
                    }
                }
            }
        }
    })();

    unsafe {
        WASI_RUNTIME = core::ptr::null_mut();
        WASM_MEMORY_BASE = core::ptr::null_mut();
        WASM_MEMORY_CAP = 0;
        WASI_ENV_STORE = core::ptr::null();
    }

    result
}
