pub mod platform;

pub trait Device {
    fn name(&self) -> &'static str;
    fn id(&self) -> usize;
}
