#![no_std]

extern crate alloc;

#[cfg(test)]
extern crate std;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicU32, Ordering};

use scarlet::sync::Mutex;

use scarlet::arch::mmio;
use scarlet::device::mailbox::{
    MailboxChannel, MailboxChannelId, MailboxClient, MailboxController, MailboxError,
    MailboxMessage, MailboxSpec,
};
use scarlet::device::manager::{DeviceManager, DriverPriority};
use scarlet::device::platform::resource::PlatformDeviceResourceType;
use scarlet::device::platform::{PlatformDeviceDriver, PlatformDeviceInfo};
use scarlet::time;
use scarlet::vm;

const ASC_CPU_CONTROL: usize = 0x0044;
const ASC_CPU_CONTROL_START: u32 = 0x10;
const ASC_MAILBOX_OFFSET: usize = 0x8000;
const ASC_CPU_MMIO_SIZE: usize = 0x1000;

const ASC_MBOX_A2I_CONTROL: usize = 0x0110;
const ASC_MBOX_I2A_CONTROL: usize = 0x0114;
const ASC_MBOX_CTRL_FULL: u32 = 1 << 16;
const ASC_MBOX_CTRL_EMPTY: u32 = 1 << 17;

const ASC_MBOX_A2I_SEND0: usize = 0x0800;
const ASC_MBOX_A2I_SEND1: usize = 0x0808;
const ASC_MBOX_I2A_RECV0: usize = 0x0830;
const ASC_MBOX_I2A_RECV1: usize = 0x0838;

const ASC_SEND_TIMEOUT_US: u64 = 1_000_000;
const ASC_POLL_DELAY_US: u64 = 1;

/// One ASC mailbox message pair.
pub struct AscMessage {
    /// Primary 64-bit payload.
    pub msg0: u64,
    /// Secondary 32-bit payload (typically endpoint ID).
    pub msg1: u32,
}

/// Apple ASC mailbox MMIO driver.
pub struct AppleAsc {
    base: usize,
    cpu_base: usize,
}

/// Mailbox channel wrapper for one Apple ASC mailbox queue.
///
/// ASC hardware exchanges messages as `(msg0: u64, msg1: u32)` pairs. This
/// channel maps them to the generic [`MailboxMessage`] layout by storing
/// `msg0` in `words[0]`, storing zero-extended `msg1` in `words[1]`, and
/// setting `len` to `2`. Messages submitted through [`MailboxChannel::try_send`]
/// must therefore provide at least two words and keep the upper 32 bits of
/// `words[1]` clear.
pub struct AppleAscChannel {
    asc: Arc<AppleAsc>,
    id: MailboxChannelId,
    client: Mutex<Option<Arc<dyn MailboxClient>>>,
}

/// Mailbox controller wrapper for one Apple ASC instance.
///
/// ASC exposes a single AP↔IOP mailbox queue, so every requested mailbox channel
/// is backed by the same hardware queue. Channel release is a no-op because the
/// queue lifetime is owned by the controller and dropped with the ASC instance.
pub struct AppleAscMailboxController {
    asc: Arc<AppleAsc>,
    phandle: u32,
    next_channel_id: AtomicU32,
}

impl AppleAsc {
    /// Create a new ASC instance from an MMIO base address.
    pub const fn new(base: usize) -> Self {
        Self {
            base,
            cpu_base: base,
        }
    }

    /// Create an ASC instance with separate mailbox and CPU-control mappings.
    ///
    /// # Arguments
    ///
    /// * `base` - Virtual base of the ASC mailbox register block.
    /// * `cpu_base` - Virtual base of the coprocessor CPU-control register block.
    ///
    /// # Returns
    ///
    /// An ASC instance using the supplied register mappings.
    pub const fn new_with_cpu_base(base: usize, cpu_base: usize) -> Self {
        Self { base, cpu_base }
    }

    /// Start the ASC IOP CPU.
    pub fn cpu_start(&self) {
        // SAFETY: `self.cpu_base` points to a mapped coprocessor MMIO region.
        let ctrl = unsafe { mmio::read32(self.cpu_base + ASC_CPU_CONTROL) };
        // SAFETY: `self.cpu_base` points to a mapped coprocessor MMIO region.
        unsafe {
            mmio::write32(
                self.cpu_base + ASC_CPU_CONTROL,
                ctrl | ASC_CPU_CONTROL_START,
            );
        }
    }

