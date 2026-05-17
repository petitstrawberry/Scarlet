//! RISC-V KVM register conversion

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Once, RwLock};

use crate::abi::linux::generic::LinuxAbi;
use crate::arch::hv::reg_index::reg;
use crate::hypervisor::{VcpuRef, VmRef};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KvmRegs {
    pub pc: u64,
    pub ra: u64,
    pub sp: u64,
    pub gp: u64,
    pub tp: u64,
    pub t0: u64,
    pub t1: u64,
    pub t2: u64,
    pub s0: u64,
    pub s1: u64,
    pub a0: u64,
    pub a1: u64,
    pub a2: u64,
    pub a3: u64,
    pub a4: u64,
    pub a5: u64,
    pub a6: u64,
    pub a7: u64,
    pub s2: u64,
    pub s3: u64,
    pub s4: u64,
    pub s5: u64,
    pub s6: u64,
    pub s7: u64,
    pub s8: u64,
    pub s9: u64,
    pub s10: u64,
    pub s11: u64,
    pub t3: u64,
    pub t4: u64,
    pub t5: u64,
    pub t6: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KvmOneReg {
    pub id: u64,
    pub addr: u64,
}

const KVM_REG_ARCH_MASK: u64 = 0xff00_0000_0000_0000;
const KVM_REG_RISCV: u64 = 0x8000_0000_0000_0000;
const KVM_REG_SIZE_MASK: u64 = 0x00f0_0000_0000_0000;
const KVM_REG_SIZE_U64: u64 = 0x0030_0000_0000_0000;

const KVM_REG_RISCV_TYPE_MASK: u64 = 0x0000_0000_ff00_0000;
const KVM_REG_RISCV_TYPE_SHIFT: u64 = 24;
const KVM_REG_RISCV_SUBTYPE_MASK: u64 = 0x0000_0000_00ff_0000;

const KVM_REG_RISCV_CONFIG: u64 = 0x01 << KVM_REG_RISCV_TYPE_SHIFT;
const KVM_REG_RISCV_CORE: u64 = 0x02 << KVM_REG_RISCV_TYPE_SHIFT;
const KVM_REG_RISCV_CSR: u64 = 0x03 << KVM_REG_RISCV_TYPE_SHIFT;
const KVM_REG_RISCV_TIMER: u64 = 0x04 << KVM_REG_RISCV_TYPE_SHIFT;
const KVM_REG_RISCV_ISA_EXT: u64 = 0x07 << KVM_REG_RISCV_TYPE_SHIFT;
const KVM_REG_RISCV_SBI_EXT: u64 = 0x08 << KVM_REG_RISCV_TYPE_SHIFT;

const KVM_REG_RISCV_CSR_GENERAL: u64 = 0x0 << 16;

const KVM_REG_RISCV_CSR_SSTATUS: u64 = 0;
const KVM_REG_RISCV_CSR_SIE: u64 = 1;
const KVM_REG_RISCV_CSR_STVEC: u64 = 2;
const KVM_REG_RISCV_CSR_SSCRATCH: u64 = 3;
const KVM_REG_RISCV_CSR_SEPC: u64 = 4;
const KVM_REG_RISCV_CSR_SCAUSE: u64 = 5;
const KVM_REG_RISCV_CSR_STVAL: u64 = 6;
const KVM_REG_RISCV_CSR_SIP: u64 = 7;
const KVM_REG_RISCV_CSR_SATP: u64 = 8;
const KVM_REG_RISCV_CSR_SCOUNTEREN: u64 = 9;
const KVM_REG_RISCV_CSR_SENVCFG: u64 = 10;

const KVM_REG_RISCV_ISA_SINGLE: u64 = 0x0 << 16;
const KVM_REG_RISCV_ISA_MULTI_EN: u64 = 0x1 << 16;
const KVM_REG_RISCV_ISA_MULTI_DIS: u64 = 0x2 << 16;
const KVM_REG_RISCV_SBI_SINGLE: u64 = 0x0 << 16;
const KVM_REG_RISCV_SBI_MULTI_EN: u64 = 0x1 << 16;
const KVM_REG_RISCV_SBI_MULTI_DIS: u64 = 0x2 << 16;

const KVM_REG_RISCV_CONFIG_ISA: u64 = 0;
const KVM_REG_RISCV_CONFIG_ZICBOM_BLOCK_SIZE: u64 = 1;
const KVM_REG_RISCV_CONFIG_MVENDORID: u64 = 2;
const KVM_REG_RISCV_CONFIG_MARCHID: u64 = 3;
const KVM_REG_RISCV_CONFIG_MIMPID: u64 = 4;
const KVM_REG_RISCV_CONFIG_ZICBOZ_BLOCK_SIZE: u64 = 5;
const KVM_REG_RISCV_CONFIG_SATP_MODE: u64 = 6;
const KVM_REG_RISCV_CONFIG_ZICBOP_BLOCK_SIZE: u64 = 7;

const KVM_REG_RISCV_CORE_MODE: u64 = 32;

const KVM_REG_RISCV_TIMER_FREQUENCY: u64 = 0;
const KVM_REG_RISCV_TIMER_TIME: u64 = 1;
const KVM_REG_RISCV_TIMER_COMPARE: u64 = 2;
const KVM_REG_RISCV_TIMER_STATE: u64 = 3;

const KVM_RISCV_MODE_U: u64 = 0;
const KVM_RISCV_MODE_S: u64 = 1;
const KVM_RISCV_TIMER_STATE_OFF: u64 = 0;
const KVM_RISCV_TIMER_STATE_ON: u64 = 1;

const KVM_RISCV_ISA_EXT_A: usize = 0;
const KVM_RISCV_ISA_EXT_C: usize = 1;
const KVM_RISCV_ISA_EXT_D: usize = 2;
const KVM_RISCV_ISA_EXT_F: usize = 3;
const KVM_RISCV_ISA_EXT_I: usize = 5;
const KVM_RISCV_ISA_EXT_M: usize = 6;

const KVM_RISCV_SBI_EXT_V01: usize = 0;
const KVM_RISCV_SBI_EXT_TIME: usize = 1;
const KVM_RISCV_SBI_EXT_IPI: usize = 2;
const KVM_RISCV_SBI_EXT_RFENCE: usize = 3;
const KVM_RISCV_SBI_EXT_SRST: usize = 4;
const KVM_RISCV_SBI_EXT_HSM: usize = 5;
const KVM_RISCV_SBI_EXT_DBCN: usize = 9;

const DEFAULT_ISA: u64 = 0x14110d;
const DEFAULT_TIMER_FREQUENCY: u64 = 10_000_000;
const DEFAULT_SATP_MODE: u64 = 8;

#[derive(Clone, Copy)]
struct KvmRiscvConfigState {
    isa: u64,
    zicbom_block_size: u64,
    mvendorid: u64,
    marchid: u64,
    mimpid: u64,
    zicboz_block_size: u64,
    satp_mode: u64,
    zicbop_block_size: u64,
}

#[derive(Clone, Copy)]
struct KvmRiscvTimerState {
    frequency: u64,
    time: u64,
    compare: u64,
    state: u64,
}

#[derive(Clone, Copy)]
struct KvmRiscvVcpuState {
    config: KvmRiscvConfigState,
    timer: KvmRiscvTimerState,
    isa_ext: [u64; 2],
    sbi_ext: u64,
    mode: u64,
}

struct KvmRiscvVcpuStateEntry {
    vcpu: VcpuRef,
    state: KvmRiscvVcpuState,
}

static KVM_RISCV_VCPU_STATES: Once<RwLock<Vec<KvmRiscvVcpuStateEntry>>> = Once::new();

const PTRACE_REG_INDEX: [u32; 32] = [
    reg::PC,
    reg::RA,
    reg::SP,
    reg::GP,
    reg::TP,
    reg::T0,
    reg::T1,
    reg::T2,
    reg::S0,
    reg::S1,
    reg::A0,
    reg::A1,
    reg::A2,
    reg::A3,
    reg::A4,
    reg::A5,
    reg::A6,
    reg::A7,
    reg::S2,
    reg::S3,
    reg::S4,
    reg::S5,
    reg::S6,
    reg::S7,
    reg::S8,
    reg::S9,
    reg::S10,
    reg::S11,
    reg::T3,
    reg::T4,
    reg::T5,
    reg::T6,
];

fn get_vcpu_states() -> &'static RwLock<Vec<KvmRiscvVcpuStateEntry>> {
    KVM_RISCV_VCPU_STATES.call_once(|| RwLock::new(Vec::new()))
}

