//! ABI-neutral, bounded calls and one-way notifications. Reply capabilities are
//! single-use; dropping one completes its caller with PeerClosed. One-way
//! deliveries serialize per dispatch object until the receiver releases them.
//! Adapters own wire formats, peer discovery, credentials and userspace handles.

use crate::sync::{IrqSpinLock, waker::Waker};
use alloc::{
    collections::VecDeque,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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

#[derive(Clone)]
pub struct RpcMessage {
    pub opcode: u32,
    pub bytes: Vec<u8>,
    /// Capabilities are carried separately from untrusted serialized bytes.
    /// An ABI adapter translates its wire handles and offsets into this list.
    pub objects: Vec<Arc<RpcObject>>,
    /// Adapter-owned lifetime resources travel with the message, including
    /// queued, cancelled and failed deliveries. The transport never interprets
    /// them; releasing the last message copy releases these resources.
    pub retained: Option<Arc<dyn Send + Sync>>,
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
    revoked: AtomicBool,
    observers: IrqSpinLock<Vec<Weak<DeathObserver>>>,
    async_gate: Arc<AsyncGate>,
}
struct DeathObserver {
    waker: Weak<Waker>,
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
            .send_to(Some(self), message, false)
            .map(|state| RpcCall { state })
    }
    /// Admit a bounded notification without a reply capability. Notifications
    /// to this object are serialized until the receiver drops its delivery.
    pub fn send_oneway(&self, message: RpcMessage) -> Result<(), RpcError> {
        self.endpoint
            .upgrade()
            .ok_or(RpcError::PeerClosed)?
            .send_to(Some(self), message, true)
            .map(|_| ())
    }
    /// Revoke only this dispatch capability, including queued and received
    /// unanswered calls. Other objects served by the endpoint remain usable.
    pub fn revoke(&self) {
        if !self.revoked.swap(true, Ordering::AcqRel) {
            if let Some(endpoint) = self.endpoint.upgrade() {
                endpoint.revoke_object(self.key);
            }
            let wakers: Vec<_> = self
                .observers
                .lock()
                .iter()
                .filter_map(Weak::upgrade)
                .filter_map(|observer| observer.waker.upgrade())
                .collect();
            for waker in wakers {
                waker.wake_all();
            }
        }
    }
    /// Observe permanent loss of this capability without retaining its server.
    /// Dropping the watch cancels registration; the readiness source is weak.
    pub fn watch_death(self: &Arc<Self>, waker: &Arc<Waker>) -> RpcDeathWatch {
        let observer = Arc::new(DeathObserver {
            waker: Arc::downgrade(waker),
        });
        {
            let mut observers = self.observers.lock();
            observers.retain(|observer| observer.strong_count() != 0);
            observers.push(Arc::downgrade(&observer));
        }
        let watch = RpcDeathWatch {
            object: self.clone(),
            _observer: observer,
        };
        // Register before checking: close/revoke cannot be missed in between.
        if watch.ready() {
            waker.wake_all();
        }
        watch
    }
}

pub struct RpcDeathWatch {
    object: Arc<RpcObject>,
    _observer: Arc<DeathObserver>,
}
impl RpcDeathWatch {
    pub fn ready(&self) -> bool {
        self.object.revoked.load(Ordering::Acquire)
            || self
                .object
                .endpoint
                .upgrade()
                .is_none_or(|endpoint| endpoint.state.lock().closed)
    }
}

enum Completion {
    Pending,
    Reply(RpcMessage),
    Failed(RpcError),
    Consumed,
}

struct AsyncGate {
    active: AtomicBool,
    waker: Weak<Waker>,
}
impl AsyncGate {
    fn new(waker: &Arc<Waker>) -> Arc<Self> {
        Arc::new(Self {
            active: AtomicBool::new(false),
            waker: Arc::downgrade(waker),
        })
    }
}

struct CallState {
    completion: IrqSpinLock<Completion>,
    waker: Arc<Waker>,
    budget: Arc<AtomicUsize>,
    notifier: IrqSpinLock<Option<Weak<Waker>>>,
    object_key: Option<u64>,
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
        let notifier = self.notifier.lock().as_ref().and_then(Weak::upgrade);
        if let Some(notifier) = notifier {
            notifier.wake_all();
        }
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
    /// Join completion with an adapter's existing readiness source. The weak
    /// reference cannot keep the receiving process alive. Publishing before
    /// rechecking completion also covers registration after an early reply.
    pub fn notify_on_completion(&self, waker: &Arc<Waker>) {
        *self.state.notifier.lock() = Some(Arc::downgrade(waker));
        if self.ready() {
            waker.wake_all();
        }
    }
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
    async_gate: Option<Arc<AsyncGate>>,
    delivered: bool,
}