    /// Stop the ASC IOP CPU by clearing START bit.
    pub fn cpu_stop(&self) {
        // SAFETY: `self.cpu_base` points to a mapped coprocessor MMIO region.
        let ctrl = unsafe { mmio::read32(self.cpu_base + ASC_CPU_CONTROL) };
        // SAFETY: `self.cpu_base` points to a mapped coprocessor MMIO region.
        unsafe {
            mmio::write32(
                self.cpu_base + ASC_CPU_CONTROL,
                ctrl & !ASC_CPU_CONTROL_START,
            );
        }
    }

    /// Check whether there is a pending IOP->AP message.
    pub fn can_recv(&self) -> bool {
        // SAFETY: `self.base` points to a mapped ASC MMIO region.
        let status = unsafe { mmio::read32(self.base + ASC_MBOX_I2A_CONTROL) };
        (status & ASC_MBOX_CTRL_EMPTY) == 0
    }

    /// Send one AP->IOP message.
    pub fn send(&self, msg: &AscMessage) -> Result<(), &'static str> {
        let start = time::current_time();
        loop {
            // SAFETY: `self.base` points to a mapped ASC MMIO region.
            let status = unsafe { mmio::read32(self.base + ASC_MBOX_A2I_CONTROL) };
            if (status & ASC_MBOX_CTRL_FULL) == 0 {
                break;
            }

            if time::current_time().saturating_sub(start) >= ASC_SEND_TIMEOUT_US {
                return Err("apple-asc: send mailbox full timeout");
            }

            time::udelay(ASC_POLL_DELAY_US);
        }

        // SAFETY: MMIO transaction ordering for ASC mailbox writes requires dsb ish.
        unsafe {
            dsb_ish();
        }

        // SAFETY: `self.base` points to a mapped ASC MMIO region.
        unsafe {
            mmio::write64(self.base + ASC_MBOX_A2I_SEND0, msg.msg0);
            mmio::write64(self.base + ASC_MBOX_A2I_SEND1, msg.msg1 as u64);
        }

        Ok(())
    }

    /// Receive one IOP->AP message.
    pub fn recv(&self, msg: &mut AscMessage) -> Result<(), &'static str> {
        if !self.can_recv() {
            return Err("apple-asc: no message available");
        }

        // SAFETY: `self.base` points to a mapped ASC MMIO region.
        unsafe {
            msg.msg0 = mmio::read64(self.base + ASC_MBOX_I2A_RECV0);
            msg.msg1 = mmio::read64(self.base + ASC_MBOX_I2A_RECV1) as u32;
        }

        // SAFETY: MMIO transaction ordering for ASC mailbox reads requires dsb ish.
        unsafe {
            dsb_ish();
        }

        Ok(())
    }

    /// Receive one message with timeout in microseconds.
    pub fn recv_timeout(&self, msg: &mut AscMessage, timeout_us: u64) -> Result<(), &'static str> {
        let start = time::current_time();
        loop {
            if self.can_recv() {
                return self.recv(msg);
            }

            if time::current_time().saturating_sub(start) >= timeout_us {
                return Err("apple-asc: recv timeout");
            }

            time::udelay(ASC_POLL_DELAY_US);
        }
    }
}

impl AppleAscChannel {
    /// Create a mailbox channel for one ASC queue.
    ///
    /// # Arguments
    ///
    /// * `asc` - ASC hardware instance backing this channel.
    /// * `id` - Controller-local channel identifier.
    /// * `client` - Optional callback sink installed before use.
    ///
    /// # Returns
    ///
    /// A new mailbox channel wrapper.
    pub fn new(
        asc: Arc<AppleAsc>,
        id: MailboxChannelId,
        client: Option<Arc<dyn MailboxClient>>,
    ) -> Self {
        Self {
            asc,
            id,
            client: Mutex::new(client),
        }
    }

    fn mailbox_to_asc(message: &MailboxMessage) -> Result<AscMessage, MailboxError> {
        if message.len < 2 || message.words[1] > u32::MAX as u64 {
            return Err(MailboxError::InvalidChannel);
        }

        Ok(AscMessage {
            msg0: message.words[0],
            msg1: message.words[1] as u32,
        })
    }