fn default_vcpu_state() -> KvmRiscvVcpuState {
    let mut isa_ext = [0u64; 2];
    set_bitmap_bit(&mut isa_ext, KVM_RISCV_ISA_EXT_A, true);
    set_bitmap_bit(&mut isa_ext, KVM_RISCV_ISA_EXT_C, true);
    set_bitmap_bit(&mut isa_ext, KVM_RISCV_ISA_EXT_D, true);
    set_bitmap_bit(&mut isa_ext, KVM_RISCV_ISA_EXT_F, true);
    set_bitmap_bit(&mut isa_ext, KVM_RISCV_ISA_EXT_I, true);
    set_bitmap_bit(&mut isa_ext, KVM_RISCV_ISA_EXT_M, true);

    let mut sbi_ext = 0u64;
    set_u64_bit(&mut sbi_ext, KVM_RISCV_SBI_EXT_V01, true);
    set_u64_bit(&mut sbi_ext, KVM_RISCV_SBI_EXT_TIME, true);
    set_u64_bit(&mut sbi_ext, KVM_RISCV_SBI_EXT_IPI, true);
    set_u64_bit(&mut sbi_ext, KVM_RISCV_SBI_EXT_RFENCE, true);
    set_u64_bit(&mut sbi_ext, KVM_RISCV_SBI_EXT_SRST, true);
    set_u64_bit(&mut sbi_ext, KVM_RISCV_SBI_EXT_HSM, true);
    set_u64_bit(&mut sbi_ext, KVM_RISCV_SBI_EXT_DBCN, true);

    KvmRiscvVcpuState {
        config: KvmRiscvConfigState {
            isa: DEFAULT_ISA,
            zicbom_block_size: 0,
            mvendorid: 0,
            marchid: 0,
            mimpid: 0,
            zicboz_block_size: 0,
            satp_mode: DEFAULT_SATP_MODE,
            zicbop_block_size: 0,
        },
        timer: KvmRiscvTimerState {
            frequency: DEFAULT_TIMER_FREQUENCY,
            time: 0,
            compare: 0,
            state: KVM_RISCV_TIMER_STATE_OFF,
        },
        isa_ext,
        sbi_ext,
        mode: KVM_RISCV_MODE_S,
    }
}

