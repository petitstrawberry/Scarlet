//! RISC-V H-extension trap information

use crate::hypervisor::trap::{AccessType, TrapType, VmTrapInfo};

use super::csr;

pub const CAUSE_ECALL_FROM_VS: u64 = 10;
pub const CAUSE_GUEST_INST_PAGE_FAULT: u64 = 20;
pub const CAUSE_GUEST_LOAD_PAGE_FAULT: u64 = 21;
pub const CAUSE_VIRTUAL_INSTRUCTION: u64 = 22;
pub const CAUSE_GUEST_STORE_PAGE_FAULT: u64 = 23;

/// RISC-V specific trap information captured from hardware CSRs
#[derive(Debug, Clone)]
pub struct RiscvTrapInfo {
    pub scause: u64,
    pub stval: u64,
    pub htval: u64,
    pub htinst: u64,
}

impl VmTrapInfo for RiscvTrapInfo {
    fn capture() -> Self {
        Self {
            scause: csr::read_scause(),
            stval: csr::read_stval(),
            htval: csr::read_htval(),
            htinst: csr::read_htinst(),
        }
    }

    fn trap_type(&self) -> TrapType {
        if self.is_interrupt() {
            return match self.cause_code() {
                5 => TrapType::TimerInterrupt,
                9 | 11 | 13 => TrapType::ExternalInterrupt,
                _ => TrapType::Unknown,
            };
        }

        match self.cause_code() {
            CAUSE_GUEST_INST_PAGE_FAULT
            | CAUSE_GUEST_LOAD_PAGE_FAULT
            | CAUSE_GUEST_STORE_PAGE_FAULT => TrapType::PageFault,
            CAUSE_ECALL_FROM_VS => TrapType::FirmwareCall,
            CAUSE_VIRTUAL_INSTRUCTION => TrapType::Halt,
            _ => TrapType::Unknown,
        }
    }

    fn gpa(&self) -> u64 {
        (self.htval << 2) | (self.stval & 0x3)
    }

    fn access_type(&self) -> AccessType {
        match self.cause_code() {
            CAUSE_GUEST_INST_PAGE_FAULT => AccessType::Execute,
            CAUSE_GUEST_STORE_PAGE_FAULT => AccessType::Write,
            _ => AccessType::Read,
        }
    }

    fn access_size(&self) -> u8 {
        if self.htinst == 0 {
            return 4;
        }
        let funct3 = (self.htinst >> 12) & 0x7;
        match funct3 {
            0 => 1,
            1 => 2,
            2 => 4,
            3 => 8,
            _ => 4,
        }
    }

    fn raw_cause(&self) -> u64 {
        self.scause
    }

    fn is_interrupt(&self) -> bool {
        (self.scause >> 63) != 0
    }
}

impl RiscvTrapInfo {
    fn cause_code(&self) -> u64 {
        self.scause & 0x7fff_ffff_ffff_ffff
    }
}
