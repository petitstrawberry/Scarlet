use core::arch::asm;

pub const ICH_LR_STATE_INVALID: u64 = 0b00 << 62;
pub const ICH_LR_STATE_PENDING: u64 = 0b01 << 62;
pub const ICH_LR_STATE_ACTIVE: u64 = 0b10 << 62;
pub const ICH_LR_STATE_PENDING_ACTIVE: u64 = 0b11 << 62;
pub const ICH_LR_HW: u64 = 1 << 61;
pub const ICH_LR_GROUP: u64 = 1 << 60;
pub const ICH_LR_PRIORITY_SHIFT: u32 = 48;
pub const ICH_LR_VINTID_MASK: u64 = (1 << 32) - 1;
pub const ICH_LR_EOI: u64 = 1 << 41;

pub const ICH_HCR_EN: u64 = 1 << 0;
pub const ICH_HCR_UIE: u64 = 1 << 1;
pub const ICH_HCR_LRENPIE: u64 = 1 << 2;
pub const ICH_HCR_NPIE: u64 = 1 << 3;
pub const ICH_HCR_VGRP0EIE: u64 = 1 << 4;
pub const ICH_HCR_VGRP1EIE: u64 = 1 << 6;
pub const ICH_HCR_TDIR: u64 = 1 << 14;

pub const ICH_VMCR_VENG0: u64 = 1 << 0;
pub const ICH_VMCR_VENG1: u64 = 1 << 1;
pub const ICH_VMCR_VFIQEN: u64 = 1 << 3;
pub const ICH_VMCR_VEOIM: u64 = 1 << 9;
pub const ICH_VMCR_VBPR1_SHIFT: u32 = 18;
pub const ICH_VMCR_VBPR0_SHIFT: u32 = 21;
pub const ICH_VMCR_VPMR_SHIFT: u32 = 24;

pub struct VgicState {
    pub num_lrs: usize,
    pub hcr: u64,
    pub vmcr: u64,
    pub lr_shadow: [u64; 16],
}

impl VgicState {
    pub fn new(num_lrs: usize) -> Self {
        Self {
            num_lrs,
            hcr: 0,
            vmcr: 0,
            lr_shadow: [0; 16],
        }
    }
}

#[inline(always)]
unsafe fn read_ich_vtr_el2() -> u64 {
    let val: u64;
    // SAFETY: caller guarantees execution at EL2 where ICH_VTR_EL2 is accessible.
    unsafe {
        asm!("mrs {}, ich_vtr_el2", out(reg) val, options(nostack));
    }
    val
}

#[inline(always)]
unsafe fn read_ich_lr(index: usize) -> u64 {
    match index {
        0 => {
            let val: u64;
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("mrs {}, ich_lr0_el2", out(reg) val, options(nostack)) };
            val
        }
        1 => {
            let val: u64;
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("mrs {}, ich_lr1_el2", out(reg) val, options(nostack)) };
            val
        }
        2 => {
            let val: u64;
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("mrs {}, ich_lr2_el2", out(reg) val, options(nostack)) };
            val
        }
        3 => {
            let val: u64;
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("mrs {}, ich_lr3_el2", out(reg) val, options(nostack)) };
            val
        }
        4 => {
            let val: u64;
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("mrs {}, ich_lr4_el2", out(reg) val, options(nostack)) };
            val
        }
        5 => {
            let val: u64;
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("mrs {}, ich_lr5_el2", out(reg) val, options(nostack)) };
            val
        }
        6 => {
            let val: u64;
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("mrs {}, ich_lr6_el2", out(reg) val, options(nostack)) };
            val
        }
        7 => {
            let val: u64;
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("mrs {}, ich_lr7_el2", out(reg) val, options(nostack)) };
            val
        }
        8 => {
            let val: u64;
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("mrs {}, ich_lr8_el2", out(reg) val, options(nostack)) };
            val
        }
        9 => {
            let val: u64;
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("mrs {}, ich_lr9_el2", out(reg) val, options(nostack)) };
            val
        }
        10 => {
            let val: u64;
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("mrs {}, ich_lr10_el2", out(reg) val, options(nostack)) };
            val
        }
        11 => {
            let val: u64;
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("mrs {}, ich_lr11_el2", out(reg) val, options(nostack)) };
            val
        }
        12 => {
            let val: u64;
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("mrs {}, ich_lr12_el2", out(reg) val, options(nostack)) };
            val
        }
        13 => {
            let val: u64;
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("mrs {}, ich_lr13_el2", out(reg) val, options(nostack)) };
            val
        }
        14 => {
            let val: u64;
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("mrs {}, ich_lr14_el2", out(reg) val, options(nostack)) };
            val
        }
        15 => {
            let val: u64;
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("mrs {}, ich_lr15_el2", out(reg) val, options(nostack)) };
            val
        }
        _ => 0,
    }
}

