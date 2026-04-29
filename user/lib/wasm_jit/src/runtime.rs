use crate::TrapCode;

const MAX_CALL_DEPTH: usize = 256;

static mut DEFAULT_FUNCTIONS: *const crate::FunctionEntry = core::ptr::null();
static mut DEFAULT_FUNCTION_COUNT: usize = 0;
static mut DEFAULT_GLOBALS: *mut crate::GlobalEntry = core::ptr::null_mut();
static mut DEFAULT_GLOBAL_COUNT: usize = 0;
static mut DEFAULT_IMPORTED_GLOBAL_COUNT: usize = 0;
static mut DEFAULT_TABLE: *const u32 = core::ptr::null();
static mut DEFAULT_TABLE_COUNT: usize = 0;

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
    /// Create a directory relative to dirfd. Returns 0 or negative errno.
    pub path_create_directory:
        unsafe extern "C" fn(dirfd: u32, path: *const u8, path_len: u32) -> i32,
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
    /// Get args count and total buffer size (including NUL terminators).
    pub args_sizes_get: unsafe extern "C" fn(argc_out: *mut u32, buf_size_out: *mut u32),
    /// Get arg at index. Writes arg bytes (including NUL terminator) to dst.
    /// Returns number of bytes written (including NUL).
    pub args_get_arg: unsafe extern "C" fn(index: u32, dst: *mut u8, dst_cap: usize) -> usize,
    /// Get filestat by path. Writes 64-byte filestat struct to buf. Returns 0 or negative errno.
    pub path_filestat_get:
        unsafe extern "C" fn(dirfd: u32, path: *const u8, path_len: u32, buf: *mut u8) -> i32,
    pub debug_print: unsafe extern "C" fn(msg: *const u8, msg_len: usize),
    pub path_unlink_file: unsafe extern "C" fn(dirfd: u32, path: *const u8, path_len: u32) -> i32,
    pub path_remove_directory:
        unsafe extern "C" fn(dirfd: u32, path: *const u8, path_len: u32) -> i32,
}

#[repr(C)]
pub struct VmContext {
    pub memory_base: *mut u8,
    pub memory_len: usize,
    pub memory_cap: usize,
    pub memory_realloc: Option<unsafe extern "C" fn(*mut u8, usize, usize) -> *mut u8>,
    pub functions: *const crate::FunctionEntry,
    pub function_count: usize,
    pub globals: *mut crate::GlobalEntry,
    pub global_count: usize,
    pub imported_global_count: usize,
    pub table: *const u32,
    pub table_count: usize,
    pub trap: TrapCode,
    pub exit_code: u32,
    pub exited: bool,
    pub call_depth: usize,
    pub host_ops: *const HostOps,
    pub imported_names: *const ImportedFuncName,
    pub imported_count: usize,
    pub debug_last_func: u32,
    pub debug_call_count: u32,
    pub debug_store_count: u32,
    pub debug_global_set_count: u32,
    pub debug_import_call_count: u32,
    pub debug_last_store_addr: u32,
    pub debug_last_store_value: u64,
    pub debug_last_global_idx: u32,
    pub debug_last_global_val: u64,
    pub debug_last_trap_seen: u32,
    pub debug_check_count: u32,
    pub debug_trace: [u32; 64],
    pub debug_trace_idx: usize,
    pub debug_mgrow_delta: [u32; 8],
    pub debug_mgrow_result: [i32; 8],
    pub debug_mgrow_count: usize,
}

impl VmContext {
    pub fn new(
        memory_base: *mut u8,
        memory_len: usize,
        memory_cap: usize,
        functions: *const crate::FunctionEntry,
        function_count: usize,
    ) -> Self {
        let functions = if functions.is_null() {
            unsafe { DEFAULT_FUNCTIONS }
        } else {
            functions
        };
        let function_count = if function_count == 0 {
            unsafe { DEFAULT_FUNCTION_COUNT }
        } else {
            function_count
        };
        let globals = unsafe { DEFAULT_GLOBALS };
        let global_count = unsafe { DEFAULT_GLOBAL_COUNT };
        let imported_global_count = unsafe { DEFAULT_IMPORTED_GLOBAL_COUNT };
        let table = unsafe { DEFAULT_TABLE };
        let table_count = unsafe { DEFAULT_TABLE_COUNT };

        Self {
            memory_base,
            memory_len,
            memory_cap,
            memory_realloc: None,
            functions,
            function_count,
            globals,
            global_count,
            imported_global_count,
            table,
            table_count,
            trap: TrapCode::None,
            exit_code: 0,
            exited: false,
            call_depth: 0,
            host_ops: core::ptr::null(),
            imported_names: core::ptr::null(),
            imported_count: 0,
            debug_last_func: 0,
            debug_call_count: 0,
            debug_store_count: 0,
            debug_global_set_count: 0,
            debug_import_call_count: 0,
            debug_last_store_addr: 0,
            debug_last_store_value: 0,
            debug_last_global_idx: 0,
            debug_last_global_val: 0,
            debug_last_trap_seen: 0,
            debug_check_count: 0,
            debug_trace: [0u32; 64],
            debug_trace_idx: 0,
            debug_mgrow_delta: [0u32; 8],
            debug_mgrow_result: [0i32; 8],
            debug_mgrow_count: 0,
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

pub fn register_module_defaults(
    functions: *const crate::FunctionEntry,
    function_count: usize,
    globals: *mut crate::GlobalEntry,
    global_count: usize,
    imported_global_count: usize,
    table: *const u32,
    table_count: usize,
) {
    unsafe {
        DEFAULT_FUNCTIONS = functions;
        DEFAULT_FUNCTION_COUNT = function_count;
        DEFAULT_GLOBALS = globals;
        DEFAULT_GLOBAL_COUNT = global_count;
        DEFAULT_IMPORTED_GLOBAL_COUNT = imported_global_count;
        DEFAULT_TABLE = table;
        DEFAULT_TABLE_COUNT = table_count;
    }
}
