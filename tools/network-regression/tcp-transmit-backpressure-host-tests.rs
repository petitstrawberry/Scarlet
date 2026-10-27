//! Production TCP admission and wake-registration methods with a finite NIC.
extern crate alloc;
use alloc::{
    collections::VecDeque,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};
struct Lock<T>(Mutex<T>);
impl<T> Lock<T> {
    fn new(value: T) -> Self {
        Self(Mutex::new(value))
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, T> {
        self.0.lock().unwrap()
    }
}
type IrqSpinLock<T> = Lock<T>;
type Lazy<T> = LazyLock<T>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SocketError {
    WouldBlock,
    TransmitBusy,
    NotConnected,
    NoRoute,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum TcpState {
    Established,
    Closed,
}
fn tcp_send_side_open(state: TcpState) -> bool {
    state == TcpState::Established
}
#[derive(Clone, Copy)]
struct Ipv4Address;
struct PeerWindow {
    bytes: u32,
}
struct TcpHeader {
    seq_number: u32,
    ack_number: u32,
}
impl TcpHeader {
    fn new(_: u16, _: u16) -> Self {
        Self {
            seq_number: 0,
            ack_number: 0,
        }
    }
    fn set_flags(&mut self, _: u8) {}
}
mod tcp_flags {
    pub const ACK: u8 = 16;
    pub const PSH: u8 = 8;
}
const MAX_SEND_BUFFER_SIZE: usize = 1024;
const MAX_RECV_BUFFER_SIZE: usize = 1024;
const MAX_UNACKED_SEGMENTS: usize = 128;
/* PRODUCTION_TX_NOTIFY */
struct TcpSocket {
    state: TcpState,
    send_buffer: Lock<VecDeque<u8>>,
    recv_buffer: Lock<VecDeque<u8>>,
    unacked_segments: Lock<Vec<usize>>,
    loopback_peer: Lock<Weak<TcpSocket>>,
    remote_ip: Lock<Option<Ipv4Address>>,
    remote_port: AtomicU16,
    local_port: AtomicU16,
    send_seq: AtomicU32,
    recv_ack: AtomicU32,
    transmit_lock: Lock<()>,
    driver_tx_blocked: AtomicBool,
    self_weak: Weak<TcpSocket>,
    budget: AtomicUsize,
    wakes: AtomicUsize,
    fail: AtomicBool,
    free_before_registration: AtomicBool,
    payload: Lock<Vec<u8>>,
}
impl TcpSocket {
    fn new(budget: usize) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            state: TcpState::Established,
            send_buffer: Lock::new(VecDeque::new()),
            recv_buffer: Lock::new(VecDeque::new()),
            unacked_segments: Lock::new(Vec::new()),
            loopback_peer: Lock::new(Weak::new()),
            remote_ip: Lock::new(Some(Ipv4Address)),
            remote_port: AtomicU16::new(80),
            local_port: AtomicU16::new(1000),
            send_seq: AtomicU32::new(0),
            recv_ack: AtomicU32::new(0),
            transmit_lock: Lock::new(()),
            driver_tx_blocked: AtomicBool::new(false),
            self_weak: weak.clone(),
            budget: AtomicUsize::new(budget),
            wakes: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
            free_before_registration: AtomicBool::new(false),
            payload: Lock::new(Vec::new()),
        })
    }
    fn get_state(&self) -> TcpState {
        self.state
    }
    fn effective_peer_mss(&self) -> usize {
        8
    }
    fn peer_send_credit(&self) -> usize {
        1024
    }
    fn deliver_loopback_data(&self, _: &[u8]) -> Result<Option<usize>, SocketError> {
        Ok(None)
    }
    fn schedule_persist_timer(&self) {}
    fn notify_write_waiters_now(&self) {
        self.wakes.fetch_add(1, Ordering::Relaxed);
    }
    fn send_segment(
        &self,
        _: Ipv4Address,
        _: TcpHeader,
        bytes: &[u8],
        _: bool,
        _: bool,
    ) -> Result<(), SocketError> {
        if self.fail.load(Ordering::Relaxed) {
            return Err(SocketError::NoRoute);
        }
        if self.budget.load(Ordering::Relaxed) == 0 {
            if self.free_before_registration.swap(false, Ordering::Relaxed) {
                notify_transmit_available();
            }
            return Err(SocketError::TransmitBusy);
        }
        self.budget.fetch_sub(1, Ordering::Relaxed);
        self.send_seq
            .fetch_add(bytes.len() as u32, Ordering::Relaxed);
        self.payload.lock().extend_from_slice(bytes);
        self.unacked_segments.lock().push(bytes.len());
        Ok(())
    }
    /* PRODUCTION_TX_METHODS */
}
#[test]
fn partial_write_retains_only_admitted_bytes_and_waits_for_capacity() {
    let socket = TcpSocket::new(2);
    let bytes = b"12345678abcdefghABCDEFGH";
    assert_eq!(socket.send_data(bytes), Ok(16));
    assert_eq!(socket.send_seq.load(Ordering::Relaxed), 16);
    assert_eq!(socket.send_buffer.lock().len(), 16);
    assert_eq!(&*socket.payload.lock(), &bytes[..16]);
    assert_eq!(socket.writable_capacity(), 0);
    assert_eq!(socket.send_data(&bytes[16..]), Err(SocketError::WouldBlock));
    socket.budget.store(1, Ordering::Relaxed);
    notify_transmit_available();
    assert_eq!(socket.wakes.load(Ordering::Relaxed), 1);
    assert!(socket.writable_capacity() > 0);
    assert_eq!(socket.send_data(&bytes[16..]), Ok(8));
    assert_eq!(&*socket.payload.lock(), bytes);
    assert_eq!(socket.send_buffer.lock().len(), 24);
}
#[test]
fn rejected_first_segment_never_advances_sequence_or_owns_application_bytes() {
    let socket = TcpSocket::new(0);
    assert_eq!(
        socket.send_data(b"not yet accepted"),
        Err(SocketError::WouldBlock)
    );
    assert_eq!(socket.send_seq.load(Ordering::Relaxed), 0);
    assert!(socket.send_buffer.lock().is_empty());
    assert!(socket.unacked_segments.lock().is_empty());
    notify_transmit_available();
}
#[test]
fn capacity_return_before_registration_is_not_lost() {
    let socket = TcpSocket::new(0);
    socket
        .free_before_registration
        .store(true, Ordering::Relaxed);
    assert_eq!(socket.send_data(b"12345678"), Err(SocketError::WouldBlock));
    assert!(!socket.driver_tx_blocked.load(Ordering::Acquire));
    assert_eq!(socket.wakes.load(Ordering::Relaxed), 1);
    assert!(socket.writable_capacity() > 0);
}
#[test]
fn one_registration_per_socket_and_dead_writers_do_not_accumulate() {
    let dead = TcpSocket::new(0);
    dead.register_driver_backpressure(TX_READY_SEQUENCE.load(Ordering::Acquire));
    drop(dead);
    let one = TcpSocket::new(0);
    let two = TcpSocket::new(0);
    one.register_driver_backpressure(TX_READY_SEQUENCE.load(Ordering::Acquire));
    one.register_driver_backpressure(TX_READY_SEQUENCE.load(Ordering::Acquire));
    two.register_driver_backpressure(TX_READY_SEQUENCE.load(Ordering::Acquire));
    assert_eq!(TX_WAITERS.lock().len(), 2);
    notify_transmit_available();
    assert_eq!(one.wakes.load(Ordering::Relaxed), 1);
    assert_eq!(two.wakes.load(Ordering::Relaxed), 1);
    assert!(TX_WAITERS.lock().is_empty());
}
#[test]
fn permanent_route_failure_is_not_reclassified_as_backpressure() {
    let socket = TcpSocket::new(1);
    socket.fail.store(true, Ordering::Relaxed);
    assert_eq!(socket.send_data(b"12345678"), Err(SocketError::NoRoute));
    assert!(!socket.driver_tx_blocked.load(Ordering::Acquire));
    assert!(socket.send_buffer.lock().is_empty());
}
