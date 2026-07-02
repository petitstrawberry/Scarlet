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
    dma_addr: Mutex<usize>,
    capacity: usize,
    linked: bool,
    producer_index: Mutex<usize>,
    cycle_state: Mutex<bool>,
}

impl DmaTrbRing {
    pub fn new(capacity: usize) -> Option<Self> {
        Self::new_inner(capacity, false, PAGE_SIZE)
    }

    /// Create an event-style TRB ring with a minimum DMA alignment.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Total TRB capacity.
    /// * `align` - Minimum physical alignment for the backing allocation.
    ///
    /// # Returns
    ///
    /// A DMA-backed ring, or `None` if allocation fails.
    pub fn new_aligned(capacity: usize, align: usize) -> Option<Self> {
        Self::new_inner(capacity, false, align)
    }

    /// Create a TRB ring whose final entry links back to the first TRB.
    ///
    /// Command and transfer rings use a Link TRB at the end of the segment so
    /// the host controller can wrap to the beginning when the producer reaches
    /// the end.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Total TRB capacity, including the final Link TRB.
    ///
    /// # Returns
    ///
    /// A DMA-backed ring, or `None` if allocation fails.
    pub fn new_linked(capacity: usize) -> Option<Self> {
        Self::new_inner(capacity, true, PAGE_SIZE)
    }

    /// Create a linked TRB ring with a minimum DMA alignment.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Total TRB capacity, including the final Link TRB.
    /// * `align` - Minimum physical alignment for the backing allocation.
    ///
    /// # Returns
    ///
    /// A DMA-backed ring, or `None` if allocation fails.
    pub fn new_linked_aligned(capacity: usize, align: usize) -> Option<Self> {
        Self::new_inner(capacity, true, align)
    }

    fn new_inner(capacity: usize, linked: bool, align: usize) -> Option<Self> {
        let trb_size = size_of::<Trb>();
        let ring_bytes = capacity * trb_size;
        let align = align.max(PAGE_SIZE);
        let granule_pages = align.div_ceil(PAGE_SIZE).max(1);
        let page_count = ring_bytes
            .div_ceil(PAGE_SIZE)
            .next_multiple_of(granule_pages);

        let pages = ContiguousPages::new_aligned(page_count, align)?;
        let vaddr = pages.as_vaddr();

        for i in 0..capacity {
            unsafe {
                let trb_ptr = (vaddr + i * trb_size) as *mut Trb;
                core::ptr::write_volatile(trb_ptr, Trb::default());
            }
        }

        let ring = Self {
            dma_addr: Mutex::new(pages.as_paddr()),
            pages,
            capacity,
            linked,
            producer_index: Mutex::new(0),
            cycle_state: Mutex::new(true),
        };

        if linked {
            ring.write_link_trb(ring.usable_capacity(), true, ring.physical_address())
                .ok()?;
        } else {
            ring.sync_for_device();
        }

        Some(ring)
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

    /// Return the DMA address programmed into xHCI TRBs and registers.
    ///
    /// # Returns
    ///
    /// Device-visible DMA address for the start of the ring.
    pub fn dma_address(&self) -> usize {
        *self.dma_addr.lock()
    }

    /// Update the device-visible DMA address for this ring.
    ///
    /// # Arguments
    ///
    /// * `dma_addr` - DMA address returned by the owning device DMA context.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the ring metadata was updated.
    pub fn set_dma_address(&self, dma_addr: usize) -> Result<(), &'static str> {
        *self.dma_addr.lock() = dma_addr;
        if self.linked {
            let cycle = *self.cycle_state.lock();
            self.write_link_trb(self.usable_capacity(), cycle, dma_addr)?;
        }
        Ok(())
    }

    /// Return the number of bytes allocated for this ring.
    ///
    /// # Returns
    ///
    /// DMA mapping length in bytes.
    pub fn dma_len(&self) -> usize {
        self.pages.len() * PAGE_SIZE
    }

    /// Clean CPU-written ring contents so the host controller can read them.
    ///
    /// # Returns
    ///
    /// This method does not return a value.
    pub fn sync_for_device(&self) {
        crate::arch::clean_dcache_to_poc_range(self.pages.as_vaddr(), self.byte_len());
    }

    /// Invalidate ring contents before reading entries written by the host controller.
    ///
    /// # Returns
    ///
    /// This method does not return a value.
    pub fn sync_for_cpu(&self) {
        crate::arch::invalidate_dcache_to_poc_range(self.pages.as_vaddr(), self.byte_len());
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
        self.sync_trb_for_device(result);
        *index += 1;

        if *index == self.usable_capacity() {
            self.write_link_trb(self.usable_capacity(), cycle, self.dma_address())?;
            *index = 0;
            *self.cycle_state.lock() = !cycle;
        }

        Ok(result)
    }

