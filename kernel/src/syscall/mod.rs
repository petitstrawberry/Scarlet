pub enum Syscall {
    Invalid = 0,
}

impl From<usize> for Syscall {
    fn from(value: usize) -> Self {
        match value {
            0 => Syscall::Invalid,
            _ => Syscall::Invalid,
        }
    }
}

pub enum SyscallError {
    Invalid = -1,
}


pub fn syscall_handler(syscall: Syscall) -> Result<usize, isize> {
    match syscall {
        Syscall::Invalid => Err(-1),
        _ => SYSCALLS[syscall as usize](),
    }
}

static NUM_OF_SYSCALLS: usize = 0;
static SYSCALLS: [fn() -> Result<usize, isize>; NUM_OF_SYSCALLS] = [
    // Add syscall functions here
];
