/// Offsets within the xHCI capability register block.
pub mod capability {
    pub const CAPLENGTH: usize = 0x00;
    pub const HCIVERSION: usize = 0x02;
    pub const HCSPARAMS1: usize = 0x04;
    pub const HCSPARAMS2: usize = 0x08;
    pub const HCSPARAMS3: usize = 0x0C;
    pub const HCCPARAMS1: usize = 0x10;
    pub const DBOFF: usize = 0x14;
    pub const RTSOFF: usize = 0x18;
}

/// Offsets within the xHCI operational register block.
pub mod operational {
    pub const USBCMD: usize = 0x00;
    pub const USBSTS: usize = 0x04;
    pub const PAGESIZE: usize = 0x08;
    pub const DNCTRL: usize = 0x14;
    pub const CRCR: usize = 0x18;
    pub const DCBAAP: usize = 0x30;
    pub const CONFIG: usize = 0x38;
    pub const PORTSC_BASE: usize = 0x400;
    pub const PORT_REGISTER_STRIDE: usize = 0x10;
}

/// Offsets within the first runtime interrupter register set.
pub mod runtime {
    pub const MFINDEX: usize = 0x00;
    pub const IR0_IMAN: usize = 0x20;
    pub const IR0_IMOD: usize = 0x24;
    pub const IR0_ERSTSZ: usize = 0x28;
    pub const IR0_ERSTBA: usize = 0x30;
    pub const IR0_ERDP: usize = 0x38;
}

/// Calculated split of the xHCI MMIO register regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisterSpace {
    pub mmio_base: usize,
    pub operational_base: usize,
    pub runtime_base: usize,
    pub doorbell_base: usize,
}

impl RegisterSpace {
    /// Creates a register layout from xHCI capability values.
    pub const fn new(
        mmio_base: usize,
        caplength: u8,
        runtime_offset: u32,
        doorbell_offset: u32,
    ) -> Self {
        Self {
            mmio_base,
            operational_base: mmio_base + caplength as usize,
            runtime_base: mmio_base + runtime_offset as usize,
            doorbell_base: mmio_base + doorbell_offset as usize,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_register_space_layout() {
        let regs = RegisterSpace::new(0x1000_0000, 0x40, 0x2000, 0x3000);
        assert_eq!(regs.operational_base, 0x1000_0040);
        assert_eq!(regs.runtime_base, 0x1000_2000);
        assert_eq!(regs.doorbell_base, 0x1000_3000);
    }
}