impl RpcRequest {
    pub fn reply(self, message: RpcMessage) -> Result<(), RpcError> {
        if self.is_oneway() {
            return Err(RpcError::Cancelled);
        }
        message.validate()?;
        self.reply.finish(Completion::Reply(message))
    }
    pub fn is_oneway(&self) -> bool {
        self.async_gate.is_some()
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
        if self.delivered {
            if let Some(gate) = &self.async_gate {
                gate.active.store(false, Ordering::Release);
                if let Some(waker) = gate.waker.upgrade() {
                    waker.wake_all();
                }
            }
        }
        let _ = self.reply.finish(Completion::Failed(RpcError::PeerClosed));
    }
}

struct EndpointState {
    closed: bool,
    next_object_key: u64,
    queue: VecDeque<RpcRequest>,
    calls: Vec<Weak<CallState>>,
    objects: Vec<Weak<RpcObject>>,
}

pub struct RpcEndpoint {
    async_gate: Arc<AsyncGate>,
    state: IrqSpinLock<EndpointState>,
    waker: Arc<Waker>,
    budget: Arc<AtomicUsize>,
}

impl RpcEndpoint {
    pub fn new() -> Arc<Self> {
        let waker = Arc::new(Waker::new_interruptible("rpc_receive"));
        Arc::new(Self {
            async_gate: AsyncGate::new(&waker),
            state: IrqSpinLock::new(EndpointState {
                closed: false,
                next_object_key: 1,
                queue: VecDeque::new(),
                calls: Vec::new(),
                objects: Vec::new(),
            }),
            waker,
            budget: Arc::new(AtomicUsize::new(0)),
        })
    }
    pub fn send(&self, message: RpcMessage) -> Result<RpcCall, RpcError> {
        self.send_to(None, message, false)
            .map(|state| RpcCall { state })
    }
    pub fn send_oneway(&self, message: RpcMessage) -> Result<(), RpcError> {
        self.send_to(None, message, true).map(|_| ())
    }
    pub fn create_object(self: &Arc<Self>) -> Result<Arc<RpcObject>, RpcError> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(RpcError::PeerClosed);
        }
        let key = state.next_object_key;
        state.next_object_key = key.checked_add(1).ok_or(RpcError::Busy)?;
        let object = Arc::new(RpcObject {
            endpoint: Arc::downgrade(self),
            key,
            revoked: AtomicBool::new(false),
            observers: IrqSpinLock::new(Vec::new()),
            async_gate: AsyncGate::new(&self.waker),
        });
        state.objects.retain(|object| object.strong_count() != 0);
        state.objects.push(Arc::downgrade(&object));
        Ok(object)
    }
    fn send_to(
        &self,
        object: Option<&RpcObject>,
        message: RpcMessage,
        oneway: bool,
    ) -> Result<Arc<CallState>, RpcError> {
        message.validate()?;
        let mut state = self.state.lock();
        if state.closed || object.is_some_and(|object| object.revoked.load(Ordering::Acquire)) {
            return Err(RpcError::PeerClosed);
        }
        let object_key = object.map(RpcObject::key);
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
            notifier: IrqSpinLock::new(None),
            object_key,
        });
        state.queue.push_back(RpcRequest {
            message,
            object_key,
            reply: call.clone(),
            async_gate: oneway
                .then(|| object.map_or_else(|| self.async_gate.clone(), |o| o.async_gate.clone())),
            delivered: false,
        });
        state.calls.push(Arc::downgrade(&call));
        drop(state);
        // Adapters can multiplex idle receivers and outstanding callers on
        // this source. A wake must reach every distinct readiness predicate.
        self.waker.wake_all();
        Ok(call)
    }
    pub fn receive(&self) -> Result<Option<RpcRequest>, RpcError> {
        let mut state = self.state.lock();
        state.queue.retain(|request| !request.cancelled());
        if let Some(index) = state.queue.iter().position(|request| {
            request
                .async_gate
                .as_ref()
                .is_none_or(|gate| !gate.active.load(Ordering::Acquire))
        }) {
            let mut request = state.queue.remove(index).unwrap();
            if let Some(gate) = &request.async_gate {
                gate.active.store(true, Ordering::Release);
            }
            request.delivered = true;
            return Ok(Some(request));
        }
        if state.closed {
            Err(RpcError::PeerClosed)
        } else {
            Ok(None)
        }
    }
    pub fn ready(&self) -> bool {
        let state = self.state.lock();
        state.closed
            || state.queue.iter().any(|request| {
                !request.cancelled()
                    && request
                        .async_gate
                        .as_ref()
                        .is_none_or(|gate| !gate.active.load(Ordering::Acquire))
            })
    }
    pub fn waker(&self) -> Arc<Waker> {
        self.waker.clone()
    }
    fn revoke_object(&self, key: u64) {
        let (removed, calls) = {
            let mut state = self.state.lock();
            let mut removed = VecDeque::new();
            let mut kept = VecDeque::new();
            while let Some(request) = state.queue.pop_front() {
                if request.object_key == Some(key) {
                    removed.push_back(request);
                } else {
                    kept.push_back(request);
                }
            }
            state.queue = kept;
            let calls: Vec<_> = state
                .calls
                .iter()
                .filter_map(Weak::upgrade)
                .filter(|call| call.object_key == Some(key))
                .collect();
            (removed, calls)
        };
        drop(removed);
        for call in calls {
            let _ = call.finish(Completion::Failed(RpcError::PeerClosed));
        }
        self.waker.wake_all();
    }
    pub fn close(&self) {
        let (queue, calls, objects) = {
            let mut state = self.state.lock();
            state.closed = true;
            (
                core::mem::take(&mut state.queue),
                core::mem::take(&mut state.calls),
                core::mem::take(&mut state.objects),
            )
        };
        // Dropping reply capabilities wakes their callers outside the queue lock.
        drop(queue);
        for call in calls {
            if let Some(call) = call.upgrade() {
                let _ = call.finish(Completion::Failed(RpcError::PeerClosed));
            }
        }
        for object in objects.into_iter().filter_map(|object| object.upgrade()) {
            object.revoke();
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
    #[test_case]
    fn rpc_oneway_serializes_each_object_until_delivery_is_released() {
        let endpoint = RpcEndpoint::new();
        let a = endpoint.create_object().unwrap();
        let b = endpoint.create_object().unwrap();
        a.send_oneway(message(1)).unwrap();
        a.send_oneway(message(2)).unwrap();
        b.send_oneway(message(3)).unwrap();
        let first = endpoint.receive().unwrap().unwrap();
        assert!(first.is_oneway());
        assert_eq!(first.message.opcode, 1);
        let independent = endpoint.receive().unwrap().unwrap();
        assert_eq!(independent.message.opcode, 3);
        assert!(!endpoint.ready());
        assert!(endpoint.receive().unwrap().is_none());
        // A synchronous call need not wait for the notification stream.
        let sync = a.send(message(4)).unwrap();
        let request = endpoint.receive().unwrap().unwrap();
        assert_eq!(request.message.opcode, 4);
        assert!(!request.is_oneway());
        request.reply(message(5)).unwrap();
        assert_eq!(sync.take_reply().unwrap().unwrap().opcode, 5);
        drop(first);
        assert!(endpoint.ready());
        let second = endpoint.receive().unwrap().unwrap();
        assert_eq!(second.message.opcode, 2);
        assert_eq!(second.reply(message(6)), Err(RpcError::Cancelled));
        drop(independent);
        drop(sync);
        assert_eq!(endpoint.budget.load(Ordering::Acquire), 0);
    }
    #[test_case]
    fn rpc_oneway_bounds_include_delivered_notifications_and_close_reclaims_queue() {
        let endpoint = RpcEndpoint::new();
        for n in 0..MAX_IN_FLIGHT {
            endpoint.send_oneway(message(n as u32)).unwrap();
        }
        let request = endpoint.receive().unwrap().unwrap();
        assert_eq!(endpoint.send_oneway(message(100)), Err(RpcError::Busy));
        assert!(!endpoint.ready());
        drop(request);
        endpoint.send_oneway(message(100)).unwrap();
        let request = endpoint.receive().unwrap().unwrap();
        assert_eq!(request.message.opcode, 1);
        endpoint.close();
        assert!(request.cancelled());
        assert_eq!(endpoint.budget.load(Ordering::Acquire), 1);
        assert_eq!(
            endpoint.send_oneway(message(101)),
            Err(RpcError::PeerClosed)
        );
        drop(request);
        assert_eq!(endpoint.budget.load(Ordering::Acquire), 0);
    }
    #[test_case]
    fn rpc_oneway_revoke_and_endpoint_drop_preserve_only_delivered_resources() {
        let endpoint = RpcEndpoint::new();
        let object = endpoint.create_object().unwrap();
        let retained = Arc::new(9usize);
        let mut msg = message(1);
        msg.retained = Some(retained.clone());
        object.send_oneway(msg.clone()).unwrap();
        object.send_oneway(msg).unwrap();
        let received = endpoint.receive().unwrap().unwrap();
        object.revoke();
        assert!(received.cancelled());
        assert!(endpoint.receive().unwrap().is_none());
        assert_eq!(Arc::strong_count(&retained), 2);
        assert_eq!(object.send_oneway(message(2)), Err(RpcError::PeerClosed));
        drop(endpoint);
        drop(received);
        assert_eq!(Arc::strong_count(&retained), 1);
    }
    #[test_case]
    fn rpc_death_watches_wake_on_revoke_close_drop_and_late_registration() {
        for mode in 0..3 {
            let endpoint = RpcEndpoint::new();
            let weak_endpoint = Arc::downgrade(&endpoint);
            let object = endpoint.create_object().unwrap();
            let sibling = endpoint.create_object().unwrap();
            let waker = Arc::new(Waker::new_interruptible("rpc-death-test"));
            let watch = object.watch_death(&waker);
            let sibling_watch = sibling.watch_death(&waker);
            assert!(!watch.ready());
            assert_eq!(Arc::strong_count(&endpoint), 1);
            assert_eq!(Arc::strong_count(&waker), 1);
            match mode {
                0 => object.revoke(),
                1 => endpoint.close(),
                _ => drop(endpoint),
            }
            assert!(watch.ready());
            assert_eq!(sibling_watch.ready(), mode != 0);
            assert!(waker.pending_wake_count_for_test() > 0);
            if mode == 2 {
                assert!(weak_endpoint.upgrade().is_none());
            }
            let late_waker = Arc::new(Waker::new_interruptible("rpc-death-late"));
            let late = object.watch_death(&late_waker);
            assert!(late.ready());
            assert!(late_waker.pending_wake_count_for_test() > 0);
        }
    }
    #[test_case]
    fn rpc_cancelled_death_watches_do_not_wake_or_accumulate() {
        let endpoint = RpcEndpoint::new();
        let object = endpoint.create_object().unwrap();
        let waker = Arc::new(Waker::new_interruptible("rpc-death-cancel"));
        for _ in 0..100 {
            drop(object.watch_death(&waker));
        }
        assert_eq!(object.observers.lock().len(), 1);
        object.revoke();
        assert_eq!(waker.pending_wake_count_for_test(), 0);
        for _ in 0..100 {
            drop(endpoint.create_object().unwrap());
        }
        assert_eq!(endpoint.state.lock().objects.len(), 2);
    }
    #[test_case]
    fn rpc_retained_resources_follow_cancelled_requests_and_consumed_replies() {
        let endpoint = RpcEndpoint::new();
        let lifetime = Arc::new(42usize);
        let weak = Arc::downgrade(&lifetime);
        let mut outgoing = message(1);
        outgoing.retained = Some(lifetime);
        let call = endpoint.send(outgoing).unwrap();
        drop(call);
        assert!(endpoint.receive().unwrap().is_none());
        assert!(weak.upgrade().is_none());

        let lifetime = Arc::new(43usize);
        let weak = Arc::downgrade(&lifetime);
        let call = endpoint.send(message(2)).unwrap();
        let request = endpoint.receive().unwrap().unwrap();
        let mut reply = message(3);
        reply.retained = Some(lifetime);
        request.reply(reply).unwrap();
        let reply = call.take_reply().unwrap().unwrap();
        let copy = reply.clone();
        drop(reply);
        drop(call);
        assert!(weak.upgrade().is_some());
        drop(copy);
        assert!(weak.upgrade().is_none());
    }
    #[test_case]
    fn rpc_completion_notifier_is_weak_and_handles_early_replies() {
        let endpoint = RpcEndpoint::new();
        for early in [false, true] {
            let call = endpoint.send(message(1)).unwrap();
            let request = endpoint.receive().unwrap().unwrap();
            let waker = Arc::new(Waker::new_interruptible("rpc-adapter-test"));
            let weak = Arc::downgrade(&waker);
            let mut request = Some(request);
            if early {
                request.take().unwrap().reply(message(2)).unwrap();
            }
            call.notify_on_completion(&waker);
            if let Some(request) = request {
                request.reply(message(2)).unwrap();
            }
            assert!(call.ready());
            assert!(waker.pending_wake_count_for_test() > 0);
            assert_eq!(Arc::strong_count(&waker), 1);
            drop(waker);
            assert!(weak.upgrade().is_none());
        }
    }
    fn message(n: u32) -> RpcMessage {
        RpcMessage {
            opcode: n,
            bytes: alloc::vec![1, 2, 3],
            objects: Vec::new(),
            retained: None,
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
    fn rpc_object_revoke_fails_only_its_calls_and_releases_retained_resources() {
        let endpoint = RpcEndpoint::new();
        let object = endpoint.create_object().unwrap();
        let alias = object.clone();
        let sibling = endpoint.create_object().unwrap();

        let completed = object.send(message(1)).unwrap();
        endpoint
            .receive()
            .unwrap()
            .unwrap()
            .reply(message(2))
            .unwrap();

        let received_lifetime = Arc::new(10usize);
        let received_weak = Arc::downgrade(&received_lifetime);
        let mut received_message = message(10);
        received_message.retained = Some(received_lifetime);
        let received = object.send(received_message).unwrap();
        let request = endpoint.receive().unwrap().unwrap();
        assert_eq!(request.object_key(), Some(object.key()));

        let sibling_call = sibling.send(message(20)).unwrap();
        let queued_lifetime = Arc::new(30usize);
        let queued_weak = Arc::downgrade(&queued_lifetime);
        let mut queued_message = message(30);
        queued_message.retained = Some(queued_lifetime);
        let queued = object.send(queued_message).unwrap();
        let direct = endpoint.send(message(40)).unwrap();
        let notifier = Arc::new(Waker::new_interruptible("rpc-revoke-test"));
        received.notify_on_completion(&notifier);
        assert_eq!(notifier.pending_wake_count_for_test(), 0);

        object.revoke();
        alias.revoke();
        assert!(request.cancelled());
        for call in [&received, &queued] {
            assert!(call.ready());
            assert!(call.waker().pending_wake_count_for_test() > 0);
            assert!(matches!(call.take_reply(), Some(Err(RpcError::PeerClosed))));
            assert!(call.take_reply().is_none());
        }
        assert!(notifier.pending_wake_count_for_test() > 0);
        assert!(queued_weak.upgrade().is_none());
        // A request already handed to the server still owns its payload.
        assert!(received_weak.upgrade().is_some());
        assert_eq!(completed.take_reply().unwrap().unwrap().opcode, 2);

        assert!(!sibling_call.ready());
        assert!(!direct.ready());
        for (call, key, opcode) in [
            (&sibling_call, Some(sibling.key()), 20),
            (&direct, None, 40),
        ] {
            let survivor = endpoint.receive().unwrap().unwrap();
            assert_eq!(survivor.object_key(), key);
            assert_eq!(survivor.message.opcode, opcode);
            survivor.reply(message(opcode + 1)).unwrap();
            assert_eq!(call.take_reply().unwrap().unwrap().opcode, opcode + 1);
        }
        assert!(endpoint.receive().unwrap().is_none());

        let reply_lifetime = Arc::new(50usize);
        let reply_weak = Arc::downgrade(&reply_lifetime);
        let mut late_reply = message(50);
        late_reply.retained = Some(reply_lifetime);
        assert_eq!(request.reply(late_reply), Err(RpcError::Cancelled));
        assert!(received_weak.upgrade().is_none());
        assert!(reply_weak.upgrade().is_none());
        assert!(received.take_reply().is_none());

        let rejected_lifetime = Arc::new(60usize);
        let rejected_weak = Arc::downgrade(&rejected_lifetime);
        let mut rejected = message(60);
        rejected.retained = Some(rejected_lifetime);
        assert!(matches!(alias.send(rejected), Err(RpcError::PeerClosed)));
        assert!(rejected_weak.upgrade().is_none());
        let next = sibling.send(message(70)).unwrap();
        endpoint
            .receive()
            .unwrap()
            .unwrap()
            .reply(message(71))
            .unwrap();
        assert_eq!(next.take_reply().unwrap().unwrap().opcode, 71);
        drop((completed, received, queued, sibling_call, direct, next));
        assert_eq!(endpoint.budget.load(Ordering::Acquire), 0);
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
