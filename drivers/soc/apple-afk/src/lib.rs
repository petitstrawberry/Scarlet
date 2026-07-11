#![no_std]

extern crate alloc;

use alloc::sync::Arc;
use core::arch::asm;
use core::mem;

use scarlet::device::remoteproc::{
    RemoteProcessor, RemoteprocError, RemoteprocMessage, RemoteprocService, RemoteprocServiceId,
};
use scarlet::early_println;
use scarlet::mem::pmm;
use scarlet::time;
use scarlet::vm;
use scarlet_driver_apple_rtkit::AppleRtkit;

// =============================================================================
// Constants
// =============================================================================

/// Block shift: all ring buffer operations use 64-byte blocks.
const BLOCK_SHIFT: u32 = 6;
/// Block size in bytes.
const BLOCK_SIZE: usize = 1 << BLOCK_SHIFT;
/// Block alignment mask.
const BLOCK_MASK: usize = BLOCK_SIZE - 1;

/// Queue entry magic value: `' POI'` (0x20504f49).
const QE_MAGIC: u32 = 0x2050_4F49;

/// Default AFK shared buffer size (256 KB).
const AFK_BUFFER_SIZE: usize = 256 * 1024;

// RBEP message type field: bits [63:48].
const RBEP_TYPE_SHIFT: u64 = 48;
const RBEP_TYPE_MASK: u64 = 0xFFFF << RBEP_TYPE_SHIFT;

/// GETBUF fields within the payload.
const GETBUF_SIZE_SHIFT: u64 = 16;
const GETBUF_SIZE_MASK: u64 = 0xFFFF << GETBUF_SIZE_SHIFT;
const GETBUF_TAG_MASK: u64 = 0xFFFF;

/// GETBUF_ACK DVA field: bits [47:0].
const GETBUF_ACK_DVA_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

/// INIT_TX / INIT_RX fields within the payload.
const INITRB_OFFSET_SHIFT: u64 = 32;
const INITRB_OFFSET_MASK: u64 = 0xFFFF << INITRB_OFFSET_SHIFT;
const INITRB_SIZE_MASK: u64 = GETBUF_SIZE_MASK;
const INITRB_TAG_MASK: u64 = GETBUF_TAG_MASK;

/// RBEP message type constants.
const RBEP_INIT: u64 = 0x80;
const RBEP_INIT_ACK: u64 = 0xa0;
const RBEP_GETBUF: u64 = 0x89;
const RBEP_GETBUF_ACK: u64 = 0xa1;
const RBEP_INIT_TX: u64 = 0x8a;
const RBEP_INIT_RX: u64 = 0x8b;
const RBEP_START: u64 = 0xa3;
const RBEP_START_ACK: u64 = 0x86;
const RBEP_SEND: u64 = 0xa2;
const RBEP_RECV: u64 = 0x85;
const RBEP_SHUTDOWN: u64 = 0xc0;
const RBEP_SHUTDOWN_ACK: u64 = 0xc1;

/// AFK handshake timeout in microseconds.
const AFK_TIMEOUT_US: u64 = 5_000_000;
/// Poll delay in microseconds.
const AFK_POLL_DELAY_US: u64 = 1;

// =============================================================================
// Data Structures
// =============================================================================

/// Ring buffer header — shared between AP and coprocessor.
///
/// Each field sits at a 64-byte-aligned offset to match Apple's DMA
/// cache line expectations. The data area follows immediately after.
///
/// ```text
/// Offset 0x00: bufsz (u32) — size of data area in bytes
/// Offset 0x40: rptr  (u32) — read pointer
/// Offset 0x80: wptr  (u32) — write pointer
/// Total: 0xC0 (192) bytes
/// ```
#[repr(C)]
struct AfkRingBufferHeader {
    bufsz: u32,
    _pad0: [u32; 15],
    rptr: u32,
    _pad1: [u32; 15],
    wptr: u32,
    _pad2: [u32; 15],
}

// SAFETY: accessed only via volatile reads/writes for cross-device sync.
unsafe impl Send for AfkRingBufferHeader {}
unsafe impl Sync for AfkRingBufferHeader {}

/// Queue entry header in the ring buffer.
///
/// Each entry is 16 bytes of header followed by `size` bytes of payload,
/// padded to 64-byte block alignment.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AfkQueueEntry {
    /// Magic value, must be `QE_MAGIC`.
    pub magic: u32,
    /// Payload size in bytes.
    pub size: u32,
    /// Channel number.
    pub channel: u32,
    /// Message type.
    pub msg_type: u32,
}

