//! Ethernet facade with a connection-scoped controlled port. Queue retirement
//! does not reclaim submitted DMA or retract data already delivered to sockets.

extern crate alloc;
use super::{
    auth::*,
    ccmp::{Error as CcmpError, KeyRing},
    gate::Gate,
};
use crate::{
    device::network::{DevicePacket, LinkObservation, MacAddress},
    network::{InterfaceStats, NetworkInterface, get_network_manager, ipv4::Ipv4Address},
    sync::{IrqRwSpinLock, IrqSpinLock},
};
use alloc::{collections::VecDeque, string::String, sync::Arc, vec::Vec};
use scarlet_abi::wireless::*;

const HELD_LIMIT: usize = 64;
#[derive(Clone, Copy, PartialEq, Eq)]
struct ConnectionObservation {
    device: u64,
    connection: u64,
    control_epoch: u64,
    authorized: bool,
    transport_state: u8,
    transport_epoch: u64,
    association: Option<Association>,
}
struct State {
    notifier: Option<Arc<dyn WirelessNotifier>>,
    gate: Gate,
    seen: Option<ConnectionObservation>,
    hold: bool,
    tx: VecDeque<DevicePacket>,
    rx: VecDeque<DevicePacket>,
    stale_tx: Option<DevicePacket>,
    stale_rx: Option<DevicePacket>,
    stats: DataTest,
    association: Option<Association>,
    eapol: EapolInbox,
    ccmp: Option<KeyRing>,
}
fn decode_eapol(
    bytes: &[u8],
    own: [u8; 6],
    association: Association,
    protected: bool,
) -> Option<Eapol> {
    if bytes.len() < 18
        || bytes[12..14] != [0x88, 0x8e]
        || bytes[6..12] != association.peer
        || (bytes[..6] != own && bytes[..6] != [1, 0x80, 0xc2, 0, 0, 3])
    {
        return None;
    }
    let len = 4 + usize::from(u16::from_be_bytes([bytes[16], bytes[17]]));
    if len > MAX_EAPOL || bytes.len() < 14 + len || !valid_eapol(&bytes[14..14 + len]) {
        return None;
    }
    let mut frame = Eapol {
        peer: association.peer,
        length: len as u32,
        flags: if protected { EAPOL_PROTECTED } else { 0 },
        ..Eapol::default()
    };
    frame.payload[..len].copy_from_slice(&bytes[14..14 + len]);
    Some(frame)
}
fn push_eapol(s: &mut State, frame: Eapol) {
    s.eapol.push(frame);
    if s.eapol.fault {
        s.gate.authorized = false;
    }
    if let Some(notifier) = &s.notifier {
        notifier.notify();
    }
}
fn wrap_mpdu(own: [u8; 6], peer: [u8; 6], mpdu: &[u8], token: Option<(u64, u64)>) -> DevicePacket {
    let mut bytes = Vec::with_capacity(14 + mpdu.len());
    bytes.extend_from_slice(&peer);
    bytes.extend_from_slice(&own);
    bytes.extend_from_slice(&[0x88, 0xb5]);
    bytes.extend_from_slice(mpdu);
    let mut packet = DevicePacket::with_data(bytes);
    packet.link_token = token;
    packet
}
pub struct WirelessInterface {
    name: String,
    transport: Arc<dyn NetworkInterface>,
    state: IrqSpinLock<State>,
    /// RX processing can generate a synchronous TX response. A separate barrier
    /// serializes stack delivery with retirement without reentering state.lock.
    ingress: IrqRwSpinLock<()>,
    ip: IrqSpinLock<Option<Ipv4Address>>,
}
impl WirelessInterface {
    pub fn new(
        name: String,
        transport: Arc<dyn NetworkInterface>,
        ccmp: bool,
    ) -> Result<Self, &'static str> {
        if !transport.supports_packet_retirement() {
            return Err("wireless: transport cannot retire software queues");
        }
        Ok(Self {
            name,
            transport,
            ingress: IrqRwSpinLock::new(()),
            ip: IrqSpinLock::new(None),
            state: IrqSpinLock::new(State {
                notifier: None,
                gate: Gate::new(),
                seen: None,
                hold: false,
                tx: VecDeque::with_capacity(HELD_LIMIT),
                rx: VecDeque::with_capacity(HELD_LIMIT),
                stale_tx: None,
                stale_rx: None,
                stats: DataTest::default(),
                association: None,
                eapol: EapolInbox::default(),
                ccmp: ccmp.then(KeyRing::default),
            }),
        })
    }
    pub fn bind_link_id(&self, id: u64) {
        self.state.lock().gate.link_id = id;
    }
    pub fn attach_notifier(&self, notifier: Arc<dyn WirelessNotifier>) {
        self.state.lock().notifier = Some(notifier);
    }

    /// Called only by the serialized radio actor, never with a carrier lock held.
    pub fn reconcile(
        &self,
        device: u64,
        connection: u64,
        epoch: u64,
        authorized: bool,
        association: Option<Association>,
    ) -> Result<(), &'static str> {
        let transport = self.transport.link_observation();
        let up = transport.state == scarlet_abi::network::LINK_UP;
        let observed = ConnectionObservation {
            device,
            connection,
            control_epoch: epoch,
            authorized,
            transport_state: transport.state,
            transport_epoch: transport.epoch,
            association,
        };
        if self.state.lock().seen == Some(observed) {
            return Ok(());
        }
        let _delivery = self.ingress.write();
        let id;
        let retirement;
        {
            let mut state = self.state.lock();
            // Complete transport retirement before releasing the state lock.
            // TX admission is blocked by that lock; pending DMA retains ownership.
            let same_association = association.is_some()
                && state.association == association
                && state
                    .seen
                    .is_some_and(|previous| previous.transport_epoch == transport.epoch);
            let retired = if same_association {
                self.transport.retire_data_packets()
            } else {
                self.transport.retire_packets()
            };
            let advanced = state.gate.retire();
            state.stats.tx_dropped += state.tx.len() as u64;
            state.stats.rx_dropped += state.rx.len() as u64;
            state.tx.clear();
            state.rx.clear();
            state.hold = false;
            state.stats.retirements += 1;
            state.seen = Some(observed);
            if state.association != association {
                state.eapol.clear();
                state.association = association;
            }
            id = state.gate.link_id;
            retirement = if advanced {
                retired
            } else {
                Err("wireless: link epochs exhausted")
            };
        }
        // NetworkManager takes lifecycle -> observation lock. State is released
        // above so no observation -> lifecycle inversion is possible.
        let configuration = get_network_manager().retire_link_ipv4(&self.name, id);
        retirement?;
        configuration?;
        if authorized {
            if !up {
                return Err("wireless: transport carrier lost");
            }
            let mut state = self.state.lock();
            if state.eapol.fault
                || state.ccmp.as_ref().is_some_and(|ring| !ring.ready())
                || !state.gate.authorize()
            {
                return Err("wireless: cannot authorize port");
            }
        }
        Ok(())
    }
    fn transport_matches(&self, state: &State) -> bool {
        let transport = self.transport.link_observation();
        !state.eapol.fault
            && transport.state == scarlet_abi::network::LINK_UP
            && state
                .seen
                .is_some_and(|seen| seen.transport_epoch == transport.epoch)
    }
    pub fn test_control(&self, operation: u32) -> Result<DataTest, &'static str> {
        match operation {
            DATA_HOLD => {
                self.state.lock().hold = true;
            }
            DATA_RELEASE => {
                let (tx, rx) = {
                    let mut s = self.state.lock();
                    s.hold = false;
                    (core::mem::take(&mut s.tx), core::mem::take(&mut s.rx))
                };
                for p in tx {
                    let _ = self.send(p);
                }
                for p in rx {
                    self.receive_packet(&p);
                }
            }
            DATA_REPLAY_STALE => {
                let (tx, rx) = {
                    let mut s = self.state.lock();
                    (s.stale_tx.take(), s.stale_rx.take())
                };
                if let Some(p) = tx {
                    let _ = self.send(p);
                }
                if let Some(p) = rx {
                    self.receive_packet(&p);
                }
            }
            DATA_STATS => {}
            _ => return Err("wireless: invalid data test operation"),
        }
        let mut stats = {
            let s = self.state.lock();
            DataTest {
                tx_queued: s.tx.len() as u64,
                rx_queued: s.rx.len() as u64,
                ..s.stats
            }
        };
        if let Some(layer) = get_network_manager().get_layer("arp") {
            if let Some(arp) = layer
                .as_any()
                .downcast_ref::<crate::network::arp::ArpLayer>()
            {
                stats.arp_queued = arp.pending_packet_count(&self.name) as u64;
            }
        }
        stats.operation = operation;
        Ok(stats)
    }
}
impl NetworkInterface for WirelessInterface {
    fn name(&self) -> &str {
        &self.name
    }
    fn mac_address(&self) -> MacAddress {
        self.transport.mac_address()
    }
    fn link_state(&self) -> u8 {
        self.link_observation().state
    }
    fn link_kind(&self) -> u8 {
        scarlet_abi::network::LINK_KIND_WIFI
    }
    fn link_epoch(&self) -> u64 {
        self.state.lock().gate.epoch
    }
    fn mtu(&self) -> u32 {
        self.transport.mtu()
    }
    fn link_observation(&self) -> LinkObservation {
        let s = self.state.lock();
        let transport = self.transport.link_observation();
        LinkObservation {
            mac: transport.mac,
            mtu: transport.mtu,
            kind: self.link_kind(),
            epoch: s.gate.epoch,
            state: if s.gate.authorized
                && !s.eapol.fault
                && transport.state == scarlet_abi::network::LINK_UP
                && s.seen
                    .is_some_and(|seen| seen.transport_epoch == transport.epoch)
            {
                scarlet_abi::network::LINK_UP
            } else {
                scarlet_abi::network::LINK_DOWN
            },
        }
    }
    fn ip_address(&self) -> Option<Ipv4Address> {
        *self.ip.lock()
    }
    fn set_ip_address(&self, ip: Ipv4Address) {
        *self.ip.lock() = Some(ip);
    }
    fn clear_ip_address(&self) {
        *self.ip.lock() = None;
    }
    fn packet_token(&self) -> Result<Option<(u64, u64)>, &'static str> {
        let s = self.state.lock();
        if !self.transport_matches(&s) {
            return Err("wireless: transport incarnation changed");
        }
        s.gate
            .token()
            .map(Some)
            .ok_or("wireless: controlled port closed")
    }
    fn receive_token(&self) -> Result<Option<(u64, u64)>, &'static str> {
        let s = self.state.lock();
        if !self.transport_matches(&s) {
            return Err("wireless: transport changed");
        }
        if s.association.is_some() {
            return Ok(Some((s.gate.link_id, s.gate.epoch)));
        }
        s.gate
            .token()
            .map(Some)
            .ok_or("wireless: no receiving port")
    }
    fn send(&self, mut packet: DevicePacket) -> Result<(), &'static str> {
        let mut s = self.state.lock();
        if !self.transport_matches(&s)
            || packet.len > packet.data.len()
            || packet.len < 14
            || s.gate.token().is_none()
            || (packet.link_token.is_some() && !s.gate.accepts(packet.link_token))
        {
            s.stats.tx_dropped += 1;
            return Err("wireless: closed port or stale TX packet");
        }
        packet.link_token = s.gate.token();
        if s.hold {
            if s.tx.len() == HELD_LIMIT {
                s.stats.tx_dropped += 1;
                return Err("wireless: test TX queue full");
            }
            s.stale_tx = Some(packet.clone());
            s.tx.push_back(packet);
            return Ok(());
        }
        // Linearizes final software submission with gate closure/queue retirement.
        if s.ccmp.is_some() {
            let association = s.association.ok_or("wireless: no CCMP association")?;
            let result = s.ccmp.as_mut().unwrap().seal(
                association,
                *self.transport.mac_address().as_bytes(),
                &packet.data[..packet.len],
                None,
            );
            let mpdu = match result {
                Ok(p) => p,
                Err(error) => {
                    if error == CcmpError::Exhausted {
                        s.gate.authorized = false;
                        s.eapol.fault = true;
                        if let Some(notifier) = &s.notifier {
                            notifier.notify();
                        }
                    }
                    s.stats.tx_dropped += 1;
                    return Err("wireless: CCMP encryption refused");
                }
            };
            // Test transport only: experimental EtherType carries a complete
            // 802.11 MPDU. Real radios submit the MPDU to their native TX ring.
            packet = wrap_mpdu(
                *self.transport.mac_address().as_bytes(),
                association.peer,
                &mpdu,
                packet.link_token,
            );
        }
        self.transport.send(packet)?;
        s.stats.tx_submitted += 1;
        Ok(())
    }
    fn receive_dropped(&self, packet: &DevicePacket) {
        if packet.len >= 14
            && packet.len <= packet.data.len()
            && matches!(&packet.data[12..14], [0x88, 0x8e] | [0x88, 0xb5])
        {
            let mut s = self.state.lock();
            if s.association.is_some() && packet.link_token == Some((s.gate.link_id, s.gate.epoch))
            {
                s.eapol.fault = true;
                if let Some(notifier) = &s.notifier {
                    notifier.notify();
                }
                s.gate.authorized = false;
            }
        }
    }
    fn prepare_receive(&self, packet: &mut DevicePacket) -> bool {
        let mut s = self.state.lock();
        let eapol = packet.len >= 14
            && packet.len <= packet.data.len()
            && packet.data[12..14] == [0x88, 0x8e];
        let protected = packet.len >= 14
            && packet.len <= packet.data.len()
            && packet.data[12..14] == [0x88, 0xb5]
            && s.ccmp.is_some();
        let admitted = if (eapol || protected) && s.association.is_some() {
            Some((s.gate.link_id, s.gate.epoch))
        } else {
            s.gate.token()
        };
        if !self.transport_matches(&s)
            || admitted.is_none()
            || (packet.link_token.is_some() && packet.link_token != admitted)
        {
            s.stats.rx_dropped += 1;
            return false;
        }
        packet.link_token = admitted;
        true
    }
    fn receive_packet(&self, packet: &DevicePacket) {
        let _delivery = self.ingress.read();
        let mut plaintext = None;
        {
            let mut s = self.state.lock();
            if packet.len < 14
                || packet.len > packet.data.len()
                || !self.transport_matches(&s)
                || packet.link_token != Some((s.gate.link_id, s.gate.epoch))
            {
                s.stats.rx_dropped += 1;
                return;
            }
            if packet.data[12..14] == [0x88, 0x8e] {
                let Some(association) = s.association else {
                    s.stats.rx_dropped += 1;
                    return;
                };
                if let Some(frame) = decode_eapol(
                    &packet.data[..packet.len],
                    *self.transport.mac_address().as_bytes(),
                    association,
                    false,
                ) {
                    push_eapol(&mut s, frame);
                } else {
                    s.stats.rx_dropped += 1;
                }
                return;
            }
            if s.ccmp.is_some() {
                if packet.data[12..14] != [0x88, 0xb5] {
                    s.stats.rx_dropped += 1;
                    return;
                }
                let Some(association) = s.association else {
                    s.stats.rx_dropped += 1;
                    return;
                };
                let control_only = s.hold || !s.gate.authorized;
                let own = *self.transport.mac_address().as_bytes();
                let crypto = s.ccmp.as_mut().unwrap();
                let result = if control_only {
                    crypto.open_control(association, own, &packet.data[14..packet.len])
                } else {
                    crypto.open(association, own, &packet.data[14..packet.len])
                };
                match result {
                    Ok(bytes) => {
                        if bytes[12..14] == [0x88, 0x8e] {
                            if let Some(frame) = decode_eapol(&bytes, own, association, true) {
                                push_eapol(&mut s, frame);
                            } else {
                                s.stats.rx_dropped += 1;
                            }
                            return;
                        }
                        let mut decoded = DevicePacket::with_data(bytes);
                        decoded.link_token = packet.link_token;
                        plaintext = Some(decoded);
                    }
                    Err(CcmpError::NotControl) if s.hold && s.gate.authorized => {}
                    Err(_) => {
                        s.stats.rx_dropped += 1;
                        return;
                    }
                }
            }
            if !s.gate.accepts(packet.link_token) {
                s.stats.rx_dropped += 1;
                return;
            }
            if s.hold {
                if s.rx.len() == HELD_LIMIT {
                    s.stats.rx_dropped += 1;
                    return;
                }
                s.stale_rx = Some(packet.clone());
                s.rx.push_back(packet.clone());
                return;
            }
            s.stats.rx_delivered += 1;
        }
        get_network_manager()
            .deliver_received_packet(&self.name, plaintext.as_ref().unwrap_or(packet));
    }
    fn poll(&self) -> Result<Vec<DevicePacket>, &'static str> {
        // Interrupt backends stamp at completion and use receive_packet. A future
        // polling backend must stamp before deferring, not after reconnection.
        Ok(Vec::new())
    }
    fn stats(&self) -> InterfaceStats {
        self.transport.stats()
    }
}

