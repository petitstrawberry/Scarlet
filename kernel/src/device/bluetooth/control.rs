//! Per-controller management, independent of UART and USB. Pairing key export
//! belongs to the open endpoint that initiated that pairing operation.
use super::classic::{Address, Name, Peer};
use crate::{
    device::{Device, DeviceType},
    library::std::usercopy::{copy_from_user, copy_to_user},
    object::capability::{
        ControlOps, MemoryMappingInfo, MemoryMappingOps, Selectable,
        selectable::{ReadyInterest, SelectWaitOutcome},
    },
    shield::authorization::{Management, permits_management},
    sync::{Mutex, Waker},
};
use alloc::{sync::Arc, vec::Vec};
use core::{
    any::Any,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use scarlet_abi::bluetooth as abi;
use zeroize::Zeroizing;

pub(super) enum Request {
    Scan,
    Pair(Address),
    Connect(Address, Option<Zeroizing<[u8; 16]>>),
    Disconnect(Address),
}

// Retain recent operation results even when a connection attempt never reached LinkUp.
const MAX_PEER_RECORDS: usize = 64;

struct PeerState {
    address: Address,
    name: Option<Name>,
    generation: u32,
    flags: u32,
    error: u32,
    pairing_owner: usize,
    key: Option<(u8, Zeroizing<[u8; 16]>)>,
}

impl PeerState {
    fn flags_for(&self, owner: usize) -> u32 {
        if owner != 0
            && self.pairing_owner == owner
            && self.key.is_some()
            && self.flags & (abi::ENCRYPTED | abi::BUSY) == abi::ENCRYPTED
        {
            self.flags | abi::KEY_READABLE
        } else {
            self.flags
        }
    }

    fn read_key(
        &self,
        owner: usize,
        generation: u32,
        output: &mut abi::Record,
    ) -> Result<(), &'static str> {
        if self.generation != generation || self.flags_for(owner) & abi::KEY_READABLE == 0 {
            return Err("pairing key unavailable");
        }
        let (kind, key) = self.key.as_ref().ok_or("no new pairing key")?;
        output[abi::KEY_TYPE] = *kind;
        output[abi::KEY].copy_from_slice(&key[..]);
        Ok(())
    }

    fn clear_key(&mut self) {
        self.key = None;
        self.pairing_owner = 0;
    }
}

pub(super) struct State {
    adapter: Address,
    flags: u32,
    error: u32,
    peers: Vec<Peer>,
    links: Vec<PeerState>,
    pub request: Option<Request>,
    selected: Option<Address>,
    generation: u32,
    link_generation: u32,
}

fn next_generation(value: u32) -> u32 {
    value.wrapping_add(1).max(1)
}

fn write_name(output: &mut abi::Record, name: Option<Name>) {
    if let Some(name) = name {
        let bytes = name.as_bytes();
        output[abi::NAME_LEN] = bytes.len() as u8;
        output[abi::NAME][..bytes.len()].copy_from_slice(bytes);
    }
}

impl State {
    fn new() -> Self {
        Self {
            adapter: [0; 6],
            flags: 0,
            error: 0,
            peers: Vec::new(),
            links: Vec::new(),
            request: None,
            selected: None,
            generation: 0,
            link_generation: 1,
        }
    }

    fn changed(&mut self) {
        self.link_generation = next_generation(self.link_generation);
    }

    pub fn ready(&mut self, adapter: Address) {
        self.adapter = adapter;
        self.flags = abi::READY;
        self.error = 0;
        self.changed();
    }

    fn live(&self) -> impl Iterator<Item = &PeerState> {
        self.links.iter().filter(|p| p.flags & abi::CONNECTED != 0)
    }

    fn peer(&self, address: Address) -> Option<&PeerState> {
        self.links.iter().find(|p| p.address == address)
    }

    fn peer_mut(&mut self, address: Address) -> Option<&mut PeerState> {
        self.links.iter_mut().find(|p| p.address == address)
    }

    pub fn name(&self, address: Address) -> Option<Name> {
        self.peer(address)?.name
    }

    pub fn set_key(&mut self, address: Address, key: [u8; 16], kind: u8) {
        if let Some(peer) = self.peer_mut(address)
            && peer.pairing_owner != 0
        {
            peer.key = Some((kind, Zeroizing::new(key)));
            self.changed();
        }
    }

    pub fn connected(&mut self, address: Address) {
        if let Some(peer) = self.peer_mut(address) {
            peer.flags = abi::CONNECTED | abi::ENCRYPTED;
            self.finish_peer(address, 0);
        }
    }

    pub fn finish_scan(&mut self, error: u32) {
        if self.flags & abi::SCANNING != 0 {
            self.flags &= !(abi::SCANNING | abi::BUSY);
            self.error = error;
        }
    }

