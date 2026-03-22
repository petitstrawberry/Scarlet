extern crate alloc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GpioPull {
    None,
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GpioIrqTrigger {
    RisingEdge,
    FallingEdge,
    HighLevel,
    LowLevel,
}

pub trait GpioController: Send + Sync {
    fn set_direction_output(&self, pin: u32, value: bool);
    fn set_direction_input(&self, pin: u32);
    fn set_value(&self, pin: u32, value: bool);
    fn get_value(&self, pin: u32) -> bool;
    fn set_pull(&self, pin: u32, pull: GpioPull);
    fn set_function(&self, pin: u32, func: u8);
    fn enable_irq(&self, pin: u32, trigger: GpioIrqTrigger);
    fn disable_irq(&self, pin: u32);
    fn ack_irq(&self, pin: u32);
}