fn with_vcpu_state<R>(vcpu: &VcpuRef, f: impl FnOnce(&mut KvmRiscvVcpuState) -> R) -> R {
    let mut states = get_vcpu_states().write();
    if let Some(entry) = states
        .iter_mut()
        .find(|entry| Arc::ptr_eq(&entry.vcpu, vcpu))
    {
        return f(&mut entry.state);
    }

    states.push(KvmRiscvVcpuStateEntry {
        vcpu: Arc::clone(vcpu),
        state: default_vcpu_state(),
    });

    if let Some(entry) = states.last_mut() {
        f(&mut entry.state)
    } else {
        f(&mut default_vcpu_state())
    }
}

fn set_bitmap_bit(bitmap: &mut [u64; 2], index: usize, enabled: bool) {
    let word = index / 64;
    let bit = index % 64;
    if word >= bitmap.len() {
        return;
    }

    if enabled {
        bitmap[word] |= 1u64 << bit;
    } else {
        bitmap[word] &= !(1u64 << bit);
    }
}

fn get_bitmap_bit(bitmap: &[u64; 2], index: usize) -> u64 {
    let word = index / 64;
    let bit = index % 64;
    if word >= bitmap.len() {
        return 0;
    }
    ((bitmap[word] >> bit) & 1) as u64
}

fn get_bitmap_word(bitmap: &[u64; 2], word: usize) -> u64 {
    bitmap.get(word).copied().unwrap_or(0)
}

fn set_bitmap_word(bitmap: &mut [u64; 2], word: usize, value: u64, enable: bool) {
    if let Some(slot) = bitmap.get_mut(word) {
        if enable {
            *slot |= value;
        } else {
            *slot &= !value;
        }
    }
}

fn set_u64_bit(bitmap: &mut u64, index: usize, enabled: bool) {
    if index >= 64 {
        return;
    }

    if enabled {
        *bitmap |= 1u64 << index;
    } else {
        *bitmap &= !(1u64 << index);
    }
}

fn get_u64_bit(bitmap: u64, index: usize) -> u64 {
    if index >= 64 {
        return 0;
    }
    ((bitmap >> index) & 1) as u64
}

fn kvm_reg_type(id: u64) -> u64 {
    id & KVM_REG_RISCV_TYPE_MASK
}

