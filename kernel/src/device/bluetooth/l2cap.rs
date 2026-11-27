//! Basic-mode BR/EDR L2CAP for SDP discovery and HID on an encrypted ACL.
//!
//! On an encrypted ACL handle, `discover` searches SDP and closes its channel
//! before delivering the result. After checking the descriptor, call
//! `open_discovered_hid` to complete pending peer opens or initiate control
//! followed by interrupt. Both channels configure in both directions before Ready.
//! The profile sends complete HIDP packets and interprets their report payloads.
//! Route incoming fragments by handle, and poll one outgoing fragment per HCI
//! credit. The caller owns transport retries, negotiation deadlines and HCI
//! disconnects. Drop the session on link loss, including before handle reuse.

extern crate alloc;
#[path = "sdp.rs"]
pub mod sdp;

use self::alloc::{collections::VecDeque, vec::Vec};

pub const CONTROL_PSM: u16 = 0x0011;
pub const INTERRUPT_PSM: u16 = 0x0013;
pub const DEFAULT_MTU: u16 = 672;
pub const MAX_MTU: u16 = 1024;
const MIN_MTU: u16 = 48;
const SIGNAL_CID: u16 = 1;
const SIGNAL_MTU: u16 = 672;
const CONTROL: usize = 0;
const INTERRUPT: usize = 1;
const LOCAL_CID: [u16; 2] = [0x0040, 0x0041];
const PSM: [u16; 2] = [CONTROL_PSM, INTERRUPT_PSM];
const TX_LIMIT: usize = 16;
const EVENT_LIMIT: usize = 16;

const REJECT: u8 = 1;
const CONNECT_REQ: u8 = 2;
const CONNECT_RSP: u8 = 3;
const CONFIG_REQ: u8 = 4;
const CONFIG_RSP: u8 = 5;
const DISCONNECT_REQ: u8 = 6;
const DISCONNECT_RSP: u8 = 7;
const ECHO_REQ: u8 = 8;
const ECHO_RSP: u8 = 9;
const INFO_REQ: u8 = 10;
const INFO_RSP: u8 = 11;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidHandle,
    InvalidMtu,
    InvalidFragment,
    MalformedSignal,
    InvalidHidp,
    InvalidPsm,
    NotOpen,
    MtuExceeded,
    QueueFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    Local,
    Peer,
    MalformedSdp,
    ConnectionRefused(u16),
    ConfigurationFailed(u16),
    CommandRejected(u16),
    QueueFull,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Discovered(Option<sdp::HidService>),
    Ready,
    Control(Vec<u8>),
    Interrupt(Vec<u8>),
    Closed(CloseReason),
}

