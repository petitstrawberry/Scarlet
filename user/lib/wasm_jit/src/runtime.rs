use crate::{CompiledFn, TrapCode};

const MAX_CALL_DEPTH: usize = 256;

pub struct ImportedFuncName {
    pub module: *const u8,
    pub module_len: usize,
    pub name: *const u8,
    pub name_len: usize,
}

/// Host operation callbacks for WASI implementation.
/// All function pointers must be valid for the lifetime of the VmContext.
#[repr(C)]
pub struct HostOps {
    /// Write bytes to fd. Returns bytes written (>=0) or negative errno.
    pub fd_write: unsafe extern "C" fn(fd: u32, data: *const u8, data_len: usize) -> i64,
    /// Read bytes from fd into buf. Returns bytes read (>=0) or negative errno.
    pub fd_read: unsafe extern "C" fn(fd: u32, buf: *mut u8, buf_len: usize) -> i64,
    /// Get clock time. Writes nanoseconds to `time`.
    pub clock_time_get: unsafe extern "C" fn(clock_id: u32, time: *mut u64),
    /// Fill buffer with random bytes.
    pub random_get: unsafe extern "C" fn(buf: *mut u8, buf_len: usize),
    /// Open a file relative to dirfd. path is UTF-8 bytes, NOT null-terminated.
    /// Returns new fd (>=3) or negative errno.
    pub path_open: unsafe extern "C" fn(
        dirfd: u32,
        path: *const u8,
        path_len: u32,
        oflags: u32,
        fdflags: u32,
    ) -> i32,
    /// Close fd. Returns 0 or negative errno.
    pub fd_close: unsafe extern "C" fn(fd: u32) -> i32,
    /// Seek in fd. Writes new offset to new_offset. Returns 0 or negative errno.
    pub fd_seek:
        unsafe extern "C" fn(fd: u32, offset: i64, whence: u32, new_offset: *mut i64) -> i32,
    /// Get current offset. Writes offset. Returns 0 or negative errno.
    pub fd_tell: unsafe extern "C" fn(fd: u32, offset: *mut i64) -> i32,
    /// Get fdstat. Writes 24-byte fdstat struct to buf. Returns 0 or negative errno.
    pub fd_fdstat_get: unsafe extern "C" fn(fd: u32, buf: *mut u8) -> i32,
    /// Get prestat for pre-opened dir. Writes 8-byte prestat to buf. Returns 0 or negative errno.
    pub fd_prestat_get: unsafe extern "C" fn(fd: u32, buf: *mut u8) -> i32,
    /// Get name of pre-opened dir. Writes up to buf_len bytes. Returns 0 or negative errno.
    pub fd_prestat_dir_name: unsafe extern "C" fn(fd: u32, buf: *mut u8, buf_len: u32) -> i32,
    /// Get filestat. Writes 64-byte filestat struct to buf. Returns 0 or negative errno.
    pub fd_filestat_get: unsafe extern "C" fn(fd: u32, buf: *mut u8) -> i32,
}

pub struct VmContext {
    pub memory_base: *mut u8,
    pub memory_len: usize,
    pub functions: *const crate::FunctionEntry,
    pub function_count: usize,
    pub trap: TrapCode,
    pub exit_code: u32,
    pub exited: bool,
    pub call_depth: usize,
    pub host_ops: *const HostOps,
    pub imported_names: *const ImportedFuncName,
    pub imported_count: usize,
}

impl VmContext {
    pub fn new(
        memory_base: *mut u8,
        memory_len: usize,
        functions: *const crate::FunctionEntry,
        function_count: usize,
    ) -> Self {
        Self {
            memory_base,
            memory_len,
            functions,
            function_count,
            trap: TrapCode::None,
            exit_code: 0,
            exited: false,
            call_depth: 0,
            host_ops: core::ptr::null(),
            imported_names: core::ptr::null(),
            imported_count: 0,
        }
    }

    pub fn imported_func_name(&self, index: u32) -> Option<(&str, &str)> {
        if index as usize >= self.imported_count {
            return None;
        }
        unsafe {
            let entry = &*self.imported_names.add(index as usize);
            let module = core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                entry.module,
                entry.module_len,
            ));
            let name = core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                entry.name,
                entry.name_len,
            ));
            Some((module, name))
        }
    }

    pub fn check_memory(&self, addr: u64, len: u64) -> bool {
        let end = addr.checked_add(len);
        match end {
            Some(e) => e as usize <= self.memory_len,
            None => false,
        }
    }

    pub fn set_trap(&mut self, trap: TrapCode) {
        self.trap = trap;
    }

    pub fn enter_call(&mut self) -> bool {
        if self.call_depth >= MAX_CALL_DEPTH {
            self.trap = TrapCode::StackOverflow;
            return false;
        }
        self.call_depth += 1;
        true
    }

    pub fn leave_call(&mut self) {
        if self.call_depth > 0 {
            self.call_depth -= 1;
        }
    }
}
