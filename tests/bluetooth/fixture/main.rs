//! Test-only controller and remote HID peers. The kernel owns the H4 host side.
#![no_std]
#![no_main]
extern crate alloc;
#[used]
static ENTRY: extern "C" fn() -> ! = scarlet::arch::aarch64::boot::linux::image_head;

use alloc::{collections::VecDeque, sync::Arc, vec, vec::Vec};
use scarlet::{
    device::bluetooth::{self, H4Transport},
    sync::{SpinLock, Waker},
};

const ADAPTER: [u8; 6] = [1, 0, 0, 0, 0, 2];
const LEFT: [u8; 6] = [0x10, 0, 0, 0, 0, 2];
const RIGHT: [u8; 6] = [0x20, 0, 0, 0, 0, 2];
const HANDLE: u16 = 0x123;
const CONTROL_CID: u16 = 0x83;
const INTERRUPT_CID: u16 = 0x84;

#[derive(Default)]
struct Controller {
    rx: VecDeque<u8>,
    selected: Option<[u8; 6]>,
    encrypted: bool,
    light_requests: u8,
    local_interrupt: u16,
    local_control: u16,
}

fn word(bytes: &[u8]) -> u16 {
    u16::from_le_bytes(bytes[..2].try_into().unwrap())
}
fn words(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

impl Controller {
    fn event(&mut self, code: u8, payload: &[u8]) {
        self.rx.extend([4, code, payload.len().try_into().unwrap()]);
        self.rx.extend(payload);
    }
    fn complete(&mut self, op: u16, result: &[u8]) {
        let [lo, hi] = op.to_le_bytes();
        self.event(0x0e, &[&[1, lo, hi][..], result].concat());
    }
    fn accepted(&mut self, op: u16) {
        let [lo, hi] = op.to_le_bytes();
        self.event(0x0f, &[0, 1, lo, hi]);
    }
    fn command(&mut self, op: u16, data: &[u8]) {
        match op {
            0x0c03 | 0x0c01 | 0x0c56 | 0x0c45 => self.complete(op, &[0]),
            0x1009 => self.complete(op, &[&[0], ADAPTER.as_slice()].concat()),
            0x1005 => self.complete(op, &[0, 128, 0, 0, 1, 0, 0, 0]), // One ACL credit.
            // Inquiry with RSSI
            0x0401 => {
                self.accepted(op);
                for address in [LEFT, RIGHT] {
                    let mut result = vec![1];
                    result.extend(address);
                    result.extend([1, 0, 0x08, 0x25, 0, 0, 0, 0xd0]);
                    self.event(0x22, &result);
                }
                self.event(1, &[0]);
            }
            // Remote Name Request
            0x0419 => {
                self.accepted(op);
                let addr = &data[..6];
                let name = if addr == RIGHT {
                    b"Joy-Con (R)"
                } else {
                    assert_eq!(addr, LEFT);
                    b"Joy-Con (L)"
                };
                let mut result = [0; 255];
                result[1..7].copy_from_slice(addr);
                result[7..7 + name.len()].copy_from_slice(name);
                self.event(7, &result);
            }
            // Create Connection
            0x0405 => {
                assert!(self.selected.is_none(), "one selected ACL at a time");
                let addr: [u8; 6] = data[..6].try_into().unwrap();
                assert!(addr == LEFT || addr == RIGHT);
                self.selected = Some(addr);
                self.accepted(op);
                self.event(
                    3,
                    &[&[0][..], &HANDLE.to_le_bytes(), &addr, &[1, 0]].concat(),
                );
            }
            // Authentication Requested
            0x0411 => {
                assert_eq!(data, HANDLE.to_le_bytes());
                self.accepted(op);
                self.event(0x17, &self.selected.unwrap()); // Link key request.
            }
            // Link Key Request Negative Reply
            0x040c => {
                assert_eq!(data, self.selected.unwrap());
                self.complete(op, &[&[0], data].concat());
                self.event(0x31, data); // IO capability request.
            }
            // IO Capability Request Reply
            0x042b => {
                let addr = self.selected.unwrap();
                assert_eq!(data, &[addr.as_slice(), &[3, 0, 4]].concat());
                self.complete(op, &[&[0], addr.as_slice()].concat());
                self.event(0x33, &[addr.as_slice(), &[0; 4]].concat());
            }
            // User Confirmation Request Reply
            0x042c => {
                assert_eq!(data, self.selected.unwrap());
                self.complete(op, &[&[0], data].concat());
                self.event(0x36, &[&[0], data].concat());
                self.event(0x18, &[data, &[data[0]; 16], &[4]].concat());
                self.event(6, &[&[0][..], &HANDLE.to_le_bytes()].concat());
            }
            // Set Connection Encryption
            0x0413 => {
                assert_eq!(data, &[HANDLE.to_le_bytes().as_slice(), &[1]].concat());
                self.accepted(op);
                self.encrypted = true;
                self.event(8, &[&[0], data].concat());
            }
            // Disconnect
            0x0406 => {
                assert_eq!(
                    self.light_requests,
                    if self.selected == Some(LEFT) { 2 } else { 1 }
                );
                self.light_requests = 0;
                assert_eq!(word(data), HANDLE);
                self.accepted(op);
                self.event(5, &[&[0][..], &HANDLE.to_le_bytes(), &[0x16]].concat());
                self.selected = None;
                self.encrypted = false;
                self.local_interrupt = 0;
            }
            _ => panic!("unexpected host HCI opcode {op:#06x}"),
        }
    }
    fn acl(&mut self, pb: u16, data: &[u8]) {
        self.rx.push_back(2);
        self.rx
            .extend(words(&[HANDLE | pb << 12, data.len() as u16]));
        self.rx.extend(data);
    }
    fn pdu(&mut self, cid: u16, data: &[u8], fragmented: bool) {
        let mut pdu = words(&[data.len() as u16, cid]);
        pdu.extend(data);
        if fragmented {
            self.acl(2, &pdu[..11]);
            self.acl(1, &pdu[11..]);
        } else {
            self.acl(2, &pdu);
        }
    }
    fn signal(&mut self, code: u8, id: u8, body: &[u8]) {
        let mut sig = vec![code, id];
        sig.extend((body.len() as u16).to_le_bytes());
        sig.extend(body);
        self.pdu(1, &sig, false);
    }
    fn signaling(&mut self, data: &[u8]) {
        let id = data[1];
        let body = &data[4..];
        assert_eq!(usize::from(word(&data[2..])), body.len());
        match data[0] {
            2 => {
                let local = word(&body[2..]);
                let remote = match word(body) {
                    0x11 => {
                        self.local_control = local;
                        CONTROL_CID
                    }
                    0x13 => {
                        self.local_interrupt = local;
                        INTERRUPT_CID
                    }
                    psm => panic!("unexpected PSM {psm:#x}"),
                };
                self.signal(3, id, &words(&[remote, local, 0, 0]));
                self.signal(4, 0xb0, &words(&[local, 0]));
            }
            4 => {
                let remote = word(body);
                let local = if remote == CONTROL_CID {
                    self.local_control
                } else {
                    assert_eq!(remote, INTERRUPT_CID);
                    self.local_interrupt
                };
                self.signal(5, id, &words(&[local, 0, 0]));
            }
            5 => assert_eq!(word(&body[4..]), 0),
            code => panic!("unexpected L2CAP signal {code}"),
        }
    }

    fn hid(&mut self, output: &[u8]) {
        assert_eq!(&output[..2], &[0xa2, 1]);
        let side = if self.selected == Some(LEFT) { 1 } else { 2 };
        let mut input = [0; 50]; // HIDP + standard HID report
        input[..2].copy_from_slice(&[0xa1, 0x21]);
        input[7..13].copy_from_slice(&[0, 0xf8, 0x7f, 0, 0xf8, 0x7f]); // Both sticks centered.
        input[15] = output[11];
        match output[11] {
            2 => {
                input[14] = 0x82;
                input[18] = side;
            }
            3 => {
                assert_eq!(output[12], 0x30);
                input[14] = 0x80;
            }
            0x30 => {
                assert_eq!(&output[12..], &[0x01], "only player LED 1 is steady");
                self.light_requests += 1;
                if side == 1 && self.light_requests == 1 {
                    return; // Lose the first ACK and exercise the host deadline.
                }
                input[14] = 0x80; // Generic firmware ACK, with current controls.
                if side == 1 {
                    input[6] = 0x44; // L + d-pad right
                } else {
                    input[4] = 0x40;
                    input[10..12].copy_from_slice(&[255, 255]); // R + stick right
                }
            }
            other => panic!("unexpected Joy-Con subcommand {other}"),
        }
        self.pdu(self.local_interrupt, &input, true);
        if output[11] == 3 {
            // Input is already live while the player light is being configured.
            input[1] = 0x30;
            if side == 1 {
                input[6] = 0x48; // L + d-pad left
            } else {
                input[4] = 0x40;
                input[10..12].copy_from_slice(&[0, 0xf0]); // R + stick left
            }
            self.pdu(self.local_interrupt, &input, true);
        }
    }
}

struct Radio(SpinLock<Controller>);
impl H4Transport for Radio {
    // All RX bytes are enqueued synchronously by the worker's own send call.
    fn start(&self, _: Arc<Waker>) -> Result<(), &'static str> {
        Ok(())
    }
    fn stop(&self) {}
    fn rx_pending(&self) -> bool {
        !self.0.lock().rx.is_empty()
    }
    fn receive(&self, bytes: &mut [u8]) -> Result<usize, &'static str> {
        let mut state = self.0.lock();
        let count = bytes.len().min(17).min(state.rx.len());
        for byte in &mut bytes[..count] {
            *byte = state.rx.pop_front().unwrap();
        }
        Ok(count)
    }
    fn send(&self, frame: &[u8]) -> Result<(), &'static str> {
        let mut state = self.0.lock();
        match frame[0] {
            1 => {
                assert_eq!(usize::from(frame[3]), frame.len() - 4);
                state.command(word(&frame[1..]), &frame[4..]);
            }
            2 => {
                assert!(state.encrypted, "L2CAP before selected peer was encrypted");
                assert_eq!(word(&frame[1..]), HANDLE | 2 << 12);
                assert_eq!(usize::from(word(&frame[3..])), frame.len() - 5);
                assert_eq!(usize::from(word(&frame[5..])), frame.len() - 9);
                state.event(0x13, &[&[1][..], &HANDLE.to_le_bytes(), &[1, 0]].concat());
                match word(&frame[7..]) {
                    1 => state.signaling(&frame[9..]),
                    INTERRUPT_CID => state.hid(&frame[9..]),
                    cid => panic!("unexpected peer CID {cid:#x}"),
                }
            }
            kind => panic!("unexpected H4 {kind}"),
        }
        Ok(())
    }
}
fn register() {
    bluetooth::register(Arc::new(Radio(SpinLock::new(Controller::default()))));
}
scarlet::late_initcall!(register);
