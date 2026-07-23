//! Shared hypervisor types for User/Kernel interface
//!
//! This module defines the data structures used for communication between
//! the kernel hypervisor and userspace VMM (Violet).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptType {
    Software,
    Timer,
    External,
}

#[derive(Debug, Clone, Copy)]
pub enum VmExit {
    MmioRead {
        epc: u64,
        addr: u64,
        size: u8,
        reg: usize,
    },
    MmioWrite {
        epc: u64,
        addr: u64,
        size: u8,
        reg: usize,
        data: u64,
    },
    FirmwareCall {
        epc: u64,
    },
    VirtualInstruction {
        epc: u64,
        inst: Option<u32>, // If the architecture provides a way to get the trapped instruction, it can be included here. Otherwise, it can be None.
        inst_len: Option<u8>, // Length of the instruction in bytes, if available. This can help the VMM determine how much to advance the guest's program counter.
    },
    IllegalInstruction {
        epc: u64,
        inst: Option<u32>, // Similar to VirtualInstruction, this can be None if the architecture doesn't provide the instruction.
        inst_len: Option<u8>, // Length of the instruction in bytes, if available.
    },
    Breakpoint {
        epc: u64,
    },
    Wfi,
    Hlt,
    Shutdown,
    FailEntry {
        hardware_entry_failure_reason: u64,
    },
    HostInterrupt,
    InternalError,
    Unknown(u64),
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VcpuExitReason {
    #[default]
    Unknown = 0,
    Io = 1,
    MmioRead = 2,
    MmioWrite = 3,
    Hlt = 4,
    Shutdown = 5,
    FailEntry = 6,
    InternalError = 7,
    FirmwareCall = 8,
    VirtualInstruction = 9,
    IllegalInstruction = 10,
    Breakpoint = 11,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MmioInfo {
    pub address: u64,
    pub data: u64,
    pub reg: u32,
    pub size: u8,
    pub is_write: bool,
    pub _padding: [u8; 2],
}

const _: () = {
    assert!(core::mem::size_of::<MmioInfo>() == 24);
    assert!(core::mem::align_of::<MmioInfo>() == 8);
    assert!(core::mem::offset_of!(MmioInfo, address) == 0);
    assert!(core::mem::offset_of!(MmioInfo, data) == 8);
    assert!(core::mem::offset_of!(MmioInfo, reg) == 16);
    assert!(core::mem::offset_of!(MmioInfo, size) == 20);
    assert!(core::mem::offset_of!(MmioInfo, is_write) == 21);
};

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct InstructionInfo {
    pub inst: u32,
    pub inst_len: u8,
    pub has_inst: bool,
    pub _padding: [u8; 6],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VcpuExit {
    pub reason: VcpuExitReason,
    pub epc: u64,
    pub mmio: MmioInfo,
    pub inst: InstructionInfo,
    pub fail_code: u64,
}

impl VcpuExit {
    pub fn from_vmexit(exit: &VmExit) -> Self {
        match exit {
            VmExit::MmioRead {
                epc,
                addr,
                size,
                reg,
            } => Self::mmio_read(*epc, *addr, *size, abi_register_index(*reg)),
            VmExit::MmioWrite {
                epc,
                addr,
                size,
                reg,
                data,
            } => Self::mmio_write(*epc, *addr, *size, abi_register_index(*reg), *data),
            VmExit::FirmwareCall { epc } => Self {
                reason: VcpuExitReason::FirmwareCall,
                epc: *epc,
                ..Default::default()
            },
            VmExit::VirtualInstruction {
                epc,
                inst,
                inst_len,
            } => Self {
                reason: VcpuExitReason::VirtualInstruction,
                epc: *epc,
                inst: InstructionInfo {
                    inst: inst.unwrap_or(0),
                    inst_len: inst_len.unwrap_or(0),
                    has_inst: inst.is_some(),
                    ..Default::default()
                },
                ..Default::default()
            },
            VmExit::IllegalInstruction {
                epc,
                inst,
                inst_len,
            } => Self {
                reason: VcpuExitReason::IllegalInstruction,
                epc: *epc,
                inst: InstructionInfo {
                    inst: inst.unwrap_or(0),
                    inst_len: inst_len.unwrap_or(0),
                    has_inst: inst.is_some(),
                    ..Default::default()
                },
                ..Default::default()
            },
            VmExit::Breakpoint { epc } => Self {
                reason: VcpuExitReason::Breakpoint,
                epc: *epc,
                ..Default::default()
            },
            VmExit::Wfi | VmExit::Hlt => Self::new(VcpuExitReason::Hlt),
            VmExit::Shutdown => Self::new(VcpuExitReason::Shutdown),
            VmExit::FailEntry {
                hardware_entry_failure_reason,
            } => Self {
                reason: VcpuExitReason::FailEntry,
                fail_code: *hardware_entry_failure_reason,
                ..Default::default()
            },
            VmExit::HostInterrupt => Self::new(VcpuExitReason::Unknown),
            VmExit::InternalError => Self::new(VcpuExitReason::InternalError),
            VmExit::Unknown(code) => Self {
                reason: VcpuExitReason::Unknown,
                fail_code: *code,
                ..Default::default()
            },
        }
    }

    pub fn new(reason: VcpuExitReason) -> Self {
        Self {
            reason,
            ..Default::default()
        }
    }

    pub fn mmio_read(epc: u64, address: u64, size: u8, reg: u32) -> Self {
        Self {
            reason: VcpuExitReason::MmioRead,
            epc,
            mmio: MmioInfo {
                address,
                size,
                reg,
                is_write: false,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub fn mmio_write(epc: u64, address: u64, size: u8, reg: u32, data: u64) -> Self {
        Self {
            reason: VcpuExitReason::MmioWrite,
            epc,
            mmio: MmioInfo {
                address,
                data,
                size,
                reg,
                is_write: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

fn abi_register_index(index: usize) -> u32 {
    u32::try_from(index).expect("internal register index must fit the SHV ABI")
}
