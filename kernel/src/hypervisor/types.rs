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
    MmioRead { addr: u64, size: u8, reg: u8 },
    MmioWrite { addr: u64, size: u8, reg: u8 },
    Hlt,
    Shutdown,
    FailEntry { hardware_entry_failure_reason: u64 },
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
    pub _padding: u32,
    pub mmio: MmioInfo,
    pub fail_code: u64,
}

impl VcpuExit {
    pub fn from_vmexit(exit: &VmExit) -> Self {
        match exit {
            VmExit::MmioRead { addr, size, reg } => Self::mmio_read(*addr, *size, *reg),
            VmExit::MmioWrite { addr, size, reg } => Self::mmio_write(*addr, *size, *reg),
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

    pub fn mmio_read(address: u64, size: u8, reg: u8) -> Self {
        Self {
            reason: VcpuExitReason::MmioRead,
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

    pub fn mmio_write(address: u64, size: u8, reg: u8) -> Self {
        Self {
            reason: VcpuExitReason::MmioWrite,
            mmio: MmioInfo {
                address,
                size,
                reg,
                is_write: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }
}
