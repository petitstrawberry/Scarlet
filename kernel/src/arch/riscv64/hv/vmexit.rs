//! VM-exit reason decoding for RISC-V H-extension

use crate::hypervisor::types::VcpuExitReason;

use super::csr;

/// Exit cause codes
pub const CAUSE_ECALL_FROM_VS: u64 = 10;
pub const CAUSE_GUEST_INST_PAGE_FAULT: u64 = 20;
pub const CAUSE_GUEST_LOAD_PAGE_FAULT: u64 = 21;
pub const CAUSE_VIRTUAL_INSTRUCTION: u64 = 22;
pub const CAUSE_GUEST_STORE_PAGE_FAULT: u64 = 23;

#[derive(Debug, Clone)]
pub struct VmExitInfo {
    pub scause: u64,
    pub stval: u64,
    pub htval: u64,
    pub htinst: u64,
    pub guest_pc: u64,
}

impl VmExitInfo {
    pub fn capture(guest_pc: u64) -> Self {
        Self {
            scause: csr::read_scause(),
            stval: csr::read_stval(),
            htval: csr::read_htval(),
            htinst: csr::read_htinst(),
            guest_pc,
        }
    }

    pub fn decode(&self) -> VcpuExitReason {
        let is_interrupt = (self.scause >> 63) != 0;
        let cause_code = self.scause & 0x7fff_ffff_ffff_ffff;

        if is_interrupt {
            return VcpuExitReason::Io;
        }

        match cause_code {
            CAUSE_GUEST_INST_PAGE_FAULT
            | CAUSE_GUEST_LOAD_PAGE_FAULT
            | CAUSE_GUEST_STORE_PAGE_FAULT => {
                if self.is_mmio() {
                    if cause_code == CAUSE_GUEST_STORE_PAGE_FAULT {
                        VcpuExitReason::MmioWrite
                    } else {
                        VcpuExitReason::MmioRead
                    }
                } else {
                    VcpuExitReason::Unknown
                }
            }
            _ => VcpuExitReason::Unknown,
        }
    }

    fn is_mmio(&self) -> bool {
        self.fault_gpa() >= 0x1000_0000
    }

    pub fn fault_gpa(&self) -> u64 {
        (self.htval << 2) | (self.stval & 0x3)
    }

    pub fn access_size(&self) -> u8 {
        if self.htinst == 0 {
            return 0;
        }
        let funct3 = (self.htinst >> 12) & 0x7;
        match funct3 {
            0 => 1,
            1 => 2,
            2 => 4,
            3 => 8,
            _ => 0,
        }
    }
}