    /// Ensure the next TRBs can be enqueued before the segment Link TRB.
    ///
    /// Transfer TDs must not be split across the Link TRB unless the TD is
    /// explicitly chained. This helper pads the remaining segment entries with
    /// No-Op TRBs and wraps to the next cycle before a multi-TRB TD is queued.
    ///
    /// # Arguments
    ///
    /// * `required` - Number of contiguous usable TRB entries required.
    /// * `padding` - TRB used to consume the trailing entries before the Link TRB.
    ///
    /// # Returns
    ///
    /// `Ok(())` when at least `required` contiguous entries are available.
    pub fn ensure_contiguous_space(
        &self,
        required: usize,
        padding: Trb,
    ) -> Result<(), &'static str> {
        if required == 0 {
            return Ok(());
        }
        if required > self.usable_capacity() {
            return Err("Requested contiguous TRB span exceeds ring segment");
        }

        loop {
            let mut index = self.producer_index.lock();
            let mut cycle = self.cycle_state.lock();
            let remaining = self.usable_capacity().saturating_sub(*index);
            if remaining >= required {
                return Ok(());
            }

            let trb_ptr = unsafe { self.trb_ptr(*index) };
            let mut trb = padding;
            trb.set_cycle(*cycle);
            unsafe {
                core::ptr::write_volatile(trb_ptr, trb);
            }
            self.sync_trb_for_device(*index);
            *index += 1;

            if *index == self.usable_capacity() {
                self.write_link_trb(self.usable_capacity(), *cycle, self.dma_address())?;
                *index = 0;
                *cycle = !*cycle;
            }
        }
    }

    pub fn enqueue_link(&self, target_paddr: usize) -> Result<(), &'static str> {
        let cycle = *self.cycle_state.lock();
        self.write_link_trb(self.usable_capacity(), cycle, target_paddr)?;
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

    /// Read an event TRB once its producer cycle bit is visible.
    ///
    /// xHCI publishes event TRBs by updating the cycle bit in the control word.
    /// The consumer must not advance the dequeue pointer until the event has a
    /// valid completion code; seeing a matching cycle with completion code zero
    /// means the event is not ready to consume yet.
    ///
    /// # Arguments
    ///
    /// * `index` - Event ring TRB index to inspect.
    /// * `expected_cycle` - Consumer cycle state expected for a new event.
    ///
    /// # Returns
    ///
    /// The completed event TRB, or `None` if no completed event is available.
    pub fn peek_completed_event(&self, index: usize, expected_cycle: bool) -> Option<Trb> {
        if index >= self.capacity {
            return None;
        }

        self.sync_trb_for_cpu(index);
        let control = unsafe {
            let control_ptr = (self.pages.as_vaddr() + index * size_of::<Trb>() + 12) as *const u32;
            core::ptr::read_volatile(control_ptr)
        };
        if ((control & 1) != 0) != expected_cycle {
            return None;
        }

        crate::arch::rmb();
        self.sync_trb_for_cpu(index);
        let trb = unsafe { core::ptr::read_volatile(self.trb_ptr(index)) };
        crate::arch::rmb();

        if ((trb.control & 1) != 0) != expected_cycle || trb.completion_code() == 0 {
            return None;
        }

        Some(trb)
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

        let _ = self.write_link_trb(self.usable_capacity(), true, self.dma_address());
        self.sync_for_device();
    }

    fn write_link_trb(
        &self,
        index: usize,
        cycle: bool,
        target_paddr: usize,
    ) -> Result<(), &'static str> {
        if index >= self.capacity {
            return Err("Link TRB index out of bounds");
        }

        let trb_ptr = unsafe { self.trb_ptr(index) };
        let mut link = Trb {
            parameter: target_paddr as u64,
            status: 0,
            control: (TrbType::Link as u32) << 10 | (1 << 1),
        };
        link.set_cycle(cycle);

        unsafe {
            core::ptr::write_volatile(trb_ptr, link);
        }
        self.sync_trb_for_device(index);

        Ok(())
    }

    fn sync_trb_for_device(&self, index: usize) {
        crate::arch::clean_dcache_to_poc_range(
            self.pages.as_vaddr() + index * size_of::<Trb>(),
            size_of::<Trb>(),
        );
    }

    fn sync_trb_for_cpu(&self, index: usize) {
        crate::arch::invalidate_dcache_to_poc_range(
            self.pages.as_vaddr() + index * size_of::<Trb>(),
            size_of::<Trb>(),
        );
    }

    fn byte_len(&self) -> usize {
        self.capacity * size_of::<Trb>()
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
    erst_dma_addr: Mutex<usize>,
    erst_count: usize,
    dequeue_index: Mutex<usize>,
    current_cycle: Mutex<bool>,
}

impl EventRing {
    pub fn new(capacity: usize) -> Option<Self> {
        Self::new_aligned(capacity, PAGE_SIZE)
    }

