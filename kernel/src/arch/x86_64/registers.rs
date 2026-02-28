/// Integer registers for x86_64
///
/// x86_64 System V ABI calling convention:
/// - Arguments: RDI, RSI, RDX, RCX, R8, R9
/// - Return: RAX
/// - Callee-saved: RBX, RBP, R12, R13, R14, R15
/// - Caller-saved: RDI, RSI, RDX, RCX, R8, R9, R10, R11
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct IntRegisters {
    pub rdi: usize,
    pub rsi: usize,
    pub rdx: usize,
    pub rcx: usize,
    pub r8: usize,
    pub r9: usize,
    pub r10: usize,
    pub r11: usize,
    pub rax: usize,
    pub rbx: usize,
    pub rbp: usize,
    pub r12: usize,
    pub r13: usize,
    pub r14: usize,
    pub r15: usize,
    pub rsp: usize,
    pub fsbase: usize,
}

impl IntRegisters {
    pub const fn new() -> Self {
        IntRegisters {
            rdi: 0,
            rsi: 0,
            rdx: 0,
            rcx: 0,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            rax: 0,
            rbx: 0,
            rbp: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rsp: 0,
            fsbase: 0,
        }
    }

    pub fn set_return_value(&mut self, value: usize) {
        self.rax = value;
    }

    pub fn get_return_value(&self) -> usize {
        self.rax
    }

    pub fn set_arg(&mut self, index: usize, value: usize) {
        match index {
            0 => self.rdi = value,
            1 => self.rsi = value,
            2 => self.rdx = value,
            3 => self.rcx = value,
            4 => self.r8 = value,
            5 => self.r9 = value,
            _ => {}
        }
    }

    pub fn get_arg(&self, index: usize) -> usize {
        match index {
            0 => self.rdi,
            1 => self.rsi,
            2 => self.rdx,
            3 => self.rcx,
            4 => self.r8,
            5 => self.r9,
            _ => 0,
        }
    }
}
