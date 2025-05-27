use core::fmt::{Result, Write};

pub trait Serial: Write {
    fn init(&mut self);
    
    fn put(&mut self, c: char) -> Result;
    fn get(&mut self) -> Option<char>;
}