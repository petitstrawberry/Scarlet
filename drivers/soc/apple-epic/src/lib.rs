#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem;

use scarlet::device::remoteproc::RemoteProcessor;
use scarlet::early_println;
use scarlet::mem::pmm;
use scarlet::sync::Mutex;
use scarlet::time;
use scarlet::vm;
use scarlet_driver_apple_afk::AfkEndpoint;

// =============================================================================
// Constants
// =============================================================================

/// EPIC header version.
const EPIC_HDR_VERSION: u8 = 2;
/// EPIC sub-header version.
const EPIC_SUBHDR_VERSION: u8 = 4;

/// DMA buffer size per EPIC endpoint (16 KB).
const EPIC_BUFFER_SIZE: usize = 0x4000;

/// Maximum number of channels per endpoint.
pub const EPIC_MAX_CHANNELS: usize = 8;

/// Service call magic value: `"epcx"`.
const EPIC_SERVICE_CALL_MAGIC: u32 = 0x6970_6378;

/// Timeout for EPIC command reply in microseconds.
const EPIC_REPLY_TIMEOUT_US: u64 = 5_000_000;

// EPIC message types (used as AFK queue entry msg_type).
const TYPE_NOTIFY: u32 = 0;
const TYPE_COMMAND: u32 = 3;
const TYPE_REPLY: u32 = 4;
const TYPE_NOTIFY_ACK: u32 = 8;

// EPIC categories.
const CAT_REPORT: u8 = 0x00;
const CAT_NOTIFY: u8 = 0x10;
const CAT_REPLY: u8 = 0x20;
const CAT_COMMAND: u8 = 0x30;

// EPIC subtypes.
const SUBTYPE_ANNOUNCE: u16 = 0x30;
const SUBTYPE_TEARDOWN: u16 = 0x32;
const SUBTYPE_RETCODE: u16 = 0x84;
const SUBTYPE_STRING: u16 = 0x8a;
const SUBTYPE_STD_SERVICE: u16 = 0xc0;

// Flags.
const FLAG_INLINE: u8 = 0x08;

#[inline(always)]
fn dma_clean(vaddr: usize, len: usize) {
    scarlet::arch::clean_dcache_to_poc_range(vaddr, len);
}

#[inline(always)]
fn dma_invalidate(vaddr: usize, len: usize) {
    scarlet::arch::invalidate_dcache_to_poc_range(vaddr, len);
}

// =============================================================================
// Wire Structures (packed, little-endian, DMA-shared)
// =============================================================================

/// EPIC message header — precedes every EPIC payload in the ring buffer.
#[repr(C, packed)]
struct EpicHdr {
    version: u8,
    seq: u16,
    _pad: u8,
    unk: u32,
    timestamp: u64,
}

/// EPIC sub-header — follows `EpicHdr`, describes the message category/type.
#[repr(C, packed)]
struct EpicSubHdr {
    length: u32,
    version: u8,
    category: u8,
    msg_type: u16,
    timestamp: u64,
    seq: u16,
    unk: u8,
    flags: u8,
    inline_len: u32,
}

/// EPIC command/reply payload — carries DMA buffer addresses and sizes.
#[repr(C, packed)]
struct EpicCmd {
    retcode: u32,
    rxbuf: u64,
    txbuf: u64,
    rxlen: u32,
    txlen: u32,
    rxcookie: u8,
    txcookie: u8,
}

/// EPIC service call header — used for `SUBTYPE_STD_SERVICE` messages.
#[repr(C, packed)]
struct EpicServiceCall {
    _pad0: [u8; 2],
    group: u16,
    command: u32,
    data_len: u32,
    magic: u32,
    _pad1: [u8; 48],
}

/// EPIC service announcement payload.
///
/// Serialized DCP properties follow the name field.
#[repr(C, packed)]
struct EpicAnnounce {
    name: [u8; 32],
}

// =============================================================================
// DMA Buffer
// =============================================================================