impl AuthenticationPort for WirelessInterface {
    fn receive_frame(&self, packet: &DevicePacket) -> Result<(), i32> {
        // Admission stamps the live gate epoch; the transport cannot know it.
        let mut stamped = packet.clone();
        if !NetworkInterface::prepare_receive(self, &mut stamped) {
            return Err(STALE_CONNECTION);
        }
        NetworkInterface::receive_packet(self, &stamped);
        Ok(())
    }
    fn block(&self) -> Result<(), i32> {
        let _delivery = self.ingress.write();
        let id;
        let result;
        let advanced;
        {
            let mut s = self.state.lock();
            advanced = s.gate.retire();
            s.stats.tx_dropped += s.tx.len() as u64;
            s.stats.rx_dropped += s.rx.len() as u64;
            s.tx.clear();
            s.rx.clear();
            s.hold = false;
            s.stats.retirements += 1;
            id = s.gate.link_id;
            result = self.transport.retire_data_packets();
        }
        let cleared = get_network_manager().retire_link_ipv4(&self.name, id);
        if !advanced {
            return Err(RESOURCE_LIMIT);
        }
        if result.is_err() || cleared.is_err() {
            return Err(FAILED);
        }
        Ok(())
    }
    fn install_ccmp(&self, scope: Association, key: &KeyInstall) -> Result<(), i32> {
        let mut s = self.state.lock();
        if s.association != Some(scope) || s.gate.authorized {
            return Err(STALE_CONNECTION);
        }
        s.ccmp
            .as_mut()
            .ok_or(NOT_SUPPORTED)?
            .install(scope, key)
            .map_err(|_| FAILED)
    }
    fn replace_group_ccmp(&self, scope: Association, key: &KeyInstall) -> Result<(), i32> {
        // Drain pre-cutover stack delivery before dropping old GTKs. PTK TX
        // continues until this short atomic commit; no link/IP retirement.
        let _delivery = self.ingress.write();
        let mut s = self.state.lock();
        if s.association != Some(scope) || !s.gate.authorized || !self.transport_matches(&s) {
            return Err(STALE_CONNECTION);
        }
        s.ccmp
            .as_mut()
            .ok_or(NOT_SUPPORTED)?
            .replace_group(scope, key)
            .map_err(|_| FAILED)
    }
    fn delete_ccmp(&self, scope: Association, key: KeyRef) -> Result<(), i32> {
        let mut s = self.state.lock();
        if s.association != Some(scope) || s.gate.authorized {
            return Err(STALE_CONNECTION);
        }
        s.ccmp
            .as_mut()
            .ok_or(NOT_SUPPORTED)?
            .delete(scope, key)
            .map_err(|_| FAILED)
    }
    fn retire_ccmp(&self) -> Result<(), i32> {
        let mut s = self.state.lock();
        if s.gate.authorized {
            return Err(FAILED);
        }
        s.ccmp.as_mut().ok_or(NOT_SUPPORTED)?.clear();
        Ok(())
    }
    fn ccmp_status(&self) -> Result<CcmpStatus, i32> {
        self.state
            .lock()
            .ccmp
            .as_ref()
            .map(KeyRing::status)
            .ok_or(NOT_SUPPORTED)
    }
    fn tx_eapol(&self, association: Association, frame: &Eapol) -> Result<(), i32> {
        let mut s = self.state.lock();
        if s.association != Some(association) || !self.transport_matches(&s) || s.eapol.fault {
            return Err(STALE_CONNECTION);
        }
        if !valid_eapol_request(frame, association.peer) {
            return Err(INVALID);
        }
        let own = *self.transport.mac_address().as_bytes();
        let mut bytes = Vec::with_capacity((14 + frame.length as usize).max(60));
        bytes.extend_from_slice(&frame.peer);
        bytes.extend_from_slice(&own);
        bytes.extend_from_slice(&[0x88, 0x8e]);
        bytes.extend_from_slice(&frame.payload[..frame.length as usize]);
        let token = Some((s.gate.link_id, s.gate.epoch));
        let packet = if frame.flags & EAPOL_PROTECTED != 0 {
            let result = s
                .ccmp
                .as_mut()
                .ok_or(NOT_SUPPORTED)?
                .seal(association, own, &bytes, None);
            let mpdu = match result {
                Ok(p) => p,
                Err(e) => {
                    if e == CcmpError::Exhausted {
                        s.gate.authorized = false;
                        s.eapol.fault = true;
                        if let Some(notifier) = &s.notifier {
                            notifier.notify();
                        }
                    }
                    return Err(FAILED);
                }
            };
            wrap_mpdu(own, association.peer, &mpdu, token)
        } else {
            bytes.resize(bytes.len().max(60), 0);
            let mut p = DevicePacket::with_data(bytes);
            p.link_token = token;
            p
        };
        self.transport.send(packet).map_err(|_| FAILED)
    }
    fn peek_eapol(&self, association: Association) -> Result<Option<Eapol>, i32> {
        let s = self.state.lock();
        if s.association != Some(association) || !self.transport_matches(&s) {
            return Err(STALE_CONNECTION);
        }
        if s.eapol.fault {
            return Err(FAILED);
        }
        Ok(s.eapol.peek())
    }
    fn consume_eapol(&self, association: Association, sequence: u64) {
        let mut s = self.state.lock();
        if s.association == Some(association) {
            s.eapol.consume(sequence);
        }
    }
    fn eapol_state(&self) -> (u64, usize, bool) {
        let s = self.state.lock();
        (s.eapol.sequence, s.eapol.len(), s.eapol.fault)
    }
}
