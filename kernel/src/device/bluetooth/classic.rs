//! Transport-independent BR/EDR host with up to eight concurrent ACL links.
//! Addresses and link keys use HCI wire order.
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
    UnknownPeer,
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
    /// Completed outbound packets for this physical handle, including during teardown.
    /// Ready reports the controller-wide ACL capacity.
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

const MAX_LINKS: usize = 8;
const COMMAND_MS: u64 = 5_000;
const INQUIRY_MS: u64 = 15_000; // Inquiry_Length 8 * 1.28 seconds plus scheduling margin.
const PAGE_MS: u64 = 20_000;
const PAIR_MS: u64 = 60_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Owner {
    Init,
    Discovery,
    Link(Address),
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
    // Teardown was not confirmed. Keep this address/handle reserved until its terminal event.
    Quarantined,
}

struct Link {
    address: Address,
    allow_pairing: bool,
    disconnect_requested: bool,
    needs_name: bool,
    phase: Phase,
    handle: Option<u16>,
    encrypted: bool,
    ssp: bool,
    new_key: Option<([u8; 16], u8)>,
    deadline: Option<u64>,
}

impl Link {
    fn loss(&self, reason: Reason) -> Option<Action> {
        match (self.phase, self.handle) {
            (Phase::Up | Phase::Disconnecting, Some(handle)) => Some(Action::LinkDown {
                address: self.address,
                handle,
                reason,
            }),
            (Phase::Closing | Phase::CancelPage | Phase::Quarantined, _) => None,
            _ => Some(Action::ConnectionFailed {
                address: self.address,
                reason,
            }),
        }
    }
}