fn kvm_reg_subtype(id: u64) -> u64 {
    id & KVM_REG_RISCV_SUBTYPE_MASK
}

fn kvm_reg_index(id: u64) -> u64 {
    id & 0x0000_ffff
}

fn kvm_reg_offset(id: u64) -> u64 {
    id & 0x00ff_ffff
}

fn validate_one_reg_id(id: u64) -> Result<(), ()> {
    if (id & KVM_REG_ARCH_MASK) != KVM_REG_RISCV {
        return Err(());
    }
    if (id & KVM_REG_SIZE_MASK) != KVM_REG_SIZE_U64 {
        return Err(());
    }
    Ok(())
}

pub fn get_one_reg(vcpu: &VcpuRef, id: u64) -> Result<u64, ()> {
    validate_one_reg_id(id)?;

    match kvm_reg_type(id) {
        KVM_REG_RISCV_CONFIG => with_vcpu_state(vcpu, |state| match kvm_reg_offset(id) {
            KVM_REG_RISCV_CONFIG_ISA => Ok(state.config.isa),
            KVM_REG_RISCV_CONFIG_ZICBOM_BLOCK_SIZE => Ok(state.config.zicbom_block_size),
            KVM_REG_RISCV_CONFIG_MVENDORID => Ok(state.config.mvendorid),
            KVM_REG_RISCV_CONFIG_MARCHID => Ok(state.config.marchid),
            KVM_REG_RISCV_CONFIG_MIMPID => Ok(state.config.mimpid),
            KVM_REG_RISCV_CONFIG_ZICBOZ_BLOCK_SIZE => Ok(state.config.zicboz_block_size),
            KVM_REG_RISCV_CONFIG_SATP_MODE => Ok(state.config.satp_mode),
            KVM_REG_RISCV_CONFIG_ZICBOP_BLOCK_SIZE => Ok(state.config.zicbop_block_size),
            _ => Err(()),
        }),
        KVM_REG_RISCV_CORE => match kvm_reg_offset(id) {
            offset @ 0..=31 => vcpu
                .get_reg(PTRACE_REG_INDEX[offset as usize])
                .map_err(|_| ()),
            KVM_REG_RISCV_CORE_MODE => with_vcpu_state(vcpu, |state| Ok(state.mode)),
            _ => Err(()),
        },
        KVM_REG_RISCV_CSR => match kvm_reg_subtype(id) {
            KVM_REG_RISCV_CSR_GENERAL => match kvm_reg_offset(id) {
                KVM_REG_RISCV_CSR_SSTATUS => vcpu.get_reg(reg::SSTATUS).map_err(|_| ()),
                KVM_REG_RISCV_CSR_SIE => vcpu.get_reg(reg::SIE).map_err(|_| ()),
                KVM_REG_RISCV_CSR_STVEC => vcpu.get_reg(reg::STVEC).map_err(|_| ()),
                KVM_REG_RISCV_CSR_SSCRATCH => vcpu.get_reg(reg::SSCRATCH).map_err(|_| ()),
                KVM_REG_RISCV_CSR_SEPC => vcpu.get_reg(reg::SEPC).map_err(|_| ()),
                KVM_REG_RISCV_CSR_SCAUSE => vcpu.get_reg(reg::SCAUSE).map_err(|_| ()),
                KVM_REG_RISCV_CSR_STVAL => vcpu.get_reg(reg::STVAL).map_err(|_| ()),
                KVM_REG_RISCV_CSR_SIP => vcpu.get_reg(reg::SIP).map_err(|_| ()),
                KVM_REG_RISCV_CSR_SATP => vcpu.get_reg(reg::SATP).map_err(|_| ()),
                KVM_REG_RISCV_CSR_SCOUNTEREN | KVM_REG_RISCV_CSR_SENVCFG => Ok(0),
                _ => Err(()),
            },
            _ => Err(()),
        },
        KVM_REG_RISCV_TIMER => with_vcpu_state(vcpu, |state| match kvm_reg_offset(id) {
            KVM_REG_RISCV_TIMER_FREQUENCY => Ok(state.timer.frequency),
            KVM_REG_RISCV_TIMER_TIME => Ok(state.timer.time),
            KVM_REG_RISCV_TIMER_COMPARE => Ok(state.timer.compare),
            KVM_REG_RISCV_TIMER_STATE => Ok(state.timer.state),
            _ => Err(()),
        }),
        KVM_REG_RISCV_ISA_EXT => with_vcpu_state(vcpu, |state| match kvm_reg_subtype(id) {
            KVM_REG_RISCV_ISA_SINGLE => {
                Ok(get_bitmap_bit(&state.isa_ext, kvm_reg_index(id) as usize))
            }
            KVM_REG_RISCV_ISA_MULTI_EN => {
                Ok(get_bitmap_word(&state.isa_ext, kvm_reg_index(id) as usize))
            }
            KVM_REG_RISCV_ISA_MULTI_DIS => {
                Ok(!get_bitmap_word(&state.isa_ext, kvm_reg_index(id) as usize))
            }
            _ => Err(()),
        }),
        KVM_REG_RISCV_SBI_EXT => with_vcpu_state(vcpu, |state| match kvm_reg_subtype(id) {
            KVM_REG_RISCV_SBI_SINGLE => Ok(get_u64_bit(state.sbi_ext, kvm_reg_index(id) as usize)),
            KVM_REG_RISCV_SBI_MULTI_EN => Ok(if (kvm_reg_index(id) as usize) == 0 {
                state.sbi_ext
            } else {
                0
            }),
            KVM_REG_RISCV_SBI_MULTI_DIS => Ok(if (kvm_reg_index(id) as usize) == 0 {
                !state.sbi_ext
            } else {
                0
            }),
            _ => Err(()),
        }),
        _ => Err(()),
    }
}