#[derive(Debug, PartialEq, Eq)]
pub struct AclFragment {
    pub handle: u16,
    /// Unshifted HCI packet boundary bits.
    pub pb: u8,
    /// HCI ACL payload, excluding the four-byte HCI ACL header.
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Request {
    code: u8,
    id: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct IncomingConnect {
    id: u8,
    psm: u16,
    peer: u16,
}

struct ConfigReply {
    id: u8,
    request: Vec<u8>,
    response: Vec<u8>,
}

struct Channel {
    peer: u16,
    pending: Option<Request>,
    // Only one request from each side can be in flight on this channel.
    incoming_options: Vec<u8>,
    invalid_continuation: bool,
    last_config_reply: Option<ConfigReply>,
    response_options: Vec<u8>,
    inbound_connection_id: Option<u8>,
    rx_mtu: u16,
    tx_mtu: u16,
    local_configured: bool,
    peer_configured: bool,
    retried_mtu: bool,
}

impl Channel {
    fn new(rx_mtu: u16) -> Self {
        Self {
            peer: 0,
            pending: None,
            incoming_options: Vec::new(),
            invalid_continuation: false,
            last_config_reply: None,
            response_options: Vec::new(),
            inbound_connection_id: None,
            rx_mtu,
            tx_mtu: DEFAULT_MTU,
            local_configured: false,
            peer_configured: false,
            retried_mtu: false,
        }
    }

    fn open(&self) -> bool {
        self.local_configured && self.peer_configured
    }

    fn idle(&self) -> bool {
        self.peer == 0 && self.pending.is_none()
    }
}

enum Profile {
    Hid([u16; 2]),
    Sdp(sdp::Client),
    SdpComplete(Option<sdp::HidService>),
    AwaitingHid([u16; 2]),
}

impl Profile {
    fn cids(&self) -> &'static [u16] {
        match self {
            Self::Hid(_) => &LOCAL_CID,
            Self::Sdp(_) | Self::SdpComplete(_) => &[0x0042],
            Self::AwaitingHid(_) => &[],
        }
    }

    fn channel(&self, cid: u16) -> Option<usize> {
        self.cids().iter().position(|&local| local == cid)
    }
}

pub struct L2capSession {
    profile: Profile,
    handle: u16,
    local_mtu: u16,
    channels: [Channel; 2],
    // At most one pending request per PSM and two per encrypted ACL.
    deferred: Vec<IncomingConnect>,
    last_id: u8,
    active_open: bool,
    ready: bool,
    closing: Option<CloseReason>,
    closed: bool,
    rx: Option<Reassembly>,
    tx: VecDeque<Vec<u8>>,
    tx_offset: usize,
    events: VecDeque<Event>,
}

fn word(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn words(values: &[u16]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn mtu_option(mtu: u16) -> [u8; 4] {
    let [lo, hi] = mtu.to_le_bytes();
    [1, 2, lo, hi]
}

impl L2capSession {
    pub fn new(handle: u16, local_mtu: u16) -> Result<Self, Error> {
        Self::with_profile(handle, local_mtu, Profile::Hid(PSM))
    }

    /// Start the SDP client on an encrypted link. The result is delivered only
    /// after its channel has been closed. An empty HID search yields None.
    pub fn discover(handle: u16, local_mtu: u16) -> Result<Self, Error> {
        let mut session = Self::with_profile(handle, local_mtu, Profile::Sdp(Default::default()))?;
        session.start()?;
        Ok(session)
    }

    /// Construct a HID session using the two PSMs advertised in the SDP record.
    pub fn for_hid(
        handle: u16,
        local_mtu: u16,
        control_psm: u16,
        interrupt_psm: u16,
    ) -> Result<Self, Error> {
        if !sdp::valid_hid_psms(control_psm, interrupt_psm) {
            return Err(Error::InvalidPsm);
        }
        Self::with_profile(
            handle,
            local_mtu,
            Profile::Hid([control_psm, interrupt_psm]),
        )
    }

    fn with_profile(handle: u16, local_mtu: u16, profile: Profile) -> Result<Self, Error> {
        if handle > 0x0eff {
            return Err(Error::InvalidHandle);
        }
        if !(MIN_MTU..=MAX_MTU).contains(&local_mtu) {
            return Err(Error::InvalidMtu);
        }
        Ok(Self {
            profile,
            handle,
            local_mtu,
            channels: core::array::from_fn(|_| Channel::new(local_mtu)),
            deferred: Vec::new(),
            last_id: 0,
            active_open: false,
            ready: false,
            closing: None,
            closed: false,
            rx: None,
            tx: VecDeque::new(),
            tx_offset: 0,
            events: VecDeque::new(),
        })
    }

    pub fn is_ready(&self) -> bool {
        self.ready && matches!(self.profile, Profile::Hid(_))
    }

    /// Includes the one-byte HIDP header on the configured interrupt channel.
    pub fn interrupt_tx_mtu(&self) -> Option<u16> {
        self.is_ready().then_some(self.channels[INTERRUPT].tx_mtu)
    }

    /// Idempotent, including if an inbound control connection is configuring.
    pub fn start(&mut self) -> Result<(), Error> {
        if self.closing.is_some() || matches!(self.profile, Profile::AwaitingHid(_)) {
            return Err(Error::NotOpen);
        }
        self.active_open = true;
        self.advance()
    }

    /// The profile has validated the discovered report descriptor. Resolve each
    /// pending transaction on this same ACL using the PSMs from its SDP record.
    pub fn open_discovered_hid(&mut self) -> Result<(), Error> {
        let Profile::AwaitingHid(psms) = self.profile else {
            return Err(Error::NotOpen);
        };
        if self.closing.is_some() {
            return Err(Error::NotOpen);
        }
        self.profile = Profile::Hid(psms);
        let mut pending = core::mem::take(&mut self.deferred);
        // Control must be established before interrupt, even if both are waiting.
        pending.sort_unstable_by_key(|req| req.psm == psms[INTERRUPT]);
        for req in pending {
            self.connect_request(req.id, req.psm, req.peer)?;
        }
        if self.channels[CONTROL].idle() {
            self.start()?;
        }
        Ok(())
    }

    pub fn disconnect(&mut self) -> Result<(), Error> {
        // The caller can cancel discovery while its final SDP disconnect is
        // in flight. A late acknowledgement must not publish that result.
        if !self.closed && matches!(self.profile, Profile::SdpComplete(_)) {
            self.profile = Profile::Sdp(sdp::Client::default());
        }
        self.begin_close(CloseReason::Local)
    }

    /// Deliver closure and discovery after the final signaling reply has been polled for HCI.
    pub fn poll_event(&mut self) -> Option<Event> {
        if (self.closed || matches!(self.profile, Profile::AwaitingHid(_))) && !self.tx.is_empty() {
            return None;
        }
        self.events.pop_front()
    }

    /// Zero length or an empty queue returns None without consuming any data.
    /// If HCI transmission fails, retry the returned fragment before polling again.
    pub fn poll_acl(&mut self, max_data_len: usize) -> Option<AclFragment> {
        if max_data_len == 0 {
            return None;
        }
        let packet = self.tx.front()?;
        let start = self.tx_offset;
        let end = start.saturating_add(max_data_len).min(packet.len());
        let data = packet[start..end].to_vec();
        self.tx_offset = end;
        if end == packet.len() {
            self.tx.pop_front();
            self.tx_offset = 0;
        }
        Some(AclFragment {
            handle: self.handle,
            pb: if start == 0 { 2 } else { 1 },
            data,
        })
    }

    pub fn send_control(&mut self, packet: &[u8]) -> Result<(), Error> {
        self.send_hid(CONTROL, packet)
    }

    pub fn send_interrupt(&mut self, packet: &[u8]) -> Result<(), Error> {
        self.send_hid(INTERRUPT, packet)
    }

    fn send_hid(&mut self, index: usize, packet: &[u8]) -> Result<(), Error> {
        if !matches!(self.profile, Profile::Hid(_)) {
            return Err(Error::NotOpen);
        }
        let channel = &self.channels[index];
        if self.closing.is_some() || !channel.open() || (index == INTERRUPT && !self.ready) {
            return Err(Error::NotOpen);
        }
        if packet.len() > usize::from(channel.tx_mtu) {
            return Err(Error::MtuExceeded);
        }
        check_hidp(index, true, packet)?;
        // Leave space for signaling even when the profile gets ahead of HCI.
        if self.tx.len() >= TX_LIMIT - 4 {
            return Err(Error::QueueFull);
        }
        self.enqueue(channel.peer, packet)
    }

    fn enqueue(&mut self, cid: u16, data: &[u8]) -> Result<(), Error> {
        if self.tx.len() >= TX_LIMIT {
            return Err(Error::QueueFull);
        }
        let mut packet = Vec::with_capacity(4 + data.len());
        packet.extend_from_slice(&(data.len() as u16).to_le_bytes());
        packet.extend_from_slice(&cid.to_le_bytes());
        packet.extend_from_slice(data);
        self.tx.push_back(packet);
        Ok(())
    }

    fn signal(&mut self, code: u8, id: u8, payload: &[u8]) -> Result<(), Error> {
        let mut command = Vec::with_capacity(4 + payload.len());
        command.extend_from_slice(&[code, id]);
        command.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        command.extend_from_slice(payload);
        // A lost signaling reply could leave the two endpoints in different states.
        if let Err(error) = self.enqueue(SIGNAL_CID, &command) {
            self.tx.clear();
            self.tx_offset = 0;
            self.closing = Some(CloseReason::QueueFull);
            self.finish_close();
            return Err(error);
        }
        Ok(())
    }

    fn request(&mut self, index: usize, code: u8, payload: &[u8]) -> Result<(), Error> {
        loop {
            self.last_id = self.last_id.wrapping_add(1).max(1);
            if !self
                .channels
                .iter()
                .any(|ch| ch.pending.is_some_and(|r| r.id == self.last_id))
            {
                break;
            }
        }
        self.signal(code, self.last_id, payload)?;
        self.channels[index].pending = Some(Request {
            code,
            id: self.last_id,
        });
        Ok(())
    }

    // Once the first fragment has left the session, its identifier and local
    // CID must stay reserved until the peer answers. Before that we can yield
    // to a device-initiated connection (or cancel a local open on shutdown).
    fn cancel_unsent_connect(&mut self, index: usize) -> bool {
        let Some(Request {
            code: CONNECT_REQ,
            id,
        }) = self.channels[index].pending
        else {
            return false;
        };
        let position = self.tx.iter().enumerate().position(|(i, pdu)| {
            (i != 0 || self.tx_offset == 0)
                && word(pdu, 2) == SIGNAL_CID
                && pdu[4..6] == [CONNECT_REQ, id]
        });
        if let Some(position) = position {
            self.tx.remove(position);
            self.channels[index].pending = None;
            return true;
        }
        false
    }

    fn advance(&mut self) -> Result<(), Error> {
        if self.closing.is_some() {
            return self.advance_close();
        }
        let count = self.profile.cids().len();
        if self.channels[..count].iter().all(Channel::open) {
            if !self.ready {
                self.ready = true;
                if let Profile::Sdp(client) = &mut self.profile {
                    let channel = &self.channels[CONTROL];
                    let packet = client.request(channel.rx_mtu, &[]);
                    self.enqueue(channel.peer, &packet)?;
                } else {
                    self.events.push_back(Event::Ready);
                }
            }
        } else if self.active_open {
            let index = if self.channels[CONTROL].open() {
                INTERRUPT
            } else {
                CONTROL
            };
            if self.channels[index].idle() {
                let psm = match self.profile {
                    Profile::Hid(psms) => psms[index],
                    Profile::Sdp(_) => 1,
                    Profile::SdpComplete(_) | Profile::AwaitingHid(_) => {
                        return Err(Error::NotOpen);
                    }
                };
                self.request(
                    index,
                    CONNECT_REQ,
                    &words(&[psm, self.profile.cids()[index]]),
                )?;
            }
        }
        Ok(())
    }

    fn begin_close(&mut self, reason: CloseReason) -> Result<(), Error> {
        if self.closing.is_none() {
            self.closing = Some(reason);
            self.ready = false;
            self.events.clear();
            // Finish an already-started ACL PDU. Cancel unsent HID reports.
            let mut first = self.tx_offset != 0;
            self.tx
                .retain(|pdu| core::mem::take(&mut first) || word(pdu, 2) == SIGNAL_CID);
        }
        if !self.awaiting_discovery() {
            let result = if matches!(
                self.profile,
                Profile::SdpComplete(None) | Profile::AwaitingHid(_)
            ) || reason == CloseReason::MalformedSdp
            {
                2 // PSM not supported
            } else {
                4 // Discovery canceled or failed
            };
            for req in core::mem::take(&mut self.deferred) {
                self.signal(CONNECT_RSP, req.id, &words(&[0, req.peer, result, 0]))?;
            }
        }
        self.advance_close()
    }

    fn advance_close(&mut self) -> Result<(), Error> {
        if self.closed {
            return Ok(());
        }
        for index in (0..self.profile.cids().len()).rev() {
            self.cancel_unsent_connect(index);
            let ch = &self.channels[index];
            if ch.idle() {
                continue;
            }
            if ch.peer != 0 && !ch.pending.is_some_and(|r| r.code == DISCONNECT_REQ) {
                let payload = words(&[ch.peer, self.profile.cids()[index]]);
                self.request(index, DISCONNECT_REQ, &payload)?;
            }
            // If a connection is pending, wait for its CID before disconnecting.
            return Ok(());
        }
        self.finish_close();
        Ok(())
    }

    fn finish_close(&mut self) {
        self.ready = false;
        self.events.clear();
        let reason = self.closing.expect("closing session");
        let event = match &mut self.profile {
            Profile::SdpComplete(result) if reason == CloseReason::Local => {
                if let Some(service) = result.take() {
                    self.profile =
                        Profile::AwaitingHid([service.control_psm, service.interrupt_psm]);
                    self.active_open = false;
                    self.closing = None;
                    self.events.push_back(Event::Discovered(Some(service)));
                    return;
                }
                Event::Discovered(None)
            }
            _ => Event::Closed(reason),
        };
        self.closed = true;
        self.rx = None;
        self.events.push_back(event);
    }

    /// Supply the unshifted PB and the HCI ACL payload for this handle.
    /// A new first fragment replaces any incomplete (possibly flushed) PDU.
    /// A malformed fragment discards its partial PDU. A full HID event queue
    /// drops the incoming packet and returns QueueFull. Signaling exhaustion
    /// instead emits Closed(QueueFull), requiring an HCI disconnect.
    pub fn receive_acl(&mut self, pb: u8, data: &[u8]) -> Result<(), Error> {
        if self.closed {
            return Err(Error::NotOpen);
        }
        let state = match pb {
            0 | 2 | 3 => {
                self.rx = None;
                Reassembly::Payload(Vec::new())
            }
            1 => self.rx.take().ok_or(Error::InvalidFragment)?,
            _ => {
                self.rx = None;
                return Err(Error::InvalidFragment);
            }
        };
        if data.is_empty() {
            return Err(Error::InvalidFragment);
        }
        let mut packet = match state {
            Reassembly::Payload(packet) => packet,
            Reassembly::Oversized(discard) => return self.discard_signal(discard, data, pb),
        };
        // Inspect the basic header before allocating for any payload, including
        // when the controller splits the header itself across ACL fragments.
        let header_bytes = 4usize.saturating_sub(packet.len()).min(data.len());
        packet.extend_from_slice(&data[..header_bytes]);
        let body = &data[header_bytes..];
        if packet.len() >= 4 {
            let len = usize::from(word(&packet, 0));
            let cid = word(&packet, 2);
            if cid == SIGNAL_CID && len > usize::from(SIGNAL_MTU) {
                return self.discard_signal(OversizedSignal::new(len), body, pb);
            }
            let mtu = match self.profile.channel(cid) {
                Some(index) => self.channels[index].rx_mtu,
                None => MAX_MTU,
            };
            if len > usize::from(mtu) {
                return Err(Error::MtuExceeded);
            }
            if body.len() > len + 4 - packet.len() {
                return Err(Error::InvalidFragment);
            }
            packet.extend_from_slice(body);
            if packet.len() == len + 4 {
                return self.receive_sdu(cid, &packet[4..]);
            }
        }
        if pb == 3 {
            return Err(Error::InvalidFragment);
        }
        self.rx = Some(Reassembly::Payload(packet));
        Ok(())
    }

    fn discard_signal(
        &mut self,
        mut discard: OversizedSignal,
        data: &[u8],
        pb: u8,
    ) -> Result<(), Error> {
        discard.consume(data)?;
        if discard.remaining == 0 {
            if let Some(id) = discard.first_request {
                self.signal(REJECT, id, &words(&[1, SIGNAL_MTU]))?;
            }
        } else if pb == 3 {
            return Err(Error::InvalidFragment);
        } else {
            self.rx = Some(Reassembly::Oversized(discard));
        }
        Ok(())
    }

    fn receive_sdu(&mut self, cid: u16, packet: &[u8]) -> Result<(), Error> {
        if cid == SIGNAL_CID {
            return self.receive_signals(packet);
        }
        let Some(index) = self.profile.channel(cid) else {
            return Ok(());
        };
        if self.closing.is_some()
            || !self.channels[index].open()
            || (index == INTERRUPT && !self.ready)
        {
            return Err(Error::NotOpen);
        }
        if let Profile::Sdp(client) = &mut self.profile {
            let channel = &self.channels[index];
            return match client.response(packet, channel.rx_mtu) {
                Ok(sdp::Step::Wait) => Ok(()),
                Ok(sdp::Step::Request(next)) => self.enqueue(channel.peer, &next),
                Ok(sdp::Step::Done(result)) => {
                    self.profile = Profile::SdpComplete(result);
                    self.begin_close(CloseReason::Local)
                }
                Err(()) => self.begin_close(CloseReason::MalformedSdp),
            };
        }
        check_hidp(index, false, packet)?;
        // Reserve a slot for Ready while only the control channel is open.
        if self.events.len() >= EVENT_LIMIT - usize::from(!self.ready) {
            return Err(Error::QueueFull);
        }
        self.events.push_back(if index == CONTROL {
            Event::Control(packet.to_vec())
        } else {
            Event::Interrupt(packet.to_vec())
        });
        Ok(())
    }

    fn receive_signals(&mut self, packet: &[u8]) -> Result<(), Error> {
        if packet.is_empty() {
            return Err(Error::MalformedSignal);
        }
        // Validate all command boundaries before applying any commands.
        let mut remaining = packet;
        while !remaining.is_empty() {
            let (_, tail) = command(remaining)?;
            remaining = tail;
        }
        remaining = packet;
        while !remaining.is_empty() && !self.closed {
            let (cmd, tail) = command(remaining)?;
            if cmd.id != 0 {
                self.receive_command(cmd.code, cmd.id, cmd.data)?;
            }
            remaining = tail;
        }
        Ok(())
    }

    fn receive_command(&mut self, code: u8, id: u8, data: &[u8]) -> Result<(), Error> {
        match code {
            CONNECT_REQ if data.len() == 4 => {
                self.connect_request(id, word(data, 0), word(data, 2))
            }
            CONNECT_RSP if data.len() == 8 => self.connect_response(id, data),
            CONFIG_REQ if data.len() >= 4 => self.config_request(id, data),
            CONFIG_RSP if data.len() >= 6 => self.config_response(id, data),
            DISCONNECT_REQ | DISCONNECT_RSP if data.len() == 4 => {
                self.disconnect_command(code, id, data)
            }
            REJECT if data.len() >= 2 => self.rejected(id, data),
            ECHO_REQ => self.signal(ECHO_RSP, id, &data[..data.len().min(44)]),
            INFO_REQ if data.len() == 2 => {
                let kind = word(data, 0);
                // Basic mode, with BR/EDR signaling as the only fixed channel.
                let mut reply = words(&[kind, 0]);
                match kind {
                    2 => reply.extend_from_slice(&0x80u32.to_le_bytes()),
                    3 => reply.extend_from_slice(&2u64.to_le_bytes()),
                    _ => reply[2] = 1,
                }
                self.signal(INFO_RSP, id, &reply)
            }
            ECHO_RSP | INFO_RSP => Ok(()),
            REJECT | CONNECT_RSP | CONFIG_RSP | DISCONNECT_RSP => Err(Error::MalformedSignal),
            _ => self.signal(REJECT, id, &words(&[0])),
        }
    }

    fn connect_request(&mut self, id: u8, psm: u16, peer: u16) -> Result<(), Error> {
        if self.awaiting_discovery() && (PSM.contains(&psm) || sdp::dynamic_psm(psm)) {
            return self.defer_connect(IncomingConnect { id, psm, peer });
        }
        let index = match &self.profile {
            Profile::Hid(psms) => psms.iter().position(|&value| value == psm),
            _ => None,
        };
        let Some(index) = index else {
            return self.signal(CONNECT_RSP, id, &words(&[0, peer, 2, 0]));
        };
        let allocated = self.channels.iter().any(|ch| ch.peer == peer);
        if peer >= 0x40 && !allocated && self.closing.is_none() && self.cancel_unsent_connect(index)
        {
            // The device that initiates control normally initiates interrupt too.
            self.active_open = false;
        }
        let channel = &self.channels[index];
        let duplicate = channel.peer == peer && channel.inbound_connection_id == Some(id);
        let result = if peer < 0x40 {
            6
        } else if duplicate {
            0
        } else if allocated {
            7
        } else if !channel.idle()
            || self.closing.is_some()
            || (index == INTERRUPT && self.channels[CONTROL].peer == 0)
        {
            4
        } else {
            0
        };
        let local = if result == 0 {
            self.profile.cids()[index]
        } else {
            0
        };
        self.signal(CONNECT_RSP, id, &words(&[local, peer, result, 0]))?;
        if result != 0 || duplicate {
            return Ok(());
        }
        self.channels[index].inbound_connection_id = Some(id);
        self.connected(index, peer)
    }

    fn awaiting_discovery(&self) -> bool {
        matches!(
            (&self.profile, self.closing),
            (Profile::Sdp(_) | Profile::AwaitingHid(_), None)
                | (Profile::SdpComplete(Some(_)), Some(CloseReason::Local))
        )
    }

    fn defer_connect(&mut self, req: IncomingConnect) -> Result<(), Error> {
        let result = if req.peer < 0x40 {
            6
        } else if self.deferred.contains(&req) {
            1 // Retransmission of the same pending request.
        } else if self.channels.iter().any(|ch| ch.peer == req.peer)
            || self.deferred.iter().any(|r| r.peer == req.peer)
        {
            7
        } else if self.deferred.len() == 2
            || self
                .deferred
                .iter()
                .any(|r| r.psm == req.psm || r.id == req.id)
        {
            4
        } else {
            self.deferred.push(req);
            1
        };
        // A pending response does not establish a channel. Assign its local
        // CID only when the service and descriptor have both been validated.
        self.signal(CONNECT_RSP, req.id, &words(&[0, req.peer, result, 0]))
    }

    fn connect_response(&mut self, id: u8, data: &[u8]) -> Result<(), Error> {
        let request = Some(Request {
            code: CONNECT_REQ,
            id,
        });
        let Some(index) = self.channels.iter().position(|ch| ch.pending == request) else {
            return Ok(());
        };
        let result = word(data, 4);
        if result == 1 {
            return Ok(());
        } // Pending, wait for final response on the same id.
        if result != 0 {
            // The specification permits invalid CIDs in unsuccessful responses.
            self.channels[index].pending = None;
            return self.begin_close(CloseReason::ConnectionRefused(result));
        }
        if word(data, 2) != self.profile.cids()[index] {
            return Ok(());
        }
        let peer = word(data, 0);
        if peer < 0x40
            || self.channels.iter().any(|ch| ch.peer == peer)
            || self.deferred.iter().any(|r| r.peer == peer)
        {
            return Err(Error::MalformedSignal);
        }
        self.connected(index, peer)
    }

    fn connected(&mut self, index: usize, peer: u16) -> Result<(), Error> {
        self.channels[index].peer = peer;
        self.channels[index].pending = None;
        if self.closing.is_some() {
            return self.advance_close();
        }
        self.request_config(index, &mtu_option(self.local_mtu))
    }

    fn request_config(&mut self, index: usize, options: &[u8]) -> Result<(), Error> {
        let mut data = words(&[self.channels[index].peer, 0]);
        data.extend_from_slice(options);
        self.request(index, CONFIG_REQ, &data)
    }

    fn config_request(&mut self, id: u8, data: &[u8]) -> Result<(), Error> {
        let cid = word(data, 0);
        let Some(index) = self
            .profile
            .channel(cid)
            .filter(|&i| self.channels[i].peer != 0)
        else {
            return self.signal(REJECT, id, &words(&[2, cid, 0]));
        };
        let flags = word(data, 2) & 1;
        let ch = &mut self.channels[index];
        if self.closing.is_some() {
            return self.signal(
                CONFIG_RSP,
                id,
                &words(&[self.channels[index].peer, flags, 2]),
            );
        }
        if let Some(cached) = &ch.last_config_reply
            && cached.id == id
            && cached.request == data
        {
            let response = cached.response.clone();
            return self.signal(CONFIG_RSP, id, &response);
        }

        let fragment = &data[4..];
        let (result, accepted, adjustment) = if ch.invalid_continuation
            || ch.incoming_options.len() + fragment.len() > usize::from(SIGNAL_MTU)
            || !complete_options(fragment)
        {
            ch.incoming_options.clear();
            ch.invalid_continuation = flags != 0;
            (2, ch.tx_mtu, Vec::new())
        } else {
            ch.incoming_options.extend_from_slice(fragment);
            if flags == 0 {
                let options = core::mem::take(&mut ch.incoming_options);
                evaluate_options(&options, ch.tx_mtu)
            } else {
                (0, ch.tx_mtu, Vec::new())
            }
        };
        let mut reply = words(&[ch.peer, flags, result.into()]);
        reply.extend_from_slice(&adjustment);
        ch.last_config_reply = Some(ConfigReply {
            id,
            request: data.to_vec(),
            response: reply.clone(),
        });
        self.signal(CONFIG_RSP, id, &reply)?;
        if flags == 0 && result == 0 {
            self.channels[index].peer_configured = true;
            self.channels[index].tx_mtu = accepted;
            self.advance()?;
        }
        Ok(())
    }

    fn config_response(&mut self, id: u8, data: &[u8]) -> Result<(), Error> {
        let Some(index) = self.profile.channel(word(data, 0)) else {
            return Ok(());
        };
        let ch = &mut self.channels[index];
        if ch.pending
            != Some(Request {
                code: CONFIG_REQ,
                id,
            })
        {
            return Ok(());
        }
        let result = word(data, 4);
        if result == 4 {
            return Ok(());
        } // Pending.
        if !matches!(result, 0 | 1) {
            return self.begin_close(CloseReason::ConfigurationFailed(result));
        }
        if ch.response_options.len() + data.len() - 6 > usize::from(SIGNAL_MTU) {
            return self.begin_close(CloseReason::ConfigurationFailed(2));
        }
        if !complete_options(&data[6..]) {
            return Err(Error::MalformedSignal);
        }
        ch.response_options.extend_from_slice(&data[6..]);
        if result == 0 && word(data, 2) & 1 != 0 {
            return self.request_config(index, &[]);
        }
        let options = core::mem::take(&mut ch.response_options);
        let proposed = match response_mtu(&options) {
            Err(_) if result != 0 => {
                return self.begin_close(CloseReason::ConfigurationFailed(result));
            }
            other => other?,
        };
        if result == 1
            && !ch.retried_mtu
            && let Some(mtu) = proposed
            && (MIN_MTU..=self.local_mtu).contains(&mtu)
            && mtu != ch.rx_mtu
        {
            ch.retried_mtu = true;
            ch.rx_mtu = mtu;
            return self.request_config(index, &mtu_option(mtu));
        }
        if result != 0 {
            return self.begin_close(CloseReason::ConfigurationFailed(result));
        }
        if let Some(mtu) = proposed {
            if !(MIN_MTU..=ch.rx_mtu).contains(&mtu) {
                return Err(Error::MalformedSignal);
            }
            ch.rx_mtu = mtu;
        }
        ch.pending = None;
        ch.local_configured = true;
        self.advance()
    }

    fn disconnect_command(&mut self, code: u8, id: u8, data: &[u8]) -> Result<(), Error> {
        let (local, peer) = if code == DISCONNECT_REQ {
            (word(data, 0), word(data, 2))
        } else {
            (word(data, 2), word(data, 0))
        };
        let index = self
            .profile
            .channel(local)
            .filter(|&i| self.channels[i].peer != 0);
        if code == DISCONNECT_REQ && index.is_none() {
            return self.signal(REJECT, id, &words(&[2, local, peer]));
        }
        let Some(index) = index else {
            return Ok(());
        };
        if self.channels[index].peer != peer {
            return Ok(());
        }
        if code == DISCONNECT_REQ {
            self.signal(DISCONNECT_RSP, id, data)?;
        }
        if code == DISCONNECT_RSP
            && self.channels[index].pending
                != Some(Request {
                    code: DISCONNECT_REQ,
                    id,
                })
        {
            return Ok(());
        }
        self.channels[index] = Channel::new(self.local_mtu);
        self.begin_close(CloseReason::Peer)
    }

    fn rejected(&mut self, id: u8, data: &[u8]) -> Result<(), Error> {
        let reason = word(data, 0);
        if !matches!((reason, data.len()), (0, 2) | (1, 4) | (2, 6)) {
            return Err(Error::MalformedSignal);
        }
        let Some(index) = self
            .channels
            .iter()
            .position(|ch| ch.pending.is_some_and(|r| r.id == id))
        else {
            return Ok(());
        };
        if self.channels[index]
            .pending
            .is_some_and(|r| r.code == DISCONNECT_REQ)
        {
            self.channels[index] = Channel::new(self.local_mtu);
            if matches!(self.profile, Profile::SdpComplete(_)) {
                self.closing = Some(CloseReason::CommandRejected(reason));
            }
        } else {
            self.channels[index].pending = None;
        }
        self.begin_close(CloseReason::CommandRejected(reason))
    }
}

struct Command<'a> {
    code: u8,
    id: u8,
    data: &'a [u8],
}

fn command(data: &[u8]) -> Result<(Command<'_>, &[u8]), Error> {
    if data.len() < 4 {
        return Err(Error::MalformedSignal);
    }
    let end = 4 + usize::from(word(data, 2));
    if end > data.len() {
        return Err(Error::MalformedSignal);
    }
    Ok((
        Command {
            code: data[0],
            id: data[1],
            data: &data[4..end],
        },
        &data[end..],
    ))
}

enum Reassembly {
    Payload(Vec<u8>),
    Oversized(OversizedSignal),
}

// An over-MTU signaling PDU requires one rejection for the first request in
// the PDU, which may follow any number of responses. Scan command headers in
// constant space even if a peer declares the largest possible L2CAP length.
struct OversizedSignal {
    remaining: usize,
    header: [u8; 4],
    used: usize,
    skip: usize,
    first_request: Option<u8>,
}

impl OversizedSignal {
    fn new(len: usize) -> Self {
        Self {
            remaining: len,
            header: [0; 4],
            used: 0,
            skip: 0,
            first_request: None,
        }
    }

    fn consume(&mut self, mut data: &[u8]) -> Result<(), Error> {
        self.remaining = self
            .remaining
            .checked_sub(data.len())
            .ok_or(Error::InvalidFragment)?;
        while !data.is_empty() {
            if self.skip != 0 {
                let n = self.skip.min(data.len());
                self.skip -= n;
                data = &data[n..];
                continue;
            }
            let n = (4 - self.used).min(data.len());
            self.header[self.used..self.used + n].copy_from_slice(&data[..n]);
            self.used += n;
            data = &data[n..];
            if self.used == 4 {
                self.used = 0;
                self.skip = usize::from(word(&self.header, 2));
                if self.skip > self.remaining + data.len() {
                    return Err(Error::MalformedSignal);
                }
                let [code, id, _, _] = self.header;
                if id != 0
                    && !matches!(
                        code,
                        REJECT | CONNECT_RSP | CONFIG_RSP | DISCONNECT_RSP | ECHO_RSP | INFO_RSP
                    )
                {
                    self.first_request.get_or_insert(id);
                }
            }
        }
        if self.remaining == 0 && self.used != 0 {
            return Err(Error::MalformedSignal);
        }
        Ok(())
    }
}

fn option(data: &[u8]) -> Result<(&[u8], &[u8]), Error> {
    if data.len() < 2 || usize::from(data[1]) + 2 > data.len() {
        return Err(Error::MalformedSignal);
    }
    Ok(data.split_at(2 + usize::from(data[1])))
}

fn complete_options(mut data: &[u8]) -> bool {
    while !data.is_empty() {
        let Ok((_, rest)) = option(data) else {
            return false;
        };
        data = rest;
    }
    true
}

// Return a basic-mode Configuration Response result, outbound MTU and reply options.
fn evaluate_options(mut data: &[u8], current: u16) -> (u8, u16, Vec<u8>) {
    let mut mtu = None;
    let mut unknown = Vec::new();
    let mut adjust = Vec::new();
    while !data.is_empty() {
        let Ok((opt, rest)) = option(data) else {
            return (2, current, Vec::new());
        };
        data = rest;
        match opt[0] & 0x7f {
            1 if opt.len() == 4 && mtu.is_none() => {
                let value = word(opt, 2);
                mtu = Some(value.min(MAX_MTU));
                if value < MIN_MTU {
                    adjust.extend_from_slice(&mtu_option(MIN_MTU));
                }
            }
            2 if opt.len() == 4 => {
                if word(opt, 2) != 0xffff {
                    adjust.extend_from_slice(&[2, 2, 0xff, 0xff]);
                }
            }
            4 if opt.len() == 11 => {
                if opt[2] != 0 {
                    adjust.extend_from_slice(&[4, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
                }
            }
            1 | 2 | 4 => return (2, current, Vec::new()),
            _ if opt[0] & 0x80 != 0 => {}
            _ => unknown.extend_from_slice(opt),
        }
    }
    // An unfragmented reply must fit the peer's minimum signaling MTU.
    if unknown.len().max(adjust.len()) > usize::from(MIN_MTU) - 10 {
        return (2, current, Vec::new());
    }
    if !unknown.is_empty() {
        return (3, current, unknown);
    }
    if !adjust.is_empty() {
        return (1, current, adjust);
    }
    let reply = mtu.map_or_else(Vec::new, |value| mtu_option(value).to_vec());
    (0, mtu.unwrap_or(current), reply)
}

fn response_mtu(mut data: &[u8]) -> Result<Option<u16>, Error> {
    let mut mtu = None;
    while !data.is_empty() {
        let (opt, rest) = option(data)?;
        data = rest;
        if opt[0] & 0x7f == 1 {
            if opt.len() != 4 || mtu.is_some() {
                return Err(Error::MalformedSignal);
            }
            mtu = Some(word(opt, 2));
        } else if opt[0] & 0x80 == 0 {
            // Our only non-default option is MTU.
            return Err(Error::MalformedSignal);
        }
    }
    Ok(mtu)
}

// Validate HIDP 1.1 host-side framing. Report IDs, report-specific lengths and
// payload semantics belong to the peer's descriptor/profile.
fn check_hidp(index: usize, outgoing: bool, data: &[u8]) -> Result<(), Error> {
    let (&header, body) = data.split_first().ok_or(Error::InvalidHidp)?;
    let valid = match (index, outgoing, header) {
        (INTERRUPT, true, 0xa2) | (INTERRUPT, false, 0xa1) => !body.is_empty(),
        (CONTROL, false, 0x00..=0x04 | 0x0e..=0x0f | 0x15) => body.is_empty(),
        (CONTROL, false, 0xa0) => matches!(body, [0 | 1]), // GET_PROTOCOL response.
        (CONTROL, false, 0xa1..=0xa3) => !body.is_empty(), // GET_REPORT response.
        (CONTROL, true, 0x13..=0x15 | 0x60 | 0x70..=0x71) => body.is_empty(),
        (CONTROL, true, 0x41..=0x43) => body.len() <= 1, // optional report ID.
        (CONTROL, true, 0x49..=0x4b) => matches!(body.len(), 2 | 3), // plus buffer size.
        (CONTROL, true, 0x51..=0x53) => !body.is_empty(),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidHidp)
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::alloc::vec;
    use super::*;

    const HANDLE: u16 = 0x0ab;
    const PEER: [u16; 2] = [0x0182, 0x0181];

    fn session() -> L2capSession {
        L2capSession::new(HANDLE, DEFAULT_MTU).unwrap()
    }

    fn pdu(cid: u16, payload: &[u8]) -> Vec<u8> {
        let mut bytes = words(&[payload.len() as u16, cid]);
        bytes.extend_from_slice(payload);
        bytes
    }

    fn cmd(code: u8, id: u8, data: &[u8]) -> Vec<u8> {
        let mut bytes = vec![code, id];
        bytes.extend_from_slice(&(data.len() as u16).to_le_bytes());
        bytes.extend_from_slice(data);
        bytes
    }

    fn receive(session: &mut L2capSession, code: u8, id: u8, data: &[u8]) {
        session
            .receive_acl(2, &pdu(1, &cmd(code, id, data)))
            .unwrap();
    }

    fn take_command(session: &mut L2capSession, expected: u8) -> (u8, Vec<u8>) {
        let acl = session.poll_acl(usize::MAX).unwrap();
        assert_eq!((acl.handle, acl.pb), (HANDLE, 2));
        assert_eq!(word(&acl.data, 0) as usize, acl.data.len() - 4);
        assert_eq!(word(&acl.data, 2), 1);
        assert_eq!(acl.data[4], expected);
        assert_ne!(acl.data[5], 0);
        assert_eq!(word(&acl.data, 6) as usize, acl.data.len() - 8);
        (acl.data[5], acl.data[8..].to_vec())
    }

    fn connect(s: &mut L2capSession, index: usize, outbound: bool) -> u8 {
        let local = LOCAL_CID[index];
        let peer = PEER[index];
        if outbound {
            let (id, payload) = take_command(s, CONNECT_REQ);
            assert_eq!(payload, words(&[PSM[index], local]));
            receive(s, CONNECT_RSP, id, &words(&[peer, local, 0, 0]));
        } else {
            receive(s, CONNECT_REQ, 201, &words(&[PSM[index], peer]));
            assert_eq!(
                take_command(s, CONNECT_RSP),
                (201, words(&[local, peer, 0, 0]))
            );
        }
        let (id, config) = take_command(s, CONFIG_REQ);
        assert_eq!(
            config,
            [words(&[peer, 0]), mtu_option(DEFAULT_MTU).to_vec()].concat()
        );
        id
    }

    fn peer_config(s: &mut L2capSession, index: usize, mtu: u16) {
        let opt = mtu_option(mtu);
        let data = [words(&[LOCAL_CID[index], 0]), opt.to_vec()].concat();
        receive(s, CONFIG_REQ, 129, &data);
        assert_eq!(
            take_command(s, CONFIG_RSP),
            (129, [words(&[PEER[index], 0, 0]), opt.to_vec()].concat())
        );
    }

    fn open(outbound: bool) -> L2capSession {
        let mut s = session();
        if outbound {
            s.start().unwrap();
            s.start().unwrap();
        }
        for index in [CONTROL, INTERRUPT] {
            let id = connect(&mut s, index, outbound);
            let response = words(&[LOCAL_CID[index], 0, 0]);
            if index == CONTROL {
                receive(&mut s, CONFIG_RSP, id, &response);
                assert!(s.poll_acl(100).is_none());
                peer_config(&mut s, index, 64);
            } else {
                peer_config(&mut s, index, 64);
                assert!(s.poll_event().is_none());
                assert!(!s.is_ready());
                assert_eq!(s.send_interrupt(&[0xa2, 1]), Err(Error::NotOpen));
                receive(&mut s, CONFIG_RSP, id, &response);
                receive(&mut s, CONFIG_RSP, id, &response);
            }
        }
        assert_eq!(s.poll_event(), Some(Event::Ready));
        assert!(s.is_ready());
        assert!(s.poll_event().is_none());
        assert!(s.poll_acl(1).is_none());
        s
    }

    #[test]
    fn outbound_and_passive_hid_use_peer_cids_and_preserve_report_bytes() {
        for outbound in [false, true] {
            let mut s = open(outbound);
            s.send_control(&[0x52, 0x01, 0x37]).unwrap();
            // The HIDP header selects the transaction. L2CAP preserves the opaque body.
            let out = [0xa2, 0x46, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0xff];
            s.send_interrupt(&out).unwrap();
            assert_eq!(
                s.poll_acl(64).unwrap().data,
                pdu(PEER[CONTROL], &[0x52, 0x01, 0x37])
            );
            assert_eq!(s.poll_acl(64).unwrap().data, pdu(PEER[INTERRUPT], &out));
            let mut input = vec![0x5a; 50];
            input[..2].copy_from_slice(&[0xa1, 0x47]);
            s.receive_acl(2, &pdu(LOCAL_CID[INTERRUPT], &input))
                .unwrap();
            assert_eq!(s.poll_event(), Some(Event::Interrupt(input)));
            s.receive_acl(2, &pdu(LOCAL_CID[CONTROL], &[0x00])).unwrap();
            assert_eq!(s.poll_event(), Some(Event::Control(vec![0])));
        }
    }

    #[test]
    fn wrong_id_cid_or_response_kind_cannot_advance_requests() {
        let mut s = session();
        s.start().unwrap();
        let (id, _) = take_command(&mut s, CONNECT_REQ);
        receive(
            &mut s,
            CONNECT_RSP,
            id + 1,
            &words(&[PEER[0], LOCAL_CID[0], 0, 0]),
        );
        receive(
            &mut s,
            CONNECT_RSP,
            id,
            &words(&[PEER[0], LOCAL_CID[1], 0, 0]),
        );
        receive(&mut s, CONNECT_RSP, id, &words(&[0, 0, 1, 2]));
        receive(&mut s, CONFIG_RSP, id, &words(&[LOCAL_CID[0], 0, 0]));
        receive(&mut s, REJECT, id + 1, &words(&[0]));
        assert!(s.poll_acl(32).is_none());
        receive(
            &mut s,
            CONNECT_RSP,
            id,
            &words(&[PEER[0], LOCAL_CID[0], 0, 0]),
        );
        let (config_id, _) = take_command(&mut s, CONFIG_REQ);
        peer_config(&mut s, CONTROL, 64);
        receive(&mut s, CONFIG_RSP, id, &words(&[LOCAL_CID[0], 0, 0]));
        receive(&mut s, CONFIG_RSP, config_id, &words(&[LOCAL_CID[1], 0, 0]));
        receive(&mut s, CONFIG_RSP, config_id, &words(&[LOCAL_CID[0], 0, 4]));
        assert_eq!(s.send_control(&[0x60]), Err(Error::NotOpen));
        assert!(s.poll_acl(32).is_none());
        receive(&mut s, CONFIG_RSP, config_id, &words(&[LOCAL_CID[0], 0, 0]));
        assert_eq!(
            take_command(&mut s, CONNECT_REQ).1,
            words(&[INTERRUPT_PSM, LOCAL_CID[1]])
        );
    }

    #[test]
    fn fragmented_reports_at_every_boundary_and_credit_sized_output() {
        let mut s = open(false);
        let data = [0xa1, 0x47, 1, 2, 3, 4, 5, 6, 7, 8];
        let frame = pdu(LOCAL_CID[1], &data);
        for n in 1..frame.len() {
            s.receive_acl(2, &frame[..n]).unwrap();
            assert!(s.poll_event().is_none());
            s.receive_acl(1, &frame[n..]).unwrap();
            assert_eq!(s.poll_event(), Some(Event::Interrupt(data.to_vec())));
        }
        let out = [0xa2, 1, 2, 3, 4, 5, 6];
        for n in 1..=12 {
            s.send_interrupt(&out).unwrap();
            assert!(s.poll_acl(0).is_none());
            let expected = pdu(PEER[1], &out);
            for (i, chunk) in expected.chunks(n).enumerate() {
                let f = s.poll_acl(n).unwrap();
                assert_eq!(f.handle, HANDLE);
                assert_eq!(f.pb, if i == 0 { 2 } else { 1 });
                assert_eq!(f.data, chunk);
            }
            assert!(s.poll_acl(n).is_none());
        }
    }

    #[test]
    fn malformed_fragments_discard_partial_and_fresh_start_recovers() {
        let mut s = open(false);
        let input = [0xa1, 0x48];
        let frame = pdu(LOCAL_CID[1], &input);
        assert_eq!(s.receive_acl(1, &[0]), Err(Error::InvalidFragment));
        s.receive_acl(2, &frame[..4]).unwrap();
        assert_eq!(s.receive_acl(1, &[1, 2, 3]), Err(Error::InvalidFragment));
        assert_eq!(s.receive_acl(1, &frame[4..]), Err(Error::InvalidFragment));
        assert_eq!(s.receive_acl(3, &frame[..3]), Err(Error::InvalidFragment));
        s.receive_acl(2, &frame[..1]).unwrap();
        assert_eq!(s.receive_acl(4, &frame), Err(Error::InvalidFragment));
        assert_eq!(s.receive_acl(1, &frame[1..]), Err(Error::InvalidFragment));
        assert_eq!(
            s.receive_acl(0, &words(&[DEFAULT_MTU + 1, LOCAL_CID[1]])),
            Err(Error::MtuExceeded)
        );
        assert_eq!(
            s.receive_acl(2, &vec![0; 65535]),
            Err(Error::InvalidFragment)
        );
        s.receive_acl(2, &frame[..3]).unwrap();
        s.receive_acl(2, &frame).unwrap();
        assert_eq!(s.poll_event(), Some(Event::Interrupt(input.to_vec())));
        assert!(s.poll_event().is_none());
    }

    #[test]
    fn configuration_continuations_are_acknowledged_and_mtu_is_directional() {
        let mut s = session();
        let id = connect(&mut s, CONTROL, false);
        let req = [words(&[LOCAL_CID[0], 1]), mtu_option(60).to_vec()].concat();
        receive(&mut s, CONFIG_REQ, 33, &req);
        assert_eq!(
            take_command(&mut s, CONFIG_RSP),
            (33, words(&[PEER[0], 1, 0]))
        );
        receive(&mut s, CONFIG_REQ, 33, &req);
        assert_eq!(
            take_command(&mut s, CONFIG_RSP),
            (33, words(&[PEER[0], 1, 0]))
        );
        let rsp = [words(&[LOCAL_CID[0], 1, 0]), mtu_option(50).to_vec()].concat();
        receive(&mut s, CONFIG_RSP, id, &rsp);
        let (next_id, config) = take_command(&mut s, CONFIG_REQ);
        assert_ne!(id, next_id);
        assert_eq!(config, words(&[PEER[0], 0]));
        receive(&mut s, CONFIG_RSP, next_id, &words(&[LOCAL_CID[0], 0, 0]));
        assert_eq!(s.send_control(&[0x60]), Err(Error::NotOpen));
        receive(&mut s, CONFIG_REQ, 34, &words(&[LOCAL_CID[0], 0]));
        assert_eq!(
            take_command(&mut s, CONFIG_RSP),
            (
                34,
                [words(&[PEER[0], 0, 0]), mtu_option(60).to_vec()].concat()
            )
        );
        let mut out = vec![0; 60];
        out[0] = 0x52;
        s.send_control(&out).unwrap();
        out.push(0);
        assert_eq!(s.send_control(&out), Err(Error::MtuExceeded));
        let mut incoming = vec![0; 50];
        incoming[0] = 0xa3;
        s.receive_acl(2, &pdu(LOCAL_CID[0], &incoming)).unwrap();
        incoming.push(0);
        assert_eq!(
            s.receive_acl(2, &pdu(LOCAL_CID[0], &incoming)),
            Err(Error::MtuExceeded)
        );
    }

    #[test]
    fn configuration_rejects_unknown_required_options_and_retries_mtu_once() {
        let mut s = session();
        let id = connect(&mut s, CONTROL, false);
        let req = |opts: &[u8]| [words(&[LOCAL_CID[0], 0]), opts.to_vec()].concat();
        for (incoming, result, reply) in [
            (&[0x37, 1, 9][..], 3, &[0x37, 1, 9][..]),
            (&[1, 2, 35, 0], 1, &[1, 2, 48, 0]),
            (&[1, 2, 60], 2, &[]),
            (&[0xb7, 1, 9], 0, &[]),
        ] {
            receive(&mut s, CONFIG_REQ, 62, &req(incoming));
            assert_eq!(
                take_command(&mut s, CONFIG_RSP),
                (62, [words(&[PEER[0], 0, result]), reply.to_vec()].concat())
            );
        }
        let negative = |mtu| [words(&[LOCAL_CID[0], 0, 1]), mtu_option(mtu).to_vec()].concat();
        receive(&mut s, CONFIG_RSP, id, &negative(128));
        let (retry, payload) = take_command(&mut s, CONFIG_REQ);
        assert_ne!(retry, id);
        assert_eq!(
            payload,
            [words(&[PEER[0], 0]), mtu_option(128).to_vec()].concat()
        );
        receive(&mut s, CONFIG_RSP, retry, &negative(64));
        let (id, data) = take_command(&mut s, DISCONNECT_REQ);
        receive(&mut s, DISCONNECT_RSP, id, &data);
        assert_eq!(
            s.poll_event(),
            Some(Event::Closed(CloseReason::ConfigurationFailed(1)))
        );
    }

    #[test]
    fn bundled_commands_incoming_refusals_and_retransmitted_connect() {
        let mut s = session();
        let a = cmd(CONNECT_REQ, 10, &words(&[0x0f, 70]));
        let b = cmd(CONNECT_REQ, 11, &words(&[CONTROL_PSM, 2]));
        let c = cmd(0xf1, 12, &[1]);
        let mut frame = pdu(1, &[a.clone(), b.clone(), c.clone()].concat());
        frame.pop();
        frame[0] -= 1;
        assert_eq!(s.receive_acl(2, &frame), Err(Error::MalformedSignal));
        assert!(s.poll_acl(64).is_none());
        let frame = pdu(1, &[a, b, c].concat());
        for (i, byte) in frame.chunks(1).enumerate() {
            s.receive_acl(if i == 0 { 2 } else { 1 }, byte).unwrap();
        }
        assert_eq!(
            take_command(&mut s, CONNECT_RSP),
            (10, words(&[0, 70, 2, 0]))
        );
        assert_eq!(
            take_command(&mut s, CONNECT_RSP),
            (11, words(&[0, 2, 6, 0]))
        );
        assert_eq!(take_command(&mut s, REJECT), (12, words(&[0])));
        connect(&mut s, CONTROL, false);
        receive(&mut s, CONNECT_REQ, 201, &words(&[CONTROL_PSM, PEER[0]]));
        assert_eq!(
            take_command(&mut s, CONNECT_RSP),
            (201, words(&[LOCAL_CID[0], PEER[0], 0, 0]))
        );
        assert!(s.poll_acl(64).is_none());
        receive(&mut s, CONNECT_REQ, 202, &words(&[INTERRUPT_PSM, PEER[0]]));
        assert_eq!(
            take_command(&mut s, CONNECT_RSP),
            (202, words(&[0, PEER[0], 7, 0]))
        );
        receive(&mut s, CONFIG_REQ, 9, &words(&[7, 0]));
        assert_eq!(take_command(&mut s, REJECT), (9, words(&[2, 7, 0])));
    }

    #[test]
    fn refusal_and_command_reject_close_only_matching_requests() {
        for code in [REJECT, CONNECT_RSP] {
            let mut s = session();
            s.start().unwrap();
            let (id, _) = take_command(&mut s, CONNECT_REQ);
            let data = if code == REJECT {
                words(&[0])
            } else {
                words(&[0, 0, 3, 0])
            };
            receive(&mut s, code, id + 1, &data);
            assert!(s.poll_event().is_none());
            receive(&mut s, code, id, &data);
            let reason = if code == REJECT {
                CloseReason::CommandRejected(0)
            } else {
                CloseReason::ConnectionRefused(3)
            };
            assert_eq!(s.poll_event(), Some(Event::Closed(reason)));
            assert_eq!(s.start(), Err(Error::NotOpen));
        }
    }

    #[test]
    fn disconnect_in_reverse_order_requires_matching_ids_and_both_cids() {
        for remote in [false, true] {
            let mut s = open(false);
            s.send_interrupt(&[0xa2, 1]).unwrap();
            receive(&mut s, DISCONNECT_REQ, 22, &words(&[LOCAL_CID[1], PEER[0]]));
            assert!(s.is_ready());
            if remote {
                let data = words(&[LOCAL_CID[1], PEER[1]]);
                receive(&mut s, DISCONNECT_REQ, 23, &data);
                assert_eq!(take_command(&mut s, DISCONNECT_RSP), (23, data));
            } else {
                s.disconnect().unwrap();
                let (id, intr) = take_command(&mut s, DISCONNECT_REQ);
                assert_eq!(intr, words(&[PEER[1], LOCAL_CID[1]]));
                receive(&mut s, DISCONNECT_RSP, id + 1, &intr);
                receive(&mut s, DISCONNECT_RSP, id, &words(&[PEER[0], LOCAL_CID[1]]));
                assert!(s.poll_acl(64).is_none());
                receive(&mut s, DISCONNECT_RSP, id, &intr);
            }
            assert!(!s.is_ready());
            assert_eq!(s.send_interrupt(&[0xa2, 3]), Err(Error::NotOpen));
            let (id, ctl) = take_command(&mut s, DISCONNECT_REQ);
            assert_eq!(ctl, words(&[PEER[0], LOCAL_CID[0]]));
            receive(&mut s, DISCONNECT_RSP, id, &ctl);
            assert_eq!(
                s.poll_event(),
                Some(Event::Closed(if remote {
                    CloseReason::Peer
                } else {
                    CloseReason::Local
                }))
            );
        }
    }

    #[test]
    fn validates_report_direction_transaction_kind_parameter_and_length() {
        let mut s = open(false);
        for bad in [
            &[][..],
            &[0xa1],
            &[0xa2, 1],
            &[0xa3, 1],
            &[0xa5, 1],
            &[0xb1, 1],
            &[0x00],
        ] {
            assert_eq!(
                s.receive_acl(2, &pdu(LOCAL_CID[1], bad)),
                Err(Error::InvalidHidp)
            );
        }
        for bad in [
            &[0x05][..],
            &[0, 1],
            &[0x15, 1],
            &[0x41, 1],
            &[0xa0, 2],
            &[0xa3],
        ] {
            assert_eq!(
                s.receive_acl(2, &pdu(LOCAL_CID[0], bad)),
                Err(Error::InvalidHidp)
            );
        }
        for bad in [&[][..], &[0xa2], &[0xa1, 1], &[0x52, 1], &[0xb2, 1]] {
            assert_eq!(s.send_interrupt(bad), Err(Error::InvalidHidp));
        }
        for bad in [
            &[0x52][..],
            &[0x4a, 1],
            &[0x42, 1, 2],
            &[0x72],
            &[0x00],
            &[0xa1, 1],
        ] {
            assert_eq!(s.send_control(bad), Err(Error::InvalidHidp));
        }
        assert!(s.poll_event().is_none());
        assert!(s.poll_acl(64).is_none());
    }

    #[test]
    fn queues_are_bounded_and_signal_exhaustion_is_terminal() {
        let mut s = open(false);
        let input = pdu(LOCAL_CID[1], &[0xa1, 1]);
        for _ in 0..EVENT_LIMIT {
            s.receive_acl(2, &input).unwrap();
        }
        assert_eq!(s.receive_acl(2, &input), Err(Error::QueueFull));
        assert!(s.poll_event().is_some());
        s.receive_acl(2, &input).unwrap();
        for _ in 0..TX_LIMIT - 4 {
            s.send_interrupt(&[0xa2, 1]).unwrap();
        }
        assert_eq!(s.send_interrupt(&[0xa2, 1]), Err(Error::QueueFull));
        for id in 1..=4 {
            receive(&mut s, ECHO_REQ, id, &[]);
        }
        assert_eq!(
            s.receive_acl(2, &pdu(1, &cmd(ECHO_REQ, 5, &[]))),
            Err(Error::QueueFull)
        );
        assert_eq!(s.poll_event(), Some(Event::Closed(CloseReason::QueueFull)));
        assert!(s.poll_event().is_none());
        assert!(s.poll_acl(64).is_none());
    }

    #[test]
    fn oversized_signaling_rejects_first_request_even_after_large_response() {
        let mut s = session();
        let first = cmd(ECHO_RSP, 8, &vec![0; 60000]);
        let payload = [
            first.clone(),
            cmd(ECHO_REQ, 9, &[1]),
            cmd(INFO_REQ, 10, &[2, 0]),
        ]
        .concat();
        for payload in [&first, &payload] {
            let frame = pdu(1, payload);
            for (i, chunk) in frame.chunks(13).enumerate() {
                s.receive_acl(if i == 0 { 2 } else { 1 }, chunk).unwrap();
            }
            if payload.len() > first.len() {
                assert_eq!(take_command(&mut s, REJECT), (9, words(&[1, SIGNAL_MTU])));
            }
            assert!(s.poll_acl(32).is_none());
        }
        receive(&mut s, ECHO_REQ, 29, &[1, 2]);
        assert_eq!(take_command(&mut s, ECHO_RSP), (29, vec![1, 2]));
    }

    #[test]
    fn closing_during_connect_waits_for_cid_and_last_remote_reply_drains_before_closed() {
        let mut outbound = session();
        outbound.start().unwrap();
        let (id, _) = take_command(&mut outbound, CONNECT_REQ);
        outbound.disconnect().unwrap();
        assert!(outbound.poll_event().is_none());
        receive(
            &mut outbound,
            CONNECT_RSP,
            id,
            &words(&[PEER[0], LOCAL_CID[0], 0, 0]),
        );
        let (id, data) = take_command(&mut outbound, DISCONNECT_REQ);
        assert_eq!(data, words(&[PEER[0], LOCAL_CID[0]]));
        receive(&mut outbound, DISCONNECT_RSP, id, &data);
        assert_eq!(
            outbound.poll_event(),
            Some(Event::Closed(CloseReason::Local))
        );

        let mut inbound = session();
        connect(&mut inbound, CONTROL, false);
        let data = words(&[LOCAL_CID[0], PEER[0]]);
        receive(&mut inbound, DISCONNECT_REQ, 51, &data);
        assert!(inbound.poll_event().is_none());
        assert_eq!(take_command(&mut inbound, DISCONNECT_RSP), (51, data));
        assert_eq!(inbound.poll_event(), Some(Event::Closed(CloseReason::Peer)));
    }

    #[test]
    fn incoming_connection_can_take_over_an_unsent_open_but_never_a_partial_pdu() {
        let mut queued = session();
        queued.start().unwrap();
        for index in [CONTROL, INTERRUPT] {
            let id = connect(&mut queued, index, false);
            peer_config(&mut queued, index, 64);
            receive(
                &mut queued,
                CONFIG_RSP,
                id,
                &words(&[LOCAL_CID[index], 0, 0]),
            );
            assert!(queued.poll_acl(32).is_none());
        }
        assert_eq!(queued.poll_event(), Some(Event::Ready));

        let mut sending = session();
        sending.start().unwrap();
        let mut original = sending.poll_acl(1).unwrap().data;
        receive(
            &mut sending,
            CONNECT_REQ,
            72,
            &words(&[CONTROL_PSM, PEER[0]]),
        );
        let rest = sending.poll_acl(32).unwrap();
        assert_eq!(rest.pb, 1);
        original.extend(rest.data);
        assert_eq!(
            original,
            pdu(
                1,
                &cmd(CONNECT_REQ, 1, &words(&[CONTROL_PSM, LOCAL_CID[0]]))
            )
        );
        assert_eq!(
            take_command(&mut sending, CONNECT_RSP),
            (72, words(&[0, PEER[0], 4, 0]))
        );

        let mut canceled = session();
        canceled.start().unwrap();
        canceled.disconnect().unwrap();
        assert_eq!(
            canceled.poll_event(),
            Some(Event::Closed(CloseReason::Local))
        );
        assert!(canceled.poll_acl(32).is_none());
    }

    #[test]
    fn mtu_and_handle_bounds_default_outbound_and_implementation_cap() {
        assert!(matches!(
            L2capSession::new(0x0f00, DEFAULT_MTU),
            Err(Error::InvalidHandle)
        ));
        for mtu in [0, MIN_MTU - 1, MAX_MTU + 1] {
            assert!(matches!(L2capSession::new(1, mtu), Err(Error::InvalidMtu)));
        }
        let mut s = session();
        let id = connect(&mut s, CONTROL, false);
        receive(&mut s, CONFIG_RSP, id, &words(&[LOCAL_CID[0], 0, 0]));
        receive(&mut s, CONFIG_REQ, 111, &words(&[LOCAL_CID[0], 0]));
        assert_eq!(
            take_command(&mut s, CONFIG_RSP),
            (111, words(&[PEER[0], 0, 0]))
        );
        let mut report = vec![0; usize::from(MAX_MTU) + 1];
        report[0] = 0x52;
        s.send_control(&report[..usize::from(DEFAULT_MTU)]).unwrap();
        assert_eq!(
            s.send_control(&report[..usize::from(DEFAULT_MTU) + 1]),
            Err(Error::MtuExceeded)
        );
        s.poll_acl(usize::MAX).unwrap();
        let options = [words(&[LOCAL_CID[0], 0]), mtu_option(u16::MAX).to_vec()].concat();
        receive(&mut s, CONFIG_REQ, 112, &options);
        assert_eq!(
            take_command(&mut s, CONFIG_RSP),
            (
                112,
                [words(&[PEER[0], 0, 0]), mtu_option(MAX_MTU).to_vec()].concat()
            )
        );
        s.send_control(&report[..usize::from(MAX_MTU)]).unwrap();
        assert_eq!(s.send_control(&report), Err(Error::MtuExceeded));
    }
}
