#![no_std]

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cmp;

use scarlet::sync::Mutex;

use scarlet::device::remoteproc::{
    RemoteProcessor, RemoteprocCrashHandler, RemoteprocError, RemoteprocFirmware,
    RemoteprocMemoryRegion, RemoteprocMessage, RemoteprocService, RemoteprocServiceClient,
    RemoteprocServiceId, RemoteprocState,
};
use scarlet::early_println;
use scarlet::time;
use scarlet_driver_apple_asc::{AppleAsc, AscMessage};

/// RTKit message with endpoint and 64-bit payload.
pub struct RtkitMessage {
    /// Endpoint number.
    pub ep: u8,
    /// Message payload.
    pub msg: u64,
}

/// RTKit shared buffer descriptor.
pub struct RtkitBuffer {
    /// Kernel virtual address.
    pub buffer: *mut u8,
    /// Buffer size in bytes.
    pub size: usize,
    /// IOVA exposed to firmware.
    pub iova: u64,
    /// Device virtual address alias.
    pub dva: u64,
}

/// RTKit management endpoint.
pub const RTKIT_EP_MGMT: u8 = 0;
/// RTKit crashlog endpoint.
pub const RTKIT_EP_CRASHLOG: u8 = 1;
/// RTKit syslog endpoint.
pub const RTKIT_EP_SYSLOG: u8 = 2;
/// RTKit debug endpoint.
pub const RTKIT_EP_DEBUG: u8 = 3;
/// RTKit ioreport endpoint.
pub const RTKIT_EP_IOREPORT: u8 = 4;
/// RTKit oslog endpoint.
pub const RTKIT_EP_OSLOG: u8 = 8;

const RTKIT_APP_ENDPOINT_START: u8 = 0x20;

const RTKIT_DEFAULT_ENDPOINTS: [u8; 5] = [
    RTKIT_EP_CRASHLOG,
    RTKIT_EP_SYSLOG,
    RTKIT_EP_DEBUG,
    RTKIT_EP_IOREPORT,
    RTKIT_EP_OSLOG,
];

const MGMT_TYPE: u64 = 0x0FF0_0000_0000_0000;
const MGMT_PWR_STATE: u64 = 0x0000_0000_0000_FFFF;

const MGMT_MSG_HELLO: u64 = 1;
const MGMT_MSG_HELLO_ACK: u64 = 2;
const MGMT_MSG_START_EP: u64 = 5;
const MGMT_MSG_IOP_PWR_STATE: u64 = 6;
const MGMT_MSG_IOP_PWR_STATE_ACK: u64 = 7;
const MGMT_MSG_EPMAP: u64 = 8;
const MGMT_MSG_AP_PWR_STATE: u64 = 0xb;

const MGMT_MSG_HELLO_MINVER: u64 = 0x0000_0000_0000_FFFF;
const MGMT_MSG_HELLO_MAXVER: u64 = 0x0000_0000_FFFF_0000;

const MGMT_MSG_EPMAP_DONE: u64 = 1 << 51;
const MGMT_MSG_EPMAP_BASE: u64 = 0x0000_0007_0000_0000;
const MGMT_MSG_EPMAP_BITMAP: u64 = 0x0000_0000_FFFF_FFFF;

const MGMT_MSG_EPMAP_REPLY_DONE: u64 = 1 << 51;
const MGMT_MSG_EPMAP_REPLY_MORE: u64 = 1;

const MGMT_MSG_START_EP_IDX: u64 = 0x0000_00FF_0000_0000;
const MGMT_MSG_START_EP_FLAG: u64 = 1 << 1;

const RTKIT_SYSLOG_TYPE: u64 = 0x0FF0_0000_0000_0000;
const RTKIT_SYSLOG_LOG: u64 = 5;
const RTKIT_SYSLOG_INIT: u64 = 8;

const RTKIT_OSLOG_TYPE: u64 = 0xFF00_0000_0000_0000;
const RTKIT_OSLOG_INIT: u64 = 1;
const RTKIT_OSLOG_ACK: u64 = 3;

const MSG_BUFFER_REQUEST: u64 = 1;
const MSG_BUFFER_REQUEST_SIZE: u64 = 0x0FF0_0000_0000_0000;
const MSG_BUFFER_REQUEST_IOVA: u64 = 0x0000_0FFF_FFFF_FFFF;

