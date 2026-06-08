use core::arch::{asm, naked_asm};

use crate::{env, exit};

/// Scarlet Native RISC-V entry trampoline.
///
/// This symbol lives in `.init` and branches to `_start`, matching the current
/// Scarlet loader contract used by `scarlet_std`.
#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_entry")]
#[unsafe(naked)]
pub extern "C" fn _entry() {
    naked_asm!(
        "
    .option norvc
    .option norelax
    .align 8
            j       _start
    ",
    );
}

unsafe extern "Rust" {
    fn main() -> i32;
}

/// Scarlet Native RISC-V process entry point.
///
/// # Arguments
///
/// * `a0` - Argument count supplied by the Scarlet ABI loader.
/// * `a1` - Argument vector pointer supplied by the Scarlet ABI loader.
///
/// # Safety
///
/// The loader must provide `a0` and `a1` according to the Scarlet Native
/// process-start ABI.
#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_start")]
pub unsafe extern "C" fn _start(a0: usize, a1: usize) -> ! {
    // Get argc and argv from RISC-V calling convention registers
    // a0 = argc, a1 = argv (set by kernel's ScarletAbi)
    let argc = a0;
    let argv = a1 as *const *const u8;

    // Calculate envp from stack layout:
    // Stack layout: argc | argv[] | envp[] | argv_strings | envp_strings
    // envp starts right after argv[] (which has argc+1 entries including NULL)
    let envp = if !argv.is_null() && argc > 0 {
        // SAFETY: The Scarlet loader guarantees `argv` points to an argc-sized
        // array followed by a null terminator and envp array.
        unsafe { argv.add(argc + 1) }
    } else {
        core::ptr::null()
    };

    // SAFETY: `_start` is only entered by the Scarlet ABI loader, which sets up
    // argc/argv/envp before transferring control.
    unsafe {
        env::init_env(argc, argv, envp);
    }

    // SAFETY: Scarlet `no_main` executables provide `extern "Rust" fn main`.
    let ret = unsafe { main() };
    exit(ret as i32);
}

/// Get the current thread's TLS (Thread Local Storage) pointer
///
/// This reads the tp register (x4) directly without a syscall for performance.
/// The tp register is set by the kernel during thread creation and is part
/// of the task's context.
///
/// # Returns
/// The current TLS base pointer, or 0 if not set
#[inline]
pub fn arch_tls_pointer() -> usize {
    let tp;
    unsafe {
        asm!(
            "mv {}, tp",
            out(reg) tp,
            options(nostack, pure, readonly)
        );
    }
    tp
}

/// Set the current thread's TLS pointer
///
/// This function performs a syscall to set the TLS pointer in the kernel.
/// Direct register writes are not recommended as the kernel needs to
/// maintain ABI state synchronization.
///
/// # Arguments
/// * `ptr` - The new TLS base pointer
///
/// # Note
/// This is typically only called during thread initialization. For most
/// use cases, you should use the TLS pointer set by the kernel during
/// thread creation.
pub fn arch_set_tls_pointer(ptr: usize) {
    // Use syscall to set TLS pointer (kernel needs to update ABI state)
    scarlet_sys::syscall1(scarlet_sys::Syscall::SetTls, ptr);
}