    fn finish_peer(&mut self, address: Address, error: u32) {
        if self.selected == Some(address) {
            self.flags &= !abi::BUSY;
            self.error = error;
        }
        self.changed();
    }

    pub fn profile_ready(&mut self, address: Address, ready: bool) {
        if let Some(peer) = self.peer_mut(address)
            && (peer.flags & abi::PROFILE_READY != 0) != ready
        {
            if ready {
                peer.flags |= abi::PROFILE_READY;
            } else {
                peer.flags &= !abi::PROFILE_READY;
            }
            self.changed();
        }
    }

    pub fn operation_failed(&mut self, address: Address, error: u32) {
        if let Some(peer) = self.peer_mut(address) {
            peer.flags &= !abi::BUSY;
            peer.error = error;
            peer.clear_key();
            self.finish_peer(address, error);
        }
    }

    pub fn ended(&mut self, address: Address, error: u32) {
        if matches!(self.request, Some(Request::Disconnect(addr)) if addr == address) {
            self.request = None;
        }
        if let Some(peer) = self.peer_mut(address) {
            peer.flags = 0;
            peer.error = error;
            peer.clear_key();
            self.finish_peer(address, error);
        }
    }

    pub fn lost_controller(&mut self) {
        self.request = None;
        self.adapter = [0; 6];
        self.selected = None;
        self.peers.clear();
        self.flags = 0;
        self.error = 0x100;
        for peer in &mut self.links {
            if peer.flags != 0 {
                peer.flags = 0;
                peer.error = self.error;
            }
            peer.clear_key();
        }
        self.changed();
    }

    pub fn update_peer(&mut self, peer: Peer) {
        if let Some(link) = self.peer_mut(peer.address)
            && peer.name.is_some()
            && link.name != peer.name
        {
            link.name = peer.name;
            self.changed();
        }
        if let Some(p) = self.peers.iter_mut().find(|p| p.address == peer.address) {
            *p = peer;
        } else if self.peers.len() < 64 {
            self.peers.push(peer);
        }
    }

    fn target(&self, cmd: u32, address: Address) -> Result<Option<Address>, &'static str> {
        if self.flags & (abi::READY | abi::BUSY) != abi::READY {
            return Err("Bluetooth adapter busy or offline");
        }
        if cmd == abi::SCAN_START {
            return Ok(None);
        }
        let address = if cmd == abi::DISCONNECT {
            let mut live = self.live();
            let only = live.next().ok_or("no Bluetooth link")?;
            if live.next().is_some() {
                return Err("specify a Bluetooth peer");
            }
            only.address
        } else {
            address
        };
        if address == [0; 6] || address == [255; 6] || address == self.adapter {
            return Err("invalid Bluetooth peer address");
        }
        let flags = self.peer(address).map_or(0, |peer| peer.flags);
        if flags & abi::BUSY != 0
            || (flags & abi::CONNECTED != 0)
                != matches!(cmd, abi::DISCONNECT | abi::DISCONNECT_PEER)
        {
            return Err("invalid Bluetooth link state");
        }
        if self.peer(address).is_none()
            && self.links.len() == MAX_PEER_RECORDS
            && self.links.iter().all(|p| p.flags != 0)
        {
            return Err("Bluetooth peer table full");
        }
        Ok(Some(address))
    }

    // Called under the same lock as target(), after the reply buffer was checked.
    fn begin(&mut self, cmd: u32, target: Option<Address>, owner: usize, generation: u32) {
        self.selected = target;
        self.generation = generation;
        self.error = 0;
        self.flags |= abi::BUSY;
        let Some(address) = target else {
            self.flags |= abi::SCANNING;
            self.peers.clear();
            return;
        };
        if self.peer(address).is_none() {
            if self.links.len() == MAX_PEER_RECORDS {
                let inactive = self.links.iter().position(|p| p.flags == 0).unwrap();
                self.links.remove(inactive);
            }
            self.links.push(PeerState {
                address,
                name: None,
                flags: 0,
                error: 0,
                generation,
                pairing_owner: 0,
                key: None,
            });
        }
        let name = self
            .peers
            .iter()
            .find(|p| p.address == address)
            .and_then(|p| p.name);
        let peer = self.peer_mut(address).unwrap();
        peer.name = name.or(peer.name);
        peer.generation = generation;
        peer.error = 0;
        peer.flags |= abi::BUSY;
        peer.clear_key();
        if cmd == abi::PAIR {
            peer.pairing_owner = owner;
        }
        self.changed();
    }

    fn write_peer(&self, peer: &PeerState, owner: usize, output: &mut abi::Record) {
        output[abi::PEER].copy_from_slice(&peer.address);
        output[abi::ADAPTER].copy_from_slice(&self.adapter);
        let flags = (self.flags & abi::READY) | peer.flags_for(owner);
        output[abi::FLAGS].copy_from_slice(&flags.to_le_bytes());
        output[abi::LAST_ERROR].copy_from_slice(&peer.error.to_le_bytes());
        write_name(output, peer.name);
    }
}

