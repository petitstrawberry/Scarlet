pub trait Serial {
    fn init(&self);
    fn write_byte(&self, c: u8);
    fn read_byte(&self) -> u8;
}