/// Per-endpoint DMA buffer pair for command/reply data exchange.
struct EpicDmaBuffer {
    /// Kernel virtual address of TX buffer (host → coprocessor).
    tx_virt: usize,
    /// CPU physical address of the TX buffer.
    tx_paddr: usize,
    /// Device virtual address of the TX buffer.
    tx_dva: u64,
    /// Kernel virtual address of RX buffer (coprocessor → host).
    rx_virt: usize,
    /// CPU physical address of the RX buffer.
    rx_paddr: usize,
    /// Device virtual address of the RX buffer.
    rx_dva: u64,
    remoteproc: Arc<dyn RemoteProcessor>,
}

impl EpicDmaBuffer {
    fn alloc(remoteproc: Arc<dyn RemoteProcessor>) -> Result<Self, &'static str> {
        let pages = (EPIC_BUFFER_SIZE + 4095) / 4096;

        let align_pages = remoteproc.dma_alignment().div_ceil(4096);
        let tx_paddr = pmm::alloc_contiguous_pages_aligned(pages, align_pages)
            .ok_or("apple-epic: failed to allocate TX DMA buffer")?;
        let rx_paddr = match pmm::alloc_contiguous_pages_aligned(pages, align_pages) {
            Some(paddr) => paddr,
            None => {
                pmm::free_contiguous_pages(tx_paddr, pages);
                return Err("apple-epic: failed to allocate RX DMA buffer");
            }
        };

        let tx_virt = vm::phys_to_virt(tx_paddr);
        let rx_virt = vm::phys_to_virt(rx_paddr);

        // Zero buffers
        unsafe {
            core::ptr::write_bytes(tx_virt as *mut u8, 0, EPIC_BUFFER_SIZE);
            core::ptr::write_bytes(rx_virt as *mut u8, 0, EPIC_BUFFER_SIZE);
        }
        dma_clean(tx_virt, EPIC_BUFFER_SIZE);
        dma_clean(rx_virt, EPIC_BUFFER_SIZE);

        let tx_dva = match remoteproc.map_dma(tx_paddr, EPIC_BUFFER_SIZE) {
            Ok(dva) => dva,
            Err(_) => {
                pmm::free_contiguous_pages(tx_paddr, pages);
                pmm::free_contiguous_pages(rx_paddr, pages);
                return Err("apple-epic: failed to map TX DMA buffer");
            }
        };
        let rx_dva = match remoteproc.map_dma(rx_paddr, EPIC_BUFFER_SIZE) {
            Ok(dva) => dva,
            Err(_) => {
                remoteproc.unmap_dma(tx_dva, EPIC_BUFFER_SIZE);
                pmm::free_contiguous_pages(tx_paddr, pages);
                pmm::free_contiguous_pages(rx_paddr, pages);
                return Err("apple-epic: failed to map RX DMA buffer");
            }
        };

        Ok(Self {
            tx_virt,
            tx_paddr,
            tx_dva,
            rx_virt,
            rx_paddr,
            rx_dva,
            remoteproc,
        })
    }
}

impl Drop for EpicDmaBuffer {
    fn drop(&mut self) {
        self.remoteproc.unmap_dma(self.tx_dva, EPIC_BUFFER_SIZE);
        self.remoteproc.unmap_dma(self.rx_dva, EPIC_BUFFER_SIZE);
        let pages = (EPIC_BUFFER_SIZE + 4095) / 4096;
        pmm::free_contiguous_pages(self.tx_paddr, pages);
        pmm::free_contiguous_pages(self.rx_paddr, pages);
    }
}

// =============================================================================
// EPIC Service
// =============================================================================

/// A discovered EPIC service on a specific channel.
pub struct EpicService {
    /// Service name (e.g., `"dcpdptx-port-epic"`).
    pub name: String,
    /// Channel number within the AFK endpoint.
    pub channel: u32,
    /// Sequence number for outgoing messages.
    seq: u16,
}

// =============================================================================
// EPIC Endpoint
// =============================================================================