#[inline(always)]
unsafe fn write_ich_lr(index: usize, val: u64) {
    match index {
        0 => {
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("msr ich_lr0_el2, {}", in(reg) val, options(nostack)) };
        }
        1 => {
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("msr ich_lr1_el2, {}", in(reg) val, options(nostack)) };
        }
        2 => {
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("msr ich_lr2_el2, {}", in(reg) val, options(nostack)) };
        }
        3 => {
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("msr ich_lr3_el2, {}", in(reg) val, options(nostack)) };
        }
        4 => {
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("msr ich_lr4_el2, {}", in(reg) val, options(nostack)) };
        }
        5 => {
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("msr ich_lr5_el2, {}", in(reg) val, options(nostack)) };
        }
        6 => {
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("msr ich_lr6_el2, {}", in(reg) val, options(nostack)) };
        }
        7 => {
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("msr ich_lr7_el2, {}", in(reg) val, options(nostack)) };
        }
        8 => {
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("msr ich_lr8_el2, {}", in(reg) val, options(nostack)) };
        }
        9 => {
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("msr ich_lr9_el2, {}", in(reg) val, options(nostack)) };
        }
        10 => {
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("msr ich_lr10_el2, {}", in(reg) val, options(nostack)) };
        }
        11 => {
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("msr ich_lr11_el2, {}", in(reg) val, options(nostack)) };
        }
        12 => {
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("msr ich_lr12_el2, {}", in(reg) val, options(nostack)) };
        }
        13 => {
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("msr ich_lr13_el2, {}", in(reg) val, options(nostack)) };
        }
        14 => {
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("msr ich_lr14_el2, {}", in(reg) val, options(nostack)) };
        }
        15 => {
            // SAFETY: caller guarantees execution at EL2 with valid VGIC LR access.
            unsafe { asm!("msr ich_lr15_el2, {}", in(reg) val, options(nostack)) };
        }
        _ => {}
    }
}

#[inline(always)]
fn guest_hcr() -> u64 {
    ICH_HCR_EN | ICH_HCR_VGRP0EIE | ICH_HCR_VGRP1EIE
}

#[inline(always)]
fn guest_vmcr() -> u64 {
    (0xFFu64 << ICH_VMCR_VPMR_SHIFT) | ICH_VMCR_VENG0 | ICH_VMCR_VENG1 | ICH_VMCR_VEOIM
}

pub fn probe_vgic() -> usize {
    // SAFETY: probing ICH_VTR_EL2 is valid while running the hypervisor at EL2.
    unsafe { ((read_ich_vtr_el2() & 0x1f) as usize) + 1 }
}

pub fn vgic_guest_entry_init(num_lrs: usize) {
    write_vmcr(guest_vmcr());
    for i in 0..num_lrs.min(16) {
        // SAFETY: LR indices are bounded to the architected 16-register maximum.
        unsafe {
            write_ich_lr(i, 0);
        }
    }
    write_hcr(guest_hcr());
    // SAFETY: ISB orders subsequent guest execution after VGIC programming.
    unsafe {
        asm!("isb", options(nostack));
    }
}

pub fn inject_virq(num_lrs: usize, vintid: u32, priority: u8, group1: bool) -> bool {
    let mut lr_val = (vintid as u64) & ICH_LR_VINTID_MASK;
    lr_val |= (priority as u64) << ICH_LR_PRIORITY_SHIFT;
    lr_val |= ICH_LR_STATE_PENDING;
    if group1 {
        lr_val |= ICH_LR_GROUP;
    }

    for i in 0..num_lrs.min(16) {
        // SAFETY: LR indices are bounded to the architected 16-register maximum.
        let current = unsafe { read_ich_lr(i) };
        if current & (3u64 << 62) == ICH_LR_STATE_INVALID {
            // SAFETY: LR indices are bounded to the architected 16-register maximum.
            unsafe {
                write_ich_lr(i, lr_val);
            }
            return true;
        }
    }
    false
}

pub fn inject_shadow_virq(state: &mut VgicState, vintid: u32, priority: u8, group1: bool) -> bool {
    let mut lr_val = (vintid as u64) & ICH_LR_VINTID_MASK;
    lr_val |= (priority as u64) << ICH_LR_PRIORITY_SHIFT;
    lr_val |= ICH_LR_STATE_PENDING;
    if group1 {
        lr_val |= ICH_LR_GROUP;
    }

    for lr in state.lr_shadow.iter_mut().take(state.num_lrs.min(16)) {
        if (*lr & ICH_LR_VINTID_MASK) == (vintid as u64)
            && (*lr & (3u64 << 62)) != ICH_LR_STATE_INVALID
        {
            *lr = lr_val;
            return true;
        }
    }

    for lr in state.lr_shadow.iter_mut().take(state.num_lrs.min(16)) {
        if (*lr & (3u64 << 62)) == ICH_LR_STATE_INVALID {
            *lr = lr_val;
            return true;
        }
    }

    false
}