pub(super) struct Shared {
    pub state: Mutex<State>,
    pub wake: Arc<Waker>,
    pub queued: AtomicBool,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State::new()),
            queued: AtomicBool::new(false),
            wake: Arc::new(Waker::new_interruptible("bluetooth")),
        }
    }
}

pub(super) struct Endpoint {
    pub shared: Arc<Shared>,
    pub id: usize,
}

impl Device for Endpoint {
    fn open(self: Arc<Self>) -> Result<Arc<dyn Device>, &'static str> {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        let id = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| "Bluetooth endpoint IDs exhausted")?;
        Ok(Arc::new(Self {
            shared: self.shared.clone(),
            id,
        }))
    }
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }
    fn name(&self) -> &'static str {
        "bluetooth"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock();
        let mut changed = false;
        for peer in &mut state.links {
            if self.id != 0 && peer.pairing_owner == self.id {
                peer.clear_key();
                changed = true;
            }
        }
        if changed {
            state.changed();
        }
    }
}
impl Selectable for Endpoint {
    fn wait_until_ready(
        &self,
        _: ReadyInterest,
        _: &mut crate::arch::Trapframe,
        _: Option<u64>,
        _: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::TimedOut
    }
}
impl MemoryMappingOps for Endpoint {
    fn get_mapping_info(&self, _: usize, _: usize) -> Result<MemoryMappingInfo, &'static str> {
        Err("Bluetooth has no mapping")
    }
    fn supports_mmap(&self) -> bool {
        false
    }
}

