use fdt::{Fdt, FdtError};

#[unsafe(link_section = ".data")]
static mut FDT_ADDR: usize = 0;

static mut MANAGER: FdtManager = FdtManager::new();

pub struct FdtManager<'a> {
    fdt: Option<Fdt<'a>>,
}

impl<'a> FdtManager<'a> {

    const fn new() -> Self {
        FdtManager {
            fdt: None,
        }
    }

    pub fn init(&mut self) -> Result<(), FdtError> {
        match unsafe { Fdt::from_ptr(FDT_ADDR as *const u8) } {
            Ok(fdt) => {
                self.fdt = Some(fdt);
            }
            Err(e) => return Err(e),
        }
        Ok(())
    }

    pub fn set_fdt_addr(addr: usize) {
        unsafe {
            FDT_ADDR = addr;
        }
    }
    
    pub fn get_fdt(&self) -> Option<&Fdt<'a>> {
        self.fdt.as_ref()
    }

    #[allow(static_mut_refs)]
    pub fn get_mut_manager() -> &'static mut FdtManager<'a> {
        unsafe { &mut MANAGER }
    }

    #[allow(static_mut_refs)]
    pub fn get_manager() -> &'static FdtManager<'a> {
        unsafe { &MANAGER }
    }
}