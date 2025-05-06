// UART driver for QEMU virt machine

use core::{fmt, ptr::{read_volatile, write_volatile}};
use core::fmt::Write;
use alloc::boxed::Box;

use crate::{early_initcall, traits::serial::Serial};


#[derive(Clone)]
pub struct Uart {
    base: usize,
}

pub const RHR_OFFSET: usize = 0x00;
pub const THR_OFFSET: usize = 0x00;
pub const LSR_OFFSET: usize = 0x05;

pub const LSR_THRE: u8 = 0x20;
pub const LSR_DR: u8 = 0x01;

impl Uart {
    pub fn new(base: usize) -> Self {
        Uart { base }
    }

    pub fn init(&self) {
    }

    fn reg_write(&self, offset: usize, value: u8) {
        let addr = self.base + offset;
        unsafe { write_volatile(addr as *mut u8, value) }
    }

    fn reg_read(&self, offset: usize) -> u8 {
        let addr = self.base + offset;
        unsafe { read_volatile(addr as *const u8) }
    }

}

impl Serial for Uart {
    fn init(&self) {
        self.init();
    }
    
    fn write_byte(&self, c: u8) {
        while self.reg_read(LSR_OFFSET) & LSR_THRE == 0 {}
        self.reg_write(THR_OFFSET, c);
    }

    // Currently, this function does not block until a byte is available.
    fn read_byte(&self) -> u8 {
        if self.reg_read(LSR_OFFSET) & LSR_DR == 0 {
            return 0;
        }
        self.reg_read(RHR_OFFSET)
    }


    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.bytes() {
            if c == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(c);
        }
        Ok(())
    }

}

impl Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        Serial::write_str(self, s)
    }
}

fn register_uart() {
    let uart = Uart::new(0x1000_0000);
    crate::device::manager::register_serial(Box::new(uart));
}

early_initcall!(register_uart);