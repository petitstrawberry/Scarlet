//! RISC-V H-extension hypervisor support
//!
//! Hardware-assisted virtualization using the RISC-V Hypervisor extension.
//! The kernel runs in HS-mode and manages guests in VS/VU-mode.

pub mod csr;
pub mod gstage;
pub mod switch;
pub mod vmexit;

use core::sync::atomic::{AtomicU16, Ordering};

use crate::environment::PAGE_SIZE;
use crate::hypervisor::exit::VmExit;
use crate::hypervisor::memory::MemorySlotFlags;

use gstage::GStagePageTable;
use switch::GuestState;

static NEXT_VMID: AtomicU16 = AtomicU16::new(1);

fn alloc_vmid() -> u16 {
    NEXT_VMID.fetch_add(1, Ordering::Relaxed)
}

pub struct ArchVm {
    gstage: GStagePageTable,
    vmid: u16,
}

impl ArchVm {
    pub fn new() -> Result<Self, &'static str> {
        let gstage = GStagePageTable::new()?;
        let vmid = alloc_vmid();
        Ok(Self { gstage, vmid })
    }

    pub fn map_memory(
        &mut self,
        guest_phys_addr: u64,
        host_phys_addr: u64,
        size: u64,
        flags: MemorySlotFlags,
    ) -> Result<(), &'static str> {
        let page_size = PAGE_SIZE as u64;
        let num_pages = (size + page_size - 1) / page_size;
        for i in 0..num_pages {
            let gpa = guest_phys_addr + i * page_size;
            let hpa = host_phys_addr + i * page_size;
            self.gstage.map_page(gpa, hpa, flags.readonly)?;
        }
        self.gstage.flush_tlb();
        Ok(())
    }

    pub fn unmap_memory(&mut self, guest_phys_addr: u64, size: u64) -> Result<(), &'static str> {
        let page_size = PAGE_SIZE as u64;
        let num_pages = (size + page_size - 1) / page_size;
        for i in 0..num_pages {
            let gpa = guest_phys_addr + i * page_size;
            self.gstage.unmap_page(gpa)?;
        }
        self.gstage.flush_tlb();
        Ok(())
    }

    pub fn hgatp_value(&self) -> u64 {
        self.gstage.hgatp_value(self.vmid)
    }
}

#[derive(Debug, Clone)]
pub struct GuestRegisters {
    pub regs: [u64; 32],
}

impl GuestRegisters {
    pub fn new() -> Self {
        Self { regs: [0; 32] }
    }

    /// Get a general-purpose register by index (0..31)
    pub fn get_gpr(&self, index: usize) -> u64 {
        self.regs[index]
    }

    /// Set a general-purpose register by index (0..31)
    ///
    /// Setting x0 is silently ignored (hardwired zero).
    pub fn set_gpr(&mut self, index: usize, value: u64) {
        if index != 0 {
            self.regs[index] = value;
        }
    }
}

impl Default for GuestRegisters {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ArchVcpu {
    guest_state: GuestState,
    hgatp: u64,
}

impl ArchVcpu {
    pub fn new(vm: &ArchVm) -> Result<Self, &'static str> {
        Ok(Self {
            guest_state: GuestState::new(),
            hgatp: vm.hgatp_value(),
        })
    }

    pub fn run(&mut self) -> Result<VmExit, &'static str> {
        let exit_info = switch::guest_enter(&mut self.guest_state, self.hgatp);
        Ok(exit_info.decode())
    }

    pub fn get_regs(&self) -> GuestRegisters {
        let mut regs = GuestRegisters::new();
        regs.regs.copy_from_slice(&self.guest_state.gprs);
        regs
    }

    pub fn set_regs(&mut self, regs: &GuestRegisters) {
        self.guest_state.gprs.copy_from_slice(&regs.regs);
    }

    pub fn get_pc(&self) -> u64 {
        self.guest_state.pc
    }

    pub fn set_pc(&mut self, pc: u64) {
        self.guest_state.pc = pc;
    }

    /// Get a general-purpose register by index (0..31)
    pub fn get_gpr(&self, index: usize) -> u64 {
        self.guest_state.gprs[index]
    }

    /// Set a general-purpose register by index (0..31)
    ///
    /// Setting x0 is silently ignored (hardwired zero).
    pub fn set_gpr(&mut self, index: usize, value: u64) {
        if index != 0 {
            self.guest_state.gprs[index] = value;
        }
    }
}