pub fn set_one_reg(vcpu: &VcpuRef, id: u64, value: u64) -> Result<(), ()> {
    validate_one_reg_id(id)?;

    match kvm_reg_type(id) {
        KVM_REG_RISCV_CONFIG => with_vcpu_state(vcpu, |state| match kvm_reg_offset(id) {
            KVM_REG_RISCV_CONFIG_ISA => {
                state.config.isa = value;
                Ok(())
            }
            KVM_REG_RISCV_CONFIG_ZICBOM_BLOCK_SIZE => {
                state.config.zicbom_block_size = value;
                Ok(())
            }
            KVM_REG_RISCV_CONFIG_MVENDORID => {
                state.config.mvendorid = value;
                Ok(())
            }
            KVM_REG_RISCV_CONFIG_MARCHID => {
                state.config.marchid = value;
                Ok(())
            }
            KVM_REG_RISCV_CONFIG_MIMPID => {
                state.config.mimpid = value;
                Ok(())
            }
            KVM_REG_RISCV_CONFIG_ZICBOZ_BLOCK_SIZE => {
                state.config.zicboz_block_size = value;
                Ok(())
            }
            KVM_REG_RISCV_CONFIG_SATP_MODE => {
                state.config.satp_mode = value;
                Ok(())
            }
            KVM_REG_RISCV_CONFIG_ZICBOP_BLOCK_SIZE => {
                state.config.zicbop_block_size = value;
                Ok(())
            }
            _ => Err(()),
        }),
        KVM_REG_RISCV_CORE => match kvm_reg_offset(id) {
            offset @ 0..=31 => vcpu
                .set_reg(PTRACE_REG_INDEX[offset as usize], value)
                .map_err(|_| ()),
            KVM_REG_RISCV_CORE_MODE => with_vcpu_state(vcpu, |state| {
                if value == KVM_RISCV_MODE_U || value == KVM_RISCV_MODE_S {
                    state.mode = value;
                    Ok(())
                } else {
                    Err(())
                }
            }),
            _ => Err(()),
        },
        KVM_REG_RISCV_CSR => match kvm_reg_subtype(id) {
            KVM_REG_RISCV_CSR_GENERAL => match kvm_reg_offset(id) {
                KVM_REG_RISCV_CSR_SSTATUS => vcpu.set_reg(reg::SSTATUS, value).map_err(|_| ()),
                KVM_REG_RISCV_CSR_SIE => vcpu.set_reg(reg::SIE, value).map_err(|_| ()),
                KVM_REG_RISCV_CSR_STVEC => vcpu.set_reg(reg::STVEC, value).map_err(|_| ()),
                KVM_REG_RISCV_CSR_SSCRATCH => vcpu.set_reg(reg::SSCRATCH, value).map_err(|_| ()),
                KVM_REG_RISCV_CSR_SEPC => vcpu.set_reg(reg::SEPC, value).map_err(|_| ()),
                KVM_REG_RISCV_CSR_SCAUSE => vcpu.set_reg(reg::SCAUSE, value).map_err(|_| ()),
                KVM_REG_RISCV_CSR_STVAL => vcpu.set_reg(reg::STVAL, value).map_err(|_| ()),
                KVM_REG_RISCV_CSR_SIP => vcpu.set_reg(reg::SIP, value).map_err(|_| ()),
                KVM_REG_RISCV_CSR_SATP => vcpu.set_reg(reg::SATP, value).map_err(|_| ()),
                KVM_REG_RISCV_CSR_SCOUNTEREN | KVM_REG_RISCV_CSR_SENVCFG => Ok(()),
                _ => Err(()),
            },
            _ => Err(()),
        },
        KVM_REG_RISCV_TIMER => with_vcpu_state(vcpu, |state| match kvm_reg_offset(id) {
            KVM_REG_RISCV_TIMER_FREQUENCY => {
                state.timer.frequency = value;
                Ok(())
            }
            KVM_REG_RISCV_TIMER_TIME => {
                state.timer.time = value;
                Ok(())
            }
            KVM_REG_RISCV_TIMER_COMPARE => {
                state.timer.compare = value;
                Ok(())
            }
            KVM_REG_RISCV_TIMER_STATE => {
                state.timer.state = if value == KVM_RISCV_TIMER_STATE_ON {
                    KVM_RISCV_TIMER_STATE_ON
                } else {
                    KVM_RISCV_TIMER_STATE_OFF
                };
                Ok(())
            }
            _ => Err(()),
        }),
        KVM_REG_RISCV_ISA_EXT => with_vcpu_state(vcpu, |state| match kvm_reg_subtype(id) {
            KVM_REG_RISCV_ISA_SINGLE => {
                set_bitmap_bit(&mut state.isa_ext, kvm_reg_index(id) as usize, value != 0);
                Ok(())
            }
            KVM_REG_RISCV_ISA_MULTI_EN => {
                set_bitmap_word(&mut state.isa_ext, kvm_reg_index(id) as usize, value, true);
                Ok(())
            }
            KVM_REG_RISCV_ISA_MULTI_DIS => {
                set_bitmap_word(&mut state.isa_ext, kvm_reg_index(id) as usize, value, false);
                Ok(())
            }
            _ => Err(()),
        }),
        KVM_REG_RISCV_SBI_EXT => with_vcpu_state(vcpu, |state| match kvm_reg_subtype(id) {
            KVM_REG_RISCV_SBI_SINGLE => {
                set_u64_bit(&mut state.sbi_ext, kvm_reg_index(id) as usize, value != 0);
                Ok(())
            }
            KVM_REG_RISCV_SBI_MULTI_EN => {
                if (kvm_reg_index(id) as usize) == 0 {
                    state.sbi_ext |= value;
                    Ok(())
                } else {
                    Err(())
                }
            }
            KVM_REG_RISCV_SBI_MULTI_DIS => {
                if (kvm_reg_index(id) as usize) == 0 {
                    state.sbi_ext &= !value;
                    Ok(())
                } else {
                    Err(())
                }
            }
            _ => Err(()),
        }),
        _ => Err(()),
    }
}