pub fn clear_virq(num_lrs: usize, vintid: u32) -> bool {
    for i in 0..num_lrs.min(16) {
        // SAFETY: LR indices are bounded to the architected 16-register maximum.
        let current = unsafe { read_ich_lr(i) };
        if (current & ICH_LR_VINTID_MASK) == (vintid as u64) {
            // SAFETY: LR indices are bounded to the architected 16-register maximum.
            unsafe {
                write_ich_lr(i, 0);
            }
            return true;
        }
    }
    false
}

pub fn clear_shadow_virq(state: &mut VgicState, vintid: u32) -> bool {
    for lr in state.lr_shadow.iter_mut().take(state.num_lrs.min(16)) {
        if (*lr & ICH_LR_VINTID_MASK) == (vintid as u64) {
            *lr = 0;
            return true;
        }
    }
    false
}

pub fn is_virq_pending(num_lrs: usize, vintid: u32) -> bool {
    for i in 0..num_lrs.min(16) {
        // SAFETY: LR indices are bounded to the architected 16-register maximum.
        let lr = unsafe { read_ich_lr(i) };
        if (lr & ICH_LR_VINTID_MASK) == (vintid as u64)
            && (lr & (3u64 << 62)) != ICH_LR_STATE_INVALID
        {
            return true;
        }
    }
    false
}

pub fn is_shadow_virq_pending(state: &VgicState, vintid: u32) -> bool {
    state
        .lr_shadow
        .iter()
        .take(state.num_lrs.min(16))
        .any(|lr| {
            (*lr & ICH_LR_VINTID_MASK) == (vintid as u64)
                && (*lr & (3u64 << 62)) != ICH_LR_STATE_INVALID
        })
}

pub fn save_lrs(num_lrs: usize, out: &mut [u64; 16]) {
    for i in 0..num_lrs.min(16) {
        // SAFETY: LR indices are bounded to the architected 16-register maximum.
        out[i] = unsafe { read_ich_lr(i) };
    }
    for slot in out.iter_mut().skip(num_lrs.min(16)) {
        *slot = 0;
    }
}

pub fn restore_lrs(num_lrs: usize, vals: &[u64; 16]) {
    for i in 0..num_lrs.min(16) {
        // SAFETY: LR indices are bounded to the architected 16-register maximum.
        unsafe {
            write_ich_lr(i, vals[i]);
        }
    }
}

pub fn read_hcr() -> u64 {
    let val: u64;
    // SAFETY: reading ICH_HCR_EL2 is valid while executing at EL2.
    unsafe {
        asm!("mrs {}, ich_hcr_el2", out(reg) val, options(nostack));
    }
    val
}

pub fn write_hcr(val: u64) {
    // SAFETY: writing ICH_HCR_EL2 is valid while executing at EL2.
    unsafe {
        asm!("msr ich_hcr_el2, {}", in(reg) val, options(nostack));
    }
}

pub fn read_vmcr() -> u64 {
    let val: u64;
    // SAFETY: reading ICH_VMCR_EL2 is valid while executing at EL2.
    unsafe {
        asm!("mrs {}, ich_vmcr_el2", out(reg) val, options(nostack));
    }
    val
}

pub fn write_vmcr(val: u64) {
    // SAFETY: writing ICH_VMCR_EL2 is valid while executing at EL2.
    unsafe {
        asm!("msr ich_vmcr_el2, {}", in(reg) val, options(nostack));
    }
}

pub fn restore_host_vgic(num_lrs: usize, hcr: u64, vmcr: u64) {
    write_hcr(hcr);
    write_vmcr(vmcr);
    for i in 0..num_lrs.min(16) {
        // SAFETY: LR indices are bounded by the hardware-reported ListRegs count.
        unsafe {
            write_ich_lr(i, 0);
        }
    }
    // SAFETY: ISB orders the restored host VGIC state before host execution resumes.
    unsafe {
        asm!("isb", options(nostack));
    }
}

pub fn restore_guest_state(state: &VgicState) {
    write_vmcr(state.vmcr);
    restore_lrs(state.num_lrs, &state.lr_shadow);
    write_hcr(state.hcr);
    // SAFETY: ISB orders subsequent guest execution after restoring VGIC state.
    unsafe {
        asm!("isb", options(nostack));
    }
}

pub fn save_guest_state(state: &mut VgicState) {
    state.hcr = read_hcr();
    state.vmcr = read_vmcr();
    save_lrs(state.num_lrs, &mut state.lr_shadow);
}