/// RTKit powered off state.
pub const RTKIT_POWER_OFF: u32 = 0x00;
/// RTKit sleep state.
pub const RTKIT_POWER_SLEEP: u32 = 0x01;
/// RTKit quiesced state.
pub const RTKIT_POWER_QUIESCED: u32 = 0x10;
/// RTKit powered on state.
pub const RTKIT_POWER_ON: u32 = 0x20;
/// RTKit init state requested during boot.
pub const RTKIT_POWER_INIT: u32 = 0x220;

const RTKIT_MIN_VERSION: u32 = 11;
const RTKIT_MAX_VERSION: u32 = 12;

const RTKIT_BOOT_TIMEOUT_US: u64 = 1_000_000;

#[inline(always)]
const fn field_get(val: u64, mask: u64) -> u64 {
    (val & mask) >> mask.trailing_zeros()
}

#[inline(always)]
const fn field_prep(mask: u64, val: u64) -> u64 {
    val << mask.trailing_zeros()
}

fn mgmt_msg(msg_type: u64, payload: u64) -> u64 {
    field_prep(MGMT_TYPE, msg_type) | payload
}

/// RTKit protocol context.
pub struct AppleRtkit {
    asc: Arc<AppleAsc>,
    iop_power: Arc<Mutex<u32>>,
    ap_power: Arc<Mutex<u32>>,
    crashed: Arc<Mutex<bool>>,
    ep_bitmap: Arc<Mutex<u64>>,
    firmware_regions: Arc<Mutex<Vec<RemoteprocMemoryRegion>>>,
    crash_handler: Arc<Mutex<Option<Arc<dyn RemoteprocCrashHandler>>>>,
}

impl AppleRtkit {
    /// Create a new RTKit protocol instance over ASC.
    pub fn new(asc: Arc<AppleAsc>) -> Self {
        Self {
            asc,
            iop_power: Arc::new(Mutex::new(RTKIT_POWER_OFF)),
            ap_power: Arc::new(Mutex::new(RTKIT_POWER_OFF)),
            crashed: Arc::new(Mutex::new(false)),
            ep_bitmap: Arc::new(Mutex::new(0)),
            firmware_regions: Arc::new(Mutex::new(Vec::new())),
            crash_handler: Arc::new(Mutex::new(None)),
        }
    }

