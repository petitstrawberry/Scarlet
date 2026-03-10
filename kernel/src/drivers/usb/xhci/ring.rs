//! xHCI TRB Ring management with DMA support.

use alloc::vec::Vec;
use core::mem::size_of;
use spin::Mutex;

use super::trb::{Trb, TrbType};
use crate::environment::PAGE_SIZE;
use crate::mem::page::ContiguousPages;

/// DMA-allocated TRB ring for xHCI command/event/transfer rings.
pub struct DmaTrbRing {
    pages: ContiguousPages,
    capacity: usize,
    producer_index: Mutex<usize>,
    cycle_state: Mutex<bool>,
}

impl DmaTrbRing {
    pub fn new(capacity: usize) -> Option<Self> {
        let trb_size = size_of::<Trb>();
        let ring_bytes = capacity * trb_size;
        let page_count = (ring_bytes + PAGE_SIZE - 1) / PAGE_SIZE;

        let pages = ContiguousPages::new(page_count)?;
        let vaddr = pages.as_vaddr();

        for i in 0..capacity {
            unsafe {
                let trb_ptr = (vaddr + i * trb_size) as *mut Trb;
                core::ptr::write_volatile(trb_ptr, Trb::default());
            }
        }

        Some(Self {
            pages,
            capacity,
            producer_index: Mutex::new(0),
            cycle_state: Mutex::new(true),
        })
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn usable_capacity(&self) -> usize {
        self.capacity.saturating_sub(1)
    }

    pub fn physical_address(&self) -> usize {
        self.pages.as_paddr()
    }

    pub fn enqueue(&self, trb: Trb) -> Result<usize, &'static str> {
        let mut index = self.producer_index.lock();

        if self.capacity < 2 {
            return Err("Ring capacity too small");
        }

        if *index >= self.usable_capacity() {
            return Err("Ring full");
        }

        let trb_ptr = unsafe { self.trb_ptr(*index) };
        let cycle = *self.cycle_state.lock();

        let mut trb = trb;
        trb.set_cycle(cycle);

        unsafe {
            core::ptr::write_volatile(trb_ptr, trb);
        }

        let result = *index;
        *index += 1;

        if *index == self.usable_capacity() {
            self.write_link_trb(self.usable_capacity(), cycle)?;
            *index = 0;
            *self.cycle_state.lock() = !cycle;
        }

        Ok(result)
    }

    pub fn enqueue_link(&self, target_paddr: usize) -> Result<(), &'static str> {
        let cycle = *self.cycle_state.lock();
        self.write_link_trb(self.usable_capacity(), cycle)?;
        let _ = target_paddr;
        Ok(())
    }

    pub fn current_producer_index(&self) -> usize {
        *self.producer_index.lock()
    }

    pub fn cycle_state(&self) -> bool {
        *self.cycle_state.lock()
    }

    pub fn peek(&self, index: usize) -> Option<Trb> {
        if index >= self.capacity {
            return None;
        }
        unsafe { Some(core::ptr::read_volatile(self.trb_ptr(index))) }
    }

    pub fn clear(&self) {
        let mut index = self.producer_index.lock();
        *index = 0;
        *self.cycle_state.lock() = true;

        for i in 0..self.capacity {
            unsafe {
                core::ptr::write_volatile(self.trb_ptr(i), Trb::default());
            }
        }

        let _ = self.write_link_trb(self.usable_capacity(), true);
    }

    fn write_link_trb(&self, index: usize, cycle: bool) -> Result<(), &'static str> {
        if index >= self.capacity {
            return Err("Link TRB index out of bounds");
        }

        let trb_ptr = unsafe { self.trb_ptr(index) };
        let mut link = Trb {
            parameter: self.physical_address() as u64,
            status: 0,
            control: (TrbType::Link as u32) << 10 | (1 << 1),
        };
        link.set_cycle(cycle);

        unsafe {
            core::ptr::write_volatile(trb_ptr, link);
        }

        Ok(())
    }

    unsafe fn trb_ptr(&self, index: usize) -> *mut Trb {
        let vaddr = self.pages.as_vaddr();
        (vaddr + index * size_of::<Trb>()) as *mut Trb
    }
}

pub struct TrbRing {
    trbs: Vec<Trb>,
    enqueue_index: usize,
    dequeue_index: usize,
    cycle: bool,
}

