pub struct GuestRoot;

impl GuestRoot {
    pub fn new() -> Result<Self, &'static str> {
        Err("Hypervisor not supported on AArch64")
    }

    pub fn map_page(&self, _gpa: u64, _hpa: u64, _readonly: bool) -> Result<(), &'static str> {
        Err("Hypervisor not supported on AArch64")
    }

    pub fn unmap_page(&self, _gpa: u64) -> Result<(), &'static str> {
        Err("Hypervisor not supported on AArch64")
    }

    pub fn root_token(&self, _vmid: u16) -> Result<u64, &'static str> {
        Err("Hypervisor not supported on AArch64")
    }

    pub fn flush_tlb(&self) {}
}
