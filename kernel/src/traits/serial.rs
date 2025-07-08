use core::{fmt::{Result, Write}, any::Any};

pub trait Serial: Write {
    /// Initializes the serial interface, preparing it for use.
    fn init(&mut self);
    
    fn put(&mut self, c: char) -> Result;
    fn get(&mut self) -> Option<char>;
    
    /// Get a mutable reference to Any for downcasting
    fn as_any_mut(&mut self) -> &mut dyn Any;
}