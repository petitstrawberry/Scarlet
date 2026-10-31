//! ABI-neutral, bounded calls and one-way notifications. Reply capabilities are
//! single-use; dropping one completes its caller with PeerClosed. One-way
//! deliveries serialize per dispatch object until the receiver releases them.
//! The transport snapshots native sender identity outside serialized data.
//! Adapters own wire formats, peer discovery and userspace handles.

use crate::object::{KernelObject, handle::HandleMetadata};
use crate::sync::{IrqSpinLock, waker::Waker};
use alloc::{
    collections::VecDeque,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub const MAX_MESSAGE_SIZE: usize = 256 * 1024;
pub const MAX_MESSAGE_OBJECTS: usize = 64;
const MAX_IN_FLIGHT: usize = 64;
pub const MAX_CALL_DEPTH: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcError {
    PeerClosed,
    Busy,
    TooLarge,
    Cancelled,
}

/// Authenticated native identity at admission. Only the transport constructs
/// this snapshot from a kernel task; serialized message bytes cannot replace it.
/// It retains no task or handle table and grants no authority to the receiver.
#[derive(Clone, Debug)]
pub struct RpcSender {
    principal: crate::shield::identity::Principal,
    subject: Option<crate::shield::Subject>,
}
impl RpcSender {
    fn capture(task: &crate::task::Task) -> Self {
        // Same lock order as a Shield label transition; hold both through the
        // snapshot so a concurrent change cannot combine two identities.
        let credentials = task.credentials.read();
        let subject = task.security_subject.read();
        Self {
            principal: credentials.principals().active,
            subject: subject.clone(),
        }
    }
    pub fn principal(&self) -> crate::shield::identity::Principal {
        self.principal
    }
    pub fn subject(&self) -> Option<&crate::shield::Subject> {
        self.subject.as_ref()
    }
}

#[derive(Clone)]
pub struct RpcMessage {
    pub opcode: u32,
    pub bytes: Vec<u8>,
    /// Capabilities are carried separately from untrusted serialized bytes.
    /// An ABI adapter translates its wire handles and offsets into this list.
    pub objects: Vec<Arc<RpcObject>>,
    /// Owned native capabilities and their access metadata. They remain owned
    /// while queued, in a reply, or retained by another message copy.
    pub capabilities: Vec<(KernelObject, HandleMetadata)>,
    /// Adapter-owned lifetime resources travel with the message, including
    /// queued, cancelled and failed deliveries. The transport never interprets
    /// them; releasing the last message copy releases these resources.
    pub retained: Option<Arc<dyn Send + Sync>>,
}

impl RpcMessage {
    fn validate(&self) -> Result<(), RpcError> {
        if self.bytes.len() > MAX_MESSAGE_SIZE
            || self.objects.len().saturating_add(self.capabilities.len()) > MAX_MESSAGE_OBJECTS
        {
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
    minimum_priority: u8,
    inherit_priority: bool,
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
    /// Continue a received call, preserving its bounded native return route.
    pub fn send_scoped(
        &self,
        message: RpcMessage,
        caller: &RpcEndpoint,
        parent: Option<&RpcContext>,
    ) -> Result<RpcCall, RpcError> {
        self.endpoint
            .upgrade()
            .ok_or(RpcError::PeerClosed)?
            .send_chained(Some(self), message, false, Some(caller), parent, 0, None)
            .map(|state| RpcCall { state })
    }
    pub fn send_scoped_with_priority(
        &self,
        message: RpcMessage,
        caller: &RpcEndpoint,
        parent: Option<&RpcContext>,
        priority: u8,
    ) -> Result<RpcCall, RpcError> {
        self.endpoint
            .upgrade()
            .ok_or(RpcError::PeerClosed)?
            .send_chained(
                Some(self),
                message,
                false,
                Some(caller),
                parent,
                priority,
                None,
            )
            .map(|state| RpcCall { state })
    }
    /// Kernel syscall adapters supply the authenticated invoking task, never a
    /// caller-selected label or numeric identity from a wire structure.
    pub(crate) fn send_scoped_from(
        &self,
        task: &crate::task::Task,
        message: RpcMessage,
        caller: &RpcEndpoint,
        parent: Option<&RpcContext>,
    ) -> Result<RpcCall, RpcError> {
        self.endpoint
            .upgrade()
            .ok_or(RpcError::PeerClosed)?
            .send_chained(
                Some(self),
                message,
                false,
                Some(caller),
                parent,
                task.fixed_priority.effective(),
                Some(task),
            )
            .map(|state| RpcCall { state })
    }
    pub(crate) fn send_oneway_from(
        &self,
        task: &crate::task::Task,
        message: RpcMessage,
    ) -> Result<(), RpcError> {
        self.endpoint
            .upgrade()
            .ok_or(RpcError::PeerClosed)?
            .send_chained(Some(self), message, true, None, None, 0, Some(task))
            .map(|_| ())
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
    receiver: Weak<Waker>,
    caller: Option<Weak<Waker>>,
    parent: Option<Weak<CallState>>,
    children: IrqSpinLock<Vec<Weak<CallState>>>,
    depth: usize,
    priority: u8,
    priority_lease: IrqSpinLock<Option<crate::task::fixed_priority::PriorityLease>>,
}

/// An unforgeable, weak continuation of a received synchronous request.
/// It grants no object access and cannot outlive a completed parent call.
#[derive(Clone)]
pub struct RpcContext {
    state: Weak<CallState>,
}

impl CallState {
    fn finish(&self, completion: Completion) -> Result<(), RpcError> {
        let mut state = self.completion.lock();
        if !matches!(*state, Completion::Pending) {
            return Err(RpcError::Cancelled);
        }
        let child_error = match &completion {
            Completion::Failed(error) => *error,
            _ => RpcError::Cancelled,
        };
        *state = completion;
        let children = core::mem::take(&mut *self.children.lock());
        let lease = self.priority_lease.lock().take();
        drop(state);
        // Revoke scheduling donation even if the receiver still holds its request.
        drop(lease);
        // The call tree is depth-bounded. Never retain a child or its server,
        // and never run cancellation under an endpoint/ancestor queue lock.
        for child in children.into_iter().filter_map(|child| child.upgrade()) {
            let _ = child.finish(Completion::Failed(child_error));
        }
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
    /// Only callbacks addressed to this precise outstanding call can reenter
    /// its waiter. Ordinary loopers cannot steal them from the process queue.
    pub fn reentrant_ready(&self, endpoint: &RpcEndpoint) -> bool {
        self.state
            .caller
            .as_ref()
            .is_some_and(|caller| caller.ptr_eq(&Arc::downgrade(&endpoint.waker)))
            && !self.ready()
            && endpoint.ready_for(Some(&self.state))
    }
    pub fn receive_reentrant(
        &self,
        endpoint: &RpcEndpoint,
    ) -> Result<Option<RpcRequest>, RpcError> {
        if !self
            .state
            .caller
            .as_ref()
            .is_some_and(|caller| caller.ptr_eq(&Arc::downgrade(&endpoint.waker)))
        {
            return Err(RpcError::Cancelled);
        }
        if self.ready() {
            return Ok(None);
        }
        endpoint.receive_for(Some(&self.state))
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
    sender: Option<RpcSender>,
    object_key: Option<u64>,
    reply: Arc<CallState>,
    async_gate: Option<Arc<AsyncGate>>,
    delivered: bool,
    route: Option<Weak<CallState>>,
}

impl RpcRequest {
    pub fn sender(&self) -> Option<&RpcSender> {
        self.sender.as_ref()
    }
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
    pub fn priority(&self) -> u8 {
        self.reply.priority
    }
    /// A delivered request owns one cancellable scheduling donation. Completion,
    /// peer close and caller cancellation revoke it before waking the waiter.
    pub fn enter_priority(&self, task: &crate::task::Task) -> Result<(), RpcError> {
        if self.priority() == 0 {
            return Ok(());
        }
        let lease = crate::task::fixed_priority::PriorityLease::enter(task, self.priority())
            .map_err(|_| RpcError::Busy)?;
        let pending = self.reply.completion.lock();
        if !matches!(*pending, Completion::Pending) {
            return Err(RpcError::Cancelled);
        }
        let mut slot = self.reply.priority_lease.lock();
        if slot.is_some() {
            return Err(RpcError::Busy);
        }
        *slot = lease;
        Ok(())
    }
    pub fn context(&self) -> Option<RpcContext> {
        (!self.is_oneway() && self.delivered).then(|| RpcContext {
            state: Arc::downgrade(&self.reply),
        })
    }
    fn routed_to(&self, route: Option<&Arc<CallState>>) -> bool {
        match (&self.route, route) {
            (None, None) => true,
            (Some(target), Some(call)) => target.ptr_eq(&Arc::downgrade(call)),
            _ => false,
        }
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
    outgoing: Vec<Weak<CallState>>,
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
                outgoing: Vec::new(),
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
    pub fn send_scoped(
        &self,
        message: RpcMessage,
        caller: &RpcEndpoint,
        parent: Option<&RpcContext>,
    ) -> Result<RpcCall, RpcError> {
        self.send_chained(None, message, false, Some(caller), parent, 0, None)
            .map(|state| RpcCall { state })
    }
    pub(crate) fn send_scoped_from(
        &self,
        task: &crate::task::Task,
        message: RpcMessage,
        caller: &RpcEndpoint,
        parent: Option<&RpcContext>,
    ) -> Result<RpcCall, RpcError> {
        self.send_chained(
            None,
            message,
            false,
            Some(caller),
            parent,
            task.fixed_priority.effective(),
            Some(task),
        )
        .map(|state| RpcCall { state })
    }
    pub(crate) fn send_oneway_from(
        &self,
        task: &crate::task::Task,
        message: RpcMessage,
    ) -> Result<(), RpcError> {
        self.send_chained(None, message, true, None, None, 0, Some(task))
            .map(|_| ())
    }
    pub fn create_object(self: &Arc<Self>) -> Result<Arc<RpcObject>, RpcError> {
        self.create_object_with_scheduling(0, false)
    }
    pub fn create_object_with_scheduling(
        self: &Arc<Self>,
        minimum_priority: u8,
        inherit_priority: bool,
    ) -> Result<Arc<RpcObject>, RpcError> {
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
            minimum_priority,
            inherit_priority,
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
        self.send_chained(object, message, oneway, None, None, 0, None)
    }
    fn send_chained(
        &self,
        object: Option<&RpcObject>,
        message: RpcMessage,
        oneway: bool,
        caller: Option<&RpcEndpoint>,
        context: Option<&RpcContext>,
        sender_priority: u8,
        task: Option<&crate::task::Task>,
    ) -> Result<Arc<CallState>, RpcError> {
        message.validate()?;
        let sender = match task {
            Some(task) => Some(RpcSender::capture(task)),
            None => crate::task::mytask().map(|task| RpcSender::capture(&task)),
        };
        let parent = context
            .map(|context| context.state.upgrade().ok_or(RpcError::Cancelled))
            .transpose()?;
        if let Some(parent) = &parent {
            if caller.is_none_or(|caller| !parent.receiver.ptr_eq(&Arc::downgrade(&caller.waker))) {
                return Err(RpcError::Cancelled);
            }
            if parent.depth >= MAX_CALL_DEPTH {
                return Err(RpcError::TooLarge);
            }
        }
        let priority = object.map_or(0, |object| {
            object
                .minimum_priority
                .max(if object.inherit_priority && !oneway {
                    sender_priority.max(parent.as_ref().map_or(0, |parent| parent.priority))
                } else {
                    0
                })
        });
        self.purge_cancelled();
        let mut state = self.state.lock();
        if state.closed || object.is_some_and(|object| object.revoked.load(Ordering::Acquire)) {
            return Err(RpcError::PeerClosed);
        }
        let object_key = object.map(RpcObject::key);
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
            receiver: Arc::downgrade(&self.waker),
            caller: caller.map(|caller| Arc::downgrade(&caller.waker)),
            parent: parent.as_ref().map(Arc::downgrade),
            children: IrqSpinLock::new(Vec::new()),
            depth: parent.as_ref().map_or(1, |parent| parent.depth + 1),
            priority,
            priority_lease: IrqSpinLock::new(None),
        });
        if let Some(parent) = &parent {
            // Admission and parent completion serialize on the same lock.
            // An endpoint close/caller exit either sees this child or prevents it.
            let pending = parent.completion.lock();
            if !matches!(*pending, Completion::Pending) {
                return Err(RpcError::Cancelled);
            }
            let mut children = parent.children.lock();
            children.retain(|child| child.strong_count() != 0);
            if children.len() >= MAX_IN_FLIGHT {
                return Err(RpcError::Busy);
            }
            children.push(Arc::downgrade(&call));
        }
        let mut route = None;
        let mut ancestor = Some(call.clone());
        while let Some(current) = ancestor {
            if current
                .caller
                .as_ref()
                .is_some_and(|caller| caller.ptr_eq(&Arc::downgrade(&self.waker)))
            {
                route = Some(Arc::downgrade(&current));
                break;
            }
            ancestor = current.parent.as_ref().and_then(Weak::upgrade);
        }
        let routed_waker = route
            .as_ref()
            .and_then(Weak::upgrade)
            .map(|call| call.waker.clone());
        // Register with the caller before publishing. Never lock two endpoint
        // queues together: simultaneous A->B/B->A admissions must not deadlock.
        drop(state);
        if let Some(caller) = caller {
            let mut caller_state = caller.state.lock();
            if caller_state.closed {
                return Err(RpcError::PeerClosed);
            }
            caller_state
                .outgoing
                .retain(|call| call.strong_count() != 0);
            if caller_state.outgoing.len() >= MAX_IN_FLIGHT {
                return Err(RpcError::Busy);
            }
            caller_state.outgoing.push(Arc::downgrade(&call));
        }
        let mut state = self.state.lock();
        if state.closed || object.is_some_and(|object| object.revoked.load(Ordering::Acquire)) {
            return Err(RpcError::PeerClosed);
        }
        if !matches!(*call.completion.lock(), Completion::Pending) {
            return Err(RpcError::Cancelled);
        }
        state.queue.push_back(RpcRequest {
            message,
            sender,
            object_key,
            reply: call.clone(),
            async_gate: oneway
                .then(|| object.map_or_else(|| self.async_gate.clone(), |o| o.async_gate.clone())),
            delivered: false,
            route,
        });
        state.calls.push(Arc::downgrade(&call));
        drop(state);
        // Adapters can multiplex idle receivers and outstanding callers on
        // this source. A wake must reach every distinct readiness predicate.
        self.waker.wake_all();
        if let Some(waker) = routed_waker {
            waker.wake_all();
        }
        Ok(call)
    }
    pub fn receive(&self) -> Result<Option<RpcRequest>, RpcError> {
        self.receive_for(None)
    }
    fn receive_for(&self, route: Option<&Arc<CallState>>) -> Result<Option<RpcRequest>, RpcError> {
        self.purge_cancelled();
        let mut state = self.state.lock();
        if let Some(index) = state.queue.iter().position(|request| {
            !request.cancelled()
                && request.routed_to(route)
                && request
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
        self.ready_for(None)
    }
    fn ready_for(&self, route: Option<&Arc<CallState>>) -> bool {
        let state = self.state.lock();
        state.closed
            || state.queue.iter().any(|request| {
                !request.cancelled()
                    && request.routed_to(route)
                    && request
                        .async_gate
                        .as_ref()
                        .is_none_or(|gate| !gate.active.load(Ordering::Acquire))
            })
    }
    pub fn waker(&self) -> Arc<Waker> {
        self.waker.clone()
    }
    fn purge_cancelled(&self) {
        let removed = {
            let mut state = self.state.lock();
            let mut removed = VecDeque::new();
            let mut kept = VecDeque::new();
            while let Some(request) = state.queue.pop_front() {
                if request.cancelled() {
                    removed.push_back(request);
                } else {
                    kept.push_back(request);
                }
            }
            state.queue = kept;
            removed
        };
        // Capability destructors may close sockets and wake another endpoint.
        drop(removed);
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
        let (queue, calls, outgoing, objects) = {
            let mut state = self.state.lock();
            state.closed = true;
            (
                core::mem::take(&mut state.queue),
                core::mem::take(&mut state.calls),
                core::mem::take(&mut state.outgoing),
                core::mem::take(&mut state.objects),
            )
        };
        // Dropping reply capabilities wakes their callers outside the queue lock.
        drop(queue);
        for call in calls.into_iter().chain(outgoing) {
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
    fn sender_task(
        policy: &Arc<crate::shield::Policy>,
        label: u32,
        principal: u32,
    ) -> crate::task::Task {
        let task = crate::task::Task::new("rpc-sender".into(), 0, crate::task::TaskType::User);
        task.credentials
            .write()
            .set_principals(crate::shield::identity::Identities::all(
                crate::shield::identity::Principal(principal),
            ))
            .unwrap();
        *task.security_subject.write() = Some(policy.subject(label).unwrap());
        task
    }
    #[test_case]
    fn rpc_sender_is_an_admission_snapshot_outside_mutable_message_bytes() {
        let policy = crate::shield::Policy::new(
            alloc::vec![(1, "sender".into()), (2, "later".into())],
            alloc::vec![],
        )
        .unwrap();
        let task = sender_task(&policy, 1, 10042);
        let endpoint = RpcEndpoint::new();
        let caller = RpcEndpoint::new();
        let call = endpoint
            .send_scoped_from(&task, message(1), &caller, None)
            .unwrap();
        task.credentials
            .write()
            .set_principals(crate::shield::identity::Identities::all(
                crate::shield::identity::Principal(7),
            ))
            .unwrap();
        *task.security_subject.write() = Some(policy.subject(2).unwrap());
        drop(task);
        let mut request = endpoint.receive().unwrap().unwrap();
        request.message.bytes = alloc::vec![0;128];
        assert_eq!(request.sender().unwrap().principal().0, 10042);
        assert_eq!(
            request.sender().unwrap().subject().unwrap().name(),
            "sender"
        );
        assert!(Arc::ptr_eq(
            request.sender().unwrap().subject().unwrap().policy(),
            &policy
        ));
        request.reply(message(2)).unwrap();
        assert!(call.take_reply().unwrap().is_ok());
    }
    #[test_case]
    fn rpc_nested_and_oneway_senders_are_the_actual_broker_not_the_parent() {
        let policy = crate::shield::Policy::new(
            alloc::vec![(1, "client".into()), (2, "broker".into())],
            alloc::vec![],
        )
        .unwrap();
        let client = sender_task(&policy, 1, 10001);
        let broker = sender_task(&policy, 2, 1000);
        let a = RpcEndpoint::new();
        let b = RpcEndpoint::new();
        let c = RpcEndpoint::new();
        let target = c.create_object().unwrap();
        let outer = b.send_scoped_from(&client, message(1), &a, None).unwrap();
        let parent = b.receive().unwrap().unwrap();
        let inner = target
            .send_scoped_from(&broker, message(2), &b, parent.context().as_ref())
            .unwrap();
        let request = c.receive().unwrap().unwrap();
        assert_eq!(request.sender().unwrap().principal().0, 1000);
        assert_eq!(
            request.sender().unwrap().subject().unwrap().name(),
            "broker"
        );
        request.reply(message(3)).unwrap();
        assert!(inner.take_reply().unwrap().is_ok());
        parent.reply(message(4)).unwrap();
        assert!(outer.take_reply().unwrap().is_ok());
        target.send_oneway_from(&broker, message(5)).unwrap();
        let notice = c.receive().unwrap().unwrap();
        assert_eq!(notice.sender().unwrap().subject().unwrap().name(), "broker");
        drop(notice);
    }
    #[test_case]
    fn rpc_sender_labels_release_on_cancel_close_and_consumed_reply() {
        for complete in 0..3 {
            let policy =
                crate::shield::Policy::new(alloc::vec![(1, "sender".into())], alloc::vec![])
                    .unwrap();
            let weak = Arc::downgrade(&policy);
            let task = sender_task(&policy, 1, 42);
            let endpoint = RpcEndpoint::new();
            let caller = RpcEndpoint::new();
            let call = endpoint
                .send_scoped_from(&task, message(1), &caller, None)
                .unwrap();
            drop(task);
            drop(policy);
            assert!(weak.upgrade().is_some());
            match complete {
                0 => {
                    drop(call);
                    assert!(endpoint.receive().unwrap().is_none());
                }
                1 => {
                    endpoint.close();
                    assert_eq!(call.take_reply().unwrap().err(), Some(RpcError::PeerClosed));
                }
                _ => {
                    let request = endpoint.receive().unwrap().unwrap();
                    request.reply(message(2)).unwrap();
                    assert!(call.take_reply().unwrap().is_ok());
                }
            }
            assert!(weak.upgrade().is_none());
        }
    }
    #[test_case]
    fn rpc_scoped_chain_routes_callbacks_only_to_the_matching_waiter() {
        let a = RpcEndpoint::new();
        let b = RpcEndpoint::new();
        let c = RpcEndpoint::new();
        let outer = b.send_scoped(message(1), &a, None).unwrap();
        let unrelated = b.send_scoped(message(9), &a, None).unwrap();
        let at_b = b.receive().unwrap().unwrap();
        let middle = c
            .send_scoped(message(2), &b, at_b.context().as_ref())
            .unwrap();
        let at_c = c.receive().unwrap().unwrap();
        let callback = a.create_object().unwrap();
        let inner = callback
            .send_scoped(message(3), &c, at_c.context().as_ref())
            .unwrap();
        assert!(!a.ready() && a.receive().unwrap().is_none());
        assert!(!unrelated.reentrant_ready(&a));
        assert!(outer.reentrant_ready(&a));
        assert!(matches!(
            outer.receive_reentrant(&b),
            Err(RpcError::Cancelled)
        ));
        let at_a = outer.receive_reentrant(&a).unwrap().unwrap();
        assert_eq!(at_a.object_key(), Some(callback.key()));
        at_a.reply(message(4)).unwrap();
        assert_eq!(inner.take_reply().unwrap().unwrap().opcode, 4);
        at_c.reply(message(5)).unwrap();
        assert_eq!(middle.take_reply().unwrap().unwrap().opcode, 5);
        at_b.reply(message(6)).unwrap();
        assert_eq!(outer.take_reply().unwrap().unwrap().opcode, 6);
        assert!(!unrelated.ready());
    }

    #[test_case]
    fn rpc_parent_cancellation_and_peer_close_cancel_queued_and_received_descendants() {
        for close_peer in [false, true] {
            let a = RpcEndpoint::new();
            let b = RpcEndpoint::new();
            let c = RpcEndpoint::new();
            let root = b.send_scoped(message(1), &a, None).unwrap();
            let at_b = b.receive().unwrap().unwrap();
            let child = c
                .send_scoped(message(2), &b, at_b.context().as_ref())
                .unwrap();
            let at_c = c.receive().unwrap().unwrap();
            let callback = a
                .send_scoped(message(3), &c, at_c.context().as_ref())
                .unwrap();
            if close_peer {
                b.close();
            } else {
                drop(root);
            }
            let error = if close_peer {
                RpcError::PeerClosed
            } else {
                RpcError::Cancelled
            };
            assert_eq!(child.take_reply().unwrap().err(), Some(error));
            assert_eq!(callback.take_reply().unwrap().err(), Some(error));
            assert!(at_b.cancelled() && at_c.cancelled());
            assert!(a.receive().unwrap().is_none());
            assert!(matches!(
                c.send_scoped(message(4), &b, at_b.context().as_ref()),
                Err(RpcError::Cancelled)
            ));
            assert_eq!(at_c.reply(message(5)), Err(RpcError::Cancelled));
        }
    }

    #[test_case]
    fn rpc_scoped_depth_is_bounded_and_contexts_cannot_impersonate_a_receiver() {
        let a = RpcEndpoint::new();
        let b = RpcEndpoint::new();
        let stranger = RpcEndpoint::new();
        let mut calls = alloc::vec![b.send_scoped(message(1), &a, None).unwrap()];
        let mut requests = alloc::vec![b.receive().unwrap().unwrap()];
        assert!(matches!(
            a.send_scoped(message(2), &stranger, requests[0].context().as_ref()),
            Err(RpcError::Cancelled)
        ));
        for _ in 1..MAX_CALL_DEPTH {
            let call = b
                .send_scoped(message(2), &b, requests.last().unwrap().context().as_ref())
                .unwrap();
            assert!(b.receive().unwrap().is_none());
            let request = call.receive_reentrant(&b).unwrap().unwrap();
            calls.push(call);
            requests.push(request);
        }
        assert!(matches!(
            b.send_scoped(message(3), &b, requests.last().unwrap().context().as_ref()),
            Err(RpcError::TooLarge)
        ));
        for (request, call) in requests.into_iter().zip(calls).rev() {
            request.reply(message(4)).unwrap();
            assert_eq!(call.take_reply().unwrap().unwrap().opcode, 4);
        }
        let weak = Arc::downgrade(&b);
        drop(b);
        assert!(weak.upgrade().is_none());
    }

    #[test_case]
    fn rpc_caller_close_cancels_its_calls_and_its_admission_limit_has_no_side_effects() {
        let caller = RpcEndpoint::new();
        let destinations: Vec<_> = (0..MAX_IN_FLIGHT + 1).map(|_| RpcEndpoint::new()).collect();
        let mut calls = Vec::new();
        for destination in destinations.iter().take(MAX_IN_FLIGHT) {
            calls.push(destination.send_scoped(message(1), &caller, None).unwrap());
        }
        assert!(matches!(
            destinations[MAX_IN_FLIGHT].send_scoped(message(2), &caller, None),
            Err(RpcError::Busy)
        ));
        assert!(destinations[MAX_IN_FLIGHT].receive().unwrap().is_none());
        caller.close();
        for call in calls {
            assert_eq!(call.take_reply().unwrap().err(), Some(RpcError::PeerClosed));
        }
        for destination in destinations {
            assert!(destination.receive().unwrap().is_none());
        }
    }

    #[test_case]
    fn rpc_cancellation_at_maximum_depth_unwinds_without_retaining_endpoints() {
        let a = RpcEndpoint::new();
        let b = RpcEndpoint::new();
        let mut calls = alloc::vec![b.send_scoped(message(1), &a, None).unwrap()];
        let mut requests = alloc::vec![b.receive().unwrap().unwrap()];
        for _ in 1..MAX_CALL_DEPTH {
            let call = b
                .send_scoped(message(2), &b, requests.last().unwrap().context().as_ref())
                .unwrap();
            requests.push(call.receive_reentrant(&b).unwrap().unwrap());
            calls.push(call);
        }
        drop(calls.remove(0));
        for call in calls {
            assert_eq!(call.take_reply().unwrap().err(), Some(RpcError::Cancelled));
        }
        assert!(requests.iter().all(RpcRequest::cancelled));
        drop(requests);
        assert!(b.receive().unwrap().is_none());
        assert!(b.send(message(3)).is_ok());
    }

    #[test_case]
    fn rpc_priority_donations_obey_object_contracts_and_are_revoked_on_cancellation() {
        let endpoint = RpcEndpoint::new();
        let caller = RpcEndpoint::new();
        let object = endpoint.create_object_with_scheduling(7, true).unwrap();
        let receiver =
            crate::task::Task::new("rpc-priority".into(), 0, crate::task::TaskType::Kernel);
        receiver.fixed_priority.configure(3);
        let call = object
            .send_scoped_with_priority(message(1), &caller, None, 9)
            .unwrap();
        let request = endpoint.receive().unwrap().unwrap();
        assert_eq!(request.priority(), 9);
        request.enter_priority(&receiver).unwrap();
        assert_eq!(receiver.fixed_priority.effective(), 9);
        drop(call);
        assert_eq!(receiver.fixed_priority.effective(), 3);
        assert!(request.cancelled());
        drop(request);
        object.send_oneway(message(2)).unwrap();
        let request = endpoint.receive().unwrap().unwrap();
        assert_eq!(request.priority(), 7);
        request.enter_priority(&receiver).unwrap();
        assert_eq!(receiver.fixed_priority.effective(), 7);
        drop(request);
        assert_eq!(receiver.fixed_priority.effective(), 3);
        let minimum_only = endpoint.create_object_with_scheduling(5, false).unwrap();
        let call = minimum_only
            .send_scoped_with_priority(message(3), &caller, None, 9)
            .unwrap();
        let request = endpoint.receive().unwrap().unwrap();
        assert_eq!(request.priority(), 5);
        request.enter_priority(&receiver).unwrap();
        request.reply(message(4)).unwrap();
        assert_eq!(receiver.fixed_priority.effective(), 3);
        assert!(call.take_reply().unwrap().is_ok());
    }

    #[test_case]
    fn rpc_native_capabilities_preserve_access_and_follow_reply_ownership() {
        use crate::object::handle::AccessMode;
        let endpoint = RpcEndpoint::new();
        let memory = Arc::new(crate::ipc::SharedMemory::new(4096, 1).unwrap());
        let weak = Arc::downgrade(&memory);
        let mut sent = message(1);
        sent.capabilities.push((
            KernelObject::SharedMemory(memory),
            HandleMetadata {
                access_mode: AccessMode::ReadOnly,
                ..HandleMetadata::default()
            },
        ));
        let call = endpoint.send(sent).unwrap();
        let request = endpoint.receive().unwrap().unwrap();
        assert_eq!(
            request.message.capabilities[0].1.access_mode,
            AccessMode::ReadOnly
        );
        let mut reply = message(2);
        reply.capabilities = request.message.capabilities.clone();
        request.reply(reply).unwrap();
        assert!(weak.upgrade().is_some());
        let reply = call.take_reply().unwrap().unwrap();
        drop(call);
        let forwarded = reply.clone();
        drop(reply);
        assert!(weak.upgrade().is_some());
        drop(forwarded);
        assert!(weak.upgrade().is_none());
    }
    #[test_case]
    fn rpc_capability_bounds_include_both_dispatch_and_native_objects() {
        let endpoint = RpcEndpoint::new();
        let object = endpoint.create_object().unwrap();
        let memory = Arc::new(crate::ipc::SharedMemory::new(4096, 1).unwrap());
        let mut sent = message(1);
        sent.objects = alloc::vec![object;MAX_MESSAGE_OBJECTS];
        sent.capabilities.push((
            KernelObject::SharedMemory(memory.clone()),
            HandleMetadata::default(),
        ));
        assert!(matches!(endpoint.send(sent), Err(RpcError::TooLarge)));
        assert_eq!(Arc::strong_count(&memory), 1);
        let mut sent = message(2);
        sent.capabilities = alloc::vec![(KernelObject::SharedMemory(memory.clone()),HandleMetadata::default());MAX_MESSAGE_OBJECTS];
        let call = endpoint.send(sent).unwrap();
        endpoint.close();
        assert!(matches!(call.take_reply(), Some(Err(RpcError::PeerClosed))));
        assert_eq!(Arc::strong_count(&memory), 1);
    }
    #[test_case]
    fn rpc_cancelled_capabilities_drop_outside_the_endpoint_lock() {
        struct OnDrop {
            endpoint: Weak<RpcEndpoint>,
            checked: Arc<AtomicBool>,
        }
        impl Drop for OnDrop {
            fn drop(&mut self) {
                let endpoint = self.endpoint.upgrade().unwrap();
                assert!(endpoint.state.try_lock().is_some());
                self.checked.store(true, Ordering::Release);
            }
        }
        for through_send in [false, true] {
            let endpoint = RpcEndpoint::new();
            let memory = Arc::new(crate::ipc::SharedMemory::new(4096, 1).unwrap());
            let weak = Arc::downgrade(&memory);
            let checked = Arc::new(AtomicBool::new(false));
            let mut sent = message(1);
            sent.capabilities.push((
                KernelObject::SharedMemory(memory),
                HandleMetadata::default(),
            ));
            sent.retained = Some(Arc::new(OnDrop {
                endpoint: Arc::downgrade(&endpoint),
                checked: checked.clone(),
            }));
            drop(endpoint.send(sent).unwrap());
            if through_send {
                drop(endpoint.send(message(2)).unwrap());
            } else {
                assert!(endpoint.receive().unwrap().is_none());
            }
            assert!(checked.load(Ordering::Acquire));
            assert!(weak.upgrade().is_none());
        }
    }
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
            capabilities: Vec::new(),
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
