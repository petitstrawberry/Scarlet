use core::arch::asm;

use crate::early_println;

use super::Trapframe;

pub mod exception;
pub mod interrupt;
pub mod kernel;
pub mod user;

pub fn print_traplog(tf: &Trapframe) {
    let cause: usize;
    let tval: usize;
    let status: usize;
    let sepc: usize;
    let stvec: usize;
    let satp: usize;
    let sscratch: usize;
    #[cfg(feature = "hypervisor")]
    let hstatus: usize;

    unsafe {
        asm!("csrr {}, scause", out(reg) cause);
        asm!("csrr {}, stval", out(reg) tval);
        asm!("csrr {}, sstatus", out(reg) status);
        asm!("csrr {}, sepc", out(reg) sepc);
        asm!("csrr {}, stvec", out(reg) stvec);
        asm!("csrr {}, satp", out(reg) satp);
        asm!("csrr {}, sscratch", out(reg) sscratch);
        #[cfg(feature = "hypervisor")]
        asm!("csrr {}, hstatus", out(reg) hstatus);
    }
    let spp = (status >> 8) & 0b1;

    early_println!("trapframe:\n{:#x?}", tf);
    early_println!("cause: {}", cause);
    early_println!("tval: 0x{:x}", tval);
    early_println!("status: 0x{:x}", status);
    early_println!("spp: {}", spp);
    early_println!("sepc: 0x{:x}", sepc);
    early_println!("stvec: 0x{:x}", stvec);
    early_println!("satp: 0x{:x}", satp);
    early_println!("sscratch: 0x{:x}", sscratch);
    #[cfg(feature = "hypervisor")]
    {
        use crate::initcall::early;

        early_println!("hstatus: 0x{:x}", hstatus);
        early_println!(
            "HSTATUS_SPV: {}",
            (hstatus & crate::arch::hv::trap::HSTATUS_SPV as usize) != 0
        );
    }
}

pub const PRIV_U_MODE: usize = 0;
pub const PRIV_S_MODE: usize = 1;

pub fn prev_mode() -> usize {
    let status: usize;
    unsafe {
        asm!("csrr {}, sstatus", out(reg) status);
    }
    (status >> 8) & 0b1
}

pub mod cause {
    pub const EXCEPTION_INSTRUCTION_ADDRESS_MISALIGNED: usize = 0;
    pub const EXCEPTION_INSTRUCTION_ACCESS_FAULT: usize = 1;
    pub const EXCEPTION_ILLEGAL_INSTRUCTION: usize = 2;
    pub const EXCEPTION_BREAKPOINT: usize = 3;
    pub const EXCEPTION_LOAD_ADDRESS_MISALIGNED: usize = 4;
    pub const EXCEPTION_LOAD_ACCESS_FAULT: usize = 5;
    pub const EXCEPTION_STORE_AMO_ADDRESS_MISALIGNED: usize = 6;
    pub const EXCEPTION_STORE_AMO_ACCESS_FAULT: usize = 7;
    pub const EXCEPTION_ENVIRONMENT_CALL_FROM_UMODE_OR_VUMODE: usize = 8;
    pub const EXCEPTION_ENVIRONMENT_CALL_FROM_SMODE: usize = 9;
    pub const EXCEPTION_ENVIRONMENT_CALL_FROM_VSMODE: usize = 10;
    pub const EXCEPTION_ENVIRONMENT_CALL_FROM_MMODE: usize = 11;
    pub const EXCEPTION_INSTRUCTION_PAGE_FAULT: usize = 12;
    pub const EXCEPTION_LOAD_PAGE_FAULT: usize = 13;
    // reserved 14
    pub const EXCEPTION_STORE_AMO_PAGE_FAULT: usize = 15;
    // 16-19 reserved
    pub const EXCEPTION_INSTRUCTION_GUEST_PAGE_FAULT: usize = 20;
    pub const EXCEPTION_LOAD_GUEST_PAGE_FAULT: usize = 21;
    pub const EXCEPTION_VIRTUAL_INSTRUCTION: usize = 22;
    pub const EXCEPTION_STORE_AMO_GUEST_PAGE_FAULT: usize = 23;
    // 24-31 designated for custom use
    // 32-47 reserved
    // 48-63 designated for custom use
    // >= 64: reserved

    ///1 0 Reserved  1 1 Supervisor software interrupt 1 2 Virtual supervisor software interrupt 1 3 Machine software interrupt 1 4 Reserved  1 5 Supervisor timer interrupt 1 6 Virtual supervisor timer interrupt 1 7 Machine timer interrupt 1 8 Reserved  1 9 Supervisor external interrupt 1 10 Virtual supervisor external interrupt 1 11 Machine external interrupt 1 12 Supervisor guest external interrupt 1 13–15 Reserved 1 ≥16 Designated for platform or custom use

    pub const INTERRUPT_SUPERVISOR_SOFTWARE: usize = 1;
    pub const INTERRUPT_VIRTUAL_SUPERVISOR_SOFTWARE: usize = 2;
    pub const INTERRUPT_MACHINE_SOFTWARE: usize = 3;
    // 4 reserved
    pub const INTERRUPT_SUPERVISOR_TIMER: usize = 5;
    pub const INTERRUPT_VIRTUAL_SUPERVISOR_TIMER: usize = 6;
    pub const INTERRUPT_MACHINE_TIMER: usize = 7;
    // 8 reserved
    pub const INTERRUPT_SUPERVISOR_EXTERNAL: usize = 9;
    pub const INTERRUPT_VIRTUAL_SUPERVISOR_EXTERNAL: usize = 10;
    pub const INTERRUPT_MACHINE_EXTERNAL: usize = 11;
    pub const INTERRUPT_SUPERVISOR_GUEST_EXTERNAL: usize = 12;
    // 13-15 reserved
    // >= 16 designated for platform or custom use
}
