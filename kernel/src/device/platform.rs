use super::Device;

pub struct PlatformDevice {
    name: &'static str,
    id: usize,
}

impl Device for PlatformDevice {
    fn name(&self) -> &'static str {
        self.name
    }

    fn id(&self) -> usize {
        self.id
    }
}