//! ABI-neutral, bounded request/reply objects. A reply capability is single-use;
//! dropping it completes the caller with PeerClosed. Adapters own wire formats,
//! peer discovery, credentials and userspace handles; this module owns delivery.

use crate::sync::{IrqSpinLock, waker::Waker};
use alloc::{
    collections::VecDeque,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::sync::atomic::{AtomicUsize, Ordering};

pub const MAX_MESSAGE_SIZE: usize = 64 * 1024;
const MAX_IN_FLIGHT: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcError {
    PeerClosed,
    Busy,
    TooLarge,
    Cancelled,
}

pub struct RpcMessage {
    pub opcode: u32,
    pub bytes: Vec<u8>,
}

enum Completion {
    Pending,
    Reply(RpcMessage),
    Failed(RpcError),
    Consumed,
}

struct CallState {
    completion: IrqSpinLock<Completion>,
    waker: Arc<Waker>,
    budget: Arc<AtomicUsize>,
}

impl CallState {
    fn finish(&self, completion: Completion) -> Result<(), RpcError> {
        let mut state = self.completion.lock();
        if !matches!(*state, Completion::Pending) {
            return Err(RpcError::Cancelled);
        }
        *state = completion;
        drop(state);
        self.waker.wake_all();
        Ok(())
    }
}

impl Drop for CallState {
    fn drop(&mut self) {
        self.budget.fetch_sub(1, Ordering::Release);
    }
}

pub struct RpcCall {
    state: Arc<CallState>,
}

impl RpcCall {
    pub fn ready(&self) -> bool {
        !matches!(*self.state.completion.lock(), Completion::Pending)
    }
    pub fn waker(&self) -> Arc<Waker> {
        self.state.waker.clone()
    }
    pub fn take_reply(&self) -> Option<Result<RpcMessage, RpcError>> {
        let mut state = self.state.completion.lock();
        if matches!(*state, Completion::Pending | Completion::Consumed) {
            return None;
        }
        Some(
            match core::mem::replace(&mut *state, Completion::Consumed) {
                Completion::Reply(message) => Ok(message),
                Completion::Failed(error) => Err(error),
                _ => unreachable!(),
            },
        )
    }
}

impl Drop for RpcCall {
    fn drop(&mut self) {
        let _ = self.state.finish(Completion::Failed(RpcError::Cancelled));
    }
}

pub struct RpcRequest {
    pub message: RpcMessage,
    reply: Arc<CallState>,
}

impl RpcRequest {
    pub fn reply(self, message: RpcMessage) -> Result<(), RpcError> {
        if message.bytes.len() > MAX_MESSAGE_SIZE {
            return Err(RpcError::TooLarge);
        }
        self.reply.finish(Completion::Reply(message))
    }
    pub fn cancelled(&self) -> bool {
        !matches!(*self.reply.completion.lock(), Completion::Pending)
    }
}

impl Drop for RpcRequest {
    fn drop(&mut self) {
        let _ = self.reply.finish(Completion::Failed(RpcError::PeerClosed));
    }
}

struct EndpointState {
    closed: bool,
    queue: VecDeque<RpcRequest>,
    calls: Vec<Weak<CallState>>,
}

pub struct RpcEndpoint {
    state: IrqSpinLock<EndpointState>,
    waker: Arc<Waker>,
    budget: Arc<AtomicUsize>,
}

impl RpcEndpoint {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: IrqSpinLock::new(EndpointState {
                closed: false,
                queue: VecDeque::new(),
                calls: Vec::new(),
            }),
            waker: Arc::new(Waker::new_interruptible("rpc_receive")),
            budget: Arc::new(AtomicUsize::new(0)),
        })
    }
    pub fn send(&self, message: RpcMessage) -> Result<RpcCall, RpcError> {
        if message.bytes.len() > MAX_MESSAGE_SIZE {
            return Err(RpcError::TooLarge);
        }
        let mut state = self.state.lock();
        if state.closed {
            return Err(RpcError::PeerClosed);
        }
        state.queue.retain(|request| !request.cancelled());
        state.calls.retain(|call| call.strong_count() > 0);
        // Senders serialize here; completion drops only decrement this budget.
        if self.budget.load(Ordering::Acquire) >= MAX_IN_FLIGHT {
            return Err(RpcError::Busy);
        }
        self.budget.fetch_add(1, Ordering::Relaxed);
        let call = Arc::new(CallState {
            completion: IrqSpinLock::new(Completion::Pending),
            waker: Arc::new(Waker::new_interruptible("rpc_reply")),
            budget: self.budget.clone(),
        });
        state.queue.push_back(RpcRequest {
            message,
            reply: call.clone(),
        });
        state.calls.push(Arc::downgrade(&call));
        drop(state);
        self.waker.wake_one();
        Ok(RpcCall { state: call })
    }
    pub fn receive(&self) -> Result<Option<RpcRequest>, RpcError> {
        let mut state = self.state.lock();
        while let Some(request) = state.queue.pop_front() {
            if !request.cancelled() {
                return Ok(Some(request));
            }
        }
        if state.closed {
            Err(RpcError::PeerClosed)
        } else {
            Ok(None)
        }
    }
    pub fn ready(&self) -> bool {
        let state = self.state.lock();
        state.closed || !state.queue.is_empty()
    }
    pub fn waker(&self) -> Arc<Waker> {
        self.waker.clone()
    }
    pub fn close(&self) {
        let (queue, calls) = {
            let mut state = self.state.lock();
            state.closed = true;
            (
                core::mem::take(&mut state.queue),
                core::mem::take(&mut state.calls),
            )
        };
        // Dropping reply capabilities wakes their callers outside the queue lock.
        drop(queue);
        for call in calls {
            if let Some(call) = call.upgrade() {
                let _ = call.finish(Completion::Failed(RpcError::PeerClosed));
            }
        }
        self.waker.wake_all();
    }
}