impl TrbRing {
    pub fn new(len: usize) -> Self {
        Self {
            trbs: alloc::vec![Trb::default(); len],
            enqueue_index: 0,
            dequeue_index: 0,
            cycle: true,
        }
    }

    pub fn len(&self) -> usize {
        self.trbs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.trbs.is_empty()
    }

    pub fn cycle(&self) -> bool {
        self.cycle
    }

    pub fn push(&mut self, mut trb: Trb) -> Result<usize, &'static str> {
        if self.trbs.is_empty() {
            return Err("TRB ring is empty");
        }

        trb.set_cycle(self.cycle);
        let index = self.enqueue_index;
        self.trbs[index] = trb;
        self.enqueue_index = (self.enqueue_index + 1) % self.trbs.len();

        if self.enqueue_index == 0 {
            self.cycle = !self.cycle;
        }

        Ok(index)
    }

    pub fn dequeue(&self) -> Option<&Trb> {
        self.trbs.get(self.dequeue_index)
    }
}

/// Event Ring Segment Table Entry (ERST entry)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ErstEntry {
    pub ring_segment_base: u64,
    pub ring_segment_size: u32,
    pub reserved: u32,
}

impl ErstEntry {
    pub const fn size() -> usize {
        size_of::<Self>()
    }
}

/// Event Ring with Segment Table support
pub struct EventRing {
    ring: DmaTrbRing,
    erst: ContiguousPages,
    erst_count: usize,
    dequeue_index: Mutex<usize>,
    current_cycle: Mutex<bool>,
}

impl EventRing {
    pub fn new(capacity: usize) -> Option<Self> {
        let ring = DmaTrbRing::new(capacity)?;

        let erst_page_count = 1;
        let erst = ContiguousPages::new(erst_page_count)?;

        let entry = ErstEntry {
            ring_segment_base: ring.physical_address() as u64,
            ring_segment_size: capacity as u32,
            reserved: 0,
        };

        unsafe {
            let erst_ptr = erst.as_vaddr() as *mut ErstEntry;
            core::ptr::write_volatile(erst_ptr, entry);
        }

        Some(Self {
            ring,
            erst,
            erst_count: 1,
            dequeue_index: Mutex::new(0),
            current_cycle: Mutex::new(true),
        })
    }

    pub fn physical_address(&self) -> usize {
        self.ring.physical_address()
    }

    pub fn erst_physical_address(&self) -> usize {
        self.erst.as_paddr()
    }

    pub fn erst_size(&self) -> u32 {
        self.erst_count as u32
    }

    pub fn capacity(&self) -> usize {
        self.ring.capacity()
    }

    pub fn has_event(&self) -> bool {
        let index = *self.dequeue_index.lock();
        if let Some(trb) = self.ring.peek(index) {
            let cycle_bit = (trb.control & 1) != 0;
            cycle_bit == *self.current_cycle.lock()
        } else {
            false
        }
    }

    pub fn dequeue(&self) -> Option<Trb> {
        let mut index = self.dequeue_index.lock();
        let current_cycle = *self.current_cycle.lock();

        if let Some(trb) = self.ring.peek(*index) {
            let cycle_bit = (trb.control & 1) != 0;

            if cycle_bit == current_cycle {
                *index += 1;

                if *index >= self.ring.capacity() {
                    *index = 0;
                    *self.current_cycle.lock() = !current_cycle;
                }

                return Some(trb);
            }
        }

        None
    }

    pub fn event_ring_dequeue_pointer(&self) -> usize {
        let index = *self.dequeue_index.lock();
        self.ring.physical_address() + index * size_of::<Trb>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_trb_ring_push_advances_index() {
        let mut ring = TrbRing::new(4);
        assert_eq!(ring.len(), 4);
        assert!(ring.cycle());

        let index = ring.push(Trb::new(TrbType::Normal)).unwrap();
        assert_eq!(index, 0);
        assert_eq!(ring.dequeue().unwrap().trb_type(), TrbType::Normal as u8);
    }

    #[test_case]
    fn test_trb_ring_cycle_toggle() {
        let mut ring = TrbRing::new(2);
        assert!(ring.cycle());

        ring.push(Trb::new(TrbType::Normal)).unwrap();
        assert!(ring.cycle());

        ring.push(Trb::new(TrbType::Normal)).unwrap();
        assert!(!ring.cycle());
    }

    #[test_case]
    fn test_erst_entry_size() {
        assert_eq!(ErstEntry::size(), 16);
    }
}
