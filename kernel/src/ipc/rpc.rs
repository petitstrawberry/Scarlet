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
pub const MAX_MESSAGE_OBJECTS: usize = 64;
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
    /// Capabilities are carried separately from untrusted serialized bytes.
    /// An ABI adapter translates its wire handles and offsets into this list.
    pub objects: Vec<Arc<RpcObject>>,
}

impl RpcMessage {
    fn validate(&self) -> Result<(), RpcError> {
        if self.bytes.len() > MAX_MESSAGE_SIZE || self.objects.len() > MAX_MESSAGE_OBJECTS {
            Err(RpcError::TooLarge)
        } else {
            Ok(())
        }
    }
}

/// A transferable reference to one dispatch object owned by an endpoint.
/// Fields are private: possession of a key alone does not grant a capability.
/// Strong references keep object identity alive, never the serving endpoint.
pub struct RpcObject {
    endpoint: Weak<RpcEndpoint>,
    key: u64,
}

impl RpcObject {
    /// Keys are unique within an endpoint and are never reused. They are local
    /// dispatch identifiers, not process-global handles or userspace pointers.
    pub fn key(&self) -> u64 {
        self.key
    }
    pub fn belongs_to(&self, endpoint: &Arc<RpcEndpoint>) -> bool {
        self.endpoint.ptr_eq(&Arc::downgrade(endpoint))
    }
    pub fn send(&self, message: RpcMessage) -> Result<RpcCall, RpcError> {
        self.endpoint
            .upgrade()
            .ok_or(RpcError::PeerClosed)?
            .send_to(Some(self.key), message)
    }
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
    object_key: Option<u64>,
    reply: Arc<CallState>,
}

