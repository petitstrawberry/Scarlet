//! Per-controller management, independent of UART and USB. Pairing key export
//! belongs to the open endpoint that initiated that pairing operation.
use super::classic::{Address, Peer};
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
    Disconnect,
}

pub(super) struct State {
    pub adapter: Address,
    pub peer: Address,
    pub flags: u32,
    pub error: u32,
    pub peers: Vec<Peer>,
    pub request: Option<Request>,
    generation: u32,
    owner: usize,
    key: Option<(u8, Zeroizing<[u8; 16]>)>,
}

impl State {
    fn new() -> Self {
        Self {
            adapter: [0; 6],
            peer: [0; 6],
            flags: 0,
            error: 0,
            peers: Vec::new(),
            request: None,
            generation: 0,
            owner: 0,
            key: None,
        }
    }
    pub fn finish(&mut self, error: u32) {
        self.flags &= !(abi::BUSY | abi::SCANNING);
        self.error = error;
        if error != 0 {
            self.clear_key();
        }
    }
    pub fn clear_key(&mut self) {
        self.key = None;
        self.flags &= !abi::KEY_READABLE;
    }
    pub fn set_key(&mut self, peer: Address, key: [u8; 16], kind: u8) {
        if self.owner != 0 && self.peer == peer {
            self.key = Some((kind, Zeroizing::new(key)));
            self.flags |= abi::KEY_READABLE;
        }
    }
    pub fn lost_controller(&mut self) {
        self.request = None;
        self.adapter = [0; 6];
        self.peer = [0; 6];
        self.peers.clear();
        self.flags = 0;
        self.finish(0x100);
    }
    pub fn update_peer(&mut self, peer: Peer) {
        if let Some(p) = self.peers.iter_mut().find(|p| p.address == peer.address) {
            *p = peer;
        } else if self.peers.len() < 64 {
            self.peers.push(peer);
        }
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
        if self.id != 0 && state.owner == self.id {
            state.owner = 0;
            state.clear_key();
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
        match cmd {
            abi::STATUS => {
                let mut flags = state.flags;
                if state.owner != self.id {
                    flags &= !abi::KEY_READABLE;
                }
                output[abi::FLAGS].copy_from_slice(&flags.to_le_bytes());
                output[abi::ADAPTER].copy_from_slice(&state.adapter);
                output[abi::PEER].copy_from_slice(&state.peer);
                output[abi::COUNT].copy_from_slice(&(state.peers.len() as u16).to_le_bytes());
                output[abi::LAST_ERROR].copy_from_slice(&state.error.to_le_bytes());
            }
            abi::SCAN_RESULT => {
                if state.generation != generation || state.flags & abi::SCANNING != 0 {
                    return Err("stale inquiry");
                }
                let index = u16::from_le_bytes(input[abi::INDEX].try_into().unwrap());
                let peer = state
                    .peers
                    .get(index as usize)
                    .ok_or("unknown Bluetooth peer")?;
                output[abi::PEER].copy_from_slice(&peer.address);
                if let Some(name) = peer.name {
                    let bytes = name.as_bytes();
                    output[abi::NAME_LEN] = bytes.len() as u8;
                    output[abi::NAME][..bytes.len()].copy_from_slice(bytes);
                }
            }
            abi::READ_KEY => {
                if state.owner != self.id
                    || state.generation != generation
                    || state.peer != addr
                    || state.flags & (abi::BUSY | abi::ENCRYPTED) != abi::ENCRYPTED
                {
                    return Err("pairing key unavailable");
                }
                let (kind, key) = state.key.as_ref().ok_or("no new pairing key")?;
                output[abi::KEY_TYPE] = *kind;
                output[abi::KEY].copy_from_slice(&key[..]);
            }
            abi::SCAN_START | abi::PAIR | abi::CONNECT | abi::DISCONNECT => {
                if state.flags & (abi::READY | abi::BUSY) != abi::READY {
                    return Err("Bluetooth adapter busy or offline");
                }
                let connected = state.flags & abi::CONNECTED != 0;
                if (cmd == abi::DISCONNECT) != connected {
                    return Err("invalid Bluetooth link state");
                }
                if matches!(cmd, abi::PAIR | abi::CONNECT)
                    && (addr == [0; 6] || addr == [255; 6] || addr == state.adapter)
                {
                    return Err("invalid Bluetooth peer address");
                }
                let request = match cmd {
                    abi::PAIR => Request::Pair(addr),
                    abi::CONNECT => {
                        let flags = u32::from_le_bytes(input[abi::FLAGS].try_into().unwrap());
                        let key = (flags & abi::KEY_SUPPLIED != 0)
                            .then(|| Zeroizing::new(input[abi::KEY].try_into().unwrap()));
                        Request::Connect(addr, key)
                    }
                    abi::SCAN_START => Request::Scan,
                    _ => Request::Disconnect,
                };
                // Validate the user's reply buffer before committing the operation.
                let next = state.generation.wrapping_add(1).max(1);
                output[abi::GENERATION].copy_from_slice(&next.to_le_bytes());
                copy_to_user(&task, arg, &output[..]).map_err(|_| "invalid Bluetooth output")?;
                state.clear_key();
                state.generation = next;
                state.owner = if cmd == abi::PAIR { self.id } else { 0 };
                state.error = 0;
                state.flags |= abi::BUSY;
                if cmd == abi::SCAN_START {
                    state.flags |= abi::SCANNING;
                    state.peers.clear();
                } else if cmd != abi::DISCONNECT {
                    state.peer = addr;
                }
                state.request = Some(request);
                self.shared.queued.store(true, Ordering::Release);
                drop(state);
                self.shared.wake.wake_one();
                return Ok(0);
            }
            _ => return Err("unsupported Bluetooth command"),
        }
        output[abi::GENERATION].copy_from_slice(&state.generation.to_le_bytes());
        copy_to_user(&task, arg, &output[..]).map_err(|_| "invalid Bluetooth output")?;
        if cmd == abi::READ_KEY {
            state.clear_key();
        }
        Ok(0)
    }
}
