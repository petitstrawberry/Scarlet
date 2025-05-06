use core::fmt::{Result, Write};

pub trait Serial: Write {
    fn init(&self);
    fn write_byte(&self, c: u8);
    fn read_byte(&self) -> u8;
    fn write_str(&mut self, s: &str) -> Result;
}