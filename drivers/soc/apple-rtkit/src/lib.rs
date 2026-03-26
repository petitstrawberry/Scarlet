#![no_std]

extern crate alloc;

use alloc::sync::Arc;
use core::cmp;

use scarlet::sync::Mutex;

use scarlet::drivers::soc::apple_asc::{AppleAsc, AscMessage};
use scarlet::early_println;
use scarlet::time;

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

const MGMT_TYPE: u64 = 0x01FF_0000_0000_0000;
const MGMT_PWR_STATE: u64 = 0x0000_0000_FFFF_FFFF;

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
const MGMT_MSG_EPMAP_BASE: u64 = 0x0000_0000_0000_0700;
const MGMT_MSG_EPMAP_BITMAP: u64 = 0x0000_0000_FFFF_FFFF;

const MGMT_MSG_EPMAP_REPLY_DONE: u64 = 1 << 51;
const MGMT_MSG_EPMAP_REPLY_MORE: u64 = 1;

const MGMT_MSG_START_EP_IDX: u64 = 0x0000_0000_FF00_0000;
const MGMT_MSG_START_EP_FLAG: u64 = 1 << 1;

const MSG_BUFFER_REQUEST: u64 = 1;
const MSG_BUFFER_REQUEST_SIZE: u64 = 0x00FF_0000_0000_0000;
const MSG_BUFFER_REQUEST_IOVA: u64 = 0x0000_07FF_FFFF_FFFF;

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
    iop_power: Mutex<u32>,
    ap_power: Mutex<u32>,
    crashed: Mutex<bool>,
    ep_bitmap: Mutex<u64>,
}

impl AppleRtkit {
    /// Create a new RTKit protocol instance over ASC.
    pub fn new(asc: Arc<AppleAsc>) -> Self {
        Self {
            asc,
            iop_power: Mutex::new(RTKIT_POWER_OFF),
            ap_power: Mutex::new(RTKIT_POWER_OFF),
            crashed: Mutex::new(false),
            ep_bitmap: Mutex::new(0),
        }
    }

    /// Perform the RTKit boot handshake.
    pub fn boot(&self) -> Result<(), &'static str> {
        self.asc.cpu_start();

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

        for ep in [
            RTKIT_EP_CRASHLOG,
            RTKIT_EP_SYSLOG,
            RTKIT_EP_DEBUG,
            RTKIT_EP_IOREPORT,
            RTKIT_EP_OSLOG,
        ] {
            self.start_ep(ep)?;
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
        *self.ap_power.lock() = RTKIT_POWER_ON;

        Ok(())
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

        msg.ep = (asc_msg.msg1 & 0x3f) as u8;
        msg.msg = asc_msg.msg0;

        if self.handle_internal_message(msg)? {
            return Ok(false);
        }

        Ok(true)
    }

    /// Start a specific RTKit endpoint.
    pub fn start_ep(&self, ep: u8) -> Result<(), &'static str> {
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

    fn wait_any_mgmt_msg(&self, timeout_us: u64) -> Result<u64, &'static str> {
        let mut asc_msg = AscMessage { msg0: 0, msg1: 0 };
        self.asc.recv_timeout(&mut asc_msg, timeout_us)?;

        let ep = (asc_msg.msg1 & 0x3f) as u8;
        if ep != RTKIT_EP_MGMT {
            let mut message = RtkitMessage {
                ep,
                msg: asc_msg.msg0,
            };
            let _ = self.handle_internal_message(&mut message)?;
            return Err("apple-rtkit: unexpected non-management message during boot");
        }

        Ok(asc_msg.msg0)
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
            let reply_payload = if done {
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
            RTKIT_EP_SYSLOG | RTKIT_EP_CRASHLOG => self.handle_buffer_request(msg),
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

    fn handle_buffer_request(&self, msg: &RtkitMessage) -> Result<bool, &'static str> {
        if (msg.msg & 0xff) != MSG_BUFFER_REQUEST {
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
            msg: MSG_BUFFER_REQUEST | field_prep(MSG_BUFFER_REQUEST_IOVA, 0),
        };
        self.send(&reply)?;

        Ok(true)
    }
}

#[used]
static SCARLET_DRIVER_APPLE_RTKIT_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
