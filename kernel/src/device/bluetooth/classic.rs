//! Transport-independent, single-link BR/EDR host. Addresses and link keys use HCI wire order.
//!
//! Feed decoded event codes and parameter slices to `on_event`. Drain `poll` after each
//! event or user request and periodically with a monotonic millisecond clock. Submit each
//! returned command exactly once, in order. A transport failure or `ControllerFault` requires
//! resetting the transport/controller and starting a fresh instance (discard old events).
//!
//! Discovery retains eight distinct peers per scan. Only an explicit selection initiates an
//! ACL. `connect` permits SSP Just Works for that attempt, and `reconnect` uses the
//! caller-provisioned SSP bond. An encrypted link of unknown name gets a separate name lookup.
//! The upper layer owns profile selection, key persistence and ACL packet accounting.

pub type Address = [u8; 6];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    NotReady,
    Busy,
    MissingKey,
    MalformedEvent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Hci(u8),
    Timeout,
    Capacity,
    EncryptionLost,
    LocalDisconnect,
    UnsupportedKey,
    InvalidControllerData,
    HardwareError(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ControllerInfo {
    pub address: Address,
    pub acl_mtu: u16,
    pub acl_packets: u16,
}

/// Names are untrusted bytes, truncated to 64 octets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Name {
    bytes: [u8; 64],
    len: u8,
}

