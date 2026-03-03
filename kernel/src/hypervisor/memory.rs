//! VM memory slot management

extern crate alloc;

#[derive(Debug, Clone, Copy, Default)]
pub struct MemorySlotFlags {
    pub readonly: bool,
}

#[derive(Debug, Clone)]
pub struct MemorySlot {
    pub slot_id: u32,
    pub guest_phys_addr: u64,
    pub memory_size: u64,
    pub host_phys_addr: u64,
    pub flags: MemorySlotFlags,
}

impl MemorySlot {
    pub fn contains_gpa(&self, gpa: u64) -> bool {
        gpa >= self.guest_phys_addr && gpa < self.guest_phys_addr + self.memory_size
    }

    pub fn gpa_to_hpa(&self, gpa: u64) -> u64 {
        self.host_phys_addr + (gpa - self.guest_phys_addr)
    }
}

pub struct MemorySlotManager {
    slots: alloc::vec::Vec<MemorySlot>,
}

impl MemorySlotManager {
    pub fn new() -> Self {
        Self {
            slots: alloc::vec::Vec::new(),
        }
    }

    pub fn set_slot(&mut self, slot: MemorySlot) -> Result<(), &'static str> {
        if slot.memory_size == 0 {
            self.slots.retain(|s| s.slot_id != slot.slot_id);
        } else {
            if let Some(existing) = self.slots.iter_mut().find(|s| s.slot_id == slot.slot_id) {
                *existing = slot;
            } else {
                self.slots.push(slot);
            }
        }
        Ok(())
    }

    pub fn find_slot(&self, gpa: u64) -> Option<&MemorySlot> {
        self.slots.iter().find(|s| s.contains_gpa(gpa))
    }
}

impl Default for MemorySlotManager {
    fn default() -> Self {
        Self::new()
    }
}
