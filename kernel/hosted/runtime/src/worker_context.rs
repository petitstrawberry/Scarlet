//! Register state and trap normalization for AArch64 signals.

#[cfg(not(target_os = "ios"))]
use crate::worker_memory::{Mapping, PatchedInstruction};

#[cfg(target_os = "linux")]
pub(super) mod linux;

#[cfg(target_vendor = "apple")]
const NZCV: u32 = 0xf000_0000;
const BREAKPOINT_CLASS: u32 = 0x3c;
const SVC_CLASS: u64 = 0x15;
const ESR_IL: u64 = 1 << 25;
pub(crate) const SYSCALL_BRK: u16 = 0x5300;

pub struct GuestStop {
    pub registers: Registers,
    pub esr: u64,
    pub far: u64,
    pub tls: u64,
    pub interrupted: bool,
}

pub struct Exception {
    pub registers: Registers,
    pub esr: u32,
    pub far: u64,
}

pub struct SyscallTrap {
    pub elr: u64,
    pub esr: u64,
}

impl Exception {
    #[cfg(target_vendor = "apple")]
    pub fn capture(context: &libc::__darwin_mcontext64) -> Self {
        Self {
            registers: Registers::capture(context),
            esr: context.__es.__esr,
            far: context.__es.__far,
        }
    }

    /// Normalize a stop using the mapping from its admitted generation.
    /// Scarlet records ELR after an AArch64 SVC.
    #[cfg(not(target_os = "ios"))]
    pub fn syscall(&self, mapping: &Mapping, word: u32) -> Option<SyscallTrap> {
        let PatchedInstruction::Syscall { immediate } =
            mapping.patched_instruction(self.registers.pc as usize, word)?
        else {
            return None;
        };
        self.translated_syscall(self.registers.pc, immediate)
    }

    pub(super) fn is_breakpoint(&self, immediate: u16) -> bool {
        self.esr >> 26 == BREAKPOINT_CLASS && self.esr & 0xffff == u32::from(immediate)
    }