/// EPIC endpoint — manages service discovery and command/reply messaging
/// on top of an AFK endpoint.
pub struct EpicEndpoint {
    afk: Arc<Mutex<AfkEndpoint>>,
    dma: EpicDmaBuffer,
    /// Outgoing EPIC-level sequence number.
    seq: u16,
    /// Discovered services keyed by channel.
    services: Vec<EpicService>,
    /// Callback for received notifications (channel, subtype, data).
    notify_handler: Option<fn(u32, u16, &[u8])>,
}

impl EpicEndpoint {
    /// Create a new EPIC endpoint over a remote processor service.
    ///
    /// # Arguments
    ///
    /// * `remoteproc` - Remote processor exposing the AFK RTKit endpoint as a service.
    /// * `endpoint` - RTKit endpoint number used by the EPIC-over-AFK protocol.
    ///
    /// # Returns
    ///
    /// A started EPIC endpoint ready for service discovery and commands.
    pub fn new(remoteproc: Arc<dyn RemoteProcessor>, endpoint: u8) -> Result<Self, &'static str> {
        let afk = Arc::new(Mutex::new(AfkEndpoint::new(remoteproc, endpoint)?));
        afk.lock().start()?;
        Self::from_afk(afk)
    }

    /// Create a new EPIC endpoint wrapping an existing AFK endpoint.
    ///
    /// The AFK endpoint must already be started before use.
    ///
    /// # Arguments
    ///
    /// * `afk` - Started AFK endpoint carrying EPIC messages.
    ///
    /// # Returns
    ///
    /// An EPIC endpoint using the supplied AFK transport.
    pub fn from_afk(afk: Arc<Mutex<AfkEndpoint>>) -> Result<Self, &'static str> {
        let remoteproc = afk.lock().remoteproc();
        let dma = EpicDmaBuffer::alloc(remoteproc)?;

        early_println!(
            "[apple-epic] DMA buffers: TX={:#x} RX={:#x} ({} bytes each)",
            dma.tx_dva,
            dma.rx_dva,
            EPIC_BUFFER_SIZE
        );

        Ok(Self {
            afk,
            dma,
            seq: 0,
            services: Vec::new(),
            notify_handler: None,
        })
    }

    /// Register a notification handler callback.
    ///
    /// Called when a `TYPE_NOTIFY` is received that is not a service announcement.
    pub fn set_notify_handler(&mut self, handler: fn(u32, u16, &[u8])) {
        self.notify_handler = Some(handler);
    }

    /// Poll for incoming messages and dispatch them.
    ///
    /// Call this periodically or from an interrupt handler. Handles:
    /// - Service announcements (`CAT_REPORT` + `SUBTYPE_ANNOUNCE`)
    /// - Notifications (`TYPE_NOTIFY`)
    /// - Command replies (matched to pending commands)
    pub fn poll(&mut self) {
        self.afk.lock().drain_rbep();

        loop {
            let action: Option<(u32, u32, Vec<u8>)> = {
                let mut afk = self.afk.lock();

                match afk.recv() {
                    Some(entry) => {
                        let payload = afk.recv_payload(&entry).to_vec();
                        let action = (entry.msg_type, entry.channel, payload);
                        afk.recv_ack();
                        Some(action)
                    }
                    None => None,
                }
            };

            match action {
                Some((msg_type, channel, payload)) => match msg_type {
                    TYPE_NOTIFY => {
                        self.handle_notify(channel, &payload);
                    }
                    TYPE_REPLY => {
                        // DCP firmware may publish a service announcement as
                        // either NOTIFY or REPLY. Asahi Linux accepts both for
                        // channels that have not been registered yet.
                        if self.is_service_announce(&payload) {
                            self.handle_notify(channel, &payload);
                        } else {
                            self.handle_notify_ack(channel, &payload);
                        }
                    }
                    _ => {
                        early_println!(
                            "[apple-epic] unhandled msg type={} ch={}",
                            msg_type,
                            channel
                        );
                    }
                },
                None => break,
            }
        }
    }

    /// Wait for service announcements and collect discovered services.
    ///
    /// Blocks until at least `min_services` services are discovered or
    /// `timeout_us` elapses.
    pub fn wait_for_services(
        &mut self,
        min_services: usize,
        timeout_us: u64,
    ) -> Result<(), &'static str> {
        let start = time::current_time();

        loop {
            if self.services.len() >= min_services {
                return Ok(());
            }

            if time::current_time().saturating_sub(start) >= timeout_us {
                early_println!(
                    "[apple-epic] timeout: found {} services, wanted {}",
                    self.services.len(),
                    min_services
                );
                if self.services.len() > 0 {
                    return Ok(());
                }
                return Err("apple-epic: no services discovered");
            }

            self.poll();
            // Small yield to avoid busy-polling
            for _ in 0..100 {
                core::hint::spin_loop();
            }
        }
    }

    /// Find a discovered service by name (prefix match).
    pub fn find_service(&self, name_prefix: &str) -> Option<&EpicService> {
        self.services
            .iter()
            .find(|s| s.name.starts_with(name_prefix))
    }

    /// Find a discovered service by channel number.
    pub fn find_service_by_channel(&self, channel: u32) -> Option<&EpicService> {
        self.services.iter().find(|s| s.channel == channel)
    }

    /// Return the first announced service channel.
    ///
    /// Some Apple firmware revisions put the matchable class in serialized
    /// announcement properties rather than in the fixed name field. Endpoints
    /// that define exactly one service can use this as a compatible fallback.
    ///
    /// # Returns
    ///
    /// First service channel, or `None` before any announcement is received.
    pub fn first_service_channel(&self) -> Option<u32> {
        self.services.first().map(|service| service.channel)
    }

    /// Get the list of discovered service names.
    pub fn service_names(&self) -> Vec<&str> {
        self.services.iter().map(|s| s.name.as_str()).collect()
    }

    /// Send a standard service call (RPC command) and wait for the reply.
    ///
    /// This is the primary interface for communicating with DCP services.
    ///
    /// # Arguments
    ///
    /// * `service` - Target service (channel number)
    /// * `group` - Service call group ID
    /// * `command` - Service call command ID
    /// * `data` - Request payload
    ///
    /// # Returns
    ///
    /// Reply payload on success, error string on failure.
    pub fn call(
        &mut self,
        service: &EpicService,
        group: u16,
        command: u32,
        data: &[u8],
    ) -> Result<Vec<u8>, &'static str> {
        self.call_by_channel(service.channel, group, command, data)
    }

    /// Like [`Self::call`] but accepts a channel number directly,
    /// avoiding the borrow conflict of passing `&EpicService` alongside `&mut self`.
    pub fn call_by_channel(
        &mut self,
        channel: u32,
        group: u16,
        command: u32,
        data: &[u8],
    ) -> Result<Vec<u8>, &'static str> {
        self.send_command(channel, group, command, data)?;
        self.wait_reply(channel, SUBTYPE_STD_SERVICE)
    }

    /// Send a raw EPIC command and wait for its DMA reply.
    ///
    /// Unlike [`Self::call_by_channel`], this does not prepend the standard
    /// service-call header. DCP's iBoot display service uses subtype `0xc0`
    /// with its own command header in the DMA buffer.
    ///
    /// # Arguments
    ///
    /// * `channel` - Announced EPIC service channel.
    /// * `subtype` - EPIC command subtype.
    /// * `data` - Complete service-specific DMA payload.
    ///
    /// # Returns
    ///
    /// Raw reply bytes copied from the endpoint RX buffer.
    pub fn call_raw_by_channel(
        &mut self,
        channel: u32,
        subtype: u16,
        data: &[u8],
    ) -> Result<Vec<u8>, &'static str> {
        self.send_raw_command(channel, subtype, data)?;
        self.wait_reply(channel, subtype)
    }

    /// Send a standard service call without waiting for a reply.
    pub fn send_command(
        &mut self,
        channel: u32,
        group: u16,
        command: u32,
        data: &[u8],
    ) -> Result<(), &'static str> {
        let total = mem::size_of::<EpicServiceCall>() + data.len();
        if total > EPIC_BUFFER_SIZE {
            return Err("apple-epic: command data too large");
        }

        let call = EpicServiceCall {
            _pad0: [0; 2],
            group,
            command,
            data_len: data.len() as u32,
            magic: EPIC_SERVICE_CALL_MAGIC,
            _pad1: [0; 48],
        };

        // Write service call header + data to TX DMA buffer
        unsafe {
            let dst = self.dma.tx_virt as *mut u8;
            core::ptr::copy_nonoverlapping(
                &call as *const EpicServiceCall as *const u8,
                dst,
                mem::size_of::<EpicServiceCall>(),
            );
            if !data.is_empty() {
                core::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    dst.add(mem::size_of::<EpicServiceCall>()),
                    data.len(),
                );
            }
        }
        dma_clean(self.dma.tx_virt, total);

        self.send_dma_command(channel, SUBTYPE_STD_SERVICE, total, None)
    }

    fn send_raw_command(
        &mut self,
        channel: u32,
        subtype: u16,
        data: &[u8],
    ) -> Result<(), &'static str> {
        if data.len() > EPIC_BUFFER_SIZE {
            return Err("apple-epic: raw command data too large");
        }

        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), self.dma.tx_virt as *mut u8, data.len());
        }
        dma_clean(self.dma.tx_virt, data.len());
        self.send_dma_command(channel, subtype, data.len(), Some(0))
    }

    fn send_dma_command(
        &mut self,
        channel: u32,
        subtype: u16,
        tx_len: usize,
        fixed_sub_seq: Option<u16>,
    ) -> Result<(), &'static str> {
        let hdr_size =
            mem::size_of::<EpicHdr>() + mem::size_of::<EpicSubHdr>() + mem::size_of::<EpicCmd>();

        let seq = self.next_seq();
        let sub_seq = fixed_sub_seq.unwrap_or(seq);
        let mut msg_buf = alloc::vec![0u8; hdr_size];
        self.write_epic_headers(
            &mut msg_buf,
            CAT_COMMAND,
            subtype,
            seq,
            sub_seq,
            mem::size_of::<EpicCmd>() as u32,
        );

        // Write EpicCmd after headers
        let cmd = EpicCmd {
            retcode: 0,
            rxbuf: self.dma.rx_dva,
            txbuf: self.dma.tx_dva,
            rxlen: EPIC_BUFFER_SIZE as u32,
            txlen: tx_len as u32,
            rxcookie: 0,
            txcookie: 0,
        };
        unsafe {
            core::ptr::copy_nonoverlapping(
                &cmd as *const EpicCmd as *const u8,
                msg_buf
                    .as_mut_ptr()
                    .add(mem::size_of::<EpicHdr>() + mem::size_of::<EpicSubHdr>()),
                mem::size_of::<EpicCmd>(),
            );
        }

        let mut afk = self.afk.lock();
        afk.send(channel, TYPE_COMMAND, &msg_buf)?;

        Ok(())
    }

    /// Wait for a reply on the specified channel.
    fn wait_reply(&mut self, channel: u32, expected_subtype: u16) -> Result<Vec<u8>, &'static str> {
        let start = time::current_time();

        loop {
            if time::current_time().saturating_sub(start) >= EPIC_REPLY_TIMEOUT_US {
                return Err("apple-epic: timeout waiting for command reply");
            }

            // Match Asahi Linux's AFK command loop: consume the RTKit
            // notification before inspecting the DMA ring. Besides providing
            // the ownership handoff, this acknowledges the ASC mailbox so a
            // following command can receive its own RBEP_RECV notification.
            self.afk.lock().drain_rbep();

            loop {
                let action: Option<(u32, u32, Vec<u8>)> = {
                    let mut afk = self.afk.lock();
                    match afk.recv() {
                        Some(entry) => {
                            let payload = afk.recv_payload(&entry).to_vec();
                            afk.recv_ack();
                            Some((entry.msg_type, entry.channel, payload))
                        }
                        None => None,
                    }
                };

                match action {
                    Some((msg_type, ch, payload)) => {
                        if ch == channel
                            && msg_type == TYPE_REPLY
                            && self.is_command_reply(&payload, expected_subtype)
                        {
                            return self.parse_reply(&payload);
                        }
                        if msg_type == TYPE_NOTIFY
                            || (msg_type == TYPE_REPLY && self.is_service_announce(&payload))
                        {
                            self.handle_notify(ch, &payload);
                        } else if ch == channel {
                            early_println!(
                                "[apple-epic] unexpected type={} on ch={} while waiting for subtype={:#x}",
                                msg_type,
                                channel,
                                expected_subtype
                            );
                        }
                    }
                    None => break,
                }
            }

            for _ in 0..100 {
                core::hint::spin_loop();
            }
        }
    }

    /// Get the underlying AFK endpoint reference.
    pub fn afk(&self) -> &Arc<Mutex<AfkEndpoint>> {
        &self.afk
    }

    // =========================================================================
    // Private: header construction
    // =========================================================================

    fn next_seq(&mut self) -> u16 {
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        seq
    }

    fn write_epic_headers(
        &self,
        buf: &mut [u8],
        category: u8,
        msg_type: u16,
        header_seq: u16,
        sub_seq: u16,
        payload_len: u32,
    ) {
        let hdr = EpicHdr {
            version: EPIC_HDR_VERSION,
            seq: header_seq,
            _pad: 0,
            unk: 0,
            timestamp: 0,
        };

        let sub = EpicSubHdr {
            length: payload_len,
            version: EPIC_SUBHDR_VERSION,
            category,
            msg_type,
            timestamp: 0,
            seq: sub_seq,
            unk: 0,
            flags: 0,
            inline_len: 0,
        };

        unsafe {
            core::ptr::copy_nonoverlapping(
                &hdr as *const EpicHdr as *const u8,
                buf.as_mut_ptr(),
                mem::size_of::<EpicHdr>(),
            );
            core::ptr::copy_nonoverlapping(
                &sub as *const EpicSubHdr as *const u8,
                buf.as_mut_ptr().add(mem::size_of::<EpicHdr>()),
                mem::size_of::<EpicSubHdr>(),
            );
        }
    }

    // =========================================================================
    // Private: message handling
    // =========================================================================

    fn is_service_announce(&self, payload: &[u8]) -> bool {
        if payload.len() < mem::size_of::<EpicHdr>() + mem::size_of::<EpicSubHdr>() {
            return false;
        }

        let sub: EpicSubHdr = unsafe {
            core::ptr::read_unaligned(
                payload.as_ptr().add(mem::size_of::<EpicHdr>()) as *const EpicSubHdr
            )
        };
        sub.category == CAT_REPORT && sub.msg_type == SUBTYPE_ANNOUNCE
    }

    fn is_command_reply(&self, payload: &[u8], expected_subtype: u16) -> bool {
        if payload.len() < mem::size_of::<EpicHdr>() + mem::size_of::<EpicSubHdr>() {
            return false;
        }

        let sub: EpicSubHdr = unsafe {
            core::ptr::read_unaligned(
                payload.as_ptr().add(mem::size_of::<EpicHdr>()) as *const EpicSubHdr
            )
        };
        sub.category == CAT_REPLY && sub.msg_type == expected_subtype
    }

    fn handle_notify(&mut self, channel: u32, payload: &[u8]) {
        if payload.len() < mem::size_of::<EpicHdr>() + mem::size_of::<EpicSubHdr>() {
            return;
        }

        let sub: EpicSubHdr = unsafe {
            core::ptr::read_unaligned(
                payload.as_ptr().add(mem::size_of::<EpicHdr>()) as *const EpicSubHdr
            )
        };

        match (sub.category, sub.msg_type) {
            (CAT_REPORT, SUBTYPE_ANNOUNCE) => {
                self.handle_announce(channel, payload);
            }
            (CAT_NOTIFY, SUBTYPE_STD_SERVICE) => {
                // Standard service notification — forward to handler
                if let Some(handler) = self.notify_handler {
                    let data_start = mem::size_of::<EpicHdr>() + mem::size_of::<EpicSubHdr>();
                    if payload.len() > data_start {
                        handler(channel, sub.msg_type, &payload[data_start..]);
                    }
                }
            }
            _ => {
                if let Some(handler) = self.notify_handler {
                    let data_start = mem::size_of::<EpicHdr>() + mem::size_of::<EpicSubHdr>();
                    if payload.len() > data_start {
                        handler(channel, sub.msg_type, &payload[data_start..]);
                    }
                }
            }
        }
    }

    fn handle_announce(&mut self, channel: u32, payload: &[u8]) {
        let announce_offset = mem::size_of::<EpicHdr>() + mem::size_of::<EpicSubHdr>();

        if payload.len() < announce_offset + mem::size_of::<EpicAnnounce>() {
            return;
        }

        let announce: EpicAnnounce = unsafe {
            core::ptr::read_unaligned(payload.as_ptr().add(announce_offset) as *const EpicAnnounce)
        };

        let name_bytes: &[u8] = &announce.name;
        let name_len = name_bytes
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(name_bytes.len());
        let name = String::from_utf8_lossy(&name_bytes[..name_len]).into_owned();

        // Check if we already have this service on this channel
        if self.services.iter().any(|s| s.channel == channel) {
            return;
        }

        early_println!("[apple-epic] service '{}' on channel {}", name, channel);

        self.services.push(EpicService {
            name,
            channel,
            seq: 0,
        });
    }

    fn handle_notify_ack(&mut self, channel: u32, payload: &[u8]) {
        if payload.len() < mem::size_of::<EpicHdr>() + mem::size_of::<EpicSubHdr>() {
            return;
        }

        let sub: EpicSubHdr = unsafe {
            core::ptr::read_unaligned(
                payload.as_ptr().add(mem::size_of::<EpicHdr>()) as *const EpicSubHdr
            )
        };

        let ack_size = mem::size_of::<EpicHdr>() + mem::size_of::<EpicSubHdr>();
        let mut ack = alloc::vec![0u8; ack_size];

        self.write_epic_headers(&mut ack, CAT_REPLY, sub.msg_type, sub.seq, sub.seq, 0);

        let mut afk = self.afk.lock();
        let _ = afk.send(channel, TYPE_NOTIFY_ACK, &ack);
    }

    /// Parse a reply message and extract the response payload.
    fn parse_reply(&self, payload: &[u8]) -> Result<Vec<u8>, &'static str> {
        let hdr_offset = mem::size_of::<EpicHdr>() + mem::size_of::<EpicSubHdr>();

        if payload.len() < hdr_offset + mem::size_of::<EpicCmd>() {
            return Err("apple-epic: reply too short for command struct");
        }

        let cmd: EpicCmd = unsafe {
            core::ptr::read_unaligned(payload.as_ptr().add(hdr_offset) as *const EpicCmd)
        };

        if cmd.retcode != 0 {
            let retcode = cmd.retcode;
            early_println!("[apple-epic: command failed with retcode={:#x}", retcode);
            return Err("apple-epic: command returned non-zero retcode");
        }

        let reported_rx_len = cmd.rxlen as usize;
        if reported_rx_len > EPIC_BUFFER_SIZE {
            return Err("apple-epic: reply rxlen exceeds buffer size");
        }

        // The DCP writes reply data through DMA into Scarlet's cacheable
        // direct map. Invalidate it after the inline command reply publishes
        // the completed length and before the CPU reads the response body.
        //
        // Some DCP firmware leaves EpicCmd.rxlen as zero. Asahi Linux does not
        // use that returned field to limit the copy: it copies the output size
        // supplied by the caller. Keep the reported length when present, but
        // fall back to the allocated RX buffer so service-specific protocols
        // such as iBoot can use their own embedded response length.
        let rx_len = if reported_rx_len == 0 {
            EPIC_BUFFER_SIZE
        } else {
            reported_rx_len
        };
        dma_invalidate(self.dma.rx_virt, rx_len);

        let reply_data =
            unsafe { core::slice::from_raw_parts(self.dma.rx_virt as *const u8, rx_len) };

        Ok(reply_data.to_vec())
    }
}

#[used]
static SCARLET_DRIVER_APPLE_EPIC_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
