use crate::hypervisor::mmio::VirtualMmioDevice;

const UART_DR: u64 = 0x00;
const UART_FR: u64 = 0x18;
const UART_FR_RXFE: u64 = 1 << 4;
const UART_FR_TXFE: u64 = 1 << 7;

pub struct Pl011Mmio {
    base: u64,
    size: u64,
}

impl Pl011Mmio {
    pub fn new(base: u64) -> Self {
        Self { base, size: 0x1000 }
    }
}

impl VirtualMmioDevice for Pl011Mmio {
    fn read(&self, offset: u64, _size: u8) -> u64 {
        match offset {
            UART_FR => UART_FR_RXFE | UART_FR_TXFE,
            _ => 0,
        }
    }

    fn write(&self, offset: u64, _size: u8, value: u64) {
        if offset == UART_DR {
            crate::print!("{}", (value as u8) as char);
        }
    }

    fn addr_range(&self) -> (u64, u64) {
        (self.base, self.size)
    }
}
