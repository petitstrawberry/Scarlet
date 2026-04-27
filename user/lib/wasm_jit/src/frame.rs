pub struct FrameLayout {
    pub param_count: u16,
    pub local_count: u16,
    pub stack_base: u16,
    pub total_slots: u16,
}

impl FrameLayout {
    pub fn new(param_count: u16, local_count: u16, max_stack: u16) -> Self {
        let stack_base = param_count + local_count;
        let total_slots = stack_base + max_stack;
        Self {
            param_count,
            local_count,
            stack_base,
            total_slots,
        }
    }

    pub fn local_slot(&self, index: u32) -> u16 {
        self.param_count + index as u16
    }

    pub fn stack_slot(&self, height: u16) -> u16 {
        self.stack_base + height
    }

    pub fn byte_size(&self) -> usize {
        self.total_slots as usize * 8
    }
}
