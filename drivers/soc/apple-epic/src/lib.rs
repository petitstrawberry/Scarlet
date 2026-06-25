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
    /// Physical address (DVA) of TX buffer.
    tx_paddr: usize,
    /// Kernel virtual address of RX buffer (coprocessor → host).
    rx_virt: usize,
    /// Physical address (DVA) of RX buffer.
    rx_paddr: usize,
}

impl EpicDmaBuffer {
    fn alloc() -> Result<Self, &'static str> {
        let pages = (EPIC_BUFFER_SIZE + 4095) / 4096;

        let tx_paddr = pmm::alloc_contiguous_pages(pages)
            .ok_or("apple-epic: failed to allocate TX DMA buffer")?;
        let rx_paddr = pmm::alloc_contiguous_pages(pages)
            .ok_or("apple-epic: failed to allocate RX DMA buffer")?;

        let tx_virt = vm::phys_to_virt(tx_paddr);
        let rx_virt = vm::phys_to_virt(rx_paddr);

        // Zero buffers
        unsafe {
            core::ptr::write_bytes(tx_virt as *mut u8, 0, EPIC_BUFFER_SIZE);
            core::ptr::write_bytes(rx_virt as *mut u8, 0, EPIC_BUFFER_SIZE);
        }

        Ok(Self {
            tx_virt,
            tx_paddr,
            rx_virt,
            rx_paddr,
        })
    }
}

impl Drop for EpicDmaBuffer {
    fn drop(&mut self) {
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
        let dma = EpicDmaBuffer::alloc()?;

        early_println!(
            "[apple-epic] DMA buffers: TX={:#x} RX={:#x} ({} bytes each)",
            dma.tx_paddr,
            dma.rx_paddr,
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
                        self.handle_notify_ack(channel, &payload);
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
        self.wait_reply(channel)
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

        let hdr_size =
            mem::size_of::<EpicHdr>() + mem::size_of::<EpicSubHdr>() + mem::size_of::<EpicCmd>();
        let tx_data = unsafe { core::slice::from_raw_parts(self.dma.tx_virt as *const u8, total) };

        let seq = self.next_seq();
        let mut msg_buf = alloc::vec![0u8; hdr_size + tx_data.len()];
        self.write_epic_headers(
            &mut msg_buf,
            CAT_COMMAND,
            SUBTYPE_STD_SERVICE,
            seq,
            tx_data.len() as u32,
        );

        // Write EpicCmd after headers
        let cmd = EpicCmd {
            retcode: 0,
            rxbuf: self.dma.rx_paddr as u64,
            txbuf: self.dma.tx_paddr as u64,
            rxlen: EPIC_BUFFER_SIZE as u32,
            txlen: total as u32,
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

        // Append TX data
        msg_buf[hdr_size..].copy_from_slice(tx_data);

        let mut afk = self.afk.lock();
        afk.send(channel, TYPE_COMMAND, &msg_buf)?;

        Ok(())
    }

    /// Wait for a reply on the specified channel.
    fn wait_reply(&mut self, channel: u32) -> Result<Vec<u8>, &'static str> {
        let start = time::current_time();

        loop {
            if time::current_time().saturating_sub(start) >= EPIC_REPLY_TIMEOUT_US {
                return Err("apple-epic: timeout waiting for command reply");
            }

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
                        if ch == channel && msg_type == TYPE_REPLY {
                            return self.parse_reply(&payload);
                        }
                        if msg_type == TYPE_NOTIFY {
                            self.handle_notify(ch, &payload);
                        } else if ch == channel {
                            early_println!(
                                "[apple-epic] unexpected type={} on ch={}",
                                msg_type,
                                channel
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
        seq: u16,
        payload_len: u32,
    ) {
        let hdr = EpicHdr {
            version: EPIC_HDR_VERSION,
            seq: self.seq,
            _pad: 0,
            unk: 0,
            timestamp: 0,
        };

        let sub_hdr_size = mem::size_of::<EpicSubHdr>() + payload_len as usize;
        let sub = EpicSubHdr {
            length: sub_hdr_size as u32,
            version: EPIC_SUBHDR_VERSION,
            category,
            msg_type,
            timestamp: 0,
            seq,
            unk: 0,
            flags: 0,
            inline_len: payload_len,
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

        self.write_epic_headers(&mut ack, CAT_REPLY, sub.msg_type, sub.seq, 0);

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

        let rx_len = cmd.rxlen as usize;
        if rx_len > EPIC_BUFFER_SIZE {
            return Err("apple-epic: reply rxlen exceeds buffer size");
        }

        let reply_data =
            unsafe { core::slice::from_raw_parts(self.dma.rx_virt as *const u8, rx_len) };

        Ok(reply_data.to_vec())
    }
}

#[used]
static SCARLET_DRIVER_APPLE_EPIC_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