impl RpcRequest {
    pub fn reply(self, message: RpcMessage) -> Result<(), RpcError> {
        message.validate()?;
        self.reply.finish(Completion::Reply(message))
    }
    pub fn object_key(&self) -> Option<u64> {
        self.object_key
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
    next_object_key: u64,
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
                next_object_key: 1,
                queue: VecDeque::new(),
                calls: Vec::new(),
            }),
            waker: Arc::new(Waker::new_interruptible("rpc_receive")),
            budget: Arc::new(AtomicUsize::new(0)),
        })
    }
    pub fn send(&self, message: RpcMessage) -> Result<RpcCall, RpcError> {
        self.send_to(None, message)
    }
    pub fn create_object(self: &Arc<Self>) -> Result<Arc<RpcObject>, RpcError> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(RpcError::PeerClosed);
        }
        let key = state.next_object_key;
        state.next_object_key = key.checked_add(1).ok_or(RpcError::Busy)?;
        Ok(Arc::new(RpcObject {
            endpoint: Arc::downgrade(self),
            key,
        }))
    }
    fn send_to(&self, object_key: Option<u64>, message: RpcMessage) -> Result<RpcCall, RpcError> {
        message.validate()?;
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
            object_key,
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
            objects: Vec::new(),
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

    #[test_case]
    fn rpc_transfers_and_forwards_object_capabilities_in_requests_and_replies() {
        let a = RpcEndpoint::new();
        let b = RpcEndpoint::new();
        let c = RpcEndpoint::new();
        let object = a.create_object().unwrap();
        let mut outgoing = message(1);
        outgoing.objects.push(object.clone());
        let first = b.send(outgoing).unwrap();
        let request = b.receive().unwrap().unwrap();
        assert_eq!(request.object_key(), None);
        assert!(Arc::ptr_eq(&request.message.objects[0], &object));
        let mut forwarded = message(2);
        forwarded.objects.push(request.message.objects[0].clone());
        let second = c.send(forwarded).unwrap();
        request.reply(message(3)).unwrap();
        assert_eq!(first.take_reply().unwrap().unwrap().opcode, 3);
        let request = c.receive().unwrap().unwrap();
        let received = request.message.objects[0].clone();
        assert!(received.belongs_to(&a));
        assert!(!received.belongs_to(&b));
        let call = received.send(message(4)).unwrap();
        let dispatch = a.receive().unwrap().unwrap();
        assert_eq!(dispatch.object_key(), Some(object.key()));
        assert_eq!(dispatch.message.opcode, 4);
        let callback = c.create_object().unwrap();
        let mut reply = message(5);
        reply.objects.push(callback.clone());
        dispatch.reply(reply).unwrap();
        let returned = call.take_reply().unwrap().unwrap();
        assert!(Arc::ptr_eq(&returned.objects[0], &callback));
        let callback_call = returned.objects[0].send(message(6)).unwrap();
        let callback_request = c.receive().unwrap().unwrap();
        assert_eq!(callback_request.object_key(), Some(callback.key()));
        callback_request.reply(message(7)).unwrap();
        assert_eq!(callback_call.take_reply().unwrap().unwrap().opcode, 7);
        request.reply(message(8)).unwrap();
        assert_eq!(second.take_reply().unwrap().unwrap().opcode, 8);
    }

    #[test_case]
    fn rpc_object_references_do_not_keep_the_owner_or_queued_self_references_alive() {
        let endpoint = RpcEndpoint::new();
        let owner = Arc::downgrade(&endpoint);
        let object = endpoint.create_object().unwrap();
        let identity = Arc::downgrade(&object);
        let mut outgoing = message(1);
        outgoing.objects.push(object.clone());
        let call = object.send(outgoing).unwrap();
        drop(endpoint);
        assert!(owner.upgrade().is_none());
        assert!(matches!(call.take_reply(), Some(Err(RpcError::PeerClosed))));
        assert!(matches!(object.send(message(2)), Err(RpcError::PeerClosed)));
        drop(object);
        assert!(identity.upgrade().is_none());
    }

    #[test_case]
    fn rpc_object_keys_are_distinct_and_close_revokes_all_dispatch() {
        let endpoint = RpcEndpoint::new();
        let first = endpoint.create_object().unwrap();
        let second = endpoint.create_object().unwrap();
        assert_ne!(first.key(), second.key());
        let call = second.send(message(1)).unwrap();
        let request = endpoint.receive().unwrap().unwrap();
        assert_eq!(request.object_key(), Some(second.key()));
        endpoint.close();
        assert!(matches!(call.take_reply(), Some(Err(RpcError::PeerClosed))));
        assert_eq!(request.reply(message(2)), Err(RpcError::Cancelled));
        assert!(matches!(first.send(message(3)), Err(RpcError::PeerClosed)));
        assert!(matches!(
            endpoint.create_object(),
            Err(RpcError::PeerClosed)
        ));
    }

    #[test_case]
    fn rpc_object_attachment_limits_reject_and_release_requests_and_replies() {
        let endpoint = RpcEndpoint::new();
        let object = endpoint.create_object().unwrap();
        let mut oversized = message(1);
        oversized.objects = alloc::vec![object.clone(); MAX_MESSAGE_OBJECTS + 1];
        assert!(matches!(endpoint.send(oversized), Err(RpcError::TooLarge)));
        assert_eq!(Arc::strong_count(&object), 1);
        let mut full = message(2);
        full.objects = alloc::vec![object.clone(); MAX_MESSAGE_OBJECTS];
        let call = endpoint.send(full).unwrap();
        let request = endpoint.receive().unwrap().unwrap();
        let mut reply = message(3);
        reply.objects = alloc::vec![object.clone(); MAX_MESSAGE_OBJECTS + 1];
        assert_eq!(request.reply(reply), Err(RpcError::TooLarge));
        assert!(matches!(call.take_reply(), Some(Err(RpcError::PeerClosed))));
        assert_eq!(Arc::strong_count(&object), 1);
        assert_eq!(endpoint.budget.load(Ordering::Acquire), 1);
        drop(call);
        assert_eq!(endpoint.budget.load(Ordering::Acquire), 0);
    }
}
