//! Two BR/EDR peers on a test-only adapter. All host-side traffic goes through H4Transport.
#![no_std]
#![no_main]
extern crate alloc;
#[used]
static ENTRY: extern "C" fn() -> ! = scarlet::arch::aarch64::boot::linux::image_head;

use alloc::{collections::VecDeque, sync::Arc, vec, vec::Vec};
use core::any::Any;
use scarlet::{
    device::{
        Device, DeviceType,
        bluetooth::{self, H4Transport},
        manager::DeviceManager,
    },
    object::capability::{
        ControlOps, MemoryMappingInfo, MemoryMappingOps, Selectable,
        selectable::{ReadyInterest, SelectWaitOutcome},
    },
    sync::{SpinLock, Waker},
};

mod sdp;
#[path = "../radio.rs"]
mod wire;

fn word(bytes: &[u8]) -> u16 {
    u16::from_le_bytes(bytes[..2].try_into().unwrap())
}
fn words(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

#[derive(Default)]
struct Wire {
    frames: VecDeque<Vec<u8>>,
    offset: usize,
    acl_in_flight: Option<u16>,
}
impl Wire {
    fn event(&mut self, kind: u8, params: &[u8]) {
        self.frames
            .push_back([&[4, kind, params.len().try_into().unwrap()][..], params].concat());
    }
    fn complete(&mut self, opcode: u16, params: &[u8]) {
        self.event(0x0e, &[&[1][..], &opcode.to_le_bytes(), params].concat());
    }
    fn accepted(&mut self, opcode: u16) {
        self.event(0x0f, &[&[0, 1][..], &opcode.to_le_bytes()].concat());
    }
    fn receive(&mut self, out: &mut [u8]) -> usize {
        // Split long frames but deliver adjacent short H4 events and ACL frames
        // in one transport read, as a controller can do on a bonded wake.
        let mut written = 0;
        while written < out.len()
            && let Some(frame) = self.frames.front()
        {
            let n = (out.len() - written).min(17).min(frame.len() - self.offset);
            out[written..written + n].copy_from_slice(&frame[self.offset..self.offset + n]);
            written += n;
            self.offset += n;
            if self.offset < frame.len() {
                break;
            }
            if frame.starts_with(&[4, 0x13]) {
                assert_eq!(self.acl_in_flight.take(), Some(word(&frame[4..])));
            }
            self.frames.pop_front();
            self.offset = 0;
        }
        written
    }
    fn acl(&mut self, handle: u16, pb: u16, data: &[u8]) {
        self.frames.push_back(
            [
                &[2][..],
                &words(&[handle | pb << 12, data.len() as u16]),
                data,
            ]
            .concat(),
        );
    }
}

struct Peer {
    address: [u8; 6],
    base_handle: u16,
    handle: Option<u16>,
    connects: u16,
    authenticated: bool,
    encrypted: bool,
    bonded: bool,
    peer_opens_hid: bool,
    auth_requested: bool,
    control_pending: bool,
    fail_disconnect: bool,
    rejected_pages: u16,
    host_pages: u16,
    local_control: u16,
    local_interrupt: u16,
    // Remote channel IDs deliberately differ by peer. Local channel IDs can legally coincide.
    remote_control: u16,
    remote_interrupt: u16,
    local_sdp: Option<u16>,
    discovery: sdp::Discovery,
    connection_requests: u16,
    output_count: usize,
    outgoing_pdu: Vec<u8>,
}
impl Peer {
    fn new(address: [u8; 6], base_handle: u16, remote: u16) -> Self {
        Self {
            address,
            base_handle,
            handle: None,
            connects: 0,
            authenticated: false,
            encrypted: false,
            bonded: false,
            peer_opens_hid: false,
            auth_requested: false,
            control_pending: false,
            fail_disconnect: false,
            rejected_pages: 0,
            host_pages: 0,
            local_control: 0,
            local_interrupt: 0,
            remote_control: remote,
            remote_interrupt: remote + 1,
            local_sdp: None,
            discovery: sdp::Discovery::default(),
            connection_requests: 0,
            output_count: 0,
            outgoing_pdu: Vec::new(),
        }
    }
    fn connected(&mut self, rx: &mut Wire, incoming: bool) {
        assert!(self.handle.is_none(), "duplicate ACL for the same peer");
        let h = self.base_handle + self.connects;
        self.handle = Some(h);
        self.connects += 1;
        self.authenticated = false;
        self.encrypted = false;
        self.peer_opens_hid = incoming && self.address == wire::HID_ADDR;
        self.auth_requested = false;
        self.control_pending = false;
        self.local_sdp = None;
        self.discovery = sdp::Discovery::default();
        self.output_count = 0;
        self.outgoing_pdu.clear();
        self.local_interrupt = 0;
        self.local_control = 0;
        rx.event(
            3,
            &[&[0][..], &h.to_le_bytes(), &self.address, &[1, 0]].concat(),
        );
        if self.peer_opens_hid {
            rx.event(0x17, &self.address); // The peer authenticates with its saved key.
        }
    }
    fn lost(&mut self, rx: &mut Wire, reason: u8) {
        let handle = self.handle.take().unwrap();
        self.authenticated = false;
        self.encrypted = false;
        self.local_sdp = None;
        self.local_control = 0;
        self.local_interrupt = 0;
        self.outgoing_pdu.clear();
        rx.event(5, &[&[0][..], &handle.to_le_bytes(), &[reason]].concat());
    }
    fn pdu(&self, rx: &mut Wire, cid: u16, data: &[u8]) {
        let pdu = [words(&[data.len() as u16, cid]).as_slice(), data].concat();
        for (i, frag) in pdu.chunks(32).enumerate() {
            rx.acl(self.handle.unwrap(), if i == 0 { 2 } else { 1 }, frag);
        }
    }
    fn signal(&self, rx: &mut Wire, code: u8, id: u8, body: &[u8]) {
        self.pdu(
            rx,
            1,
            &[&[code, id][..], &(body.len() as u16).to_le_bytes(), body].concat(),
        );
    }
    fn signaling(&mut self, rx: &mut Wire, data: &[u8]) {
        let id = data[1];
        let body = &data[4..];
        assert_eq!(word(&data[2..]) as usize, body.len());
        match data[0] {
            2 => {
                self.connection_requests += 1;
                let local = word(&body[2..]);
                let remote = match word(body) {
                    1 => {
                        assert!(self.local_sdp.replace(local).is_none());
                        self.remote_interrupt + 1
                    }
                    0x11 => {
                        assert!(
                            !self.peer_opens_hid,
                            "host duplicated the peer's HID control open"
                        );
                        assert_eq!(self.address, wire::HID_ADDR, "HID opened on a non-HID peer");
                        assert!(self.discovery.identity_complete && self.local_sdp.is_none());
                        assert_eq!(self.local_control, 0);
                        self.local_control = local;
                        self.remote_control
                    }
                    0x13 => {
                        assert!(
                            !self.peer_opens_hid,
                            "host duplicated the peer's HID interrupt open"
                        );
                        assert_ne!(self.local_control, 0);
                        assert_eq!(self.local_interrupt, 0);
                        self.local_interrupt = local;
                        self.remote_interrupt
                    }
                    psm => panic!("unexpected PSM {psm:#x}"),
                };
                self.signal(rx, 3, id, &words(&[remote, local, 0, 0]));
                self.signal(rx, 4, remote as u8, &words(&[local, 0]));
            }
            3 => {
                assert!(self.peer_opens_hid && self.encrypted);
                assert_eq!(body.len(), 8);
                let remote = word(&body[2..]);
                assert_eq!(id, remote as u8, "wrong identifier for peer's HID open");
                let result = word(&body[4..]);
                if result == 1 {
                    assert_eq!(remote, self.remote_control);
                    assert_eq!(self.local_control, 0);
                    self.control_pending = true;
                    return;
                }
                assert_eq!(result, 0, "host refused the peer's HID open");
                assert!(
                    self.control_pending
                        && self.discovery.identity_complete
                        && self.local_sdp.is_none()
                );
                let local = word(body);
                assert!(local >= 0x40);
                let channel = if remote == self.remote_control {
                    &mut self.local_control
                } else {
                    assert_eq!(remote, self.remote_interrupt);
                    &mut self.local_interrupt
                };
                assert_eq!(*channel, 0);
                *channel = local;
                self.signal(rx, 4, id.wrapping_add(0x10), &words(&[local, 0]));
            }
            4 => {
                let remote = word(body);
                let local = if remote == self.remote_interrupt + 1 {
                    self.local_sdp.unwrap()
                } else if remote == self.remote_control {
                    self.local_control
                } else {
                    assert_eq!(remote, self.remote_interrupt);
                    self.local_interrupt
                };
                self.signal(rx, 5, id, &words(&[local, 0, 0]));
            }
            5 => {
                assert_eq!(word(&body[4..]), 0);
                if self.peer_opens_hid && word(body) == self.remote_control {
                    self.signal(
                        rx,
                        2,
                        self.remote_interrupt as u8,
                        &words(&[0x13, self.remote_interrupt]),
                    );
                }
            }
            6 => {
                let remote = word(body);
                if remote == self.remote_interrupt + 1 {
                    assert_eq!(self.local_sdp.take(), Some(word(&body[2..])));
                } else {
                    let local = if remote == self.remote_interrupt {
                        &mut self.local_interrupt
                    } else {
                        assert_eq!(remote, self.remote_control);
                        &mut self.local_control
                    };
                    assert_ne!(*local, 0, "disconnect on a closed channel");
                    assert_eq!(word(&body[2..]), *local);
                    *local = 0;
                }
                self.signal(rx, 7, id, body);
            }
            code => panic!("unexpected L2CAP signal {code}"),
        }
    }
    fn output_fragment(&mut self, rx: &mut Wire, pb: u16, fragment: &[u8]) {
        match pb {
            2 => {
                assert!(self.outgoing_pdu.is_empty(), "overlapping outbound PDUs");
                assert!(fragment.len() >= 4);
            }
            1 => assert!(!self.outgoing_pdu.is_empty(), "unexpected continuation"),
            _ => panic!("unexpected ACL boundary {pb}"),
        }
        self.outgoing_pdu.extend_from_slice(fragment);
        let expected = 4 + usize::from(word(&self.outgoing_pdu));
        assert!(self.outgoing_pdu.len() <= expected);
        if self.outgoing_pdu.len() < expected {
            return;
        }
        let pdu = core::mem::take(&mut self.outgoing_pdu);
        match word(&pdu[2..]) {
            1 => self.signaling(rx, &pdu[4..]),
            cid if cid == self.remote_interrupt + 1 => {
                let reply = self
                    .discovery
                    .response(&pdu[4..], self.address == wire::HID_ADDR);
                self.pdu(rx, self.local_sdp.unwrap(), &reply);
            }
            cid if cid == self.remote_interrupt => {
                assert_ne!(self.local_interrupt, 0);
                let report = [wire::OUTPUT_LONG, wire::OUTPUT_SHORT][self.output_count % 2];
                assert_eq!(pdu[4], 0xa2, "expected HIDP DATA output");
                assert_eq!(&pdu[5..], report, "output report changed or coalesced");
                self.output_count += 1;
            }
            cid => panic!("unexpected peer CID {cid:#x}"),
        }
    }
}

struct Controller {
    page_scan: bool,
    rx: Wire,
    peers: [Peer; 2],
    wake: Option<Arc<Waker>>,
}
impl Controller {
    fn new() -> Self {
        Self {
            rx: Wire::default(),
            page_scan: false,
            peers: [
                Peer::new(wire::HID_ADDR, 0x123, 0x83),
                Peer::new(wire::OTHER_ADDR, 0x234, 0x93),
            ],
            wake: None,
        }
    }
    fn command(&mut self, op: u16, data: &[u8]) {
        let rx = &mut self.rx;
        match op {
            0x0c03 | 0x0c01 | 0x0c56 | 0x0c45 => rx.complete(op, &[0]),
            0x0c1a => {
                assert!(matches!(data, [0] | [2]));
                self.page_scan = data == [2];
                rx.complete(op, &[0]);
            }
            0x1009 => rx.complete(op, &[&[0], wire::ADAPTER.as_slice()].concat()),
            0x1003 => {
                assert!(data.is_empty());
                rx.complete(op, &[0, 0x20, 0, 0, 0, 0, 0, 0, 0]);
            }
            0x1005 => rx.complete(op, &[0, 32, 0, 0, 1, 0, 0, 0]),
            0x0401 => {
                rx.accepted(op);
                for peer in &self.peers {
                    let class = if peer.address == wire::HID_ADDR {
                        [0x40, 5, 0]
                    } else {
                        [0, 1, 0]
                    };
                    rx.event(
                        0x22,
                        &[&[1][..], &peer.address, &[1, 0], &class, &[0, 0, 0xd0]].concat(),
                    );
                }
                rx.event(1, &[0]);
            }
            0x0411 | 0x0413 | 0x0406 => {
                let h = word(data);
                let p = self
                    .peers
                    .iter_mut()
                    .find(|p| p.handle == Some(h))
                    .expect("unknown handle");
                if op == 0x0406 && core::mem::take(&mut p.fail_disconnect) {
                    rx.event(0x0f, &[0x0c, 1, op as u8, (op >> 8) as u8]);
                    return;
                }
                rx.accepted(op);
                match op {
                    0x0411 => {
                        p.auth_requested = true;
                        if !p.peer_opens_hid {
                            rx.event(0x17, &p.address);
                        } else if p.authenticated {
                            rx.event(6, &[&[0][..], &h.to_le_bytes()].concat());
                        }
                    }
                    0x0413 => {
                        assert!(p.authenticated, "encryption before authentication");
                        assert_eq!(data[2], 1);
                        p.encrypted = true;
                        rx.event(8, &[&[0][..], data].concat());
                    }
                    _ => p.lost(rx, 0x16),
                }
            }
            _ => {
                let addr = data.get(..6).expect("expected address");
                let p = self
                    .peers
                    .iter_mut()
                    .find(|p| p.address == addr)
                    .expect("unknown address");
                let reply = [&[0][..], addr].concat();
                match op {
                    0x0419 => {
                        rx.accepted(op);
                        let mut result = [0; 255];
                        result[1..7].copy_from_slice(addr);
                        let name = if p.address == wire::HID_ADDR {
                            wire::HID_NAME
                        } else {
                            wire::OTHER_NAME
                        };
                        result[7..7 + name.len()].copy_from_slice(name.as_bytes());
                        rx.event(7, &result);
                    }
                    0x040a => {
                        p.rejected_pages += 1;
                        rx.accepted(op);
                    }
                    0x0405 | 0x0409 => {
                        if op == 0x0409 {
                            // WAKE advertises the Peripheral major class. The peer
                            // accepts either role and does not report a Role Change.
                            assert_eq!(&data[6..], &[0]);
                        } else {
                            p.host_pages += 1;
                        }
                        rx.accepted(op);
                        p.connected(rx, op == 0x0409);
                    }
                    0x040c => {
                        assert!(!p.bonded, "reconnect must use the stored link key");
                        rx.complete(op, &reply);
                        rx.event(0x31, addr);
                    }
                    0x042b => {
                        assert_eq!(&data[6..], &[3, 0, 4]);
                        rx.complete(op, &reply);
                        rx.event(0x33, &[addr, &[0; 4]].concat());
                    }
                    0x042c => {
                        rx.complete(op, &reply);
                        rx.event(0x36, &reply);
                        rx.event(0x18, &[addr, &[addr[0]; 16], &[4]].concat());
                        p.bonded = true;
                        p.authenticated = true;
                        rx.event(6, &[&[0][..], &p.handle.unwrap().to_le_bytes()].concat());
                    }
                    0x040b => {
                        assert!(p.bonded);
                        assert_eq!(&data[6..], &[addr[0]; 16]);
                        p.authenticated = true;
                        rx.complete(op, &reply);
                        let handle = p.handle.unwrap().to_le_bytes();
                        if p.peer_opens_hid {
                            // Both fit in one receive(). The host must consume LinkUp
                            // before dispatching this ACL, even if its local auth completes later.
                            p.encrypted = true;
                            rx.event(8, &[&[0][..], &handle, &[1]].concat());
                            p.signal(
                                rx,
                                2,
                                p.remote_control as u8,
                                &words(&[0x11, p.remote_control]),
                            );
                        }
                        if p.auth_requested {
                            rx.event(6, &[&[0][..], &handle].concat());
                        }
                    }
                    _ => panic!("unexpected host HCI opcode {op:#06x}"),
                }
            }
        }
    }
    fn acl(&mut self, frame: &[u8]) {
        let header = word(&frame[1..]);
        let handle = header & 0x0fff;
        assert!(frame.len() - 5 <= 32);
        assert_eq!(usize::from(word(&frame[3..])), frame.len() - 5);
        let p = self
            .peers
            .iter_mut()
            .find(|p| p.handle == Some(handle))
            .expect("unknown ACL");
        assert!(p.encrypted, "host sent plaintext ACL");
        assert!(
            self.rx.acl_in_flight.replace(handle).is_none(),
            "one shared ACL credit exceeded"
        );
        p.output_fragment(&mut self.rx, header >> 12, &frame[5..]);
        // All peer responses are delivered before this credit return, testing backpressure.
        self.rx
            .event(0x13, &[&[1][..], &handle.to_le_bytes(), &[1, 0]].concat());
    }
}

struct Radio(SpinLock<Controller>);
impl H4Transport for Radio {
    fn start(&self, wake: Arc<Waker>) -> Result<(), &'static str> {
        self.0.lock().wake = Some(wake);
        Ok(())
    }
    fn stop(&self) {
        self.0.lock().wake = None;
    }
    fn rx_pending(&self) -> bool {
        !self.0.lock().rx.frames.is_empty()
    }
    fn receive(&self, out: &mut [u8]) -> Result<usize, &'static str> {
        Ok(self.0.lock().rx.receive(out))
    }
    fn send(&self, frame: &[u8]) -> Result<(), &'static str> {
        let mut c = self.0.lock();
        match frame[0] {
            1 => {
                assert_eq!(usize::from(frame[3]), frame.len() - 4);
                c.command(word(&frame[1..]), &frame[4..]);
            }
            2 => c.acl(frame),
            kind => panic!("unexpected H4 {kind}"),
        }
        Ok(())
    }
}