    /// Create an event ring with a minimum DMA alignment.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Number of event TRBs in the ring segment.
    /// * `align` - Minimum physical alignment for DMA allocations.
    ///
    /// # Returns
    ///
    /// A DMA-backed event ring, or `None` if allocation fails.
    pub fn new_aligned(capacity: usize, align: usize) -> Option<Self> {
        let align = align.max(PAGE_SIZE);
        let granule_pages = align.div_ceil(PAGE_SIZE).max(1);
        let ring = DmaTrbRing::new_aligned(capacity, align)?;

        let erst_page_count = 1usize.next_multiple_of(granule_pages);
        let erst = ContiguousPages::new_aligned(erst_page_count, align)?;

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
            erst_dma_addr: Mutex::new(erst.as_paddr()),
            erst,
            erst_count: 1,
            dequeue_index: Mutex::new(0),
            current_cycle: Mutex::new(true),
        })
    }

    pub fn physical_address(&self) -> usize {
        self.ring.physical_address()
    }

    /// Return the DMA address for the event ring segment.
    ///
    /// # Returns
    ///
    /// Device-visible address for event TRBs.
    pub fn dma_address(&self) -> usize {
        self.ring.dma_address()
    }

    pub fn erst_physical_address(&self) -> usize {
        self.erst.as_paddr()
    }

    /// Return the DMA address for the event ring segment table.
    ///
    /// # Returns
    ///
    /// Device-visible address for ERST entries.
    pub fn erst_dma_address(&self) -> usize {
        *self.erst_dma_addr.lock()
    }

    /// Return the number of bytes allocated for the event ring segment.
    ///
    /// # Returns
    ///
    /// DMA mapping length in bytes.
    pub fn dma_len(&self) -> usize {
        self.ring.dma_len()
    }

    /// Return the number of bytes allocated for the event ring segment table.
    ///
    /// # Returns
    ///
    /// DMA mapping length in bytes.
    pub fn erst_dma_len(&self) -> usize {
        self.erst.len() * PAGE_SIZE
    }

    /// Update device-visible addresses for the event ring and ERST.
    ///
    /// # Arguments
    ///
    /// * `ring_dma_addr` - DMA address for the event TRB segment.
    /// * `erst_dma_addr` - DMA address for the ERST page.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the ERST entry was updated.
    pub fn set_dma_addresses(
        &self,
        ring_dma_addr: usize,
        erst_dma_addr: usize,
    ) -> Result<(), &'static str> {
        self.ring.set_dma_address(ring_dma_addr)?;
        *self.erst_dma_addr.lock() = erst_dma_addr;
        let entry = ErstEntry {
            ring_segment_base: ring_dma_addr as u64,
            ring_segment_size: self.ring.capacity() as u32,
            reserved: 0,
        };
        unsafe {
            let erst_ptr = self.erst.as_vaddr() as *mut ErstEntry;
            core::ptr::write_volatile(erst_ptr, entry);
        }
        self.sync_for_device();
        Ok(())
    }

    pub fn erst_size(&self) -> u32 {
        self.erst_count as u32
    }

    /// Clean CPU-written event-ring metadata so the host controller can read it.
    ///
    /// # Returns
    ///
    /// This method does not return a value.
    pub fn sync_for_device(&self) {
        self.ring.sync_for_device();
        crate::arch::clean_dcache_to_poc_range(
            self.erst.as_vaddr(),
            self.erst_count * size_of::<ErstEntry>(),
        );
    }

    pub fn capacity(&self) -> usize {
        self.ring.capacity()
    }

    /// Return the current event-ring dequeue index.
    ///
    /// # Returns
    ///
    /// Current TRB index that software will inspect next.
    pub fn current_dequeue_index(&self) -> usize {
        *self.dequeue_index.lock()
    }

    /// Return the current event-ring cycle state expected by software.
    ///
    /// # Returns
    ///
    /// Current cycle bit value for valid event TRBs.
    pub fn current_cycle_state(&self) -> bool {
        *self.current_cycle.lock()
    }

    /// Read an event-ring TRB without advancing the dequeue pointer.
    ///
    /// # Arguments
    ///
    /// * `index` - TRB index to read.
    ///
    /// # Returns
    ///
    /// The TRB at `index`, or `None` when the index is outside the ring.
    pub fn peek(&self, index: usize) -> Option<Trb> {
        self.ring.sync_for_cpu();
        self.ring.peek(index)
    }

    pub fn has_event(&self) -> bool {
        self.ring.sync_for_cpu();
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

        if let Some(trb) = self.ring.peek_completed_event(*index, current_cycle) {
            *index += 1;

            if *index >= self.ring.capacity() {
                *index = 0;
                *self.current_cycle.lock() = !current_cycle;
            }

            return Some(trb);
        }

        None
    }

    pub fn event_ring_dequeue_pointer(&self) -> usize {
        let index = *self.dequeue_index.lock();
        self.ring.dma_address() + index * size_of::<Trb>()
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
