//! VM-exit reason decoding for RISC-V H-extension
//!
//! When a guest running in VS/VU-mode causes a trap back to HS-mode,
//! we inspect the scause, htval, htinst, and stval CSRs to determine
//! the reason the guest exited.

use crate::hypervisor::exit::VmExit;

use super::csr;

/// Raw information captured at VM-exit time
#[derive(Debug, Clone)]
pub struct VmExitInfo {
    /// Supervisor cause register (trap cause)
    pub scause: u64,
    /// Supervisor trap value
    pub stval: u64,
    /// Hypervisor trap value (GPA bits [63:2])
    pub htval: u64,
    /// Hypervisor trap instruction (faulting instruction or zero)
    pub htinst: u64,
    /// Guest program counter at time of exit
    pub guest_pc: u64,
}

impl VmExitInfo {
    /// Capture VM-exit information from CSRs.
    ///
    /// Must be called immediately after the guest trap, before any
    /// CSR state is clobbered.
    pub fn capture(guest_pc: u64) -> Self {
        Self {
            scause: csr::read_scause(),
            stval: csr::read_stval(),
            htval: csr::read_htval(),
            htinst: csr::read_htinst(),
            guest_pc,
        }
    }

    /// Decode the raw exit info into a high-level VmExit reason.
    pub fn decode(&self) -> VmExit {
        let is_interrupt = (self.scause >> 63) != 0;
        let cause_code = self.scause & 0x7fff_ffff_ffff_ffff;

        if is_interrupt {
            // Guest was interrupted by a host interrupt.
            // The hypervisor should handle the interrupt normally and
            // re-enter the guest.
            return VmExit::Unknown(self.scause);
        }

        match cause_code {
            // Ecall from VS-mode
            csr::CAUSE_ECALL_FROM_VS => VmExit::FirmwareCall,
            // Instruction guest-page fault
            csr::CAUSE_GUEST_INST_PAGE_FAULT => {
                let fault_gpa = self.fault_gpa();
                VmExit::InstPageFault { gpa: fault_gpa }
            }
            // Load guest-page fault
            csr::CAUSE_GUEST_LOAD_PAGE_FAULT => {
                let fault_gpa = self.fault_gpa();
                let size = self.access_size();
                VmExit::LoadPageFault {
                    gpa: fault_gpa,
                    size,
                }
            }
            // Virtual instruction exception
            csr::CAUSE_VIRTUAL_INSTRUCTION => VmExit::Unknown(self.scause),
            // Store/AMO guest-page fault
            csr::CAUSE_GUEST_STORE_PAGE_FAULT => {
                let fault_gpa = self.fault_gpa();
                let size = self.access_size();
                VmExit::StorePageFault {
                    gpa: fault_gpa,
                    size,
                    data: 0, // actual data must be decoded from the faulting instruction
                }
            }
            _ => VmExit::Unknown(self.scause),
        }
    }

    /// Reconstruct the full faulting Guest Physical Address.
    ///
    /// htval contains GPA >> 2. The low 2 bits come from stval.
    fn fault_gpa(&self) -> u64 {
        (self.htval << 2) | (self.stval & 0x3)
    }

    /// Try to determine the access size from htinst.
    ///
    /// If htinst is zero, the access size is unknown and we return 0.
    fn access_size(&self) -> u8 {
        if self.htinst == 0 {
            return 0;
        }
        // htinst holds a "transformed instruction" per the spec.
        // Bits [14:12] encode funct3, which gives the access width.
        let funct3 = (self.htinst >> 12) & 0x7;
        match funct3 {
            0 => 1, // LB/SB
            1 => 2, // LH/SH
            2 => 4, // LW/SW
            3 => 8, // LD/SD
            _ => 0,
        }
    }
}
