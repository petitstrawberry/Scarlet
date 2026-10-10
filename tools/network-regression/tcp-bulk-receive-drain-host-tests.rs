//! Extracted production receive methods; host locks, state, profiling and task
//! waits model their boundaries. This is a semantics test, not a speed test.
#![allow(dead_code)]
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, MutexGuard};
use std::sync::atomic::{AtomicBool, Ordering};

thread_local! {
    static HELD: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    static EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static REGISTER: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    static WAIT: RefCell<VecDeque<(WaitResult, Box<dyn FnOnce()>)>> = RefCell::new(VecDeque::new());
}
fn event(value: impl Into<String>) { EVENTS.with(|events| events.borrow_mut().push(value.into())); }
fn events() -> Vec<String> { EVENTS.with(|events| events.borrow().clone()) }
fn reset() {
    assert!(HELD.with(|held| held.borrow().is_empty()));
    EVENTS.with(|events| events.borrow_mut().clear());
    REGISTER.with(|hook| *hook.borrow_mut() = None);
    WAIT.with(|queue| queue.borrow_mut().clear());
}
fn locked(name: &'static str) -> bool { HELD.with(|held| held.borrow().contains(&name)) }
struct Lock<T> { name: &'static str, data: Mutex<T> }
struct Guard<'a, T> { name: &'static str, data: MutexGuard<'a, T> }
impl<T> Lock<T> {
    fn new(name: &'static str, data: T) -> Self { Self { name, data: Mutex::new(data) } }
    fn lock(&self) -> Guard<'_, T> {
        assert!(!locked(self.name), "recursive modeled lock");
        let data = self.data.lock().unwrap();
        HELD.with(|held| held.borrow_mut().push(self.name));
        event(format!("lock:{}", self.name));
        Guard { name: self.name, data }
    }
}
impl<T> Deref for Guard<'_, T> { type Target=T; fn deref(&self)->&T { &self.data } }
impl<T> DerefMut for Guard<'_, T> { fn deref_mut(&mut self)->&mut T { &mut self.data } }
impl<T> Drop for Guard<'_, T> {
    fn drop(&mut self) {
        HELD.with(|held| {
            let mut held = held.borrow_mut();
            let index = held.iter().position(|name| *name == self.name).unwrap();
            held.remove(index);
        });
        event(format!("unlock:{}", self.name));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TcpState { Established, FinWait1, FinWait2, CloseWait, Closing, LastAck, TimeWait, Closed, Listen, SynSent, SynReceived }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SocketError { WouldBlock, NotConnected, Interrupted }
#[derive(Clone, Copy)]
enum WaitResult { Woken, TimedOut, Interrupted }
mod arch { #[derive(Default)] pub struct Trapframe { pub waits: usize } }
mod sync {
    pub use crate::Waker;
}
mod net_profile {
    pub enum Stage { TcpDrain }
    pub struct Span;
    pub fn begin(_: Stage, bytes: usize, capacity: usize) -> Span {
        assert!(crate::locked("recv_buffer")); assert_eq!(capacity, 0);
        crate::event(format!("profile_begin:{bytes}")); Span
    }
    impl Drop for Span {
        fn drop(&mut self) {
            assert!(crate::locked("recv_buffer")); crate::event("profile_end");
        }
    }
}
pub struct Waker;
impl Waker {
    fn new_interruptible(name: &'static str) -> Self {
        assert_eq!(name, "tcp_recv"); assert!(locked("recv_waker"));
        event("register");
        let hook = REGISTER.with(|slot| slot.borrow_mut().take());
        if let Some(hook) = hook { hook(); }
        Self
    }
    fn wait_with_timeout_result(&self, task: usize, trapframe: &mut arch::Trapframe,
                                timeout: Option<u64>) -> WaitResult {
        assert!(HELD.with(|held| held.borrow().is_empty()));
        assert_eq!(task, 37); assert_eq!(timeout, Some(123)); trapframe.waits += 1;
        event("wait");
        let (result, hook) = WAIT.with(|queue| queue.borrow_mut().pop_front().expect("unplanned wait"));
        hook(); result
    }
}

/* PRODUCTION_STATE_PREDICATES */
/* PRODUCTION_DRAIN_HELPER */

struct TcpSocket {
    recv_buffer: Lock<VecDeque<u8>>,
    recv_waker: Lock<Option<Arc<Waker>>>,
    blocking_mode: AtomicBool,
    state: Cell<TcpState>,
    window_update: Cell<bool>,
}
impl TcpSocket {
    fn new(data: VecDeque<u8>, state: TcpState, blocking: bool, window: bool) -> Arc<Self> {
        Arc::new(Self { recv_buffer: Lock::new("recv_buffer", data),
            recv_waker: Lock::new("recv_waker", None), blocking_mode: AtomicBool::new(blocking),
            state: Cell::new(state), window_update: Cell::new(window) })
    }
    fn get_state(&self) -> TcpState { assert!(!locked("recv_buffer")); self.state.get() }
    fn update_recv_window_after_drain(&self, remaining: usize) -> bool {
        assert!(locked("recv_buffer")); event(format!("window:{remaining}")); self.window_update.get()
    }
    fn send_window_update_ack(&self) {
        assert!(HELD.with(|held| held.borrow().is_empty())); event("ack");
    }
    fn read_timeout_ns(&self) -> Option<u64> { Some(123) }
    /* PRODUCTION_RECEIVE_METHODS */
}

fn invoke(socket: &TcpSocket, output: &mut [u8], blocking_method: bool) -> (Result<usize,SocketError>, usize) {
    let mut trapframe = arch::Trapframe::default();
    let result = if blocking_method { socket.recv_blocking(output,37,&mut trapframe) }
                 else { socket.recv_data(output) };
    (result, trapframe.waits)
}
fn remaining(socket: &TcpSocket) -> Vec<u8> {
    // Inspect the actual deque without adding lock-model events to the trace.
    socket.recv_buffer.data.lock().unwrap().iter().copied().collect()
}
fn check_data(data: VecDeque<u8>, output_len: usize, blocking_method: bool, ack: bool) {
    reset(); let bytes: Vec<u8> = data.iter().copied().collect();
    let capacity = data.capacity();
    let copied = bytes.len().min(output_len);
    let socket = TcpSocket::new(data,TcpState::Established,true,ack);
    let mut output = vec![0xa5;output_len];
    let (result,waits) = invoke(&socket,&mut output,blocking_method);
    assert_eq!(result,Ok(copied)); assert_eq!(waits,0);
    assert_eq!(&output[..copied],&bytes[..copied]);
    assert!(output[copied..].iter().all(|value| *value==0xa5));
    assert_eq!(remaining(&socket),bytes[copied..]);
    assert_eq!(socket.recv_buffer.data.lock().unwrap().capacity(),capacity);
    assert_eq!(events(),vec!["lock:recv_buffer".into(), format!("profile_begin:{copied}"),
        "profile_end".into(),format!("window:{}",bytes.len()-copied),"unlock:recv_buffer".into()]
        .into_iter().chain(ack.then_some("ack".into())).collect::<Vec<_>>());
}
fn wrapped() -> VecDeque<u8> {
    let mut data = VecDeque::with_capacity(8);
    data.extend(0..8);
    for _ in 0..5 { data.pop_front(); }
    data.extend(8..13);
    let (first,second)=data.as_slices();
    assert!(!first.is_empty()&&!second.is_empty()); data
}

#[test]
fn contiguous_partial_keeps_suffix_and_output_tail() {
    for method in [false,true] { check_data(VecDeque::from(vec![1,2,3,4,5]),3,method,false); }
}
#[test]
fn contiguous_full_and_oversized_reads() {
    for method in [false,true] { for size in [5,9] { check_data(VecDeque::from(vec![1,2,3,4,5]),size,method,false); } }
}
#[test]
fn wrapped_reads_before_at_and_across_first_slice_boundary() {
    let first=wrapped().as_slices().0.len();
    for method in [false,true] { for size in [1,first,first+1,7,8,12] { check_data(wrapped(),size,method,false); } }
}
#[test]
fn large_wrapped_deque_reads_copy_across_boundary_without_changing_capacity() {
    let capacity=65536;
    for method in [false,true] { for size in [1460,16384,65536,65539] {
        // VecDeque::clone may normalize its layout, so construct each wrapped
        // input separately and verify both physical slices immediately before use.
        let mut data=VecDeque::with_capacity(capacity);
        data.extend((0..capacity).map(|index| index as u8));
        for _ in 0..capacity-9 { data.pop_front(); }
        data.extend((capacity..capacity*2-9).map(|index| index as u8));
        assert_eq!(data.as_slices().0.len(),9);
        assert!(!data.as_slices().1.is_empty());
        check_data(data,size,method,false);
    } }
}
#[test]
fn window_ack_occurs_after_profile_and_receive_guard_drop() {
    for method in [false,true] { check_data(wrapped(),6,method,true); }
}
#[test]
fn buffered_data_precedes_eof_or_connection_error() {
    for method in [false,true] { for state in [TcpState::Closed,TcpState::Listen] {
        reset(); let socket=TcpSocket::new(VecDeque::from(vec![1,2]),state,true,false); let mut output=[9;4];
        assert_eq!(invoke(&socket,&mut output,method),(Ok(2),0)); assert_eq!(output,[1,2,9,9]);
        assert_eq!(remaining(&socket),[]); assert!(events().contains(&"profile_begin:2".into()));
    } }
}
#[test]
fn zero_length_nonblocking_output_preserves_buffer_and_existing_wouldblock() {
    reset(); let socket=TcpSocket::new(wrapped(),TcpState::Established,true,true); let before=remaining(&socket);
    assert_eq!(invoke(&socket,&mut[],false),(Err(SocketError::WouldBlock),0));
    assert_eq!(remaining(&socket),before);
    assert_eq!(events(),["lock:recv_buffer","profile_begin:0","profile_end","window:8","unlock:recv_buffer"]);
}
#[test]
fn zero_length_blocking_method_nonblocking_mode_retains_buffer_without_drain() {
    reset(); let socket=TcpSocket::new(wrapped(),TcpState::Established,false,true); let before=remaining(&socket);
    assert_eq!(invoke(&socket,&mut[],true),(Err(SocketError::WouldBlock),0));
    assert_eq!(remaining(&socket),before);
    assert_eq!(events(),["lock:recv_buffer","unlock:recv_buffer"]);
}
#[test]
fn empty_nonblocking_receive_state_results_and_zero_byte_profile() {
    for state in [TcpState::Established,TcpState::FinWait1,TcpState::FinWait2,TcpState::CloseWait,
                  TcpState::Closing,TcpState::LastAck,TcpState::TimeWait,TcpState::Closed,
                  TcpState::Listen,TcpState::SynSent,TcpState::SynReceived] {
        reset(); let socket=TcpSocket::new(VecDeque::new(),state,true,true); let mut output=[0xa5;4];
        let expected=if tcp_receive_side_open(state) { Err(SocketError::WouldBlock) }
                     else if tcp_receive_side_eof(state) { Ok(0) } else { Err(SocketError::NotConnected) };
        assert_eq!(invoke(&socket,&mut output,false),(expected,0)); assert_eq!(output,[0xa5;4]);
        assert_eq!(remaining(&socket),[]);
        assert_eq!(events(),["lock:recv_buffer","profile_begin:0","profile_end","window:0","unlock:recv_buffer"]);
    }
}
#[test]
fn empty_blocking_eof_and_disconnected_results_do_not_drain_or_register() {
    for state in [TcpState::CloseWait,TcpState::Closing,TcpState::LastAck,TcpState::TimeWait,
                  TcpState::Closed,TcpState::Listen,TcpState::SynSent,TcpState::SynReceived] {
        reset(); let socket=TcpSocket::new(VecDeque::new(),state,true,true); let mut output=[0xa5;4];
        let expected=if tcp_receive_side_eof(state) { Ok(0) } else { Err(SocketError::NotConnected) };
        assert_eq!(invoke(&socket,&mut output,true),(expected,0)); assert_eq!(output,[0xa5;4]);
        assert_eq!(events(),["lock:recv_buffer","unlock:recv_buffer"]);
    }
}
#[test]
fn empty_nonblocking_mode_blocking_method_does_not_drain_or_register() {
    reset(); let socket=TcpSocket::new(VecDeque::new(),TcpState::Established,false,true); let mut output=[0xa5;4];
    assert_eq!(invoke(&socket,&mut output,true),(Err(SocketError::WouldBlock),0));
    assert_eq!(output,[0xa5;4]); assert_eq!(events(),["lock:recv_buffer","unlock:recv_buffer"]);
}
#[test]
fn interrupted_and_timeout_waits_preserve_buffer_output_and_profile() {
    for (wait,error) in [(WaitResult::Interrupted,SocketError::Interrupted),(WaitResult::TimedOut,SocketError::WouldBlock)] {
        reset(); let socket=TcpSocket::new(VecDeque::new(),TcpState::Established,true,true); let mut output=[0xa5;4];
        WAIT.with(|queue| queue.borrow_mut().push_back((wait,Box::new(||{}))));
        assert_eq!(invoke(&socket,&mut output,true),(Err(error),1)); assert_eq!(output,[0xa5;4]);
        assert_eq!(remaining(&socket),[]); assert!(!events().iter().any(|event| event.starts_with("profile")||event.starts_with("window")||event=="ack"));
    }
}
#[test]
fn wakeup_data_drains_on_next_iteration_without_holding_guard_during_wait() {
    reset(); let socket=TcpSocket::new(VecDeque::new(),TcpState::Established,true,true); let captured=socket.clone();
    WAIT.with(|queue| queue.borrow_mut().push_back((WaitResult::Woken,Box::new(move|| {
        captured.recv_buffer.data.lock().unwrap().extend([1,2,3]);
    })))); let mut output=[0xa5;5];
    assert_eq!(invoke(&socket,&mut output,true),(Ok(3),1)); assert_eq!(output,[1,2,3,0xa5,0xa5]);
    assert_eq!(remaining(&socket),[]); assert_eq!(events().last().unwrap(),"ack");
    assert_eq!(events().iter().filter(|event| event.starts_with("profile_begin")).count(),1);
}
#[test]
fn arrival_after_waker_registration_is_rechecked_without_waiting() {
    reset(); let socket=TcpSocket::new(VecDeque::new(),TcpState::Established,true,false); let captured=socket.clone();
    REGISTER.with(|hook| *hook.borrow_mut()=Some(Box::new(move||{
        captured.recv_buffer.data.lock().unwrap().extend([4,5,6]);
    }))); let mut output=[0xa5;2];
    assert_eq!(invoke(&socket,&mut output,true),(Ok(2),0)); assert_eq!(output,[4,5]);
    assert_eq!(remaining(&socket),[6]); assert!(!events().contains(&"wait".into()));
}
#[test]
fn state_change_after_registration_returns_eof_without_drain() {
    reset(); let socket=TcpSocket::new(VecDeque::new(),TcpState::Established,true,true); let captured=socket.clone();
    REGISTER.with(|hook| *hook.borrow_mut()=Some(Box::new(move||captured.state.set(TcpState::CloseWait))));
    let mut output=[0xa5;2]; assert_eq!(invoke(&socket,&mut output,true),(Ok(0),0));
    assert_eq!(output,[0xa5;2]); assert!(!events().iter().any(|event| event.starts_with("profile")));
}
#[test]
fn repeated_partial_reads_preserve_fifo_across_wrap_and_refill() {
    for method in [false,true] {
        reset(); let socket=TcpSocket::new(wrapped(),TcpState::Established,true,false);
        let mut collected=Vec::new();
        for _ in 0..3 { let mut output=[0xa5;3]; let (result,waits)=invoke(&socket,&mut output,method);
            let len=result.unwrap(); assert_eq!(waits,0); collected.extend_from_slice(&output[..len]); }
        assert_eq!(collected,(5..13).collect::<Vec<u8>>()); assert_eq!(remaining(&socket),[]);
        socket.recv_buffer.data.lock().unwrap().extend([90,91,92]); let mut output=[0xa5;5];
        assert_eq!(invoke(&socket,&mut output,method),(Ok(3),0)); assert_eq!(output,[90,91,92,0xa5,0xa5]);
    }
}
#[test]
fn exhaustive_short_deque_offsets_and_read_lengths_match_fifo_reference() {
    for capacity in 1..20 { for offset in 0..capacity { for count in 1..=capacity {
        for length in 1..=capacity+2 { for method in [false,true] {
            let mut data=VecDeque::with_capacity(capacity); data.extend(0..capacity as u8);
            for _ in 0..offset { data.pop_front(); } data.extend(capacity as u8..(capacity+offset) as u8);
            data.truncate(count);
            assert_eq!(data.as_slices().0.len(),count.min(capacity-offset));
            assert_eq!(data.as_slices().1.len(),count.saturating_sub(capacity-offset));
            check_data(data,length,method,false);
        } }
    } } }
}
