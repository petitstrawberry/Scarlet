//! Shared types for Type-2 hypervisor (userspace)
//!
//! Mirror of kernel types in `kernel/src/hypervisor/types.rs`.

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
    pub size: u8,
    pub reg: u8,
    pub is_write: bool,
    pub _padding: [u8; 5],
}

impl MmioInfo {
    pub fn is_write(&self) -> bool {
        self.is_write
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct InstructionInfo {
    pub inst: u32,
    pub inst_len: u8,
    pub has_inst: bool,
    pub _padding: [u8; 6],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VcpuExit {
    pub reason: VcpuExitReason,
    pub epc: u64,
    pub mmio: MmioInfo,
    pub inst: InstructionInfo,
    pub fail_code: u64,
}

impl Default for VcpuExit {
    fn default() -> Self {
        Self {
            reason: VcpuExitReason::Unknown,
            epc: 0,
            mmio: MmioInfo::default(),
            inst: InstructionInfo::default(),
            fail_code: 0,
        }
    }
}