pub struct Classic {
    ready: bool,
    fault: bool,
    terminal: Option<Reason>,
    info: ControllerInfo,
    inquiry_mode: u8,
    credits: u8,
    credit_deadline: Option<u64>,
    commands: Queue<Queued, { MAX_LINKS * 2 }>,
    pending: Option<Pending>,
    // A tick can expire every link just before an event returns credits for every
    // handle. Reserve a third batch for controller-fault notifications.
    actions: Queue<Action, { MAX_LINKS * 3 + 2 }>,
    scan: Scan,
    scan_deadline: Option<u64>,
    scan_result: Result<(), Reason>,
    link_name: Option<Address>,
    seen: [Option<Seen>; 8],
    links: [Option<Link>; MAX_LINKS],
    keys: [Option<(Address, [u8; 16])>; MAX_LINKS + 1],
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
            seen: [None; 8],
            links: [const { None }; MAX_LINKS],
            keys: [None; MAX_LINKS + 1],
        }
    }

    pub fn start_inquiry(&mut self) -> Result<(), Error> {
        self.check_management_idle()?;
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
        self.select(addr, false)
    }

    fn link(&self, addr: Address) -> Option<&Link> {
        self.links
            .iter()
            .flatten()
            .find(|link| link.address == addr)
    }

    fn link_mut(&mut self, addr: Address) -> Option<&mut Link> {
        self.links
            .iter_mut()
            .flatten()
            .find(|link| link.address == addr)
    }

    fn address_for_handle(&self, handle: u16) -> Option<Address> {
        self.links
            .iter()
            .flatten()
            .find(|link| link.handle == Some(handle))
            .map(|link| link.address)
    }

    fn key(&self, addr: Address) -> Option<&[u8; 16]> {
        self.keys
            .iter()
            .flatten()
            .find(|(address, _)| *address == addr)
            .map(|(_, key)| key)
    }

    /// Cache a caller-provisioned SSP bond. Active peers retain their keys. When full,
    /// reuse an inactive peer's slot; the caller owns persistent storage.
    pub fn provide_link_key(&mut self, addr: Address, key: [u8; 16]) {
        let slot = self
            .keys
            .iter()
            .position(|entry| matches!(entry, Some((a, _)) if *a == addr))
            .or_else(|| self.keys.iter().position(Option::is_none))
            .or_else(|| {
                self.keys
                    .iter()
                    .position(|entry| entry.is_some_and(|(a, _)| self.link(a).is_none()))
            })
            .expect("one more bond slot than active links");
        self.keys[slot] = Some((addr, key));
    }

    /// Cancel this peer's attempt or close its ACL. Drain poll immediately.
    /// A confirmed local close reports LinkDown with LocalDisconnect. An attempt
    /// that has yet to reach LinkUp reports ConnectionFailed. After a teardown
    /// failure the address can be retried without a second loss notification.
    /// Returns NotReady before initialization or after controller failure, and
    /// UnknownPeer when the address has neither an attempt nor a physical link.
    pub fn disconnect_peer(&mut self, address: Address) -> Result<(), Error> {
        if !self.ready {
            return Err(Error::NotReady);
        }
        let link = self.link_mut(address).ok_or(Error::UnknownPeer)?;
        link.disconnect_requested = true;
        Ok(())
    }

    /// Compatibility for single-link callers. With multiple peers the address is required.
    pub fn disconnect(&mut self) -> Result<(), Error> {
        if !self.ready {
            return Err(Error::NotReady);
        }
        let mut peers = self.links.iter().flatten();
        let Some(first) = peers.next() else {
            return Ok(());
        };
        if peers.next().is_some() {
            return Err(Error::Busy);
        }
        self.disconnect_peer(first.address)
    }

    /// Earliest absolute wake-up, zero for immediate work, or None to wait for input.
    pub fn next_deadline_ms(&self) -> Option<u64> {
        if self.actions.0[0].is_some() || self.terminal.is_some() {
            return Some(0);
        }
        if self.fault {
            return None;
        }
        if self.links.iter().flatten().any(|l| l.disconnect_requested)
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
            self.links.iter().flatten().filter_map(|l| l.deadline).min(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn check_management_idle(&self) -> Result<(), Error> {
        if !self.ready {
            return Err(Error::NotReady);
        }
        if self
            .links
            .iter()
            .flatten()
            .any(|l| !matches!(l.phase, Phase::Up | Phase::Quarantined))
        {
            return Err(Error::Busy);
        }
        Ok(())
    }

    fn select(&mut self, address: Address, allow_pairing: bool) -> Result<(), Error> {
        self.check_management_idle()?;
        if self.link(address).is_some() {
            return Err(Error::Busy);
        }
        if !allow_pairing && self.key(address).is_none() {
            return Err(Error::MissingKey);
        }
        let slot = self
            .links
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(Error::Busy)?;
        *slot = Some(Link {
            disconnect_requested: false,
            needs_name: false,
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
        // Reserve a final notification for every link if the controller fails.
        if self.actions.0[self.actions.0.len() - MAX_LINKS - 1].is_some() {
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
        self.keys.fill(None);
        for link in self.links.iter_mut().filter_map(Option::take) {
            if let Some(action) = link.loss(reason) {
                self.actions.push(action);
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
        let selecting = self
            .links
            .iter()
            .flatten()
            .find(|l| l.phase == Phase::Queued);
        let cmd = match self.scan {
            Scan::Queued => Command::new(INQUIRY, &[0x33, 0x8b, 0x9e, 8, 0]),
            Scan::Inquiry if selecting.is_some() || self.scan_result.is_err() => {
                Command::new(INQUIRY_CANCEL, &[])
            }
            Scan::Name(addr) if selecting.is_some() || self.scan_result.is_err() => {
                Command::addressed(NAME_CANCEL, addr, &[])
            }
            Scan::Names => {
                let addr = self
                    .seen
                    .iter()
                    .flatten()
                    .find(|s| s.needs_name)
                    .map(|s| s.peer.address)?;
                self.paging(REMOTE_NAME, addr)
            }
            Scan::Idle => {
                if let Some(link) = selecting {
                    return Some(Queued {
                        command: self.paging(CREATE, link.address),
                        owner: Owner::Link(link.address),
                    });
                }
                // Post-link names share the controller's paging procedure with Create.
                if self.check_management_idle().is_err() {
                    return None;
                }
                let link = self
                    .links
                    .iter()
                    .flatten()
                    .find(|l| l.phase == Phase::Up && l.needs_name)?;
                self.paging(REMOTE_NAME, link.address)
            }
            _ => return None,
        };
        Some(Queued {
            command: cmd,
            owner: Owner::Discovery,
        })
    }

    fn sent(&mut self, owner: Owner, command: &Command, now: u64) {
        match command.opcode {
            INQUIRY => {
                self.scan = Scan::Inquiry;
                self.scan_deadline = Some(now.saturating_add(INQUIRY_MS));
            }
            REMOTE_NAME => {
                let addr = address(command.params());
                if self.scan == Scan::Idle {
                    self.link_name = Some(addr);
                    self.scan_result = Ok(());
                    self.link_mut(addr).unwrap().needs_name = false;
                }
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
                if command.opcode == NAME_CANCEL
                    && self.scan_result.is_ok()
                    && let Some(link) = self.link_name.and_then(|addr| self.link_mut(addr))
                {
                    // A new selection preempted this link's lookup. Resume it after paging.
                    link.needs_name = true;
                }
                self.scan = if command.opcode == INQUIRY_CANCEL {
                    Scan::CancelInquiry
                } else {
                    Scan::CancelName(address(command.params()))
                };
                self.scan_deadline = Some(now.saturating_add(COMMAND_MS));
            }
            CREATE => {
                if let Some(l) = self.link_mut(address(command.params())) {
                    l.phase = Phase::Paging;
                    l.deadline = Some(now.saturating_add(PAGE_MS));
                }
            }
            DISCONNECT | CREATE_CANCEL => {
                if let Owner::Link(addr) = owner
                    && let Some(l) = self.link_mut(addr)
                {
                    l.deadline = Some(now.saturating_add(COMMAND_MS * 2));
                }
            }
            _ => (),
        }
    }

    fn scan_finished(&self) -> bool {
        let selecting = self
            .links
            .iter()
            .flatten()
            .any(|l| l.phase == Phase::Queued);
        match self.scan {
            Scan::Queued => selecting,
            Scan::Names => {
                selecting
                    || self.scan_result.is_err()
                    || !self.seen.iter().flatten().any(|s| s.needs_name)
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
                    self.sent(q.owner, &q.command, now_ms);
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
        for i in 0..MAX_LINKS {
            if let Some(l) = self.links[i].as_mut()
                && core::mem::take(&mut l.disconnect_requested)
            {
                let addr = l.address;
                self.stop_link(addr, Reason::LocalDisconnect);
            }
        }
        if self.pending.as_ref().is_some_and(|p| now >= p.deadline)
            || self.credit_deadline.is_some_and(|d| now >= d)
                && (self.commands.0[0].is_some() || self.planned().is_some())
        {
            // Without an acknowledgement/credit the command stream cannot be resynchronized.
            self.fail(Reason::Timeout);
            return;
        }
        if self.scan_deadline.is_some_and(|d| now >= d) {
            self.scan_deadline = None;
            match self.scan {
                Scan::Inquiry | Scan::Name(_) => self.scan_result = Err(Reason::Timeout),
                Scan::CancelInquiry | Scan::CancelName(_) => {
                    self.scan_result = Err(Reason::Timeout);
                    self.finish_scan();
                }
                _ => (),
            }
        }
        for i in 0..MAX_LINKS {
            if let Some(l) = self.links[i]
                .as_ref()
                .filter(|l| l.deadline.is_some_and(|d| now >= d))
            {
                let addr = l.address;
                if matches!(
                    l.phase,
                    Phase::Closing | Phase::CancelPage | Phase::Disconnecting
                ) {
                    self.quarantine(addr, Reason::Timeout);
                } else {
                    self.stop_link(addr, Reason::Timeout);
                }
            }
        }
    }

    fn quarantine(&mut self, addr: Address, reason: Reason) {
        self.report_loss(addr, reason);
        if let Some(l) = self.link_mut(addr) {
            l.phase = Phase::Quarantined;
            l.deadline = None;
        }
        self.revoke(Owner::Link(addr));
    }

    fn stop_link(&mut self, addr: Address, reason: Reason) {
        let Some(l) = self.link(addr) else { return };
        let (phase, handle) = (l.phase, l.handle);
        if phase == Phase::Disconnecting && reason != Reason::LocalDisconnect {
            // Encryption or authentication was lost while a local close was in flight.
            self.report_loss(addr, reason);
            if let Some(l) = self.link_mut(addr) {
                l.phase = Phase::Closing;
            }
            return;
        }
        if matches!(
            phase,
            Phase::Closing | Phase::CancelPage | Phase::Disconnecting
        ) || (phase == Phase::Quarantined && reason != Reason::LocalDisconnect)
        {
            return;
        }
        if phase == Phase::Queued {
            self.end_link(addr, reason);
            return;
        }
        self.cancel_link_name(addr);
        // A failed Create status means there is no page to cancel or terminal event to await.
        if phase != Phase::Paging {
            self.revoke(Owner::Link(addr));
        }
        let wait_for_disconnect = reason == Reason::LocalDisconnect && phase == Phase::Up;
        if !wait_for_disconnect {
            self.report_loss(addr, reason);
        }
        let Some(l) = self.link_mut(addr) else { return };
        l.new_key = None;
        // The shared command FIFO may still be serving another handle. Start the
        // teardown deadline in sent(), when the controller actually gets the command.
        l.deadline = None;
        if let Some(handle) = handle {
            l.phase = if wait_for_disconnect {
                Phase::Disconnecting
            } else {
                Phase::Closing
            };
            self.enqueue(Owner::Link(addr), disconnect(handle));
        } else {
            l.phase = Phase::CancelPage;
            self.enqueue(
                Owner::Link(addr),
                Command::addressed(CREATE_CANCEL, addr, &[]),
            );
        }
    }

    fn report_loss(&mut self, addr: Address, reason: Reason) {
        if let Some(action) = self.link(addr).and_then(|l| l.loss(reason)) {
            self.emit(action);
        }
    }

    fn cancel_link_name(&mut self, addr: Address) {
        if self.link_name == Some(addr) {
            self.scan_result = Err(Reason::LocalDisconnect);
        }
    }

    fn end_link(&mut self, addr: Address, reason: Reason) {
        self.cancel_link_name(addr);
        self.revoke(Owner::Link(addr));
        self.report_loss(addr, reason);
        if let Some(slot) = self
            .links
            .iter_mut()
            .find(|s| s.as_ref().is_some_and(|l| l.address == addr))
        {
            *slot = None;
        }
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
            return self.ack(code, p);
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
                if let Some(addr) = self.address_for_handle(handle) {
                    if p[0] == 0 {
                        let reason = if self
                            .link(addr)
                            .is_some_and(|l| l.phase == Phase::Disconnecting)
                        {
                            Reason::LocalDisconnect
                        } else {
                            Reason::Hci(p[3])
                        };
                        self.end_link(addr, reason);
                    } else {
                        self.stop_link(addr, Reason::Hci(p[0]));
                    }
                }
            }
            0x06 if p.len() == 3 => self.auth_complete(now_ms, p),
            0x07 if p.len() >= 7 => self.remote_name_complete(p),
            0x08 if p.len() == 4 => self.encryption_change(p),
            0x59 if p.len() == 5 => self.encryption_change(p),
            0x13 => self.completed_packets(p)?,
            0x16 | 0x17 | 0x31 | 0x34 | 0x35 if p.len() == 6 => {
                self.security_reply(code, address(p))
            }
            0x33 if p.len() == 10 => self.security_reply(code, address(p)),
            0x18 if p.len() == 23 => self.link_key(p),
            0x30 if p.len() == 3 => {
                if p[0] != 0
                    && let Some(addr) = self.address_for_handle(word(&p[1..]))
                {
                    self.stop_link(addr, Reason::Hci(p[0]));
                }
            }
            0x36 if p.len() == 7 => {
                let addr = address(&p[1..]);
                if self.link(addr).is_some_and(|l| {
                    l.allow_pairing && matches!(l.phase, Phase::Authenticating | Phase::Encrypting)
                }) {
                    if p[0] != 0 {
                        self.stop_link(addr, Reason::Hci(p[0]));
                    } else {
                        self.confirm_ssp(addr);
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

    fn ack(&mut self, code: u8, p: &[u8]) -> Result<(), Error> {
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
        // Some controllers return only Status on errors. If a return address is
        // present, it must identify the pending command's peer even on failure.
        if code == 0x0e
            && (status == 0 || result.len() > 1)
            && pending
                .reply_address
                .is_some_and(|a| result.len() != 7 || result[1..] != a)
        {
            return Err(Error::MalformedEvent);
        }
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
                (Owner::Link(addr), CREATE) => self.end_link(addr, reason),
                (Owner::Link(addr), DISCONNECT) if status == 0x02 => self.end_link(addr, reason),
                (Owner::Link(addr), DISCONNECT) => self.quarantine(addr, reason),
                (Owner::Link(_), CREATE_CANCEL) => (),
                (Owner::Link(addr), _) => self.stop_link(addr, reason),
                _ => (),
            }
        } else if let Owner::Link(addr) = owner
            && opcode == KEY_REPLY
        {
            self.confirm_ssp(addr);
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
            if let Some(link) = self.link_mut(addr) {
                link.needs_name = false;
            }
            if let Some(s) = self.seen_slot(addr, self.link_name == Some(addr)) {
                s.peer.name = Some(Name::from_bytes(&p[7..]));
                s.needs_name = false;
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
        let mut totals = [0u16; MAX_LINKS];
        for pair in pairs.chunks_exact(4) {
            let handle = word(pair);
            if let Some(i) = self
                .links
                .iter()
                .position(|slot| slot.as_ref().is_some_and(|l| l.handle == Some(handle)))
            {
                totals[i] = totals[i]
                    .checked_add(word(&pair[2..]))
                    .ok_or(Error::MalformedEvent)?;
            }
        }
        for (i, packets) in totals.into_iter().enumerate() {
            if packets != 0
                && let Some(handle) = self.links[i].as_ref().and_then(|l| l.handle)
            {
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
    fn connection_complete(&mut self, now: u64, p: &[u8]) -> Result<(), Error> {
        let status = p[0];
        let handle = word(&p[1..]);
        let addr = address(&p[3..]);
        let selected = self.link(addr).is_some_and(|l| {
            l.handle.is_none()
                && matches!(
                    l.phase,
                    Phase::Paging | Phase::CancelPage | Phase::Quarantined
                )
        });
        if status != 0 {
            if selected {
                self.end_link(addr, Reason::Hci(status));
            }
            return Ok(());
        }
        if handle > 0x0eff {
            return Err(Error::MalformedEvent);
        }
        if let Some(existing) = self.address_for_handle(handle) {
            if existing != addr || p[9] != 1 {
                self.fail(Reason::InvalidControllerData);
            }
            return Ok(());
        }
        if !selected || p[9] != 1 {
            self.enqueue(Owner::Other, disconnect(handle));
            return Ok(());
        }
        let l = self.link_mut(addr).unwrap();
        l.handle = Some(handle);
        if matches!(l.phase, Phase::CancelPage | Phase::Quarantined) {
            // Cancellation raced link creation. Keep the handle quarantined until disconnection.
            l.phase = Phase::Closing;
            l.deadline = None;
            self.revoke(Owner::Link(addr));
            self.enqueue(Owner::Link(addr), disconnect(handle));
        } else {
            l.phase = Phase::Authenticating;
            l.encrypted = p[10] == 1;
            l.deadline = Some(now.saturating_add(PAIR_MS));
            self.enqueue(
                Owner::Link(addr),
                Command::new(AUTHENTICATE, &handle.to_le_bytes()),
            );
        }
        Ok(())
    }

    fn auth_complete(&mut self, now: u64, p: &[u8]) {
        let handle = word(&p[1..]);
        let Some(addr) = self.address_for_handle(handle) else {
            return;
        };
        if p[0] != 0 {
            self.stop_link(addr, Reason::Hci(p[0]));
            return;
        }
        let l = self.link_mut(addr).unwrap();
        if l.phase != Phase::Authenticating {
            return;
        }
        l.phase = Phase::Encrypting;
        if l.encrypted {
            self.maybe_link_up(addr);
        } else {
            l.deadline = Some(now.saturating_add(PAGE_MS));
            let [lo, hi] = handle.to_le_bytes();
            self.enqueue(Owner::Link(addr), Command::new(ENCRYPT, &[lo, hi, 1]));
        }
    }

    fn encryption_change(&mut self, p: &[u8]) {
        let Some(addr) = self.address_for_handle(word(&p[1..])) else {
            return;
        };
        if p[0] != 0 {
            self.stop_link(addr, Reason::Hci(p[0]));
        } else if !matches!(p[3], 1 | 2) {
            self.stop_link(addr, Reason::EncryptionLost);
        } else {
            let l = self.link_mut(addr).unwrap();
            l.encrypted = true;
            if l.phase == Phase::Encrypting {
                self.maybe_link_up(addr);
            }
        }
    }

    fn confirm_ssp(&mut self, addr: Address) {
        if let Some(l) = self.link_mut(addr) {
            l.ssp = true;
            self.maybe_link_up(addr);
        }
    }

    fn maybe_link_up(&mut self, addr: Address) {
        let Some(l) = self
            .link_mut(addr)
            .filter(|l| l.phase == Phase::Encrypting && l.encrypted && l.ssp)
        else {
            return;
        };

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
        let known_name = self
            .seen
            .iter()
            .flatten()
            .any(|s| s.peer.address == addr && s.complete_name);
        self.link_mut(addr).unwrap().needs_name = !known_name;
    }

    fn security_reply(&mut self, code: u8, addr: Address) {
        let selected = self.link(addr).filter(|l| {
            matches!(
                l.phase,
                Phase::Authenticating | Phase::Encrypting | Phase::Up
            )
        });
        let pairing = selected.is_some_and(|l| l.allow_pairing && l.phase == Phase::Authenticating);
        let key = self
            .key(addr)
            .filter(|_| selected.is_some_and(|l| !l.allow_pairing || l.phase == Phase::Up));
        let cmd = match code {
            0x16 => Command::addressed(NO_PIN, addr, &[]),
            0x17 => match key {
                Some(key) => Command::addressed(KEY_REPLY, addr, key),
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
            Owner::Link(addr)
        } else {
            Owner::Other
        };
        self.enqueue(owner, cmd);
    }

    fn link_key(&mut self, p: &[u8]) {
        let addr = address(p);
        let has_key = self.key(addr).is_some();
        let Some(l) = self.link_mut(addr).filter(|l| {
            matches!(
                l.phase,
                Phase::Authenticating | Phase::Encrypting | Phase::Up
            )
        }) else {
            return;
        };
        let kind = p[22];
        // Only SSP combination keys and changes to an existing SSP key are supported.
        if !(matches!(kind, 4 | 5 | 7 | 8) && l.allow_pairing || kind == 6 && has_key) {
            self.stop_link(addr, Reason::UnsupportedKey);
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
            self.confirm_ssp(addr);
        }
    }
}

#[cfg(all(test, not(target_os = "none")))]
#[path = "classic_tests.rs"]
mod tests;