pub fn read_regs_to_kvm(vcpu: &VcpuRef) -> KvmRegs {
    let mut buf = [0u64; 32];
    for (slot, &idx) in PTRACE_REG_INDEX.iter().enumerate() {
        buf[slot] = vcpu.get_reg(idx).unwrap_or(0);
    }
    unsafe { core::mem::transmute(buf) }
}

pub fn write_kvm_to_regs(vcpu: &VcpuRef, kvm_regs: &KvmRegs) {
    let buf: &[u64; 32] = unsafe { core::mem::transmute(kvm_regs) };
    for (&idx, &val) in PTRACE_REG_INDEX.iter().zip(buf.iter()) {
        let _ = vcpu.set_reg(idx, val);
    }
}

pub fn complete_mmio_read(vcpu: &VcpuRef, target_reg: u8, size: u8, value: u64) {
    if target_reg == 0 {
        return;
    }

    let mask = match size {
        1 => 0xff,
        2 => 0xffff,
        4 => 0xffff_ffff,
        _ => !0,
    };
    let _ = vcpu.set_reg(target_reg as u32, value & mask);
}

// ---------------------------------------------------------------------------
// Arch hook types (matching aarch64.rs interface)
// ---------------------------------------------------------------------------

pub enum FirmwareCallResult {
    Handled,
    SystemOff,
    SystemReset,
    ForwardToUserspace,
}