    fn asc_to_mailbox(message: &AscMessage) -> MailboxMessage {
        MailboxMessage {
            words: [message.msg0, message.msg1 as u64, 0, 0],
            len: 2,
        }
    }
}

impl MailboxChannel for AppleAscChannel {
    fn id(&self) -> MailboxChannelId {
        self.id
    }

    fn try_send(&self, message: &MailboxMessage) -> Result<(), MailboxError> {
        let asc_message = Self::mailbox_to_asc(message)?;
        self.asc
            .send(&asc_message)
            .map_err(|_| MailboxError::HardwareError)?;

        if let Some(client) = self.client.lock().as_ref() {
            client.tx_done(self.id);
        }

        Ok(())
    }

    fn try_recv(&self) -> Result<Option<MailboxMessage>, MailboxError> {
        if !self.asc.can_recv() {
            return Ok(None);
        }

        let mut asc_message = AscMessage { msg0: 0, msg1: 0 };
        self.asc
            .recv(&mut asc_message)
            .map_err(|_| MailboxError::HardwareError)?;

        Ok(Some(Self::asc_to_mailbox(&asc_message)))
    }

    /// Send using ASC's built-in mailbox-full timeout.
    ///
    /// The current ASC MMIO primitive already waits up to `ASC_SEND_TIMEOUT_US`
    /// while polling the transmit FIFO, so this implementation accepts
    /// `timeout_us` for the generic trait contract but delegates to
    /// [`Self::try_send`] instead of overriding the hardware timeout.
    fn send_timeout(&self, message: &MailboxMessage, timeout_us: u64) -> Result<(), MailboxError> {
        let _ = timeout_us;
        self.try_send(message)
    }

    fn set_client(&self, client: Option<Arc<dyn MailboxClient>>) -> Result<(), MailboxError> {
        *self.client.lock() = client;
        Ok(())
    }

    fn poll(&self) -> Result<(), MailboxError> {
        if self.asc.can_recv() {
            if let Some(client) = self.client.lock().as_ref() {
                client.rx_ready(self.id);
            }
        }

        Ok(())
    }
}

impl AppleAscMailboxController {
    /// Create a mailbox controller for an ASC hardware instance.
    ///
    /// # Arguments
    ///
    /// * `asc` - ASC hardware instance backing requested channels.
    /// * `phandle` - Firmware phandle used to register this controller.
    ///
    /// # Returns
    ///
    /// A new ASC mailbox controller wrapper.
    pub const fn new(asc: Arc<AppleAsc>, phandle: u32) -> Self {
        Self {
            asc,
            phandle,
            next_channel_id: AtomicU32::new(0),
        }
    }

    /// Return the firmware phandle used to register this controller.
    ///
    /// # Returns
    ///
    /// Firmware phandle for this controller.
    pub const fn phandle(&self) -> u32 {
        self.phandle
    }
}

impl MailboxController for AppleAscMailboxController {
    fn name(&self) -> &'static str {
        "apple-asc-mailbox"
    }

    fn request_channel(
        &self,
        spec: &MailboxSpec,
        client: Option<Arc<dyn MailboxClient>>,
    ) -> Result<Arc<dyn MailboxChannel>, MailboxError> {
        let _ = spec;
        let channel_id = MailboxChannelId(self.next_channel_id.fetch_add(1, Ordering::Relaxed));
        Ok(Arc::new(AppleAscChannel::new(
            Arc::clone(&self.asc),
            channel_id,
            client,
        )))
    }

    fn release_channel(&self, channel: MailboxChannelId) {
        let _ = channel;
    }
}

/// Registry of probed ASC mailbox instances.
static ASC_REGISTRY: Mutex<Vec<Arc<AppleAsc>>> = Mutex::new(Vec::new());
static ASC_PHANDLE_REGISTRY: Mutex<Vec<(u32, Arc<AppleAsc>)>> = Mutex::new(Vec::new());

/// Get a probed ASC mailbox instance by index.
///
/// # Arguments
///
/// * `id` - Zero-based ASC registration index.
///
/// # Returns
///
/// ASC instance registered at `id`, or `None` when missing.
pub fn get_apple_asc(id: u32) -> Option<Arc<AppleAsc>> {
    ASC_REGISTRY.lock().get(id as usize).map(Arc::clone)
}

