use core::{fmt::{Result, Write}, any::Any};

pub trait Serial: Write {
    fn put(&self, c: char) -> Result;
    fn get(&self) -> Option<char>;
    
    /// Get a mutable reference to Any for downcasting
    fn as_any_mut(&mut self) -> &mut dyn Any;
}