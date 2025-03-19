use fdt::{Fdt, FdtError};

pub struct FdtManager {
    fdt_bytes: &'static[u8],
}

impl FdtManager {
    pub const fn new() -> Self {
        FdtManager {
            fdt_bytes: &[],
        }
    }

    pub fn load_fdt(&mut self, data: &'static [u8]) {
        self.fdt_bytes = data
    }

    pub fn parse_fdt(&self) -> Result<Fdt, FdtError> {
        Fdt::new(self.fdt_bytes)
    }
}