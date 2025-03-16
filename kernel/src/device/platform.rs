use super::Device;

pub struct PlatformDevice {
    pub device: Device,
    pub mmio_base: usize,
    pub mmio_size: usize,
}