    pub(super) fn translated_syscall(&self, guest_pc: u64, immediate: u16) -> Option<SyscallTrap> {
        if !self.is_breakpoint(SYSCALL_BRK) {
            return None;
        }
        Some(SyscallTrap {
            elr: guest_pc.checked_add(4)?,
            esr: (SVC_CLASS << 26) | ESR_IL | u64::from(immediate),
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Registers {
    pub x: [u64; 31],
    pub sp: u64,
    pub pc: u64,
    pub nzcv: u32,
    pub q: [u128; 32],
    pub fpcr: u32,
    pub fpsr: u32,
}

impl Registers {
    /// Capture the saved registers before changing the host's resume context.
    /// This does not read the signal handler's live registers or use TLS.
    #[cfg(target_vendor = "apple")]
    pub fn capture(context: &libc::__darwin_mcontext64) -> Self {
        let state = &context.__ss;
        let mut x = [0; 31];
        x[..29].copy_from_slice(&state.__x);
        x[29] = state.__fp;
        x[30] = state.__lr;
        Self {
            x,
            sp: state.__sp,
            pc: state.__pc,
            nzcv: state.__cpsr & NZCV,
            q: context.__ns.__v,
            fpcr: context.__ns.__fpcr,
            fpsr: context.__ns.__fpsr,
        }
    }

    /// Preserve host-controlled status bits in the kernel-provided context.
    #[cfg(target_vendor = "apple")]
    pub fn restore(self, context: &mut libc::__darwin_mcontext64) {
        let state = &mut context.__ss;
        state.__x.copy_from_slice(&self.x[..29]);
        state.__fp = self.x[29];
        state.__lr = self.x[30];
        state.__sp = self.sp;
        state.__pc = self.pc;
        state.__cpsr = (state.__cpsr & !NZCV) | (self.nzcv & NZCV);
        context.__ns.__v = self.q;
        context.__ns.__fpcr = self.fpcr;
        context.__ns.__fpsr = self.fpsr;
    }
}

#[cfg(all(test, not(target_os = "ios")))]
mod tests {
    use super::*;
    use crate::{
        ram::{RamArena, host_page_size},
        worker_memory::{Protection, SYSCALL_BREAKPOINT},
    };
    use std::{
        ffi::c_void,
        process::Command,
        ptr,
        sync::atomic::{AtomicPtr, Ordering},
    };

    const ENTRY: usize = 0x78_0000_0000;
    const TRAP_PC: usize = ENTRY + 4;
    static TEXT: AtomicPtr<Mapping> = AtomicPtr::new(ptr::null_mut());

    unsafe extern "C" fn stop(signal: i32, info: *mut libc::siginfo_t, raw: *mut c_void) {
        #[cfg(target_os = "macos")]
        let _ = (signal, info);
        #[cfg(target_os = "macos")]
        let context = unsafe { &mut *(*raw.cast::<libc::ucontext_t>()).uc_mcontext };
        #[cfg(target_os = "macos")]
        let exception = Exception::capture(context);
        #[cfg(target_os = "linux")]
        let mut context = unsafe { linux::SignalContext::new(raw) };
        #[cfg(target_os = "linux")]
        let exception = unsafe { context.exception(signal, info) };
        let mapping = TEXT.load(Ordering::Acquire);
        if mapping.is_null() || exception.registers.pc as usize != TRAP_PC {
            unsafe { libc::_exit(1) };
        }
        // SAFETY: This test thread owns the mapping and cannot retire it
        // until the native call returns from this signal.
        let mapping = unsafe { &*mapping };
        let word = unsafe { ptr::with_exposed_provenance::<u32>(TRAP_PC).read() };
        let Some(syscall) = exception.syscall(mapping, word) else {
            unsafe { libc::_exit(1) };
        };
        let mut registers = exception.registers;
        if syscall.esr != 0x5600_0123 || registers.x[0] != 42 {
            unsafe { libc::_exit(1) };
        }
        registers.x[0] = 43;
        registers.pc = syscall.elr;
        #[cfg(target_os = "macos")]
        registers.restore(context);
        #[cfg(target_os = "linux")]
        context.restore(registers);
    }

    #[test]
    fn published_syscall_stops_and_resumes_at_native_pc() {
        const CHILD: &str = "SCARLET_WORKER_TRAP_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "worker_context::tests::published_syscall_stops_and_resumes_at_native_pc",
                ])
                .env(CHILD, "1")
                .status()
                .unwrap();
            assert!(status.success(), "Native trap child exited with {status}");
            return;
        }

        let page = host_page_size().unwrap();
        let arena = RamArena::new(page as u64, page).unwrap();
        let extent = arena.resolve(page as u64, page).unwrap();
        // mov w0, #42; svc #0x123; ret
        let code = [0x5280_0540_u32, 0xd400_2461, 0xd65f_03c0];
        let mut alternate_stack = vec![0_u128; 128 * 1024 / size_of::<u128>()];
        let stack = libc::stack_t {
            ss_sp: alternate_stack.as_mut_ptr().cast(),
            ss_size: std::mem::size_of_val(alternate_stack.as_slice()),
            ss_flags: 0,
        };
        unsafe {
            ptr::copy_nonoverlapping(code.as_ptr(), extent.alias.as_ptr().cast(), code.len());
            assert_eq!(libc::sigaltstack(&stack, ptr::null_mut()), 0);
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
            action.sa_sigaction = stop as *const () as usize;
            assert_eq!(libc::sigaction(libc::SIGTRAP, &action, ptr::null_mut()), 0);
        }
        let mut text = unsafe {
            Mapping::install(
                ENTRY,
                page,
                extent.backing,
                extent.offset,
                Protection::ReadExecute(&[0..12]),
            )
        }
        .unwrap();
        assert_eq!(
            text.patched_instruction(TRAP_PC, SYSCALL_BREAKPOINT),
            Some(PatchedInstruction::Syscall { immediate: 0x123 }),
        );
        TEXT.store(&raw mut text, Ordering::Release);
        let call: extern "C" fn() -> u64 = unsafe { std::mem::transmute(text.address()) };
        assert_eq!(call(), 43);
        TEXT.store(ptr::null_mut(), Ordering::Release);
        let disabled = libc::stack_t {
            ss_flags: libc::SS_DISABLE,
            ..stack
        };
        assert_eq!(unsafe { libc::sigaltstack(&disabled, ptr::null_mut()) }, 0);
    }
}
