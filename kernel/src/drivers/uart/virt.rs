// UART driver for QEMU virt machine

use core::{fmt, any::Any, ptr::{read_volatile, write_volatile}};
use core::fmt::Write;
use alloc::boxed::Box;

use crate::{early_initcall, traits::serial::Serial, device::{Device, DeviceType, char::CharDevice}};


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

    fn reg_write(&self, offset: usize, value: u8) {
        let addr = self.base + offset;
        unsafe { write_volatile(addr as *mut u8, value) }
    }

    fn reg_read(&self, offset: usize) -> u8 {
        let addr = self.base + offset;
        unsafe { read_volatile(addr as *const u8) }
    }

    fn write_byte_internal(&self, c: u8) {
        while self.reg_read(LSR_OFFSET) & LSR_THRE == 0 {}
        self.reg_write(THR_OFFSET, c);
    }

    fn read_byte_internal(&self) -> u8 {
        if self.reg_read(LSR_OFFSET) & LSR_DR == 0 {
            return 0;
        }
        self.reg_read(RHR_OFFSET)
    }
}

impl Serial for Uart {
    fn init(&mut self) {
        // Initialization code for the UART can be added here if needed.
        // For now, we assume the UART is already initialized by the QEMU virt machine.
    }

    fn put(&mut self, c: char) -> fmt::Result {
        self.write_byte_internal(c as u8); // Block until ready
        Ok(())
    }

    fn get(&mut self) -> Option<char> {
        if self.can_read() {
            Some(self.read_byte_internal() as char)
        } else {
            None
        }
    }
}

impl Device for Uart {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "virt-uart"
    }

    fn id(&self) -> usize {
        0
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
    
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    
    fn as_char_device(&mut self) -> Option<&mut dyn CharDevice> {
        Some(self)
    }
}

impl CharDevice for Uart {
    fn read_byte(&mut self) -> Option<u8> {
        if self.can_read() {
            Some(self.read_byte_internal())
        } else {
            None
        }
    }

    fn write_byte(&mut self, byte: u8) -> Result<(), &'static str> {
        self.write_byte_internal(byte); // Block until ready
        Ok(())
    }

    fn can_read(&self) -> bool {
        self.reg_read(LSR_OFFSET) & LSR_DR != 0
    }

    fn can_write(&self) -> bool {
        self.reg_read(LSR_OFFSET) & LSR_THRE != 0
    }
    
}

impl Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            if c == '\n' {
                self.put('\r')?; // Convert newline to carriage return + newline
            }
            self.put(c)?;
        }
        Ok(())
    }
}

fn register_uart() {
    let uart = Uart::new(0x1000_0000);
    crate::device::manager::register_serial(Box::new(uart));
}

early_initcall!(register_uart);