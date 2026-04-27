use crate::{CompiledFn, TrapCode};

const MAX_CALL_DEPTH: usize = 256;

pub type HostWriteFn = unsafe extern "C" fn(*const u8, usize);

pub struct VmContext {
    pub memory_base: *mut u8,
    pub memory_len: usize,
    pub functions: *const crate::FunctionEntry,
    pub function_count: usize,
    pub trap: TrapCode,
    pub call_depth: usize,
    pub host_write: Option<HostWriteFn>,
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
            call_depth: 0,
            host_write: None,
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
