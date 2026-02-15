//! Shared hypervisor types for User/Kernel interface

#[derive(Debug, Clone)]
pub enum VmExit {
    Hlt,
    Shutdown,
    FirmwareCall,
    SystemEvent { event_type: u64 },
    FailEntry { hardware_entry_failure_reason: u64 },
    InstPageFault { gpa: u64 },
    LoadPageFault { gpa: u64, size: u8 },
    StorePageFault { gpa: u64, size: u8, data: u64 },
    IllegalInstruction { gpa: u64, instruction: Option<u32> },
    MmioRead { addr: u64, size: u8 },
    MmioWrite { addr: u64, size: u8, data: u64 },
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
    pub is_write: bool,
    pub _padding: [u8; 6],
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
            VmExit::MmioRead { addr, size } => Self::mmio_read(*addr, *size),
            VmExit::MmioWrite { addr, size, data } => Self::mmio_write(*addr, *size, *data),
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
            _ => Self::new(VcpuExitReason::Unknown),
        }
    }

    pub fn new(reason: VcpuExitReason) -> Self {
        Self {
            reason,
            ..Default::default()
        }
    }

    pub fn mmio_read(address: u64, size: u8) -> Self {
        Self {
            reason: VcpuExitReason::MmioRead,
            mmio: MmioInfo {
                address,
                size,
                is_write: false,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub fn mmio_write(address: u64, size: u8, data: u64) -> Self {
        Self {
            reason: VcpuExitReason::MmioWrite,
            mmio: MmioInfo {
                address,
                data,
                size,
                is_write: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub fn needs_userspace(&self) -> bool {
        !matches!(self.reason, VcpuExitReason::Io)
    }
}
