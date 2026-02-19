//! Shared hypervisor types for User/Kernel interface
//!
//! This module defines the data structures used for communication between
//! the kernel hypervisor and userspace VMM (Violet).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptType {
    Timer,
    External,
}

#[derive(Debug, Clone, Copy)]
pub enum VmExit {
    MmioRead {
        epc: u64,
        addr: u64,
        size: u8,
        reg: u8,
    },
    MmioWrite {
        epc: u64,
        addr: u64,
        size: u8,
        reg: u8,
        data: u64,
    },
    FirmwareCall {
        epc: u64,
    },
    Hlt,
    Shutdown,
    FailEntry {
        hardware_entry_failure_reason: u64,
    },
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
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MmioInfo {
    pub address: u64,
    pub data: u64,
    pub size: u8,
    pub reg: u8,
    pub is_write: bool,
    pub _padding: [u8; 5],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VcpuExit {
    pub reason: VcpuExitReason,
    pub epc: u64,
    pub mmio: MmioInfo,
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
            } => Self::mmio_read(*epc, *addr, *size, *reg),
            VmExit::MmioWrite {
                epc,
                addr,
                size,
                reg,
                data,
            } => Self::mmio_write(*epc, *addr, *size, *reg, *data),
            VmExit::FirmwareCall { epc } => Self {
                reason: VcpuExitReason::FirmwareCall,
                epc: *epc,
                ..Default::default()
            },
            VmExit::Hlt => Self::new(VcpuExitReason::Hlt),
            VmExit::Shutdown => Self::new(VcpuExitReason::Shutdown),
            VmExit::FailEntry {
                hardware_entry_failure_reason,
            } => Self {
                reason: VcpuExitReason::FailEntry,
                fail_code: *hardware_entry_failure_reason,
                ..Default::default()
            },
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

    pub fn mmio_read(epc: u64, address: u64, size: u8, reg: u8) -> Self {
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

    pub fn mmio_write(epc: u64, address: u64, size: u8, reg: u8, data: u64) -> Self {
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