impl Name {
    fn from_bytes(input: &[u8]) -> Self {
        let mut bytes = [0; 64];
        let len = input.iter().take(64).take_while(|b| **b != 0).count();
        bytes[..len].copy_from_slice(&input[..len]);
        Self {
            bytes,
            len: len as u8,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    pub address: Address,
    pub class_of_device: [u8; 3],
    pub rssi: Option<i8>,
    pub name: Option<Name>,
    pub page_scan_repetition_mode: u8,
    /// Bit 15 set when the offset is valid for paging commands.
    pub clock_offset: u16,
}

/// Command parameters can contain a link key. Do not log them.
pub struct Command {
    pub opcode: u16,
    bytes: [u8; 22],
    len: u8,
}

impl Command {
    fn new(opcode: u16, params: &[u8]) -> Self {
        let mut bytes = [0; 22];
        bytes[..params.len()].copy_from_slice(params);
        Self {
            opcode,
            bytes,
            len: params.len() as u8,
        }
    }

    fn addressed(opcode: u16, addr: Address, extra: &[u8]) -> Self {
        let mut cmd = Self::new(opcode, &addr);
        cmd.bytes[6..6 + extra.len()].copy_from_slice(extra);
        cmd.len += extra.len() as u8;
        cmd
    }

    pub fn params(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
}

pub enum Action {
    Command(Command),
    Ready(ControllerInfo),
    PeerFound(Peer),
    InquiryComplete(Result<(), Reason>),
    /// The upper layer owns persistence. Call provide_link_key to authorize future reuse.
    LinkKey {
        address: Address,
        key: [u8; 16],
        key_type: u8,
    },
    LinkUp {
        address: Address,
        handle: u16,
    },
    LinkDown {
        address: Address,
        handle: u16,
        reason: Reason,
    },
    ConnectionFailed {
        address: Address,
        reason: Reason,
    },
    /// Completed outbound ACL packets for this link. Initial capacity is in Ready.
    AclCredits {
        handle: u16,
        packets: u16,
    },
    ControllerFault(Reason),
}

const INQUIRY: u16 = 0x0401;
const INQUIRY_CANCEL: u16 = 0x0402;
const CREATE: u16 = 0x0405;
const DISCONNECT: u16 = 0x0406;
const CREATE_CANCEL: u16 = 0x0408;
const REJECT: u16 = 0x040a;
const KEY_REPLY: u16 = 0x040b;
const NO_KEY: u16 = 0x040c;
const NO_PIN: u16 = 0x040e;
const AUTHENTICATE: u16 = 0x0411;
const ENCRYPT: u16 = 0x0413;
const REMOTE_NAME: u16 = 0x0419;
const NAME_CANCEL: u16 = 0x041a;
const REJECT_SYNC: u16 = 0x042a;
const IO_REPLY: u16 = 0x042b;
const CONFIRM: u16 = 0x042c;
const NO_CONFIRM: u16 = 0x042d;
const NO_PASSKEY: u16 = 0x042f;
const NO_OOB: u16 = 0x0433;
const NO_IO: u16 = 0x0434;
const EVENT_MASK: u16 = 0x0c01;
const RESET: u16 = 0x0c03;
const INQUIRY_MODE: u16 = 0x0c45;
const SSP_MODE: u16 = 0x0c56;
const BUFFER_SIZE: u16 = 0x1005;
const LOCAL_ADDR: u16 = 0x1009;

const COMMAND_MS: u64 = 5_000;
const INQUIRY_MS: u64 = 15_000; // Inquiry_Length 8 * 1.28 seconds plus scheduling margin.
const PAGE_MS: u64 = 20_000;
const PAIR_MS: u64 = 60_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Owner {
    Init,
    Discovery,
    Link,
    Other,
}

struct Queued {
    command: Command,
    owner: Owner,
}

struct Pending {
    reply_address: Option<Address>,
    opcode: u16,
    owner: Owner,
    deadline: u64,
}

// Tiny bounded FIFOs; compacting also lets us revoke queued commands on link loss.
struct Queue<T, const N: usize>([Option<T>; N]);

impl<T, const N: usize> Queue<T, N> {
    const fn new() -> Self {
        Self([const { None }; N])
    }

    fn push(&mut self, value: T) -> bool {
        if let Some(slot) = self.0.iter_mut().find(|v| v.is_none()) {
            *slot = Some(value);
            true
        } else {
            false
        }
    }

    fn pop(&mut self) -> Option<T> {
        let item = self.0[0].take()?;
        self.0.rotate_left(1);
        Some(item)
    }

    fn retain(&mut self, keep: impl Fn(&T) -> bool) {
        let mut next = 0;
        for i in 0..N {
            if let Some(v) = self.0[i].take() {
                if keep(&v) {
                    self.0[next] = Some(v);
                    next += 1;
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scan {
    Idle,
    Queued,
    Inquiry,
    CancelInquiry,
    Names,
    Name(Address),
    CancelName(Address),
}

#[derive(Clone, Copy)]
struct Seen {
    peer: Peer,
    dirty: bool,
    needs_name: bool,
    complete_name: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Queued,
    Paging,
    CancelPage,
    Authenticating,
    Encrypting,
    Up,
    Disconnecting, // User-requested close of an encrypted link; report completion on disconnection.
    Closing,
}

struct Link {
    address: Address,
    allow_pairing: bool,
    phase: Phase,
    handle: Option<u16>,
    encrypted: bool,
    ssp: bool,
    new_key: Option<([u8; 16], u8)>,
    deadline: Option<u64>,
}

pub struct Classic {
    ready: bool,
    fault: bool,
    terminal: Option<Reason>,
    info: ControllerInfo,
    inquiry_mode: u8,
    credits: u8,
    credit_deadline: Option<u64>,
    commands: Queue<Queued, 8>,
    pending: Option<Pending>,
    actions: Queue<Action, 8>,
    scan: Scan,
    scan_deadline: Option<u64>,
    scan_result: Result<(), Reason>,
    link_name: Option<Address>,
    disconnect_requested: bool,
    seen: [Option<Seen>; 8],
    link: Option<Link>,
    key: Option<(Address, [u8; 16])>,
}

impl Classic {
    pub fn new() -> Self {
        let mut commands = Queue::new();
        commands.push(Queued {
            command: Command::new(RESET, &[]),
            owner: Owner::Init,
        });
        Self {
            ready: false,
            fault: false,
            terminal: None,
            info: ControllerInfo::default(),
            inquiry_mode: 2,
            credits: 1,
            credit_deadline: None,
            commands,
            pending: None,
            actions: Queue::new(),
            scan: Scan::Idle,
            scan_deadline: None,
            scan_result: Ok(()),
            link_name: None,
            disconnect_requested: false,
            seen: [None; 8],
            link: None,
            key: None,
        }
    }

    pub fn start_inquiry(&mut self) -> Result<(), Error> {
        self.check_idle_link()?;
        if self.scan != Scan::Idle {
            return Err(Error::Busy);
        }
        self.seen.fill(None);
        self.scan = Scan::Queued;
        self.scan_result = Ok(());
        Ok(())
    }

    /// Select this address for an outgoing ACL and permit SSP Just Works during authentication.
    pub fn connect(&mut self, addr: Address) -> Result<(), Error> {
        self.select(addr, true)
    }

    /// Select a bonded peer. Never consent to a fresh pairing on this attempt.
    pub fn reconnect(&mut self, addr: Address) -> Result<(), Error> {
        self.check_idle_link()?;
        if !self.key.as_ref().is_some_and(|(a, _)| *a == addr) {
            return Err(Error::MissingKey);
        }
        self.select(addr, false)
    }

    /// Install or replace the one host-side SSP bond. The caller supplies a key of known
    /// SSP origin (its type is unavailable from the key bytes). Provisioning never selects a peer.
    pub fn provide_link_key(&mut self, addr: Address, key: [u8; 16]) {
        self.key = Some((addr, key));
    }

    /// Stop the selected attempt or link; drain poll immediately after the request.
    pub fn disconnect(&mut self) -> Result<(), Error> {
        if !self.ready {
            return Err(Error::NotReady);
        }
        if self.link.is_some() {
            self.disconnect_requested = true;
        }
        Ok(())
    }

    /// Earliest absolute wake-up, zero for immediate work, or None to wait for input.
    pub fn next_deadline_ms(&self) -> Option<u64> {
        if self.actions.0[0].is_some() || self.terminal.is_some() {
            return Some(0);
        }
        if self.fault {
            return None;
        }
        if self.disconnect_requested
            || self.scan_finished()
            || self.seen.iter().flatten().any(|s| s.dirty)
            || self.pending.is_none()
                && (self.credits > 0 || self.credit_deadline.is_none())
                && (self.commands.0[0].is_some() || self.planned().is_some())
        {
            return Some(0);
        }
        [
            self.pending.as_ref().map(|p| p.deadline),
            self.credit_deadline,
            self.scan_deadline,
            self.link.as_ref().and_then(|l| l.deadline),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn check_idle_link(&self) -> Result<(), Error> {
        if !self.ready {
            return Err(Error::NotReady);
        }
        if self.link.is_some() {
            return Err(Error::Busy);
        }
        Ok(())
    }

    fn select(&mut self, address: Address, allow_pairing: bool) -> Result<(), Error> {
        self.check_idle_link()?;
        self.link = Some(Link {
            address,
            allow_pairing,
            phase: Phase::Queued,
            handle: None,
            encrypted: false,
            ssp: false,
            new_key: None,
            deadline: None,
        });
        Ok(())
    }
}

impl Default for Classic {
    fn default() -> Self {
        Self::new()
    }
}

impl Classic {
    fn enqueue(&mut self, owner: Owner, command: Command) {
        if !self.commands.push(Queued { owner, command }) {
            self.fail(Reason::Capacity);
        }
    }

    fn emit(&mut self, action: Action) {
        // Reserve one slot for the active link's final notification on controller failure.
        if self.actions.0[6].is_some() {
            self.fail(Reason::Capacity);
        } else {
            self.actions.push(action);
        }
    }

    fn fail(&mut self, reason: Reason) {
        if self.fault {
            return;
        }
        self.fault = true;
        self.ready = false;
        self.terminal = Some(reason);
        self.commands = Queue::new();
        self.pending = None;
        self.key = None;
        if let Some(link) = self.link.take() {
            match (link.phase, link.handle) {
                (Phase::Up | Phase::Disconnecting, Some(handle)) => {
                    self.actions.push(Action::LinkDown {
                        address: link.address,
                        handle,
                        reason,
                    });
                }
                (Phase::Closing | Phase::CancelPage, _) => (),
                _ => {
                    self.actions.push(Action::ConnectionFailed {
                        address: link.address,
                        reason,
                    });
                }
            }
        }
    }

    fn revoke(&mut self, owner: Owner) {
        self.commands.retain(|q| q.owner != owner);
        // Still wait for the acknowledgement and honor its credits; it belongs to an old procedure.
        if let Some(p) = self.pending.as_mut().filter(|p| p.owner == owner) {
            p.owner = Owner::Other;
        }
    }

    fn finish_scan(&mut self) {
        self.scan = Scan::Idle;
        self.scan_deadline = None;
        self.revoke(Owner::Discovery);
        if self.link_name.take().is_none() {
            self.emit(Action::InquiryComplete(self.scan_result));
        }
    }

    fn paging(&self, opcode: u16, address: Address) -> Command {
        let peer = self
            .seen
            .iter()
            .flatten()
            .find(|s| s.peer.address == address);
        let (mode, offset) = peer.map_or((2, 0), |s| {
            (s.peer.page_scan_repetition_mode, s.peer.clock_offset)
        });
        let [lo, hi] = offset.to_le_bytes();
        if opcode == CREATE {
            // DM/DH 1/3/5-slot packets, allow role switch.
            Command::addressed(CREATE, address, &[0x18, 0xcc, mode, 0, lo, hi, 1])
        } else {
            Command::addressed(REMOTE_NAME, address, &[mode, 0, lo, hi])
        }
    }

    fn planned(&self) -> Option<Queued> {
        let selecting = self.link.as_ref().is_some_and(|l| l.phase == Phase::Queued);
        let cmd = match self.scan {
            Scan::Queued => Command::new(INQUIRY, &[0x33, 0x8b, 0x9e, 8, 0]),
            Scan::Inquiry if selecting || self.scan_result.is_err() => {
                Command::new(INQUIRY_CANCEL, &[])
            }
            Scan::Name(addr) if selecting || self.scan_result.is_err() => {
                Command::addressed(NAME_CANCEL, addr, &[])
            }
            Scan::Names => {
                let addr = self.link_name.or_else(|| {
                    self.seen
                        .iter()
                        .flatten()
                        .find(|s| s.needs_name)
                        .map(|s| s.peer.address)
                })?;
                self.paging(REMOTE_NAME, addr)
            }
            Scan::Idle if selecting => {
                return Some(Queued {
                    command: self.paging(CREATE, self.link.as_ref()?.address),
                    owner: Owner::Link,
                });
            }
            _ => return None,
        };
        Some(Queued {
            command: cmd,
            owner: Owner::Discovery,
        })
    }

    fn sent(&mut self, command: &Command, now: u64) {
        match command.opcode {
            INQUIRY => {
                self.scan = Scan::Inquiry;
                self.scan_deadline = Some(now.saturating_add(INQUIRY_MS));
            }
            REMOTE_NAME => {
                let addr = address(command.params());
                self.scan = Scan::Name(addr);
                self.scan_deadline = Some(now.saturating_add(PAGE_MS));
                if let Some(s) = self
                    .seen
                    .iter_mut()
                    .flatten()
                    .find(|s| s.peer.address == addr)
                {
                    s.needs_name = false;
                }
            }
            INQUIRY_CANCEL | NAME_CANCEL => {
                self.scan = if command.opcode == INQUIRY_CANCEL {
                    Scan::CancelInquiry
                } else {
                    Scan::CancelName(address(command.params()))
                };
                self.scan_deadline = Some(now.saturating_add(COMMAND_MS));
            }
            CREATE => {
                if let Some(l) = &mut self.link {
                    l.phase = Phase::Paging;
                    l.deadline = Some(now.saturating_add(PAGE_MS));
                }
            }
            _ => (),
        }
    }

    fn scan_finished(&self) -> bool {
        let selecting = self.link.as_ref().is_some_and(|l| l.phase == Phase::Queued);
        match self.scan {
            Scan::Queued => selecting,
            Scan::Names => {
                selecting
                    || self.scan_result.is_err()
                    || (self.link_name.is_none()
                        && !self.seen.iter().flatten().any(|s| s.needs_name))
            }
            _ => false,
        }
    }

    /// Poll until None after each input and periodically to enforce deadlines.
    pub fn poll(&mut self, now_ms: u64) -> Option<Action> {
        self.tick(now_ms);
        if !self.fault && self.scan_finished() {
            // Publish the final remote-name update before management observes scan completion.
            if let Some(s) = self.seen.iter_mut().flatten().find(|s| s.dirty) {
                s.dirty = false;
                return Some(Action::PeerFound(s.peer));
            }
            self.finish_scan();
        }
        if let Some(a) = self.actions.pop() {
            return Some(a);
        }
        if self.fault {
            return self.terminal.take().map(Action::ControllerFault);
        }
        if self.pending.is_none() {
            let planned = self.planned();
            let work = self.commands.0[0].is_some() || planned.is_some();
            if self.credits == 0 && work {
                self.credit_deadline
                    .get_or_insert(now_ms.saturating_add(COMMAND_MS));
            } else if self.credits > 0 {
                if let Some(q) = self.commands.pop().or(planned) {
                    self.sent(&q.command, now_ms);
                    self.credits -= 1;
                    self.pending = Some(Pending {
                        reply_address: match q.command.opcode {
                            KEY_REPLY | NO_KEY | NO_PIN | IO_REPLY | NO_IO | CONFIRM
                            | NO_CONFIRM | NO_PASSKEY | NO_OOB | CREATE_CANCEL | NAME_CANCEL => {
                                Some(address(q.command.params()))
                            }
                            _ => None,
                        },
                        opcode: q.command.opcode,
                        owner: q.owner,
                        deadline: now_ms.saturating_add(COMMAND_MS),
                    });
                    return Some(Action::Command(q.command));
                }
            }
            if !work {
                self.credit_deadline = None;
            }
        }
        let s = self.seen.iter_mut().flatten().find(|s| s.dirty)?;
        s.dirty = false;
        Some(Action::PeerFound(s.peer))
    }

    fn tick(&mut self, now: u64) {
        if self.fault {
            return;
        }
        if core::mem::take(&mut self.disconnect_requested) {
            self.stop_link(Reason::LocalDisconnect, now);
        }
        if self.pending.as_ref().is_some_and(|p| now >= p.deadline)
            || self.credit_deadline.is_some_and(|d| now >= d)
        {
            // Without an acknowledgement/credit the command stream cannot be resynchronized.
            self.fail(Reason::Timeout);
            return;
        }
        if self.scan_deadline.is_some_and(|d| now >= d) {
            self.scan_deadline = None;
            match self.scan {
                Scan::Inquiry | Scan::Name(_) => self.scan_result = Err(Reason::Timeout),
                Scan::CancelInquiry | Scan::CancelName(_) => self.fail(Reason::Timeout),
                _ => (),
            }
        }
        if let Some(l) = self
            .link
            .as_ref()
            .filter(|l| l.deadline.is_some_and(|d| now >= d))
        {
            match l.phase {
                Phase::Closing | Phase::CancelPage | Phase::Disconnecting => {
                    self.fail(Reason::Timeout)
                }
                _ => self.stop_link(Reason::Timeout, now),
            }
        }
    }

    fn stop_link(&mut self, reason: Reason, now: u64) {
        let Some(l) = self.link.as_ref() else { return };
        let (addr, phase, handle) = (l.address, l.phase, l.handle);
        if phase == Phase::Disconnecting && reason != Reason::LocalDisconnect {
            // Encryption or authentication was lost while a local close was in flight.
            self.report_loss(addr, phase, handle, reason);
            if let Some(l) = &mut self.link {
                l.phase = Phase::Closing;
            }
            return;
        }
        if matches!(
            phase,
            Phase::Closing | Phase::CancelPage | Phase::Disconnecting
        ) {
            return;
        }
        self.cancel_link_name();
        // A failed Create status means there is no page to cancel or terminal event to await.
        if phase != Phase::Paging {
            self.revoke(Owner::Link);
        }
        let wait_for_disconnect = reason == Reason::LocalDisconnect && phase == Phase::Up;
        if !wait_for_disconnect {
            self.report_loss(addr, phase, handle, reason);
        }
        let Some(l) = self.link.as_mut() else { return };
        l.new_key = None;
        l.deadline = Some(now.saturating_add(COMMAND_MS * 2));
        if let Some(handle) = handle {
            l.phase = if wait_for_disconnect {
                Phase::Disconnecting
            } else {
                Phase::Closing
            };
            self.enqueue(Owner::Link, disconnect(handle));
        } else if phase == Phase::Paging {
            l.phase = Phase::CancelPage;
            self.enqueue(Owner::Link, Command::addressed(CREATE_CANCEL, addr, &[]));
        } else {
            self.link = None;
        }
    }

    fn report_loss(&mut self, address: Address, phase: Phase, handle: Option<u16>, reason: Reason) {
        match (phase, handle) {
            (Phase::Up | Phase::Disconnecting, Some(handle)) => self.emit(Action::LinkDown {
                address,
                handle,
                reason,
            }),
            (Phase::Closing | Phase::CancelPage, _) => (),
            _ => self.emit(Action::ConnectionFailed { address, reason }),
        }
    }

    fn cancel_link_name(&mut self) {
        if self.link_name.is_some() {
            self.scan_result = Err(Reason::LocalDisconnect);
        }
    }

    fn end_link(&mut self, reason: Reason) {
        self.cancel_link_name();
        if let Some(l) = self.link.as_ref() {
            self.report_loss(l.address, l.phase, l.handle, reason);
        }
        self.link = None;
        self.revoke(Owner::Link);
    }
}

fn address(data: &[u8]) -> Address {
    // Callers have validated the enclosing fixed-size HCI fields.
    data[..6].try_into().unwrap()
}

fn word(data: &[u8]) -> u16 {
    u16::from_le_bytes([data[0], data[1]])
}

fn disconnect(handle: u16) -> Command {
    let [lo, hi] = handle.to_le_bytes();
    Command::new(DISCONNECT, &[lo, hi, 0x13])
}

impl Classic {
    /// Accept a decoded HCI event (no transport header, event code, or length in params).
    pub fn on_event(&mut self, now_ms: u64, code: u8, p: &[u8]) -> Result<(), Error> {
        self.tick(now_ms);
        if self.fault {
            return Err(Error::NotReady);
        }
        if p.len() > 255 {
            return Err(Error::MalformedEvent);
        }
        if code == 0x0e || code == 0x0f {
            return self.ack(now_ms, code, p);
        }
        if code == 0x10 {
            let [error] = *p else {
                return Err(Error::MalformedEvent);
            };
            self.fail(Reason::HardwareError(error));
            return Ok(());
        }
        if !self.ready {
            return Ok(());
        }
        match code {
            0x01 if p.len() == 1 => {
                if matches!(self.scan, Scan::Inquiry | Scan::CancelInquiry) {
                    self.scan_deadline = None;
                    if p[0] != 0 {
                        self.scan_result = Err(Reason::Hci(p[0]));
                    }
                    self.scan = Scan::Names;
                }
            }
            0x02 | 0x22 | 0x2f => self.inquiry_results(code, p)?,
            0x03 if p.len() == 11 => self.connection_complete(now_ms, p)?,
            0x04 if p.len() == 10 => {
                // Outgoing-only policy, including when the incoming address matches our selection.
                self.enqueue(
                    Owner::Other,
                    Command::addressed(
                        if p[9] == 1 { REJECT } else { REJECT_SYNC },
                        address(p),
                        &[0x0f],
                    ),
                );
            }
            0x05 if p.len() == 4 => {
                let handle = word(&p[1..]);
                if p[0] == 0 {
                    // Controller handles may be reused immediately. Retire an unsent rejection
                    // teardown if that unsolicited ACL disappeared while command credits were zero.
                    self.commands.retain(|q| {
                        !(q.command.opcode == DISCONNECT && word(q.command.params()) == handle)
                    });
                }
                if self.has_handle(handle) {
                    if p[0] == 0 {
                        self.end_link(Reason::Hci(p[3]));
                    } else {
                        self.stop_link(Reason::Hci(p[0]), now_ms);
                    }
                }
            }
            0x06 if p.len() == 3 => self.auth_complete(now_ms, p),
            0x07 if p.len() >= 7 => self.remote_name_complete(p),
            0x08 if p.len() == 4 => self.encryption_change(now_ms, p),
            0x59 if p.len() == 5 => self.encryption_change(now_ms, p),
            0x13 => self.completed_packets(p)?,
            0x16 | 0x17 | 0x31 | 0x34 | 0x35 if p.len() == 6 => {
                self.security_reply(code, address(p))
            }
            0x33 if p.len() == 10 => self.security_reply(code, address(p)),
            0x18 if p.len() == 23 => self.link_key(now_ms, p),
            0x30 if p.len() == 3 => {
                if p[0] != 0 && self.has_handle(word(&p[1..])) {
                    self.stop_link(Reason::Hci(p[0]), now_ms);
                }
            }
            0x36 if p.len() == 7 => {
                if self.link.as_ref().is_some_and(|l| {
                    l.address == address(&p[1..])
                        && l.allow_pairing
                        && matches!(l.phase, Phase::Authenticating | Phase::Encrypting)
                }) {
                    if p[0] != 0 {
                        self.stop_link(Reason::Hci(p[0]), now_ms);
                    } else {
                        self.confirm_ssp();
                    }
                }
            }
            0x01 | 0x03..=0x08 | 0x16..=0x18 | 0x30 | 0x31 | 0x33..=0x36 | 0x59 => {
                return Err(Error::MalformedEvent);
            }
            _ => (),
        }
        Ok(())
    }

    fn ack(&mut self, now: u64, code: u8, p: &[u8]) -> Result<(), Error> {
        let (credits, opcode, result) = match code {
            0x0e if p.len() >= 3 => (p[0], word(&p[1..]), &p[3..]),
            0x0f if p.len() == 4 => (p[1], word(&p[2..]), &p[..1]),
            _ => return Err(Error::MalformedEvent),
        };
        // Opcode zero is a standalone credit update, with no return parameters.
        if opcode != 0 && result.is_empty() {
            return Err(Error::MalformedEvent);
        }
        self.credits = credits;
        if credits > 0 {
            self.credit_deadline = None;
        }
        let Some(pending) = self.pending.as_ref().filter(|p| p.opcode == opcode) else {
            return Ok(());
        };
        let status = result[0];
        if status == 0 {
            let asynchronous = matches!(
                opcode,
                INQUIRY
                    | CREATE
                    | DISCONNECT
                    | REJECT
                    | REJECT_SYNC
                    | REMOTE_NAME
                    | AUTHENTICATE
                    | ENCRYPT
            );
            if (code == 0x0f) != asynchronous
                || (opcode == LOCAL_ADDR && result.len() != 7)
                || (opcode == BUFFER_SIZE && result.len() != 8)
                || pending
                    .reply_address
                    .is_some_and(|a| result.len() != 7 || result[1..] != a)
            {
                return Err(Error::MalformedEvent);
            }
        }
        let owner = self.pending.take().unwrap().owner;
        if owner == Owner::Init {
            self.init_ack(opcode, result);
        } else if status != 0 {
            let reason = Reason::Hci(status);
            match (owner, opcode) {
                (Owner::Discovery, REMOTE_NAME) => self.name_finished(),
                (Owner::Discovery, INQUIRY) => {
                    self.scan_result = Err(reason);
                    self.finish_scan();
                }
                // Cancellation can race the terminal procedure event. Keep its deadline.
                (Owner::Discovery, NAME_CANCEL | INQUIRY_CANCEL) => (),
                (Owner::Link, CREATE) => self.end_link(reason),
                (Owner::Link, DISCONNECT) if status == 0x02 => self.end_link(reason),
                (Owner::Link, DISCONNECT) => self.fail(reason),
                (Owner::Link, CREATE_CANCEL) => (),
                (Owner::Link, _) => self.stop_link(reason, now),
                _ => (),
            }
        } else if owner == Owner::Link && opcode == KEY_REPLY {
            self.confirm_ssp();
        } else if owner == Owner::Discovery
            && opcode == INQUIRY_CANCEL
            && self.scan == Scan::CancelInquiry
        {
            self.finish_scan();
        }
        Ok(())
    }

    fn init_ack(&mut self, opcode: u16, result: &[u8]) {
        let status = result[0];
        if status != 0 {
            if opcode == INQUIRY_MODE && matches!(status, 0x01 | 0x11 | 0x12) {
                // Controllers with RSSI-only or standard inquiry still provide usable discovery.
                if self.inquiry_mode == 2 {
                    self.inquiry_mode = 1;
                    self.enqueue(Owner::Init, Command::new(INQUIRY_MODE, &[1]));
                } else {
                    self.inquiry_mode = 0; // Reset default.
                    self.initialized();
                }
            } else {
                self.fail(Reason::Hci(status));
            }
            return;
        }
        let next = match opcode {
            RESET => Command::new(LOCAL_ADDR, &[]),
            LOCAL_ADDR => {
                self.info.address = address(&result[1..]);
                Command::new(BUFFER_SIZE, &[])
            }
            BUFFER_SIZE => {
                self.info.acl_mtu = word(&result[1..]);
                self.info.acl_packets = word(&result[4..]);
                if self.info.acl_mtu == 0 || self.info.acl_packets == 0 {
                    self.fail(Reason::InvalidControllerData);
                    return;
                }
                let mut mask = 0u64;
                for event in [
                    1, 2, 3, 4, 5, 6, 7, 8, 0x10, 0x16, 0x17, 0x18, 0x22, 0x2f, 0x30, 0x31, 0x32,
                    0x33, 0x34, 0x35, 0x36,
                ] {
                    mask |= 1u64 << (event - 1);
                }
                Command::new(EVENT_MASK, &mask.to_le_bytes())
            }
            EVENT_MASK => Command::new(SSP_MODE, &[1]),
            SSP_MODE => Command::new(INQUIRY_MODE, &[self.inquiry_mode]),
            INQUIRY_MODE => {
                self.initialized();
                return;
            }
            _ => return,
        };
        self.enqueue(Owner::Init, next);
    }

    fn initialized(&mut self) {
        self.ready = true;
        self.emit(Action::Ready(self.info));
    }

    fn inquiry_results(&mut self, code: u8, p: &[u8]) -> Result<(), Error> {
        let (&count, records) = p.split_first().ok_or(Error::MalformedEvent)?;
        let stride = if code == 0x2f { 254 } else { 14 };
        if count == 0
            || records.len() != usize::from(count) * stride
            || (code == 0x2f && count != 1)
        {
            return Err(Error::MalformedEvent);
        }
        if self.scan != Scan::Inquiry {
            return Ok(());
        }
        for r in records.chunks_exact(stride) {
            let addr = address(r);
            let class = if code == 0x02 { 9 } else { 8 };
            let Some(slot) = self.seen_slot(addr, false) else {
                continue;
            };
            slot.peer.page_scan_repetition_mode = r[6].min(2);
            slot.peer
                .class_of_device
                .copy_from_slice(&r[class..class + 3]);
            slot.peer.clock_offset = word(&r[class + 3..]) | 0x8000;
            if code != 0x02 {
                slot.peer.rssi = Some(r[13] as i8);
            }
            if code == 0x2f {
                if let Some((name, complete)) = eir_name(&r[14..]) {
                    if !slot.complete_name || complete {
                        slot.peer.name = Some(name);
                        slot.needs_name = !complete;
                        slot.complete_name = complete;
                    }
                }
            }
            slot.dirty = true;
        }
        Ok(())
    }

    fn seen_slot(&mut self, addr: Address, selected: bool) -> Option<&mut Seen> {
        let i = self
            .seen
            .iter()
            .position(|s| s.is_some_and(|s| s.peer.address == addr))
            .or_else(|| self.seen.iter().position(Option::is_none))
            .or_else(|| selected.then_some(self.seen.len() - 1))?;
        if !self.seen[i].is_some_and(|s| s.peer.address == addr) {
            self.seen[i] = Some(Seen {
                peer: Peer {
                    address: addr,
                    class_of_device: [0; 3],
                    name: None,
                    rssi: None,
                    page_scan_repetition_mode: 2,
                    clock_offset: 0,
                },
                dirty: false,
                needs_name: true,
                complete_name: false,
            });
        }
        self.seen[i].as_mut()
    }

    fn remote_name_complete(&mut self, p: &[u8]) {
        let addr = address(&p[1..]);
        if !matches!(self.scan, Scan::Name(a) | Scan::CancelName(a) if a == addr) {
            return;
        }
        if p[0] == 0 && !p[7..].is_empty() && p[7] != 0 {
            if let Some(s) = self.seen_slot(addr, false) {
                s.peer.name = Some(Name::from_bytes(&p[7..]));
                s.complete_name = true;
                s.dirty = true;
            }
        }
        self.name_finished();
    }

    fn name_finished(&mut self) {
        self.scan_deadline = None;
        if self.link_name.is_some() {
            self.finish_scan();
        } else {
            self.scan = Scan::Names;
        }
    }

    fn completed_packets(&mut self, p: &[u8]) -> Result<(), Error> {
        let (&count, pairs) = p.split_first().ok_or(Error::MalformedEvent)?;
        if pairs.len() != usize::from(count) * 4 {
            return Err(Error::MalformedEvent);
        }
        if let Some(l) = self.link.as_ref().filter(|l| l.phase == Phase::Up) {
            let handle = l.handle.unwrap();
            let mut packets = 0u16;
            for pair in pairs.chunks_exact(4).filter(|pair| word(pair) == handle) {
                packets = packets
                    .checked_add(word(&pair[2..]))
                    .ok_or(Error::MalformedEvent)?;
            }
            if packets != 0 {
                self.emit(Action::AclCredits { handle, packets });
            }
        }
        Ok(())
    }
}

fn eir_name(mut data: &[u8]) -> Option<(Name, bool)> {
    let mut shortened = None;
    while let Some((&len, rest)) = data.split_first() {
        if len == 0 || usize::from(len) > rest.len() {
            break;
        }
        let (field, tail) = rest.split_at(usize::from(len));
        if field.len() > 1 && matches!(field[0], 8 | 9) {
            let name = Name::from_bytes(&field[1..]);
            if name.len != 0 {
                if field[0] == 9 {
                    return Some((name, true));
                }
                shortened = Some((name, false));
            }
        }
        data = tail;
    }
    shortened
}

impl Classic {
    fn has_handle(&self, handle: u16) -> bool {
        self.link.as_ref().is_some_and(|l| l.handle == Some(handle))
    }

    fn connection_complete(&mut self, now: u64, p: &[u8]) -> Result<(), Error> {
        let status = p[0];
        let handle = word(&p[1..]);
        let addr = address(&p[3..]);
        let selected = self.link.as_ref().is_some_and(|l| {
            l.address == addr && matches!(l.phase, Phase::Paging | Phase::CancelPage)
        });
        if status != 0 {
            if selected {
                self.end_link(Reason::Hci(status));
            }
            return Ok(());
        }
        if handle > 0x0eff {
            return Err(Error::MalformedEvent);
        }
        if self.has_handle(handle) {
            if !self
                .link
                .as_ref()
                .is_some_and(|l| l.address == addr && p[9] == 1)
            {
                self.fail(Reason::InvalidControllerData);
            }
            return Ok(());
        }
        if !selected || p[9] != 1 {
            self.enqueue(Owner::Other, disconnect(handle));
            return Ok(());
        }
        let l = self.link.as_mut().unwrap();
        l.handle = Some(handle);
        if l.phase == Phase::CancelPage {
            // Cancellation raced link creation. Keep the handle quarantined until disconnection.
            l.phase = Phase::Closing;
            l.deadline = Some(now.saturating_add(COMMAND_MS * 2));
            self.revoke(Owner::Link);
            self.enqueue(Owner::Link, disconnect(handle));
        } else {
            l.phase = Phase::Authenticating;
            l.encrypted = p[10] == 1;
            l.deadline = Some(now.saturating_add(PAIR_MS));
            self.enqueue(
                Owner::Link,
                Command::new(AUTHENTICATE, &handle.to_le_bytes()),
            );
        }
        Ok(())
    }

    fn auth_complete(&mut self, now: u64, p: &[u8]) {
        let handle = word(&p[1..]);
        if !self.has_handle(handle) {
            return;
        }
        if p[0] != 0 {
            self.stop_link(Reason::Hci(p[0]), now);
            return;
        }
        let l = self.link.as_mut().unwrap();
        if l.phase != Phase::Authenticating {
            return;
        }
        l.phase = Phase::Encrypting;
        if l.encrypted {
            self.maybe_link_up();
        } else {
            l.deadline = Some(now.saturating_add(PAGE_MS));
            let [lo, hi] = handle.to_le_bytes();
            self.enqueue(Owner::Link, Command::new(ENCRYPT, &[lo, hi, 1]));
        }
    }

    fn encryption_change(&mut self, now: u64, p: &[u8]) {
        if !self.has_handle(word(&p[1..])) {
            return;
        }
        if p[0] != 0 {
            self.stop_link(Reason::Hci(p[0]), now);
        } else if !matches!(p[3], 1 | 2) {
            self.stop_link(Reason::EncryptionLost, now);
        } else {
            let l = self.link.as_mut().unwrap();
            l.encrypted = true;
            if l.phase == Phase::Encrypting {
                self.maybe_link_up();
            }
        }
    }

    fn confirm_ssp(&mut self) {
        if let Some(l) = self.link.as_mut() {
            l.ssp = true;
            self.maybe_link_up();
        }
    }

    fn maybe_link_up(&mut self) {
        let Some(l) = self
            .link
            .as_mut()
            .filter(|l| l.phase == Phase::Encrypting && l.encrypted && l.ssp)
        else {
            return;
        };

        let addr = l.address;
        let handle = l.handle.unwrap();
        l.phase = Phase::Up;
        l.deadline = None;
        let new_key = l.new_key.take();
        if let Some((key, key_type)) = new_key {
            self.emit(Action::LinkKey {
                address: addr,
                key,
                key_type,
            });
        }
        if self.fault {
            return;
        }
        self.emit(Action::LinkUp {
            address: addr,
            handle,
        });
        if self.fault {
            return;
        }
        // A stored bond can be selected before inquiry in this boot. Resolve its name
        // independently of link readiness so unsupported/nameless devices still get an ACL.
        if self.seen_slot(addr, true).is_some_and(|s| !s.complete_name) {
            self.link_name = Some(addr);
            self.scan_result = Ok(());
            self.scan = Scan::Names;
        }
    }

    fn security_reply(&mut self, code: u8, addr: Address) {
        let selected = self.link.as_ref().filter(|l| {
            l.address == addr
                && matches!(
                    l.phase,
                    Phase::Authenticating | Phase::Encrypting | Phase::Up
                )
        });
        let pairing = selected.is_some_and(|l| l.allow_pairing && l.phase == Phase::Authenticating);
        let key = self.key.as_ref().filter(|(a, _)| {
            *a == addr && selected.is_some_and(|l| !l.allow_pairing || l.phase == Phase::Up)
        });
        let cmd = match code {
            0x16 => Command::addressed(NO_PIN, addr, &[]),
            0x17 => match key {
                Some((_, key)) => Command::addressed(KEY_REPLY, addr, key),
                None => Command::addressed(NO_KEY, addr, &[]),
            },
            // NoInputNoOutput, no OOB data, general bonding without MITM.
            0x31 if pairing => Command::addressed(IO_REPLY, addr, &[3, 0, 4]),
            0x31 => Command::addressed(NO_IO, addr, &[0x18]),
            0x33 if pairing => Command::addressed(CONFIRM, addr, &[]),
            0x33 => Command::addressed(NO_CONFIRM, addr, &[]),
            0x34 => Command::addressed(NO_PASSKEY, addr, &[]),
            0x35 => Command::addressed(NO_OOB, addr, &[]),
            _ => return,
        };
        // Positive replies for a lost link must never survive in the command queue.
        let owner = if selected.is_some() {
            Owner::Link
        } else {
            Owner::Other
        };
        self.enqueue(owner, cmd);
    }

    fn link_key(&mut self, now: u64, p: &[u8]) {
        let addr = address(p);
        let Some(l) = self.link.as_mut().filter(|l| {
            l.address == addr
                && matches!(
                    l.phase,
                    Phase::Authenticating | Phase::Encrypting | Phase::Up
                )
        }) else {
            return;
        };
        let kind = p[22];
        // Only SSP combination keys and changes to an existing SSP key are supported.
        if !(matches!(kind, 4 | 5 | 7 | 8) && l.allow_pairing
            || kind == 6 && self.key.as_ref().is_some_and(|(a, _)| *a == addr))
        {
            self.stop_link(Reason::UnsupportedKey, now);
            return;
        }
        let key = p[6..22].try_into().unwrap();
        if l.phase == Phase::Up {
            self.emit(Action::LinkKey {
                address: addr,
                key,
                key_type: kind,
            });
        } else {
            l.new_key = Some((key, kind));
            self.confirm_ssp();
        }
    }
}

#[cfg(test)]
#[path = "classic_tests.rs"]
mod tests;
