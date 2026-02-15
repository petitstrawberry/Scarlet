//! RISC-V H-extension hypervisor support

pub mod csr;
pub mod guest_vcpu;
pub mod mmu;
pub mod reg_index;
pub mod switch;
pub mod trap;

pub use guest_vcpu::{
    GuestCsrState, GuestVcpu, clear_current_guest_vcpu, current_guest_vcpu, set_current_guest_vcpu,
};
pub use reg_index::reg;
pub use switch::{arch_guest_trap_exit, run_guest_loop, run_guest_loop_return_addr};
pub use trap::RiscvTrapInfo;

pub const HSTATUS_SPV: u64 = 1 << 7;

use crate::arch::Trapframe;
use crate::hypervisor::trap::VmTrapInfo;
use core::ptr::addr_of_mut;

static mut LAST_TRAP_INFO: Option<RiscvTrapInfo> = None;

pub fn get_last_trap_info() -> Option<RiscvTrapInfo> {
    unsafe { (*addr_of_mut!(LAST_TRAP_INFO)).take() }
}

pub fn is_guest_trap() -> bool {
    use core::arch::asm;
    let hstatus: u64;
    unsafe {
        asm!("csrr {0}, hstatus", out(reg) hstatus);
    }
    (hstatus & HSTATUS_SPV) != 0
}

fn clear_guest_mode() {
    use core::arch::asm;
    let mut hstatus: u64;
    unsafe {
        asm!("csrr {0}, hstatus", out(reg) hstatus);
        hstatus &= !HSTATUS_SPV;
        asm!("csrw hstatus, {0}", in(reg) hstatus);
    }
}

pub fn guest_trap_handler(trapframe: &mut Trapframe) -> bool {
    if !is_guest_trap() {
        return false;
    }

    let vcpu = unsafe { guest_vcpu::current_guest_vcpu() };

    vcpu.store(trapframe);
    vcpu.save_csrs();

    let trap_info = RiscvTrapInfo::capture();

    unsafe {
        *addr_of_mut!(LAST_TRAP_INFO) = Some(trap_info);
    }

    clear_guest_mode();
    unsafe {
        guest_vcpu::clear_current_guest_vcpu();
    }

    true
}

pub fn set_guest_root_pagetable(token: u64) {
    use core::arch::asm;
    unsafe {
        asm!("csrw hgatp, {0}", in(reg) token);
        asm!("hfence.gvma zero, zero");
    }
}

pub fn set_guest_asid(asid: usize) {
    use core::arch::asm;
    unsafe {
        asm!("csrw asid, {0}", in(reg) asid);
    }
}
