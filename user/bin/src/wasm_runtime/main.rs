#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![allow(unsafe_op_in_unsafe_fn, dead_code)]

extern crate alloc;
#[cfg(not(test))]
extern crate scarlet_std as std;
#[cfg(test)]
extern crate std;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use core::cell::UnsafeCell;
use std::fs::{File, OpenOptions, create_directory};
use std::io::SeekFrom;
use std::{format, println, vec::Vec};
use wasm_jit::runtime::HostOps;

const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];

const ESUCCESS: u32 = 0;
const EBADF: u32 = 8;
const EINVAL: u32 = 28;
const ENOENT: u32 = 44;
const ENOSYS: u32 = 52;

const FILETYPE_CHARACTER_DEVICE: u8 = 2;
const FILETYPE_DIRECTORY: u8 = 3;
const FILETYPE_REGULAR_FILE: u8 = 4;

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
    fn new() -> Self {
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
        fds.insert(
            PREOPEN_FD,
            FdEntry {
                kind: FdKind::PreopenDir {
                    path: "/".to_string(),
                },
            },
        );
        Self {
            fds,
            next_fd: PREOPEN_FD + 1,
            preopen_path: "/".to_string(),
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
        Some(_) => return -(EBADF as i32),
        None => return -(EBADF as i32),
    };

    let path_bytes = core::slice::from_raw_parts(path, path_len as usize);
    let path_str = match core::str::from_utf8(path_bytes) {
        Ok(path) => path,
        Err(_) => return -(EINVAL as i32),
    };

    let full_path = resolve_path(base, path_str);
    let mut options = OpenOptions::new();
    let wants_write = (oflags & 0x1) != 0 || (fdflags & 0x1) != 0 || (fdflags & 0x10) != 0;
    options.read(true);
    if wants_write {
        options.write(true);
    }
    if (fdflags & 0x1) != 0 {
        options.append(true);
    }
    if (fdflags & 0x10) != 0 {
        options.truncate(true);
    }
    if (oflags & 0x1) != 0 {
        options.create(true);
    }

    let file = match options.open(full_path.as_str()) {
        Ok(file) => file,
        Err(_) => return -(ENOENT as i32),
    };

    let fd = rt.alloc_fd();
    let kind = if path_is_directory(full_path.as_str()) {
        FdKind::PreopenDir { path: full_path }
    } else {
        FdKind::File(UnsafeCell::new(file))
    };
    rt.fds.insert(fd, FdEntry { kind });
    fd as i32
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
    match create_directory(full_path.as_str()) {
        Ok(()) => ESUCCESS as i32,
        Err(_) => -(ENOENT as i32),
    }
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

    let seek_from = match whence {
        0 => {
            if offset < 0 {
                return -(EINVAL as i32);
            }
            SeekFrom::Start(offset as u64)
        }
        1 => SeekFrom::Current(offset),
        2 => SeekFrom::End(offset),
        _ => return -(EINVAL as i32),
    };

    match &entry.kind {
        FdKind::File(cell) => {
            let file = &mut *cell.get();
            match file.seek(seek_from) {
                Ok(pos) => {
                    *new_offset = pos as i64;
                    ESUCCESS as i32
                }
                Err(_) => -(EBADF as i32),
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
        return path.to_string();
    }
    if base == "/" {
        return format!("/{}", path);
    }
    format!("{}/{}", base.trim_end_matches('/'), path)
}

fn path_is_directory(path: &str) -> bool {
    if path == "/" || path.ends_with('/') {
        return true;
    }

    std::fs::list_directory(path).is_ok()
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
        println!("usage: wasm-runtime <file.wasm> [args...]");
        return 1;
    }

    let wasm_path = &args[1];
    let wasm_args = if args.len() > 2 { &args[2..] } else { &[] };

    match run_wasm(wasm_path, wasm_args) {
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

fn run_wasm(wasm_path: &str, _args: &[std::string::String]) -> Result<i32, std::string::String> {
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

    execute_wasm(&wasm_bytes)
}

fn execute_wasm(wasm_bytes: &[u8]) -> Result<i32, std::string::String> {
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

    let mut wasi_rt = Box::new(WasiRuntime::new());
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
        println!(
            "DBG: initial_pages={} min_mem_pages={} data_pages={}",
            memory_pages_initial, module.min_memory_pages, data_pages
        );
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
                    Ok(0)
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
    }

    result
}
