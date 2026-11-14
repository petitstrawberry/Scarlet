//! User instruction memory for tests that enter syscall handlers directly.
use crate::{arch::Trapframe, environment::PAGE_SIZE, task::Task};

pub(crate) fn syscall_frame(task: &Task) -> Trapframe {
    const CODE: usize = 0x10000;
    assert!(task.vm_manager.translate_to_kva(CODE).is_none());
    task.allocate_stack_pages(CODE, 1).unwrap();
    #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
    let instruction = 0x0000_0073u32; // ECALL
    #[cfg(target_arch = "aarch64")]
    let instruction = 0xd400_0001u32; // SVC #0
    // Each syscall advances the saved PC. Populate a full page for fixtures
    // that invoke several handlers using the same frame.
    let mut code = [0u8; PAGE_SIZE];
    for word in code.chunks_exact_mut(4) {
        word.copy_from_slice(&instruction.to_ne_bytes());
    }
    crate::library::std::usercopy::copy_to_user(task, CODE, &code).unwrap();
    let mut frame = Trapframe::new();
    frame.set_pc(CODE as u64);
    frame
}