impl ControlOps for Endpoint {
    fn control(&self, cmd: u32, arg: usize) -> Result<i32, &'static str> {
        let task = crate::task::mytask().ok_or("no Bluetooth caller")?;
        if self.id == 0 || !permits_management(&task, Management::PhysicalSeat) {
            return Err("Bluetooth management permission denied");
        }
        let mut input = Zeroizing::new([0u8; abi::RECORD_SIZE]);
        copy_from_user(&task, arg, &mut input[..]).map_err(|_| "invalid Bluetooth input")?;
        let mut output = Zeroizing::new([0u8; abi::RECORD_SIZE]);
        let addr: Address = input[abi::PEER].try_into().unwrap();
        let generation = u32::from_le_bytes(input[abi::GENERATION].try_into().unwrap());
        let mut state = self.shared.state.lock();
        let mut reply_generation = state.generation;
        match cmd {
            abi::STATUS => {
                let peer = state
                    .selected
                    .and_then(|addr| state.peer(addr))
                    .filter(|p| p.flags != 0)
                    .or_else(|| state.live().next());
                let mut flags = state.flags;
                if let Some(peer) = peer {
                    flags |= peer.flags & !abi::BUSY;
                    if peer.generation == state.generation {
                        flags |= peer.flags_for(self.id) & abi::KEY_READABLE;
                    }
                    output[abi::PEER].copy_from_slice(&peer.address);
                }
                output[abi::FLAGS].copy_from_slice(&flags.to_le_bytes());
                output[abi::ADAPTER].copy_from_slice(&state.adapter);
                output[abi::COUNT].copy_from_slice(&(state.peers.len() as u16).to_le_bytes());
                output[abi::LAST_ERROR].copy_from_slice(&state.error.to_le_bytes());
            }
            abi::LINKS => {
                reply_generation = state.link_generation;
                output[abi::COUNT].copy_from_slice(&(state.live().count() as u16).to_le_bytes());
            }
            abi::LINK_RESULT => {
                if state.link_generation != generation {
                    return Err("stale Bluetooth link snapshot");
                }
                let index = u16::from_le_bytes(input[abi::INDEX].try_into().unwrap());
                let peer = state
                    .live()
                    .nth(usize::from(index))
                    .ok_or("unknown Bluetooth link")?;
                state.write_peer(peer, self.id, &mut output);
                reply_generation = state.link_generation;
            }
            abi::PEER_STATUS => {
                let peer = state.peer(addr).ok_or("unknown Bluetooth peer")?;
                state.write_peer(peer, self.id, &mut output);
                reply_generation = peer.generation;
            }
            abi::SCAN_RESULT => {
                if state.generation != generation || state.flags & abi::SCANNING != 0 {
                    return Err("stale inquiry");
                }
                let index = u16::from_le_bytes(input[abi::INDEX].try_into().unwrap());
                let peer = state
                    .peers
                    .get(usize::from(index))
                    .ok_or("unknown Bluetooth peer")?;
                output[abi::PEER].copy_from_slice(&peer.address);
                write_name(&mut output, peer.name);
            }
            abi::READ_KEY => {
                let peer = state.peer(addr).ok_or("unknown Bluetooth peer")?;
                peer.read_key(self.id, generation, &mut output)?;
                reply_generation = generation;
            }
            abi::SCAN_START | abi::PAIR | abi::CONNECT | abi::DISCONNECT | abi::DISCONNECT_PEER => {
                let target = state.target(cmd, addr)?;
                let request = match (cmd, target) {
                    (abi::SCAN_START, None) => Request::Scan,
                    (abi::PAIR, Some(addr)) => Request::Pair(addr),
                    (abi::CONNECT, Some(addr)) => {
                        let flags = u32::from_le_bytes(input[abi::FLAGS].try_into().unwrap());
                        let key = (flags & abi::KEY_SUPPLIED != 0)
                            .then(|| Zeroizing::new(input[abi::KEY].try_into().unwrap()));
                        Request::Connect(addr, key)
                    }
                    (_, Some(addr)) => Request::Disconnect(addr),
                    _ => return Err("missing Bluetooth peer"),
                };
                let next = next_generation(state.generation);
                output[abi::GENERATION].copy_from_slice(&next.to_le_bytes());
                if let Some(addr) = target {
                    output[abi::PEER].copy_from_slice(&addr);
                }
                copy_to_user(&task, arg, &output[..]).map_err(|_| "invalid Bluetooth output")?;
                state.begin(cmd, target, self.id, next);
                state.request = Some(request);
                self.shared.queued.store(true, Ordering::Release);
                drop(state);
                self.shared.wake.wake_one();
                return Ok(0);
            }
            _ => return Err("unsupported Bluetooth command"),
        }
        output[abi::GENERATION].copy_from_slice(&reply_generation.to_le_bytes());
        copy_to_user(&task, arg, &output[..]).map_err(|_| "invalid Bluetooth output")?;
        if cmd == abi::READ_KEY {
            state.peer_mut(addr).unwrap().clear_key();
            state.changed();
        }
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn two_peers_keep_separate_results_and_bond_exports_during_scan_and_loss() {
        let mut state = State::new();
        state.ready([9; 6]);
        let (left, right) = ([1; 6], [2; 6]);
        for (addr, owner, generation) in [(left, 11, 1), (right, 22, 2)] {
            let target = state.target(abi::PAIR, addr).unwrap();
            state.begin(abi::PAIR, target, owner, generation);
            state.set_key(addr, [generation as u8; 16], 4);
            state.connected(addr);
            let p = state.peer(addr).unwrap();
            assert_eq!(p.flags, abi::CONNECTED | abi::ENCRYPTED);
            assert_ne!(p.flags_for(owner) & abi::KEY_READABLE, 0);
            assert_eq!(state.flags, abi::READY);
            state.profile_ready(addr, true);
        }
        assert_eq!(state.live().count(), 2);
        assert!(state.target(abi::DISCONNECT, [0; 6]).is_err());
        assert_eq!(state.target(abi::DISCONNECT_PEER, right), Ok(Some(right)));
        let first = state.peer(left).unwrap();
        let mut record = [0; abi::RECORD_SIZE];
        assert!(first.read_key(22, 1, &mut record).is_err());
        assert!(first.read_key(11, 2, &mut record).is_err());
        assert_eq!(record[abi::KEY], [0; 16]);
        first.read_key(11, 1, &mut record).unwrap();
        assert_eq!(record[abi::KEY], [1; 16]);

        let target = state.target(abi::SCAN_START, [0; 6]).unwrap();
        state.begin(abi::SCAN_START, target, 11, 3);
        let snapshot = state.link_generation;
        state.ended(left, 8);
        assert_ne!(snapshot, state.link_generation);
        assert_eq!(state.flags, abi::READY | abi::BUSY | abi::SCANNING);
        assert_eq!(state.error, 0);
        assert_eq!(state.peer(left).unwrap().error, 8);
        assert!(
            state
                .peer(left)
                .unwrap()
                .read_key(11, 1, &mut record)
                .is_err()
        );
        state.finish_scan(0);
        assert_eq!(state.target(abi::DISCONNECT, [0; 6]), Ok(Some(right)));
        let second = state.peer(right).unwrap();
        assert_eq!(
            second.flags,
            abi::CONNECTED | abi::ENCRYPTED | abi::PROFILE_READY
        );
        second.read_key(22, 2, &mut record).unwrap();
        assert_eq!(record[abi::KEY], [2; 16]);
    }
}
