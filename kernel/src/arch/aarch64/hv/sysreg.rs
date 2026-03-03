#[derive(Debug, Clone, Default)]
pub struct GuestSystemRegs {
    pub vbar_el1: u64,
    pub elr_el1: u64,
    pub spsr_el1: u64,
    pub ttbr0_el1: u64,
    pub ttbr1_el1: u64,
}

impl GuestSystemRegs {
    pub fn save() -> Self {
        Self::default()
    }

    pub fn restore(&self) {}
}

#[derive(Debug, Clone, Default)]
pub struct HypervisorSystemRegs {
    pub vbar_el2: u64,
}

impl HypervisorSystemRegs {
    pub fn save() -> Self {
        Self::default()
    }

    pub fn restore(&self) {}
}