impl AfkQueueEntry {
    /// Header size in bytes (4 × u32).
    pub const HEADER_SIZE: usize = mem::size_of::<Self>();
}

/// One ring buffer direction (TX or RX).
struct AfkRingBuffer {
    /// Pointer to the shared ring buffer header.
    hdr: *mut AfkRingBufferHeader,
    /// Pointer to the data area following the header.
    buf: *mut u8,
    /// Data area size in bytes.
    bufsz: u32,
    /// Whether this ring buffer has been initialized.
    ready: bool,
}

// SAFETY: accessed only under the endpoint lock.
unsafe impl Send for AfkRingBuffer {}
unsafe impl Sync for AfkRingBuffer {}

/// Shared DMA buffer backing the TX and RX ring buffers.
struct AfkSharedBuffer {
    /// Kernel virtual address.
    virt: *mut u8,
    /// CPU physical address backing the shared mapping.
    paddr: usize,
    /// Device virtual address sent to the coprocessor.
    dva: u64,
    /// Total buffer size in bytes.
    size: usize,
    /// Tag from GETBUF, used to validate INIT_TX/INIT_RX messages.
    tag: u32,
}

// SAFETY: accessed only under the endpoint lock.
unsafe impl Send for AfkSharedBuffer {}
unsafe impl Sync for AfkSharedBuffer {}

/// AFK endpoint state.
///
/// Manages a pair of ring buffers (TX and RX) within a single shared DMA
/// buffer, communicating with one coprocessor endpoint via RBEP.
pub struct AfkEndpoint {
    remoteproc: Arc<dyn RemoteProcessor>,
    service: Arc<dyn RemoteprocService>,
    ep: u8,
    shared: Option<AfkSharedBuffer>,
    tx: AfkRingBuffer,
    rx: AfkRingBuffer,
    started: bool,
}

// =============================================================================
// Helpers
// =============================================================================

#[inline(always)]
const fn block_align_up(val: usize) -> usize {
    (val + BLOCK_SIZE - 1) & !BLOCK_MASK
}

#[inline(always)]
fn field_get(val: u64, mask: u64) -> u64 {
    (val & mask) >> mask.trailing_zeros()
}

#[inline(always)]
fn field_prep(mask: u64, val: u64) -> u64 {
    val << mask.trailing_zeros()
}

fn rbep_msg(msg_type: u64, payload: u64) -> u64 {
    field_prep(RBEP_TYPE_MASK, msg_type) | payload
}

fn rbep_type(msg: u64) -> u64 {
    field_get(msg, RBEP_TYPE_MASK)
}

#[inline(always)]
fn dma_clean(ptr: *const u8, len: usize) {
    scarlet::arch::clean_dcache_to_poc_range(ptr as usize, len);
}

#[inline(always)]
fn dma_invalidate(ptr: *const u8, len: usize) {
    scarlet::arch::invalidate_dcache_to_poc_range(ptr as usize, len);
}

/// DMA write barrier — ensures preceding writes are visible to the coprocessor.
#[inline(always)]
unsafe fn dma_wmb() {
    // SAFETY: AArch64 inner-shareable write barrier for DMA ordering.
    unsafe {
        asm!("dsb ishst", options(nostack, nomem, preserves_flags));
    }
}

/// DMA full memory barrier — ensures ordering of all reads and writes.
#[inline(always)]
unsafe fn dma_mb() {
    // SAFETY: AArch64 inner-shareable full barrier for DMA ordering.
    unsafe {
        asm!("dsb ish", options(nostack, nomem, preserves_flags));
    }
}

// =============================================================================
// Implementation
// =============================================================================

impl AfkEndpoint {
    /// Return the remote processor that owns this endpoint's DMA address space.
    ///
    /// # Returns
    ///
    /// Shared remote processor reference.
    pub fn remoteproc(&self) -> Arc<dyn RemoteProcessor> {
        self.remoteproc.clone()
    }