    /// Perform the RTKit boot handshake.
    pub fn boot(&self) -> Result<(), &'static str> {
        self.boot_inner(&RTKIT_DEFAULT_ENDPOINTS, false, true)
    }

    /// Wake RTKit without asserting CPU_RUN.
    ///
    /// This follows the same RTKit power-state handshake as [`Self::boot`],
    /// but leaves coprocessor release to firmware or a device-specific driver.
    /// Only default system endpoints are started during the handshake.
    ///
    /// # Returns
    ///
    /// `Ok(())` when RTKit reaches the powered-on state.
    pub fn wake(&self) -> Result<(), &'static str> {
        self.boot_inner(&RTKIT_DEFAULT_ENDPOINTS, false, false)
    }

    /// Perform the RTKit boot handshake and start required endpoints.
    ///
    /// This variant is for clients that must bind to a specific RTKit endpoint
    /// during boot. Missing requested endpoints are treated as probe failures,
    /// while [`Self::boot`] only starts its default service endpoints when they
    /// are advertised by firmware.
    ///
    /// # Arguments
    ///
    /// * `endpoints` - RTKit endpoint IDs that must be started.
    ///
    /// # Returns
    ///
    /// `Ok(())` when RTKit reaches the powered-on state and all requested
    /// endpoints were started.
    pub fn boot_with_endpoints(&self, endpoints: &[u8]) -> Result<(), &'static str> {
        self.boot_inner(endpoints, true, true)
    }

    /// Wake RTKit and start required endpoints without asserting CPU_RUN.
    ///
    /// Some Apple coprocessors are already released by platform firmware or by
    /// device-specific control registers. Those devices still use the RTKit
    /// power-state handshake, but writing the generic ASC CPU_RUN bit is not
    /// part of their startup sequence.
    ///
    /// # Arguments
    ///
    /// * `endpoints` - RTKit endpoint IDs that must be started.
    ///
    /// # Returns
    ///
    /// `Ok(())` when RTKit reaches the powered-on state and all requested
    /// endpoints were started.
    pub fn wake_with_endpoints(&self, endpoints: &[u8]) -> Result<(), &'static str> {
        self.boot_inner(endpoints, true, false)
    }

    fn boot_inner(
        &self,
        endpoints: &[u8],
        require_endpoints: bool,
        start_cpu: bool,
    ) -> Result<(), &'static str> {
        if start_cpu {
            self.asc.cpu_start();
        }

        self.send(&RtkitMessage {
            ep: RTKIT_EP_MGMT,
            msg: mgmt_msg(
                MGMT_MSG_IOP_PWR_STATE,
                field_prep(MGMT_PWR_STATE, RTKIT_POWER_INIT as u64),
            ),
        })?;

        let hello = self.wait_mgmt_msg(MGMT_MSG_HELLO, RTKIT_BOOT_TIMEOUT_US)?;
        self.handle_hello(hello)?;

        self.handle_epmap_sequence()?;

        for &ep in endpoints {
            if self.endpoint_supported(ep) {
                self.start_ep(ep)?;
            } else if require_endpoints {
                return Err("apple-rtkit: requested endpoint unavailable");
            }
        }

        loop {
            let msg = self.wait_any_mgmt_msg(RTKIT_BOOT_TIMEOUT_US)?;
            let msg_type = field_get(msg, MGMT_TYPE);
            if msg_type == MGMT_MSG_IOP_PWR_STATE_ACK {
                let pwr = field_get(msg, MGMT_PWR_STATE) as u32;
                *self.iop_power.lock() = pwr;
                if pwr == RTKIT_POWER_ON {
                    break;
                }
            }
        }

        self.send(&RtkitMessage {
            ep: RTKIT_EP_MGMT,
            msg: mgmt_msg(
                MGMT_MSG_AP_PWR_STATE,
                field_prep(MGMT_PWR_STATE, RTKIT_POWER_ON as u64),
            ),
        })?;

        loop {
            let msg = self.wait_any_mgmt_msg(RTKIT_BOOT_TIMEOUT_US)?;
            let msg_type = field_get(msg, MGMT_TYPE);
            if msg_type == MGMT_MSG_AP_PWR_STATE {
                let pwr = field_get(msg, MGMT_PWR_STATE) as u32;
                *self.ap_power.lock() = pwr;
                if pwr == RTKIT_POWER_ON {
                    break;
                }
            }
        }

        Ok(())
    }

    fn endpoint_supported(&self, ep: u8) -> bool {
        let Some(bit) = 1u64.checked_shl(ep as u32) else {
            return false;
        };
        (*self.ep_bitmap.lock() & bit) != 0
    }

    /// Send one endpoint message.
    pub fn send(&self, msg: &RtkitMessage) -> Result<(), &'static str> {
        let asc_msg = AscMessage {
            msg0: msg.msg,
            msg1: msg.ep as u32,
        };
        self.asc.send(&asc_msg)
    }

    /// Receive one message, handling system endpoints internally.
    pub fn recv(&self, msg: &mut RtkitMessage) -> Result<bool, &'static str> {
        if !self.asc.can_recv() {
            return Ok(false);
        }

        let mut asc_msg = AscMessage { msg0: 0, msg1: 0 };
        self.asc.recv(&mut asc_msg)?;

        msg.ep = asc_msg.msg1 as u8;
        msg.msg = asc_msg.msg0;

        if self.handle_internal_message(msg)? {
            return Ok(false);
        }

        Ok(true)
    }

    /// Start a specific RTKit endpoint.
    pub fn start_ep(&self, ep: u8) -> Result<(), &'static str> {
        if !self.endpoint_supported(ep) {
            return Err("apple-rtkit: endpoint unavailable");
        }

        if ep >= RTKIT_APP_ENDPOINT_START && !self.is_running() {
            return Err("apple-rtkit: application endpoint requires running RTKit");
        }

        let payload = field_prep(MGMT_MSG_START_EP_IDX, ep as u64) | MGMT_MSG_START_EP_FLAG;
        self.send(&RtkitMessage {
            ep: RTKIT_EP_MGMT,
            msg: mgmt_msg(MGMT_MSG_START_EP, payload),
        })
    }

    /// Check whether RTKit is fully running.
    pub fn is_running(&self) -> bool {
        *self.iop_power.lock() == RTKIT_POWER_ON
            && *self.ap_power.lock() == RTKIT_POWER_ON
            && !*self.crashed.lock()
    }

    /// Check whether RTKit reported crash state.
    pub fn is_crashed(&self) -> bool {
        *self.crashed.lock()
    }

    /// Notify a registered remoteproc crash handler if RTKit has crashed.
    ///
    /// The current RTKit driver only records a processor-level crash flag while
    /// polling messages. Consumers that detect or poll crash state can call this
    /// helper to bridge that state into the generic remoteproc callback until
    /// endpoint-specific crash reporting is wired directly into every handler.
    pub fn check_and_notify_crash(&self) {
        if !*self.crashed.lock() {
            return;
        }

        if let Some(handler) = self.crash_handler.lock().as_ref() {
            handler.crashed(RemoteprocServiceId(RTKIT_EP_CRASHLOG as u32), 0);
        }
    }

    fn wait_any_mgmt_msg(&self, timeout_us: u64) -> Result<u64, &'static str> {
        let start = time::current_time();
        loop {
            let elapsed = time::current_time().saturating_sub(start);
            if elapsed >= timeout_us {
                return Err("apple-rtkit: timeout waiting for management message");
            }

            let mut asc_msg = AscMessage { msg0: 0, msg1: 0 };
            self.asc.recv_timeout(&mut asc_msg, timeout_us - elapsed)?;

            let ep = asc_msg.msg1 as u8;
            if ep == RTKIT_EP_MGMT {
                return Ok(asc_msg.msg0);
            }

            let mut message = RtkitMessage {
                ep,
                msg: asc_msg.msg0,
            };
            let _ = self.handle_internal_message(&mut message)?;
        }
    }

    fn wait_mgmt_msg(&self, expected_type: u64, timeout_us: u64) -> Result<u64, &'static str> {
        let start = time::current_time();
        loop {
            let elapsed = time::current_time().saturating_sub(start);
            if elapsed >= timeout_us {
                return Err("apple-rtkit: timeout waiting for management message");
            }

            let remaining = timeout_us - elapsed;
            let msg = self.wait_any_mgmt_msg(remaining)?;
            if field_get(msg, MGMT_TYPE) == expected_type {
                return Ok(msg);
            }
        }
    }

    fn handle_hello(&self, hello_msg: u64) -> Result<(), &'static str> {
        let minver = field_get(hello_msg, MGMT_MSG_HELLO_MINVER) as u32;
        let maxver = field_get(hello_msg, MGMT_MSG_HELLO_MAXVER) as u32;

        let negotiated = cmp::min(maxver, RTKIT_MAX_VERSION);
        if negotiated < RTKIT_MIN_VERSION || negotiated < minver {
            return Err("apple-rtkit: unsupported RTKit version range");
        }

        let ack_payload = field_prep(MGMT_MSG_HELLO_MINVER, negotiated as u64)
            | field_prep(MGMT_MSG_HELLO_MAXVER, negotiated as u64);
        self.send(&RtkitMessage {
            ep: RTKIT_EP_MGMT,
            msg: mgmt_msg(MGMT_MSG_HELLO_ACK, ack_payload),
        })
    }

    fn handle_epmap_sequence(&self) -> Result<(), &'static str> {
        loop {
            let msg = self.wait_mgmt_msg(MGMT_MSG_EPMAP, RTKIT_BOOT_TIMEOUT_US)?;

            let base = field_get(msg, MGMT_MSG_EPMAP_BASE) as u32;
            let bitmap = field_get(msg, MGMT_MSG_EPMAP_BITMAP);
            let shift = base.saturating_mul(32);
            if shift < 64 {
                *self.ep_bitmap.lock() |= bitmap << shift;
            }

            let done = (msg & MGMT_MSG_EPMAP_DONE) != 0;
            let reply_payload = field_prep(MGMT_MSG_EPMAP_BASE, base as u64)
                | if done {
                MGMT_MSG_EPMAP_REPLY_DONE
            } else {
                MGMT_MSG_EPMAP_REPLY_MORE
            };

            self.send(&RtkitMessage {
                ep: RTKIT_EP_MGMT,
                msg: mgmt_msg(MGMT_MSG_EPMAP, reply_payload),
            })?;

            if done {
                break;
            }
        }

        Ok(())
    }

    fn handle_internal_message(&self, msg: &mut RtkitMessage) -> Result<bool, &'static str> {
        match msg.ep {
            RTKIT_EP_MGMT => self.handle_mgmt_message(msg.msg).map(|_| true),
            RTKIT_EP_CRASHLOG => self.handle_crashlog_message(msg),
            RTKIT_EP_SYSLOG => self.handle_syslog_message(msg),
            RTKIT_EP_IOREPORT => self.handle_ioreport_message(msg),
            RTKIT_EP_OSLOG => self.handle_oslog_message(msg),
            _ => Ok(false),
        }
    }

    fn handle_mgmt_message(&self, mgmt_msg_raw: u64) -> Result<(), &'static str> {
        let msg_type = field_get(mgmt_msg_raw, MGMT_TYPE);
        match msg_type {
            MGMT_MSG_IOP_PWR_STATE_ACK => {
                let pwr = field_get(mgmt_msg_raw, MGMT_PWR_STATE) as u32;
                *self.iop_power.lock() = pwr;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_crashlog_message(&self, msg: &RtkitMessage) -> Result<bool, &'static str> {
        self.handle_buffer_request(msg)
    }

    fn handle_syslog_message(&self, msg: &RtkitMessage) -> Result<bool, &'static str> {
        match field_get(msg.msg, RTKIT_SYSLOG_TYPE) {
            MSG_BUFFER_REQUEST => self.handle_buffer_request(msg),
            RTKIT_SYSLOG_INIT => Ok(true),
            RTKIT_SYSLOG_LOG => {
                self.send(msg)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn handle_ioreport_message(&self, msg: &RtkitMessage) -> Result<bool, &'static str> {
        match field_get(msg.msg, RTKIT_SYSLOG_TYPE) {
            MSG_BUFFER_REQUEST => self.handle_buffer_request(msg),
            0x8 | 0xc => {
                self.send(msg)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn handle_oslog_message(&self, msg: &RtkitMessage) -> Result<bool, &'static str> {
        if field_get(msg.msg, RTKIT_OSLOG_TYPE) != RTKIT_OSLOG_INIT {
            return Ok(false);
        }

        self.send(&RtkitMessage {
            ep: msg.ep,
            msg: field_prep(RTKIT_OSLOG_TYPE, RTKIT_OSLOG_ACK),
        })?;
        Ok(true)
    }

    fn handle_buffer_request(&self, msg: &RtkitMessage) -> Result<bool, &'static str> {
        if field_get(msg.msg, RTKIT_SYSLOG_TYPE) != MSG_BUFFER_REQUEST {
            return Ok(false);
        }

        let requested_size = field_get(msg.msg, MSG_BUFFER_REQUEST_SIZE);
        early_println!(
            "[apple-rtkit] endpoint {} buffer request size={:#x}, replying dummy iova=0",
            msg.ep,
            requested_size
        );

        let reply = RtkitMessage {
            ep: msg.ep,
            msg: field_prep(RTKIT_SYSLOG_TYPE, MSG_BUFFER_REQUEST)
                | field_prep(MSG_BUFFER_REQUEST_SIZE, requested_size)
                | field_prep(MSG_BUFFER_REQUEST_IOVA, 0),
        };
        self.send(&reply)?;

        Ok(true)
    }
}

impl RemoteProcessor for AppleRtkit {
    fn name(&self) -> &'static str {
        "apple-rtkit"
    }

    fn state(&self) -> RemoteprocState {
        if *self.crashed.lock() {
            return RemoteprocState::Crashed;
        }

        let iop_power = *self.iop_power.lock();
        let ap_power = *self.ap_power.lock();
        if iop_power == RTKIT_POWER_ON && ap_power == RTKIT_POWER_ON {
            RemoteprocState::Running
        } else if iop_power == RTKIT_POWER_SLEEP || ap_power == RTKIT_POWER_SLEEP {
            RemoteprocState::Suspended
        } else if iop_power == RTKIT_POWER_INIT {
            RemoteprocState::Loading
        } else {
            RemoteprocState::Offline
        }
    }

    fn load(&self, firmware: &RemoteprocFirmware) -> Result<(), RemoteprocError> {
        // Apple RTKit firmware is already loaded by m1n1 before Scarlet boots.
        // Keep the discovered regions for future diagnostics/mapping work, but
        // do not attempt to copy firmware bytes here.
        *self.firmware_regions.lock() = firmware.regions.clone();
        Ok(())
    }

    fn boot(&self) -> Result<(), RemoteprocError> {
        AppleRtkit::boot(self).map_err(|_| RemoteprocError::BootFailed)
    }

    fn shutdown(&self) -> Result<(), RemoteprocError> {
        // This driver does not yet implement RTKit's graceful power-down
        // protocol, so stopping the ASC CPU is a best-effort shutdown.
        self.asc.cpu_stop();
        *self.iop_power.lock() = RTKIT_POWER_OFF;
        *self.ap_power.lock() = RTKIT_POWER_OFF;
        Ok(())
    }

    fn suspend(&self) -> Result<(), RemoteprocError> {
        // TODO: negotiate RTKit sleep/quiesce states before exposing suspend.
        Err(RemoteprocError::NotSupported)
    }

    fn resume(&self) -> Result<(), RemoteprocError> {
        // TODO: resume from RTKit sleep/quiesce states once suspend exists.
        Err(RemoteprocError::NotSupported)
    }

    fn register_crash_handler(
        &self,
        handler: Arc<dyn RemoteprocCrashHandler>,
    ) -> Result<(), RemoteprocError> {
        *self.crash_handler.lock() = Some(handler);
        Ok(())
    }

    fn get_service(&self, id: RemoteprocServiceId) -> Option<Arc<dyn RemoteprocService>> {
        let endpoint = u8::try_from(id.0).ok()?;
        Some(Arc::new(AppleRtkitService::new(
            self.clone_for_service(),
            endpoint,
        )))
    }
}

impl AppleRtkit {
    fn clone_for_service(&self) -> Arc<Self> {
        Arc::new(Self {
            asc: self.asc.clone(),
            iop_power: self.iop_power.clone(),
            ap_power: self.ap_power.clone(),
            crashed: self.crashed.clone(),
            ep_bitmap: self.ep_bitmap.clone(),
            firmware_regions: self.firmware_regions.clone(),
            crash_handler: self.crash_handler.clone(),
        })
    }
}

/// Remoteproc service wrapper for one Apple RTKit endpoint.
pub struct AppleRtkitService {
    rtkit: Arc<AppleRtkit>,
    endpoint: u8,
    client: Mutex<Option<Arc<dyn RemoteprocServiceClient>>>,
}

impl AppleRtkitService {
    /// Create a remoteproc service wrapper for one RTKit endpoint.
    ///
    /// # Arguments
    ///
    /// * `rtkit` - Shared RTKit protocol instance backing the service.
    /// * `endpoint` - RTKit endpoint number exposed as a remoteproc service.
    ///
    /// # Returns
    ///
    /// A service wrapper that sends and receives messages through `rtkit`.
    pub fn new(rtkit: Arc<AppleRtkit>, endpoint: u8) -> Self {
        Self {
            rtkit,
            endpoint,
            client: Mutex::new(None),
        }
    }

    fn endpoint_name(&self) -> &'static str {
        match self.endpoint {
            RTKIT_EP_MGMT => "rtkit-mgmt",
            RTKIT_EP_CRASHLOG => "rtkit-crashlog",
            RTKIT_EP_SYSLOG => "rtkit-syslog",
            RTKIT_EP_DEBUG => "rtkit-debug",
            RTKIT_EP_IOREPORT => "rtkit-ioreport",
            RTKIT_EP_OSLOG => "rtkit-oslog",
            _ => "rtkit-ep-unknown",
        }
    }
}

