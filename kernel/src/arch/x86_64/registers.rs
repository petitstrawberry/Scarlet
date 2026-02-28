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
        }
    }
}