impl Drop for RpcEndpoint {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn message(n: u32) -> RpcMessage {
        RpcMessage {
            opcode: n,
            bytes: alloc::vec![1, 2, 3],
        }
    }
    #[test_case]
    fn rpc_routes_replies_to_their_callers_even_out_of_order() {
        let endpoint = RpcEndpoint::new();
        let a = endpoint.send(message(1)).unwrap();
        let b = endpoint.send(message(2)).unwrap();
        let ra = endpoint.receive().unwrap().unwrap();
        let rb = endpoint.receive().unwrap().unwrap();
        assert_eq!(ra.message.opcode, 1);
        rb.reply(message(22)).unwrap();
        assert!(!a.ready());
        assert_eq!(b.take_reply().unwrap().unwrap().opcode, 22);
        ra.reply(message(11)).unwrap();
        assert_eq!(a.take_reply().unwrap().unwrap().opcode, 11);
        assert!(a.take_reply().is_none());
    }
    #[test_case]
    fn rpc_close_and_dropped_reply_capabilities_complete_waiters() {
        let endpoint = RpcEndpoint::new();
        let a = endpoint.send(message(1)).unwrap();
        let request = endpoint.receive().unwrap().unwrap();
        let b = endpoint.send(message(2)).unwrap();
        endpoint.close();
        assert!(a.ready());
        assert!(matches!(b.take_reply(), Some(Err(RpcError::PeerClosed))));
        drop(request);
        assert!(matches!(a.take_reply(), Some(Err(RpcError::PeerClosed))));
        assert!(matches!(
            endpoint.send(message(3)),
            Err(RpcError::PeerClosed)
        ));
    }
    #[test_case]
    fn rpc_budget_includes_dequeued_unanswered_calls_and_releases_after_cancel() {
        let endpoint = RpcEndpoint::new();
        let calls: Vec<_> = (0..MAX_IN_FLIGHT)
            .map(|n| endpoint.send(message(n as u32)).unwrap())
            .collect();
        let request = endpoint.receive().unwrap().unwrap();
        assert!(matches!(endpoint.send(message(99)), Err(RpcError::Busy)));
        drop(calls);
        drop(request);
        assert!(endpoint.receive().unwrap().is_none());
        assert!(endpoint.send(message(100)).is_ok());
    }
}
