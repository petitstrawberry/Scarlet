//! Thin interpreter entry: std startup, process-owned loader, original entry.

#[cfg(not(all(
    target_os = "scarlet",
    any(target_arch = "aarch64", target_arch = "riscv64")
)))]
compile_error!("scarlet-ld requires an AArch64 or RISC-V64 Scarlet Native std toolchain");

#[used]
#[unsafe(no_mangle)]
static mut __scarlet_ld_initial_stack: usize = 0;

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".section .text._scarlet_ld_start,\"ax\",%progbits",
    ".global _scarlet_ld_start",
    ".type _scarlet_ld_start,%function",
    "_scarlet_ld_start:",
    "mov x2, sp",
    "adrp x3, __scarlet_ld_initial_stack",
    "str x2, [x3, :lo12:__scarlet_ld_initial_stack]",
    "b _start",
    ".size _scarlet_ld_start, .-_scarlet_ld_start",
    ".global __scarlet_ld_enter",
    ".type __scarlet_ld_enter,%function",
    "__scarlet_ld_enter:",
    "mov x9, x0",
    "mov sp, x1",
    "ldr x0, [sp]",
    "add x1, sp, #8",
    "mov x30, xzr",
    "br x9",
    ".size __scarlet_ld_enter, .-__scarlet_ld_enter",
);

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    ".section .text._scarlet_ld_start,\"ax\",@progbits",
    ".global _scarlet_ld_start",
    ".type _scarlet_ld_start,@function",
    "_scarlet_ld_start:",
    ".option push",
    ".option norelax",
    "lla t0, __scarlet_ld_initial_stack",
    "sd sp, 0(t0)",
    "lla gp, __global_pointer$",
    ".option pop",
    "tail _start",
    ".size _scarlet_ld_start, .-_scarlet_ld_start",
    ".global __scarlet_ld_enter",
    ".type __scarlet_ld_enter,@function",
    "__scarlet_ld_enter:",
    "mv t0, a0",
    "mv sp, a1",
    "ld a0, 0(sp)",
    "addi a1, sp, 8",
    "li ra, 0",
    "jr t0",
    ".size __scarlet_ld_enter, .-__scarlet_ld_enter",
);

unsafe extern "C" {
    fn __scarlet_ld_enter(entry: usize, stack: usize) -> !;
}

fn main() {
    // SAFETY: The assembly entry stores this once before std initializes.
    let stack = unsafe { __scarlet_ld_initial_stack };
    // SAFETY: Only the interpreter startup calls this, while the application's
    // original kernel stack and mappings are intact and its code has not run.
    match unsafe { scarlet_dl::initialize(stack) } {
        Ok(entry) => {
            // SAFETY: Runtime has relocated/protected all dependencies and
            // checked the main entry against AT_ENTRY. The trampoline restores
            // the kernel stack and argc/argv ABI before jumping there.
            unsafe { __scarlet_ld_enter(entry, stack) }
        }
        Err(error) => {
            report_error(&error);
            std::process::exit(127);
        }
    }
}

/// Bootstrap processes may have no stderr handle. Error reporting must never
/// hide the loader's actual failure behind a std printing panic.
fn report_error(error: &impl std::fmt::Display) {
    use std::io::Write;
    if std::io::stderr()
        .write_fmt(format_args!("scarlet-ld: {error}\n"))
        .is_err()
    {
        struct Console;
        impl std::fmt::Write for Console {
            fn write_str(&mut self, text: &str) -> std::fmt::Result {
                for byte in text.bytes() {
                    // SAFETY: Putchar takes one scalar character and retains
                    // no pointers. Its result is irrelevant during failure.
                    unsafe {
                        scarlet_sys::syscall1(scarlet_sys::Syscall::Putchar, usize::from(byte));
                    }
                }
                Ok(())
            }
        }
        let _ = std::fmt::Write::write_fmt(&mut Console, format_args!("scarlet-ld: {error}\n"));
    }
}