    /// Create a new AFK endpoint (not started).
    ///
    /// # Arguments
    ///
    /// * `remoteproc` - Remote processor exposing the AFK RTKit endpoint as a service.
    /// * `ep` - RTKit endpoint number used by the AFK protocol.
    ///
    /// # Returns
    ///
    /// A new AFK endpoint bound to the requested remoteproc service.
    ///
    /// Call [`start`](Self::start) to perform the full RBEP handshake.
    pub fn new(remoteproc: Arc<dyn RemoteProcessor>, ep: u8) -> Result<Self, &'static str> {
        let service = remoteproc
            .get_service(RemoteprocServiceId(ep as u32))
            .ok_or("apple-afk: RTKit service not found")?;

        Ok(Self {
            remoteproc,
            service,
            ep,
            shared: None,
            tx: AfkRingBuffer {
                hdr: core::ptr::null_mut(),
                buf: core::ptr::null_mut(),
                bufsz: 0,
                ready: false,
            },
            rx: AfkRingBuffer {
                hdr: core::ptr::null_mut(),
                buf: core::ptr::null_mut(),
                bufsz: 0,
                ready: false,
            },
            started: false,
        })
    }

    /// Create a new AFK endpoint from an Apple RTKit instance.
    ///
    /// # Arguments
    ///
    /// * `rtkit` - Apple RTKit processor implementing [`RemoteProcessor`].
    /// * `ep` - RTKit endpoint number used by the AFK protocol.
    ///
    /// # Returns
    ///
    /// A new AFK endpoint bound to the requested RTKit service.
    pub fn from_rtkit(rtkit: Arc<AppleRtkit>, ep: u8) -> Result<Self, &'static str> {
        let remoteproc: Arc<dyn RemoteProcessor> = rtkit;
        Self::new(remoteproc, ep)
    }

    /// Perform the full RBEP initialization handshake.
    ///
    /// Sequence: START_EP → INIT/INIT_ACK → GETBUF/GETBUF_ACK →
    ///           INIT_TX → INIT_RX → START/START_ACK
    pub fn start(&mut self) -> Result<(), &'static str> {
        self.send_rbep(RBEP_INIT, 0)?;
        self.wait_rbep_type(RBEP_INIT_ACK)?;

        self.handle_getbuf()?;

        self.wait_init_ringbuffers()?;

        self.send_rbep(RBEP_START, 0)?;
        self.wait_rbep_type(RBEP_START_ACK)?;

        self.started = true;
        early_println!("[apple-afk] ep {}: started", self.ep);
        Ok(())
    }

    /// Whether the endpoint has been fully started.
    pub fn is_started(&self) -> bool {
        self.started
    }

    /// Enqueue a message into the TX ring buffer and notify the coprocessor.
    ///
    /// Returns an error if the ring buffer is full.
    pub fn send(&mut self, channel: u32, msg_type: u32, data: &[u8]) -> Result<(), &'static str> {
        if !self.tx.ready {
            return Err("apple-afk: TX ring buffer not ready");
        }

        let rb = &self.tx;
        dma_invalidate(rb.hdr as *const u8, mem::size_of::<AfkRingBufferHeader>());
        let rptr = self.read_rptr(rb.hdr);
        let wptr = self.read_wptr(rb.hdr);

        let entry_total = AfkQueueEntry::HEADER_SIZE + data.len();
        let advance = block_align_up(entry_total);

        if !self.has_space(wptr, rptr, advance, rb.bufsz as usize) {
            return Err("apple-afk: TX ring buffer full");
        }

        let hdr_ptr = unsafe { rb.buf.add(wptr as usize) } as *mut AfkQueueEntry;
        let mut new_wptr = wptr as usize;

        // Write queue entry header
        unsafe {
            core::ptr::addr_of_mut!((*hdr_ptr).magic).write_volatile(QE_MAGIC);
            core::ptr::addr_of_mut!((*hdr_ptr).size).write_volatile(data.len() as u32);
            core::ptr::addr_of_mut!((*hdr_ptr).channel).write_volatile(channel);
            core::ptr::addr_of_mut!((*hdr_ptr).msg_type).write_volatile(msg_type);
        }

        new_wptr += AfkQueueEntry::HEADER_SIZE;

        // Handle wrap: if payload won't fit above wptr, write sentinel at start
        if data.len() > rb.bufsz as usize - new_wptr {
            let sentinel = rb.buf as *mut AfkQueueEntry;
            unsafe {
                core::ptr::addr_of_mut!((*sentinel).magic).write_volatile(QE_MAGIC);
                core::ptr::addr_of_mut!((*sentinel).size).write_volatile(data.len() as u32);
                core::ptr::addr_of_mut!((*sentinel).channel).write_volatile(channel);
                core::ptr::addr_of_mut!((*sentinel).msg_type).write_volatile(msg_type);
            }
            new_wptr = AfkQueueEntry::HEADER_SIZE;
        }

        // Write payload
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), rb.buf.add(new_wptr), data.len());
        }
        new_wptr += data.len();

        // 64-byte align and wrap
        new_wptr = block_align_up(new_wptr);
        if new_wptr >= rb.bufsz as usize {
            new_wptr = 0;
        }

        // The shared allocation uses Scarlet's cacheable direct map. Clean the
        // queue data before publishing the new write pointer, mirroring the
        // ownership guarantees of Asahi Linux's coherent DMA allocation.
        dma_clean(rb.buf as *const u8, rb.bufsz as usize);

        // Barrier + update write pointer
        unsafe {
            dma_wmb();
            core::ptr::addr_of_mut!((*rb.hdr).wptr).write_volatile(new_wptr as u32);
        }
        // SAFETY: a ready TX ring owns a valid shared header mapping.
        let wptr_ptr = unsafe { core::ptr::addr_of!((*rb.hdr).wptr) };
        dma_clean(wptr_ptr as *const u8, mem::size_of::<u32>());

        // Notify coprocessor
        self.send_rbep(RBEP_SEND, new_wptr as u64)?;

        Ok(())
    }

    /// Peek at the next message in the RX ring buffer.
    ///
    /// Returns a copy of the queue entry header if a message is available.
    /// Call [`recv_payload`](Self::recv_payload) to access the payload data,
    /// then [`recv_ack`](Self::recv_ack) to advance the read pointer.
    pub fn recv(&mut self) -> Option<AfkQueueEntry> {
        if !self.rx.ready {
            return None;
        }

        let rb = &self.rx;
        dma_invalidate(rb.hdr as *const u8, mem::size_of::<AfkRingBufferHeader>());
        let rptr = self.read_rptr(rb.hdr);
        let wptr = self.read_wptr(rb.hdr);
        if rptr == wptr || rptr as usize > rb.bufsz as usize - AfkQueueEntry::HEADER_SIZE {
            return None;
        }

        // Asahi Linux performs dma_rmb() here because its allocation is DMA
        // coherent. Scarlet's direct map is cacheable, so invalidate the queue
        // entry explicitly before inspecting it.
        dma_invalidate(
            unsafe { rb.buf.add(rptr as usize) } as *const u8,
            AfkQueueEntry::HEADER_SIZE,
        );
        unsafe {
            dma_mb();
        }
        let hdr_ptr = unsafe { rb.buf.add(rptr as usize) } as *const AfkQueueEntry;

        let magic = unsafe { core::ptr::addr_of!((*hdr_ptr).magic).read_volatile() };
        if magic != QE_MAGIC {
            return None;
        }

        let size = unsafe { core::ptr::addr_of!((*hdr_ptr).size).read_volatile() } as usize;

        // Handle wrap: if entry crosses buffer end, re-read from start
        if rptr as usize + AfkQueueEntry::HEADER_SIZE + size > rb.bufsz as usize {
            dma_invalidate(rb.buf as *const u8, AfkQueueEntry::HEADER_SIZE);
            unsafe {
                core::ptr::addr_of_mut!((*rb.hdr).rptr).write_volatile(0);
            }
            let wrapped = rb.buf as *const AfkQueueEntry;
            let wrapped_magic = unsafe { core::ptr::addr_of!((*wrapped).magic).read_volatile() };
            if wrapped_magic != QE_MAGIC {
                return None;
            }
        }

        // Read entry header (re-read in case of wrap)
        let final_rptr = self.read_rptr(rb.hdr);
        let final_hdr = unsafe { rb.buf.add(final_rptr as usize) } as *const AfkQueueEntry;
        let final_size = unsafe { core::ptr::addr_of!((*final_hdr).size).read_volatile() } as usize;
        if final_rptr as usize + AfkQueueEntry::HEADER_SIZE + final_size > rb.bufsz as usize {
            return None;
        }
        dma_invalidate(
            final_hdr as *const u8,
            AfkQueueEntry::HEADER_SIZE + final_size,
        );
        let entry = unsafe {
            let p = final_hdr;
            AfkQueueEntry {
                magic: core::ptr::addr_of!((*p).magic).read_volatile(),
                size: core::ptr::addr_of!((*p).size).read_volatile(),
                channel: core::ptr::addr_of!((*p).channel).read_volatile(),
                msg_type: core::ptr::addr_of!((*p).msg_type).read_volatile(),
            }
        };

        Some(entry)
    }

    /// Get the payload data for a received queue entry.
    ///
    /// The returned slice borrows `self` and is valid until
    /// [`recv_ack`](Self::recv_ack) is called.
    pub fn recv_payload(&self, entry: &AfkQueueEntry) -> &[u8] {
        let rb = &self.rx;
        let rptr = self.read_rptr(rb.hdr);

        let data_offset = if rptr as usize + AfkQueueEntry::HEADER_SIZE + entry.size as usize
            > rb.bufsz as usize
        {
            // Wrapped: payload is at buffer start + header size
            AfkQueueEntry::HEADER_SIZE
        } else {
            rptr as usize + AfkQueueEntry::HEADER_SIZE
        };

        unsafe { core::slice::from_raw_parts(rb.buf.add(data_offset), entry.size as usize) }
    }

    /// Acknowledge a received message and advance the RX read pointer.
    pub fn recv_ack(&mut self) {
        let rb = &self.rx;
        let rptr = self.read_rptr(rb.hdr);
        let hdr_ptr = unsafe { rb.buf.add(rptr as usize) } as *const AfkQueueEntry;

        let magic = unsafe { core::ptr::addr_of!((*hdr_ptr).magic).read_volatile() };
        if magic != QE_MAGIC {
            return;
        }

        unsafe {
            dma_mb();
        }

        let size = unsafe { core::ptr::addr_of!((*hdr_ptr).size).read_volatile() } as usize;
        let mut new_rptr = rptr as usize + AfkQueueEntry::HEADER_SIZE + size;
        new_rptr = block_align_up(new_rptr);
        if new_rptr >= rb.bufsz as usize {
            new_rptr = 0;
        }

        unsafe {
            core::ptr::addr_of_mut!((*rb.hdr).rptr).write_volatile(new_rptr as u32);
        }
        // SAFETY: a ready RX ring owns a valid shared header mapping.
        let rptr_ptr = unsafe { core::ptr::addr_of!((*rb.hdr).rptr) };
        dma_clean(rptr_ptr as *const u8, mem::size_of::<u32>());
    }

    /// Shutdown the AFK endpoint.
    pub fn shutdown(&mut self) -> Result<(), &'static str> {
        if !self.started {
            return Ok(());
        }

        self.send_rbep(RBEP_SHUTDOWN, 0)?;
        let _ = self.wait_rbep_type(RBEP_SHUTDOWN_ACK);
        self.started = false;
        early_println!("[apple-afk] ep {}: shutdown", self.ep);
        Ok(())
    }

    /// Get the underlying remoteproc service.
    pub fn service(&self) -> &Arc<dyn RemoteprocService> {
        &self.service
    }

    /// Get the RTKit endpoint number.
    pub fn endpoint(&self) -> u8 {
        self.ep
    }

    // =========================================================================
    // Private: RBEP handshake
    // =========================================================================

    fn send_rbep(&self, msg_type: u64, payload: u64) -> Result<(), &'static str> {
        self.service
            .send(&RemoteprocMessage::one(rbep_msg(msg_type, payload)))
            .map_err(remoteproc_error)
    }

    fn wait_rbep_type(&self, expected: u64) -> Result<u64, &'static str> {
        let start = time::current_time();
        loop {
            let elapsed = time::current_time().saturating_sub(start);
            if elapsed >= AFK_TIMEOUT_US {
                return Err("apple-afk: timeout waiting for RBEP message");
            }

            match self.recv_rbep() {
                Ok(Some(msg)) => {
                    if rbep_type(msg) == expected {
                        return Ok(msg);
                    }
                    early_println!(
                        "[apple-afk] ep {}: unexpected msg type={:#x} during handshake",
                        self.ep,
                        rbep_type(msg)
                    );
                }
                Ok(None) => time::udelay(AFK_POLL_DELAY_US),
                Err(_) => time::udelay(AFK_POLL_DELAY_US),
            }
        }
    }

    /// Handle GETBUF: allocate a shared DMA buffer and reply with its DVA.
    fn handle_getbuf(&mut self) -> Result<(), &'static str> {
        let msg = self.wait_rbep_type(RBEP_GETBUF)?;
        let size_blocks = field_get(msg, GETBUF_SIZE_MASK) as usize;
        let tag = field_get(msg, GETBUF_TAG_MASK) as u32;
        let size = size_blocks << BLOCK_SHIFT;

        let pages = (size + 4095) / 4096;
        let align_pages = self.remoteproc.dma_alignment().div_ceil(4096);
        let paddr = pmm::alloc_contiguous_pages_aligned(pages, align_pages)
            .ok_or("apple-afk: failed to allocate shared buffer")?;

        let virt = vm::phys_to_virt(paddr);

        // Zero the buffer
        unsafe {
            core::ptr::write_bytes(virt as *mut u8, 0, size);
        }

        // The kernel direct map is cacheable, unlike the coherent DMA mapping
        // used by Asahi Linux.  Hand ownership of the initialized buffer to
        // DCP explicitly so dirty zero-filled cache lines cannot hide firmware
        // ring-buffer initialization.
        dma_clean(virt as *const u8, size);

        let dva = match self.remoteproc.map_dma(paddr, size) {
            Ok(dva) => dva,
            Err(error) => {
                pmm::free_contiguous_pages(paddr, pages);
                return Err(remoteproc_error(error));
            }
        };

        early_println!(
            "[apple-afk] ep {}: shared buffer {} bytes at paddr={:#x}, dva={:#x}",
            self.ep,
            size,
            paddr,
            dva
        );

        self.shared = Some(AfkSharedBuffer {
            virt: virt as *mut u8,
            paddr,
            dva,
            size,
            tag,
        });

        self.send_rbep(RBEP_GETBUF_ACK, dva & GETBUF_ACK_DVA_MASK)
    }

    /// Wait for INIT_TX and INIT_RX messages and set up ring buffers.
    fn wait_init_ringbuffers(&mut self) -> Result<(), &'static str> {
        let mut tx_done = false;
        let mut rx_done = false;

        while !tx_done || !rx_done {
            let start = time::current_time();
            let mut got_msg = false;

            while !got_msg {
                let elapsed = time::current_time().saturating_sub(start);
                if elapsed >= AFK_TIMEOUT_US {
                    return Err("apple-afk: timeout waiting for INIT_TX/INIT_RX");
                }

                match self.recv_rbep() {
                    Ok(Some(msg)) => {
                        got_msg = true;
                        match rbep_type(msg) {
                            RBEP_INIT_TX => {
                                let shared =
                                    self.shared.as_ref().ok_or("apple-afk: no shared buffer")?;
                                Self::init_ring_buffer(&mut self.tx, shared, msg, "TX")?;
                                tx_done = true;
                            }
                            RBEP_INIT_RX => {
                                let shared =
                                    self.shared.as_ref().ok_or("apple-afk: no shared buffer")?;
                                Self::init_ring_buffer(&mut self.rx, shared, msg, "RX")?;
                                rx_done = true;
                            }
                            _ => {
                                early_println!(
                                    "[apple-afk] ep {}: unexpected type={:#x}",
                                    self.ep,
                                    rbep_type(msg)
                                );
                            }
                        }
                    }
                    Ok(None) => time::udelay(AFK_POLL_DELAY_US),
                    Err(_) => time::udelay(AFK_POLL_DELAY_US),
                }
            }
        }

        if !self.tx.ready || !self.rx.ready {
            return Err("apple-afk: ring buffers not initialized");
        }

        Ok(())
    }

    /// Initialize one ring buffer (TX or RX) from an INIT_TX/INIT_RX message.
    fn init_ring_buffer(
        rb: &mut AfkRingBuffer,
        shared: &AfkSharedBuffer,
        msg: u64,
        label: &str,
    ) -> Result<(), &'static str> {
        let offset = field_get(msg, INITRB_OFFSET_MASK) as usize;
        let size = field_get(msg, INITRB_SIZE_MASK) as usize;
        let tag = field_get(msg, INITRB_TAG_MASK) as u32;

        if tag != shared.tag {
            return Err("apple-afk: ring buffer tag mismatch");
        }

        let base = offset << BLOCK_SHIFT;
        let total_size = size << BLOCK_SHIFT;

        if base + total_size > shared.size {
            return Err("apple-afk: ring buffer out of bounds");
        }

        let hdr_ptr = unsafe { shared.virt.add(base) } as *mut AfkRingBufferHeader;
        dma_invalidate(hdr_ptr as *const u8, mem::size_of::<AfkRingBufferHeader>());
        let hdr_bufsz = unsafe { core::ptr::addr_of!((*hdr_ptr).bufsz).read_volatile() } as usize;

        // Validate: bufsz + header size == total ring buffer size
        if hdr_bufsz + mem::size_of::<AfkRingBufferHeader>() != total_size {
            return Err("apple-afk: ring buffer size mismatch");
        }

        let buf_ptr = unsafe { hdr_ptr.add(1) } as *mut u8;

        rb.hdr = hdr_ptr;
        rb.buf = buf_ptr;
        rb.bufsz = hdr_bufsz as u32;
        rb.ready = true;

        early_println!(
            "[apple-afk] ep {}: {} ring buffer at +{:#x}, data={:#x} bytes",
            label,
            label,
            base,
            hdr_bufsz
        );

        Ok(())
    }

    // =========================================================================
    // Private: ring buffer helpers
    // =========================================================================

    fn read_rptr(&self, hdr: *mut AfkRingBufferHeader) -> u32 {
        // SAFETY: hdr points to a valid, DMA-shared ring buffer header.
        unsafe { core::ptr::addr_of!((*hdr).rptr).read_volatile() }
    }

    fn read_wptr(&self, hdr: *mut AfkRingBufferHeader) -> u32 {
        // SAFETY: hdr points to a valid, DMA-shared ring buffer header.
        unsafe { core::ptr::addr_of!((*hdr).wptr).read_volatile() }
    }

    /// Check whether `needed` bytes can be written at the current position.
    fn has_space(&self, wptr: u32, rptr: u32, needed: usize, bufsz: usize) -> bool {
        let w = wptr as usize;
        let r = rptr as usize;

        if w < r {
            // Wrapped: space between wptr and rptr
            needed < r - w
        } else {
            // Not wrapped: check if fits above wptr, or after wrapping to start
            let space_above = bufsz - w;
            let fits_above = needed < space_above || (needed == space_above && r != 0);
            if fits_above {
                true
            } else {
                // Try fitting after wrap
                needed < r
            }
        }
    }

    fn recv_rbep(&self) -> Result<Option<u64>, &'static str> {
        self.service
            .try_recv()
            .map(|message| message.map(|message| message.words[0]))
            .map_err(remoteproc_error)
    }

    /// Drain pending RBEP notifications from the RTKit mailbox.
    ///
    /// The coprocessor sends an RBEP_RECV for every write to the RX ring
    /// buffer. These must be consumed to keep the mailbox from filling up
    /// and to provide an implicit DMA read barrier before the AP inspects
    /// the ring buffer contents.
    pub fn drain_rbep(&self) {
        while let Ok(Some(msg)) = self.recv_rbep() {
            let msg_type = rbep_type(msg);
            if msg_type != RBEP_RECV {
                early_println!(
                    "[apple-afk] ep {}: unexpected runtime RBEP type={:#x}",
                    self.ep,
                    msg_type
                );
            }
        }
    }
}