pub fn handle_firmware_call_in_kernel(vcpu: &VcpuRef) -> FirmwareCallResult {
    use crate::arch::hv::reg_index::reg;
    handle_sbi_in_kernel_impl(vcpu)
}

fn handle_sbi_in_kernel_impl(vcpu: &VcpuRef) -> FirmwareCallResult {
    use crate::arch::hv::reg_index::reg;

    let extension_id = vcpu.get_reg(reg::A7).unwrap_or(0);
    let function_id = vcpu.get_reg(reg::A6).unwrap_or(0);
    let arg0 = vcpu.get_reg(reg::A0).unwrap_or(0);

    match extension_id {
        sbi_ext::BASE => {
            let ret1 = match function_id {
                0 => (sbi_err::SUCCESS, 2),
                1 => (sbi_err::SUCCESS, 1),
                2 => (sbi_err::SUCCESS, 0),
                3 => {
                    let queried = vcpu.get_reg(reg::A0).unwrap_or(0);
                    let available = matches!(
                        queried,
                        sbi_ext::BASE
                            | sbi_ext::_PUTCHAR
                            | sbi_ext::SRST
                            | sbi_ext::DBCN
                            | sbi_ext::SUSP
                            | sbi_ext::TIME
                            | sbi_ext::IPI
                            | sbi_ext::RFNC
                            | sbi_ext::HSM
                    );
                    (sbi_err::SUCCESS, if available { 1 } else { 0 })
                }
                4 => (sbi_err::SUCCESS, 0),
                5 => (sbi_err::SUCCESS, 0),
                6 => (sbi_err::SUCCESS, 0),
                _ => (sbi_err::NOT_SUPPORTED, 0),
            };
            let _ = vcpu.set_reg(reg::A0, ret1.0);
            let _ = vcpu.set_reg(reg::A1, ret1.1);
            FirmwareCallResult::Handled
        }
        sbi_ext::TIME => match function_id {
            0 => {
                crate::arch::hv::trap::set_sbi_timer_next_event(arg0);
                let _ = vcpu.set_reg(reg::A0, sbi_err::SUCCESS);
                FirmwareCallResult::Handled
            }
            _ => {
                let _ = vcpu.set_reg(reg::A0, sbi_err::NOT_SUPPORTED);
                let _ = vcpu.set_reg(reg::A1, 0);
                FirmwareCallResult::Handled
            }
        },
        sbi_ext::IPI => match function_id {
            0 => {
                let _ = vcpu.set_reg(reg::A0, sbi_err::SUCCESS);
                FirmwareCallResult::Handled
            }
            _ => {
                let _ = vcpu.set_reg(reg::A0, sbi_err::NOT_SUPPORTED);
                let _ = vcpu.set_reg(reg::A1, 0);
                FirmwareCallResult::Handled
            }
        },
        sbi_ext::RFNC => match function_id {
            0..=6 => {
                let _ = vcpu.set_reg(reg::A0, sbi_err::SUCCESS);
                FirmwareCallResult::Handled
            }
            _ => {
                let _ = vcpu.set_reg(reg::A0, sbi_err::NOT_SUPPORTED);
                let _ = vcpu.set_reg(reg::A1, 0);
                FirmwareCallResult::Handled
            }
        },
        sbi_ext::HSM => {
            let arg1 = vcpu.get_reg(reg::A1).unwrap_or(0);
            let ret = match function_id {
                0 => {
                    if arg0 == 0 {
                        (sbi_err::ALREADY_STARTED, 0)
                    } else {
                        (sbi_err::INVALID_PARAM, 0)
                    }
                }
                1 => (sbi_err::ERR_DENIED, 0),
                2 => {
                    if arg0 == 0 {
                        (sbi_err::SUCCESS, hsm_state::STARTED)
                    } else {
                        (sbi_err::INVALID_PARAM, 0)
                    }
                }
                3 => (sbi_err::NOT_SUPPORTED, 0),
                _ => (sbi_err::NOT_SUPPORTED, 0),
            };
            let _ = vcpu.set_reg(reg::A0, ret.0);
            let _ = vcpu.set_reg(reg::A1, ret.1);
            FirmwareCallResult::Handled
        }
        sbi_ext::_PUTCHAR | sbi_ext::DBCN | sbi_ext::SUSP => FirmwareCallResult::ForwardToUserspace,
        sbi_ext::SRST => FirmwareCallResult::ForwardToUserspace,
        _ => {
            let _ = vcpu.set_reg(reg::A0, sbi_err::NOT_SUPPORTED);
            let _ = vcpu.set_reg(reg::A1, 0);
            FirmwareCallResult::Handled
        }
    }
}