// This endpoint exists only in the QA kernel. Control arguments are scalar; no user pointers.
impl ControlOps for Radio {
    fn control(&self, cmd: u32, arg: usize) -> Result<i32, &'static str> {
        let (result, wake) = {
            let mut controller = self.0.lock();
            let Controller {
                peers,
                rx,
                wake,
                page_scan,
            } = &mut *controller;
            let p = &mut peers[(cmd & 1) as usize];
            let result = match cmd & !1 {
                wire::SEND_INPUT if p.encrypted && p.local_interrupt != 0 => {
                    let report = match arg {
                        0 => wire::PRESS,
                        1 => wire::RELEASE,
                        _ => return Err("unknown input report"),
                    };
                    p.pdu(rx, p.local_interrupt, &[&[0xa1][..], report].concat());
                    0
                }
                wire::CONNECTION_REQUESTS => return Ok(i32::from(p.connection_requests)),
                wire::OPEN_CHANNELS => {
                    return Ok(i32::from(p.local_control != 0) + i32::from(p.local_interrupt != 0));
                }
                wire::OUTPUT_COUNT => return Ok(p.output_count as i32),
                wire::FAIL_DISCONNECT if p.encrypted => {
                    p.fail_disconnect = true;
                    return Ok(0);
                }
                wire::PAGE_SCAN_ENABLED => return Ok(i32::from(*page_scan)),
                wire::CONTROLLER_FAULT => {
                    *page_scan = false;
                    rx.event(0x10, &[1]);
                    0
                }
                wire::REJECTED_PAGES => return Ok(i32::from(p.rejected_pages)),
                wire::HOST_PAGES => return Ok(i32::from(p.host_pages)),
                wire::WAKE if *page_scan && p.handle.is_none() => {
                    rx.event(0x04, &[&p.address[..], &[0, 5, 0, 1]].concat());
                    1
                }
                wire::UNPLUG if p.encrypted => {
                    p.lost(rx, 0x08);
                    0
                }
                _ => return Err("invalid QA radio command or offline peer"),
            };
            (result, wake.clone())
        };
        if let Some(w) = wake {
            w.wake_one();
        }
        Ok(result)
    }
}
impl Device for Radio {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }
    fn name(&self) -> &'static str {
        "bt-qa-radio"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
impl Selectable for Radio {
    fn wait_until_ready(
        &self,
        _: ReadyInterest,
        _: &mut scarlet::arch::Trapframe,
        _: Option<u64>,
        _: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::TimedOut
    }
}
impl MemoryMappingOps for Radio {
    fn get_mapping_info(&self, _: usize, _: usize) -> Result<MemoryMappingInfo, &'static str> {
        Err("no mapping")
    }
    fn supports_mmap(&self) -> bool {
        false
    }
}
fn register() {
    let radio = Arc::new(Radio(SpinLock::new(Controller::new())));
    DeviceManager::get_manager().register_device_with_name("bt-qa-radio".into(), radio.clone());
    bluetooth::register(radio);
}
scarlet::late_initcall!(register);
