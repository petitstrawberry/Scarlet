// UART driver for QEMU virt machine

use core::ptr::{read_volatile, write_volatile};

use crate::traits::serial::Serial;


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

    fn read_byte(&self) -> u8 {
        while self.reg_read(LSR_OFFSET) & LSR_DR == 0 {}
        self.reg_read(RHR_OFFSET)
    }
}
