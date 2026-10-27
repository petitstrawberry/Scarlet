//! Execute production ARP admission/publication with deterministic race schedules.
#![allow(dead_code)]
extern crate alloc;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, OnceLock, RwLock,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
};
#[derive(Debug)]
struct IrqSpinLock<T>(Mutex<T>);
impl<T> IrqSpinLock<T> {
    fn new(t: T) -> Self {
        Self(Mutex::new(t))
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, T> {
        self.0.lock().unwrap()
    }
}
#[derive(Debug)]
struct IrqRwSpinLock<T>(RwLock<T>);
impl<T> IrqRwSpinLock<T> {
    fn new(t: T) -> Self {
        Self(RwLock::new(t))
    }
    fn read(&self) -> std::sync::RwLockReadGuard<'_, T> {
        self.0.read().unwrap()
    }
    fn write(&self) -> std::sync::RwLockWriteGuard<'_, T> {
        self.0.write().unwrap()
    }
}
mod socket {
    #[derive(Debug)]
    pub enum SocketError {
        InvalidPacket,
        Other(String),
    }
}
mod packet { /* PRODUCTION_PACKET */
}
use packet::PacketBuffer;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ipv4Address([u8; 4]);
mod network {
    pub mod ethernet {
        pub mod ether_type {
            pub const IPV4: u16 = 0x0800;
        }
    }
}
struct LayerContext(BTreeMap<String, Vec<u8>>);
impl LayerContext {
    fn new() -> Self {
        Self(BTreeMap::new())
    }
    fn set(&mut self, k: &str, v: &[u8]) {
        self.0.insert(k.into(), v.into());
    }
}
struct FakeEther {
    sent: Mutex<Vec<PacketBuffer>>,
    retire_on_send: AtomicBool,
}
impl FakeEther {
    fn send_owned(&self, p: PacketBuffer, c: &LayerContext, _: &[()]) -> Result<(), ()> {
        assert_eq!(c.0["eth_dst_mac"], MAC);
        assert_eq!(c.0["eth_type"], 0x0800u16.to_be_bytes());
        if self.retire_on_send.swap(false, Ordering::SeqCst) {
            MANAGER.epoch.fetch_add(1, Ordering::SeqCst);
        }
        if p.link_token()
            .is_some_and(|t| t != (42, MANAGER.epoch.load(Ordering::SeqCst)))
        {
            return Err(());
        }
        self.sent.lock().unwrap().push(p);
        Ok(())
    }
}
struct FakeManager {
    epoch: AtomicU64,
}
static MANAGER: FakeManager = FakeManager {
    epoch: AtomicU64::new(1),
};
fn ethernet() -> &'static FakeEther {
    static ETH: OnceLock<FakeEther> = OnceLock::new();
    ETH.get_or_init(|| FakeEther {
        sent: Mutex::new(Vec::new()),
        retire_on_send: AtomicBool::new(false),
    })
}
fn get_network_manager() -> &'static FakeManager {
    &MANAGER
}
struct FakeInterface;
impl FakeInterface {
    fn packet_token(&self) -> Result<Option<(u64, u64)>, ()> {
        Ok(Some((42, MANAGER.epoch.load(Ordering::SeqCst))))
    }
}
impl FakeManager {
    fn get_interface(&self, _: &str) -> Option<FakeInterface> {
        Some(FakeInterface)
    }
    fn get_layer(&self, _: &str) -> Option<&'static FakeEther> {
        Some(ethernet())
    }
}
/* PRODUCTION_ARP_TYPES */
struct ArpLayer {
    cache: IrqRwSpinLock<BTreeMap<ArpCacheKey, ArpCacheEntry>>,
    pending: IrqRwSpinLock<BTreeMap<ArpPendingKey, ArpPendingEntry>>,
    packet_counter: AtomicU32,
}
impl ArpLayer {
    fn fixture() -> Self {
        MANAGER.epoch.store(1, Ordering::SeqCst);
        ethernet().sent.lock().unwrap().clear();
        ethernet().retire_on_send.store(false, Ordering::SeqCst);
        Self {
            cache: IrqRwSpinLock::new(BTreeMap::new()),
            pending: IrqRwSpinLock::new(BTreeMap::new()),
            packet_counter: AtomicU32::new(0),
        }
    }
    fn get_local_mac_for_interface(&self, _: &str) -> Option<[u8; 6]> {
        Some([9; 6])
    }
    /* PRODUCTION_ARP_METHODS */
}
const IP: Ipv4Address = Ipv4Address([10, 0, 2, 2]);
const MAC: [u8; 6] = [2, 0, 0, 0, 0, 1];
fn payload(n: u8) -> PacketBuffer {
    let mut p = PacketBuffer::from_vec(vec![n; 40]);
    p.set_link_token(Some((42, 1)));
    p
}
#[test]
fn reply_before_late_enqueue_sends_instead_of_orphaning_packet() {
    let a = ArpLayer::fixture();
    a.resolve_pending("wifi", IP, MAC);
    a.queue_owned_packet_on_interface("wifi", IP, payload(7));
    assert_eq!(a.pending_packet_count("wifi"), 0);
    let s = ethernet().sent.lock().unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].as_slice(), [7; 40]);
    assert_eq!(s[0].link_token(), Some((42, 1)));
}
#[test]
fn enqueue_before_reply_is_returned_to_the_reply_flush() {
    let a = ArpLayer::fixture();
    a.queue_owned_packet_on_interface("wifi", IP, payload(8));
    assert_eq!(a.pending_packet_count("wifi"), 1);
    let q = a.resolve_pending("wifi", IP, MAC).unwrap();
    assert_eq!(q.packet_queue.lock()[0].as_slice(), [8; 40]);
    assert_eq!(a.pending_packet_count("wifi"), 0);
    assert_eq!(a.lookup_on_interface("wifi", IP), Some(MAC));
}
#[test]
fn concurrent_resolution_and_admission_never_strand_waiters() {
    for _ in 0..32 {
        let a = Arc::new(ArpLayer::fixture());
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let b = barrier.clone();
        let sender = a.clone();
        let thread = std::thread::spawn(move || {
            b.wait();
            for n in 0..128 {
                sender.queue_owned_packet_on_interface("wifi", IP, payload(n));
            }
        });
        barrier.wait();
        let resolved = a.resolve_pending("wifi", IP, MAC);
        thread.join().unwrap();
        let queued = resolved.map_or(0, |p| p.packet_queue.lock().len());
        assert_eq!(queued + ethernet().sent.lock().unwrap().len(), 128);
        assert_eq!(a.pending_packet_count("wifi"), 0);
    }
}
#[test]
fn retirement_rejects_late_old_tokens_without_recreating_waiters() {
    let a = ArpLayer::fixture();
    a.queue_owned_packet_on_interface("wifi", IP, payload(1));
    MANAGER.epoch.store(2, Ordering::SeqCst);
    a.clear_interface("wifi");
    a.queue_owned_packet_on_interface("wifi", IP, payload(2));
    assert_eq!(a.pending_packet_count("wifi"), 0);
    assert!(ethernet().sent.lock().unwrap().is_empty());
}
#[test]
fn retirement_after_lookup_still_receives_original_packet_token() {
    let a = ArpLayer::fixture();
    a.resolve_pending("wifi", IP, MAC);
    ethernet().retire_on_send.store(true, Ordering::SeqCst);
    a.queue_owned_packet_on_interface("wifi", IP, payload(3));
    assert!(ethernet().sent.lock().unwrap().is_empty());
    assert_eq!(a.pending_packet_count("wifi"), 0);
}
