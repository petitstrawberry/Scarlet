//! Guest world-switch for RISC-V H-extension
//!
//! Entry/exit flow:
//! 1. `guest_enter()` saves host stvec, redirects stvec → `_guest_trap_vector`
//! 2. Writes VS-mode CSRs, sets hstatus.SPV, sepc = guest PC
//! 3. Loads guest GPRs, executes `sret` → VS-mode
//! 4. On guest trap: CPU vectors to `_guest_trap_vector` in HS-mode
//! 5. `_guest_trap_vector` saves guest GPRs, restores host sp & callee-saved regs
//! 6. Control returns to `guest_enter` which reads exit CSRs, restores stvec

use core::arch::{asm, global_asm};

use super::csr;
use super::vmexit::VmExitInfo;

/// Full guest CPU state saved/restored across world-switches.
///
/// `#[repr(C)]` — assembly accesses fields at fixed offsets.
#[repr(C)]
#[derive(Clone)]
pub struct GuestState {
    pub gprs: [u64; 32], // 0..248  (index × 8)
    pub pc: u64,         // 256
    pub hstatus: u64,    // 264
    pub vsstatus: u64,   // 272
    pub vsie: u64,       // 280
    pub vstvec: u64,     // 288
    pub vsscratch: u64,  // 296
    pub vsepc: u64,      // 304
    pub vscause: u64,    // 312
    pub vstval: u64,     // 320
    pub vsip: u64,       // 328
    pub vsatp: u64,      // 336
    pub host_sp: u64,    // 344 — host sp stashed here for _guest_trap_vector
}

impl GuestState {
    pub fn new() -> Self {
        Self {
            gprs: [0; 32],
            pc: 0,
            hstatus: 0,
            vsstatus: 0,
            vsie: 0,
            vstvec: 0,
            vsscratch: 0,
            vsepc: 0,
            vscause: 0,
            vstval: 0,
            vsip: 0,
            vsatp: 0,
            host_sp: 0,
        }
    }
}

impl Default for GuestState {
    fn default() -> Self {
        Self::new()
    }
}

// _guest_trap_vector: HS-mode trap handler for guest exits.
//
// On entry: sscratch = GuestState*, all GPRs hold guest values.
// Saves guest GPRs → GuestState, guest PC (sepc) → GuestState.pc,
// restores host sp from GuestState.host_sp, pops host callee-saved
// registers, and returns to guest_enter's call site.
global_asm!(
    ".option norvc",
    ".option norelax",
    ".align 4",
    ".global _guest_trap_vector",
    "_guest_trap_vector:",
    // a0 ↔ sscratch: now a0 = GuestState*, sscratch = guest a0
    "csrrw a0, sscratch, a0",
    // Save guest GPRs (x0 is zero, x10/a0 handled via sscratch)
    "sd x1,    8(a0)",
    "sd x2,   16(a0)",
    "sd x3,   24(a0)",
    "sd x4,   32(a0)",
    "sd x5,   40(a0)",
    "sd x6,   48(a0)",
    "sd x7,   56(a0)",
    "sd x8,   64(a0)",
    "sd x9,   72(a0)",
    "sd x11,  88(a0)",
    "sd x12,  96(a0)",
    "sd x13, 104(a0)",
    "sd x14, 112(a0)",
    "sd x15, 120(a0)",
    "sd x16, 128(a0)",
    "sd x17, 136(a0)",
    "sd x18, 144(a0)",
    "sd x19, 152(a0)",
    "sd x20, 160(a0)",
    "sd x21, 168(a0)",
    "sd x22, 176(a0)",
    "sd x23, 184(a0)",
    "sd x24, 192(a0)",
    "sd x25, 200(a0)",
    "sd x26, 208(a0)",
    "sd x27, 216(a0)",
    "sd x28, 224(a0)",
    "sd x29, 232(a0)",
    "sd x30, 240(a0)",
    "sd x31, 248(a0)",
    // Save guest a0 from sscratch
    "csrr t0, sscratch",
    "sd t0, 80(a0)",
    // Save guest PC (sepc → offset 256)
    "csrr t0, sepc",
    "sd t0, 256(a0)",
    // Restore host sp from GuestState.host_sp (offset 344)
    "ld sp, 344(a0)",
    // Pop host callee-saved registers (pushed by guest_enter)
    "ld ra,   0(sp)",
    "ld s0,   8(sp)",
    "ld s1,  16(sp)",
    "ld s2,  24(sp)",
    "ld s3,  32(sp)",
    "ld s4,  40(sp)",
    "ld s5,  48(sp)",
    "ld s6,  56(sp)",
    "ld s7,  64(sp)",
    "ld s8,  72(sp)",
    "ld s9,  80(sp)",
    "ld s10, 88(sp)",
    "ld s11, 96(sp)",
    "addi sp, sp, 112",
    // Return to guest_enter
    "ret",
);

unsafe extern "C" {
    fn _guest_trap_vector();
}