impl RemoteprocService for AppleRtkitService {
    fn id(&self) -> RemoteprocServiceId {
        RemoteprocServiceId(self.endpoint as u32)
    }

    fn name(&self) -> &'static str {
        self.endpoint_name()
    }

    fn send(&self, message: &RemoteprocMessage) -> Result<(), RemoteprocError> {
        if message.len == 0 {
            return Err(RemoteprocError::TransportError);
        }

        self.rtkit
            .send(&RtkitMessage {
                ep: self.endpoint,
                msg: message.words[0],
            })
            .map_err(|_| RemoteprocError::TransportError)
    }

    fn try_recv(&self) -> Result<Option<RemoteprocMessage>, RemoteprocError> {
        let mut rtkit_message = RtkitMessage { ep: 0, msg: 0 };
        // The existing RTKit receive primitive consumes the next non-internal
        // endpoint message globally and does not yet maintain per-endpoint
        // queues, so this service currently accepts whichever endpoint message
        // is pending and exposes its payload through this wrapper.
        let received = self
            .rtkit
            .recv(&mut rtkit_message)
            .map_err(|_| RemoteprocError::TransportError)?;
        if !received {
            return Ok(None);
        }

        Ok(Some(RemoteprocMessage::one(rtkit_message.msg)))
    }

    fn set_client(
        &self,
        client: Option<Arc<dyn RemoteprocServiceClient>>,
    ) -> Result<(), RemoteprocError> {
        *self.client.lock() = client;
        Ok(())
    }
}

#[used]
static SCARLET_DRIVER_APPLE_RTKIT_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
