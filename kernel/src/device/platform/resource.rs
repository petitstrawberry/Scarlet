pub struct PlatformDeviceResource {
    pub res_type: PlatformDeviceResourceType,
    pub start: usize,
    pub end: usize,
}

pub enum PlatformDeviceResourceType {
    MEM,
    IO,
    IRQ,
    DMA,
}