/// Get a probed ASC mailbox instance by firmware phandle.
///
/// # Arguments
///
/// * `phandle` - Firmware phandle of the ASC mailbox controller node.
///
/// # Returns
///
/// ASC instance registered for `phandle`, or `None` when the ASC has not probed.
pub fn get_apple_asc_by_phandle(phandle: u32) -> Option<Arc<AppleAsc>> {
    ASC_PHANDLE_REGISTRY
        .lock()
        .iter()
        .find(|(registered, _)| *registered == phandle)
        .map(|(_, asc)| Arc::clone(asc))
}

#[inline(always)]
unsafe fn dsb_ish() {
    // SAFETY: Emits the required AArch64 barrier instruction for MMIO ordering.
    unsafe {
        asm!("dsb ish", options(nostack, nomem, preserves_flags));
    }
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let resource = device
        .get_resources()
        .iter()
        .find(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM))
        .ok_or("apple-asc: no memory resource")?;

    let paddr = resource.start;
    let size = resource
        .end
        .checked_sub(resource.start)
        .and_then(|value| value.checked_add(1))
        .ok_or("apple-asc: invalid memory resource")?;

    let base = vm::ioremap(paddr, size).map_err(|_| "apple-asc: ioremap failed")?;
    let cpu_paddr = paddr
        .checked_sub(ASC_MAILBOX_OFFSET)
        .ok_or("apple-asc: invalid mailbox base")?;
    let cpu_base = vm::ioremap(cpu_paddr, ASC_CPU_MMIO_SIZE)
        .map_err(|_| "apple-asc: CPU control ioremap failed")?;
    let asc = Arc::new(AppleAsc::new_with_cpu_base(base, cpu_base));

    ASC_REGISTRY.lock().push(Arc::clone(&asc));

    let phandle = device
        .property("phandle")
        .and_then(|p| p.as_usize())
        .map(|v| v as u32)
        .or_else(|| {
            device
                .property("linux,phandle")
                .and_then(|p| p.as_usize())
                .map(|v| v as u32)
        })
        .unwrap_or(0);

    if phandle != 0 {
        ASC_PHANDLE_REGISTRY
            .lock()
            .push((phandle, Arc::clone(&asc)));
    }

    let controller = Arc::new(AppleAscMailboxController::new(asc, phandle));
    DeviceManager::get_manager().register_mailbox_controller(controller.phandle(), controller);

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_apple_asc_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-asc-mailbox",
        probe_fn,
        remove_fn,
        alloc::vec![
            "apple,t8103-asc-mailbox",
            "apple,asc-mailbox-v4",
            "apple,asc-mailbox",
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

scarlet::driver_initcall!(register_apple_asc_driver);

#[used]
static SCARLET_DRIVER_APPLE_ASC_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mailbox_to_asc_uses_first_two_words() {
        let message = MailboxMessage {
            words: [0x1122_3344_5566_7788, 0xaabb_ccdd, 0xffff, 0xeeee],
            len: 2,
        };

        let asc_message = AppleAscChannel::mailbox_to_asc(&message).unwrap();

        assert_eq!(asc_message.msg0, 0x1122_3344_5566_7788);
        assert_eq!(asc_message.msg1, 0xaabb_ccdd);
    }

    #[test]
    fn asc_to_mailbox_zero_extends_msg1_and_sets_len() {
        let asc_message = AscMessage {
            msg0: 0x8877_6655_4433_2211,
            msg1: 0x1234_5678,
        };

        let message = AppleAscChannel::asc_to_mailbox(&asc_message);

        assert_eq!(message.words, [0x8877_6655_4433_2211, 0x1234_5678, 0, 0]);
        assert_eq!(message.len, 2);
    }

    #[test]
    fn mailbox_to_asc_rejects_missing_second_word() {
        let message = MailboxMessage::one(0x1234);

        assert_eq!(
            AppleAscChannel::mailbox_to_asc(&message),
            Err(MailboxError::InvalidChannel)
        );
    }

    #[test]
    fn mailbox_to_asc_rejects_msg1_upper_bits() {
        let message = MailboxMessage {
            words: [0x1, 0x1_0000_0000, 0, 0],
            len: 2,
        };

        assert_eq!(
            AppleAscChannel::mailbox_to_asc(&message),
            Err(MailboxError::InvalidChannel)
        );
    }
}