fn remoteproc_error(error: RemoteprocError) -> &'static str {
    match error {
        RemoteprocError::InvalidState => "apple-afk: remoteproc invalid state",
        RemoteprocError::FirmwareMissing => "apple-afk: remoteproc firmware missing",
        RemoteprocError::LoadFailed => "apple-afk: remoteproc load failed",
        RemoteprocError::BootFailed => "apple-afk: remoteproc boot failed",
        RemoteprocError::ShutdownFailed => "apple-afk: remoteproc shutdown failed",
        RemoteprocError::Crashed => "apple-afk: remoteproc crashed",
        RemoteprocError::ServiceNotFound => "apple-afk: remoteproc service not found",
        RemoteprocError::TransportError => "apple-afk: remoteproc transport error",
        RemoteprocError::NotSupported => "apple-afk: remoteproc operation not supported",
        RemoteprocError::Busy => "apple-afk: remoteproc busy",
    }
}

impl Drop for AfkEndpoint {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.take() {
            self.remoteproc.unmap_dma(shared.dva, shared.size);
            let pages = (shared.size + 4095) / 4096;
            pmm::free_contiguous_pages(shared.paddr, pages);
            early_println!(
                "[apple-afk] ep {}: freed shared buffer at {:#x}",
                self.ep,
                shared.paddr
            );
        }
    }
}

#[used]
static SCARLET_DRIVER_APPLE_AFK_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
