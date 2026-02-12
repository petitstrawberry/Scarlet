//! Guest memory slot management
//!
//! Manages the mapping between Guest Physical Addresses (GPA) and
//! Host Physical Addresses (HPA).

extern crate alloc;
use alloc::vec::Vec;

/// Flags for memory slot configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemorySlotFlags {
    /// Memory is read-only for the guest
    pub readonly: bool,
}

impl Default for MemorySlotFlags {
    fn default() -> Self {
        Self { readonly: false }
    }
}

/// A single guest physical memory region mapping
#[derive(Debug, Clone)]
pub struct MemorySlot {
    /// Slot identifier
    pub slot_id: u32,
    /// Guest physical address start
    pub guest_phys_addr: u64,
    /// Size of the region in bytes
    pub memory_size: u64,
    /// Host physical address backing this region
    pub host_phys_addr: u64,
    /// Slot flags
    pub flags: MemorySlotFlags,
}

/// Manages memory slots for a VM
pub struct MemorySlotManager {
    slots: Vec<MemorySlot>,
    max_slots: u32,
}

impl MemorySlotManager {
    /// Create a new memory slot manager
    pub fn new(max_slots: u32) -> Self {
        Self {
            slots: Vec::new(),
            max_slots,
        }
    }

    /// Add or update a memory slot
    pub fn set_slot(&mut self, slot: MemorySlot) -> Result<(), &'static str> {
        for existing in &self.slots {
            if existing.slot_id == slot.slot_id {
                continue;
            }
            let new_start = slot.guest_phys_addr;
            let new_end = slot.guest_phys_addr + slot.memory_size;
            let existing_start = existing.guest_phys_addr;
            let existing_end = existing.guest_phys_addr + existing.memory_size;
            if new_start < existing_end && new_end > existing_start {
                return Err("Memory slot overlap");
            }
        }

        self.slots.retain(|s| s.slot_id != slot.slot_id);

        if self.slots.len() >= self.max_slots as usize {
            return Err("Maximum number of memory slots reached");
        }

        if slot.memory_size > 0 {
            self.slots.push(slot);
        }

        Ok(())
    }

    /// Look up the host physical address for a guest physical address
    pub fn translate(&self, guest_phys_addr: u64) -> Option<(u64, &MemorySlot)> {
        for slot in &self.slots {
            let start = slot.guest_phys_addr;
            let end = start + slot.memory_size;
            if guest_phys_addr >= start && guest_phys_addr < end {
                let offset = guest_phys_addr - start;
                return Some((slot.host_phys_addr + offset, slot));
            }
        }
        None
    }

    /// Get all memory slots
    pub fn slots(&self) -> &[MemorySlot] {
        &self.slots
    }
}