pub(super) fn guest_enter(guest: &mut GuestState, guest_root_token: u64) -> VmExitInfo {
    // Save host stvec and redirect to our guest trap vector
    let saved_stvec: u64;
    unsafe { asm!("csrr {}, stvec", out(reg) saved_stvec) };
    let guest_vector = _guest_trap_vector as usize as u64;
    unsafe { asm!("csrw stvec, {}", in(reg) guest_vector) };

    // Load VS-mode CSRs
    csr::write_vsstatus(guest.vsstatus);
    csr::write_vsie(guest.vsie);
    csr::write_vstvec(guest.vstvec);
    csr::write_vsscratch(guest.vsscratch);
    csr::write_vsepc(guest.vsepc);
    csr::write_vscause(guest.vscause);
    csr::write_vstval(guest.vstval);
    csr::write_vsip(guest.vsip);
    csr::write_vsatp(guest.vsatp);

    // hstatus.SPV = 1 → sret enters VS-mode
    let mut hstatus = guest.hstatus;
    hstatus |= csr::HSTATUS_SPV;
    csr::write_hstatus(hstatus);

    csr::write_sepc(guest.pc);

    csr::write_hgatp(guest_root_token);
    csr::hfence_gvma_all();

    // sstatus: SPP=0 (→ VS-mode), SPIE=1 (interrupts enabled in guest)
    let mut sstatus = csr::read_sstatus();
    sstatus &= !(1u64 << 8);
    sstatus |= 1u64 << 5;
    csr::write_sstatus(sstatus);

    let gs_ptr = guest as *mut GuestState;

    // SAFETY: Saves host callee-saved regs, stores host sp in GuestState.host_sp,
    // puts GuestState* in sscratch, loads guest GPRs, sret into guest.
    // On guest trap: _guest_trap_vector saves guest state, restores host, rets here.
    unsafe {
        asm!(
            // Push host callee-saved
            "addi sp, sp, -112",
            "sd ra,   0(sp)",
            "sd s0,   8(sp)",
            "sd s1,  16(sp)",
            "sd s2,  24(sp)",
            "sd s3,  32(sp)",
            "sd s4,  40(sp)",
            "sd s5,  48(sp)",
            "sd s6,  56(sp)",
            "sd s7,  64(sp)",
            "sd s8,  72(sp)",
            "sd s9,  80(sp)",
            "sd s10, 88(sp)",
            "sd s11, 96(sp)",
            // Stash host sp for _guest_trap_vector (offset 344)
            "sd sp, 344({gs})",
            // GuestState* → sscratch for _guest_trap_vector
            "csrw sscratch, {gs}",
            // Load guest GPRs (skip x0=zero, x10=a0 loaded last)
            "ld x1,    8({gs})",
            "ld x2,   16({gs})",
            "ld x3,   24({gs})",
            "ld x4,   32({gs})",
            "ld x5,   40({gs})",
            "ld x6,   48({gs})",
            "ld x7,   56({gs})",
            "ld x8,   64({gs})",
            "ld x9,   72({gs})",
            "ld x11,  88({gs})",
            "ld x12,  96({gs})",
            "ld x13, 104({gs})",
            "ld x14, 112({gs})",
            "ld x15, 120({gs})",
            "ld x16, 128({gs})",
            "ld x17, 136({gs})",
            "ld x18, 144({gs})",
            "ld x19, 152({gs})",
            "ld x20, 160({gs})",
            "ld x21, 168({gs})",
            "ld x22, 176({gs})",
            "ld x23, 184({gs})",
            "ld x24, 192({gs})",
            "ld x25, 200({gs})",
            "ld x26, 208({gs})",
            "ld x27, 216({gs})",
            "ld x28, 224({gs})",
            "ld x29, 232({gs})",
            "ld x30, 240({gs})",
            "ld x31, 248({gs})",
            // Load guest a0 last (overwrites our base pointer)
            "ld x10,  80({gs})",
            // sret → VS-mode (hstatus.SPV=1, sstatus.SPP=0)
            "sret",
            // _guest_trap_vector restores host state and rets here
            gs = in(reg) gs_ptr,
            clobber_abi("C"),
        );
    }

    // Save VS-mode CSRs back (guest may have modified them)
    guest.hstatus = csr::read_hstatus();
    guest.vsstatus = csr::read_vsstatus();
    guest.vsie = csr::read_vsie();
    guest.vstvec = csr::read_vstvec();
    guest.vsscratch = csr::read_vsscratch();
    guest.vsepc = csr::read_vsepc();
    guest.vscause = csr::read_vscause();
    guest.vstval = csr::read_vstval();
    guest.vsip = csr::read_vsip();
    guest.vsatp = csr::read_vsatp();

    let exit_info = VmExitInfo::capture(guest.pc);

    // Restore host stvec
    unsafe { asm!("csrw stvec, {}", in(reg) saved_stvec) };

    // Disable G-stage translation
    csr::write_hgatp(0);
    csr::hfence_gvma_all();

    exit_info
}
