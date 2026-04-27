use crate::{CompiledFn, TrapCode};

const MAX_CALL_DEPTH: usize = 256;

pub type HostWriteFn = unsafe extern "C" fn(*const u8, usize);

pub struct ImportedFuncName {
    pub module: *const u8,
    pub module_len: usize,
    pub name: *const u8,
    pub name_len: usize,
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
    pub host_write: Option<HostWriteFn>,
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
            host_write: None,
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