pub fn write_firmware_exit(
    kvm_run: &mut super::KvmRun,
    _exit: &crate::hypervisor::types::VmExit,
    vcpu: &VcpuRef,
) {
    use super::{KVM_EXIT_RISCV_SBI, KVM_EXIT_SHUTDOWN};
    use crate::arch::hv::reg_index::reg;

    let extension_id = vcpu.get_reg(reg::A7).unwrap_or(0);
    if extension_id == sbi_ext::SRST {
        kvm_run.exit_reason = KVM_EXIT_SHUTDOWN;
    } else {
        kvm_run.exit_reason = KVM_EXIT_RISCV_SBI;
        let sbi = unsafe { &mut kvm_run.exit_data.riscv_sbi };
        sbi.args = [
            vcpu.get_reg(reg::A0).unwrap_or(0),
            vcpu.get_reg(reg::A1).unwrap_or(0),
            vcpu.get_reg(reg::A2).unwrap_or(0),
            vcpu.get_reg(reg::A3).unwrap_or(0),
            vcpu.get_reg(reg::A4).unwrap_or(0),
            vcpu.get_reg(reg::A5).unwrap_or(0),
        ];
        sbi.ret = [0u64; 2];
        sbi.extension_id = extension_id;
        sbi.function_id = vcpu.get_reg(reg::A6).unwrap_or(0);
    }
}

pub fn check_extension(_cap: usize) -> Option<usize> {
    None
}

pub fn validate_device_type(_device_type: u32) -> Result<(), ()> {
    Err(())
}

pub fn set_device_attr(_vm: &VmRef, _attr: &super::KvmDeviceAttr) -> Result<Option<usize>, ()> {
    Err(())
}

pub fn get_device_attr(_vm: &VmRef, _attr: &super::KvmDeviceAttr) -> Result<Option<usize>, ()> {
    Err(())
}

pub fn has_device_attr(_vm: &VmRef, _attr: &super::KvmDeviceAttr) -> Result<Option<usize>, ()> {
    Err(())
}

mod sbi_ext {
    pub const BASE: u64 = 0x10;
    pub const _PUTCHAR: u64 = 0x01;
    pub const SRST: u64 = 0x5352_5354;
    pub const DBCN: u64 = 0x4442_434E;
    pub const SUSP: u64 = 0x5355_5350;
    pub const TIME: u64 = 0x5449_4D45;
    pub const IPI: u64 = 0x0073_5049;
    pub const RFNC: u64 = 0x5246_4E43;
    pub const HSM: u64 = 0x0048_534D;
}

mod sbi_err {
    pub const SUCCESS: u64 = 0;
    pub const ERR_DENIED: u64 = u64::MAX - 2;
    pub const NOT_SUPPORTED: u64 = u64::MAX - 1;
    pub const INVALID_PARAM: u64 = u64::MAX - 3;
    pub const ALREADY_STARTED: u64 = u64::MAX - 4;
    pub const ALREADY_STOPPED: u64 = u64::MAX - 5;
}

mod hsm_state {
    pub const STARTED: u64 = 0;
    pub const STOPPED: u64 = 1;
    pub const START_PENDING: u64 = 2;
    pub const STOP_PENDING: u64 = 3;
    pub const SUSPENDED: u64 = 4;
}

// ---------------------------------------------------------------------------
// Arch hook: VM-level ioctl dispatch (RISC-V stub)
// ---------------------------------------------------------------------------

pub fn handle_vm_ioctl(
    _request: u32,
    _arg: usize,
    _vm: &VmRef,
    _abi: &mut LinuxAbi,
) -> Result<Option<usize>, ()> {
    Ok(None)
}

// ---------------------------------------------------------------------------
// Arch hook: vCPU-level ioctl dispatch (RISC-V stub)
// ---------------------------------------------------------------------------

pub fn handle_vcpu_ioctl(_request: u32, _arg: usize, _vcpu: &VcpuRef) -> Result<Option<usize>, ()> {
    Ok(None)
}
