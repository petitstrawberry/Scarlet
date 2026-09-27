//! ABI-independent connections between notification counters and virtual IRQs.
//!
//! A level event stays asserted until the interrupt controller reports guest
//! deactivation. Completion lowers the line before notifying every connected
//! device to re-evaluate its state. No Linux descriptor or GSI lives here.

use crate::ipc::counter::{CounterObject, CounterSubscription, CounterWriteListener};
use crate::sync::IrqSpinLock;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IrqRoute {
    pub vcpu: u32,
    pub interrupt: u32,
}

/// Implementations must not call back into the event registry or sleep.
pub trait InterruptTarget: Send + Sync {
    fn set_level(&self, route: IrqRoute, asserted: bool);
    fn pulse(&self, route: IrqRoute) -> bool;
}

struct Connection {
    id: u64,
    route: IrqRoute,
    target: Arc<dyn InterruptTarget>,
    trigger: Arc<dyn CounterObject>,
    resample: Option<Arc<dyn CounterObject>>,
    asserted: bool,
    _subscription: CounterSubscription,
}

struct State {
    next_id: u64,
    connections: Vec<Connection>,
}

pub struct InterruptEvents {
    state: IrqSpinLock<State>,
}

struct Trigger {
    events: Weak<InterruptEvents>,
    id: u64,
}

impl CounterWriteListener for Trigger {
    fn on_counter_write(&self, _: u64) {
        if let Some(events) = self.events.upgrade() {
            events.trigger(self.id);
        }
    }
}

impl InterruptEvents {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: IrqSpinLock::new(State {
                next_id: 1,
                connections: Vec::new(),
            }),
        })
    }

    pub fn bind(
        self: &Arc<Self>,
        route: IrqRoute,
        target: Arc<dyn InterruptTarget>,
        trigger: Arc<dyn CounterObject>,
        resample: Option<Arc<dyn CounterObject>>,
    ) -> Result<(), &'static str> {
        if resample
            .as_ref()
            .is_some_and(|counter| counter.identity() == trigger.identity())
        {
            return Err("Trigger and resample counters must differ");
        }
        let id =
            {
                let mut state = self.state.lock();
                if state
                    .connections
                    .iter()
                    .any(|entry| entry.trigger.identity() == trigger.identity())
                {
                    return Err("Counter already connected");
                }
                if state.connections.iter().any(|entry| {
                    entry.route == route && entry.resample.is_some() != resample.is_some()
                }) {
                    return Err("Cannot mix edge and level event sources on one line");
                }
                let id = state.next_id;
                state.next_id = id.checked_add(1).ok_or("IRQ event IDs exhausted")?;
                // Publish while holding the registry lock: a concurrent callback
                // cannot observe an unregistered connection and lose the write.
                let subscription = trigger.subscribe_write(Arc::new(Trigger {
                    events: Arc::downgrade(self),
                    id,
                }));
                state.connections.push(Connection {
                    id,
                    route,
                    target,
                    trigger,
                    resample,
                    asserted: false,
                    _subscription: subscription,
                });
                id
            };
        self.trigger(id); // Consume notifications written before registration.
        Ok(())
    }

    fn trigger(&self, id: u64) {
        let mut state = self.state.lock();
        let Some(entry) = state.connections.iter_mut().find(|entry| entry.id == id) else {
            return;
        };
        if entry.trigger.take_notifications() == 0 {
            return;
        }
        if entry.resample.is_some() {
            entry.asserted = true;
            entry.target.set_level(entry.route, true);
        } else {
            entry.asserted = !entry.target.pulse(entry.route);
        }
    }

    /// Invalidate the connection before detaching its observer. A writer that
    /// already captured the old callback finds no matching ID and does nothing.
    pub fn unbind(&self, route: IrqRoute, trigger: &dyn CounterObject) {
        let mut state = self.state.lock();
        if let Some(index) = state.connections.iter().position(|entry| {
            entry.route == route && entry.trigger.identity() == trigger.identity()
        }) {
            let entry = state.connections.remove(index);
            if entry.resample.is_some()
                && !state
                    .connections
                    .iter()
                    .any(|other| other.route == route && other.asserted)
            {
                entry.target.set_level(route, false);
            }
        }
    }

    /// Capture the connection generation when a level IRQ enters a list register.
    /// Newly bound devices must not receive completion of an older delivery.
    pub fn delivery_generation(&self, route: IrqRoute) -> u64 {
        let state = self.state.lock();
        Self::generation(&state, route)
    }

    /// Serialize LR publication with trigger/deassign, including the target's
    /// pending-bit check. The callback must not sleep or re-enter this registry.
    pub fn with_delivery(&self, route: IrqRoute, deliver: impl FnOnce(u64)) {
        let state = self.state.lock();
        deliver(Self::generation(&state, route));
    }

    fn generation(state: &State, route: IrqRoute) -> u64 {
        if !state
            .connections
            .iter()
            .any(|entry| entry.route == route && entry.resample.is_some() && entry.asserted)
        {
            return 0;
        }
        state
            .connections
            .iter()
            .filter(|entry| entry.route == route && entry.resample.is_some())
            .map(|entry| entry.id)
            .max()
            .unwrap_or(0)
    }

    /// Called only for a consumed software LR, not for a pending/active LR or
    /// a line cleared by the host. Notification callbacks run without our lock.
    pub fn complete(&self, route: IrqRoute, generation: u64) {
        if generation == 0 {
            return;
        }
        let notifications = {
            let mut state = self.state.lock();
            let mut notifications = Vec::new();
            let mut target = None;
            if !state.connections.iter().any(|entry| {
                entry.route == route
                    && entry.id <= generation
                    && entry.resample.is_some()
                    && entry.asserted
            }) {
                return;
            }
            for entry in state.connections.iter_mut().filter(|entry| {
                entry.route == route && entry.id <= generation && entry.resample.is_some()
            }) {
                entry.asserted = false;
                target = Some(entry.target.clone());
                notifications.push(entry.resample.as_ref().unwrap().clone());
            }
            if let Some(target) = target {
                // A source attached after this delivery can still hold the line.
                let asserted = state
                    .connections
                    .iter()
                    .any(|entry| entry.route == route && entry.asserted);
                target.set_level(route, asserted);
            }
            notifications
        };
        // These completions were committed before any concurrent unbind. The
        // owned counters keep in-flight notifications safe after handle closure.
        for counter in notifications {
            counter.notify();
        }
    }

    /// Restore asserted levels when a vCPU is created after its event bindings.
    pub fn replay_levels(&self, vcpu: u32) {
        let mut state = self.state.lock();
        for entry in state
            .connections
            .iter_mut()
            .filter(|entry| entry.route.vcpu == vcpu && entry.asserted)
        {
            if entry.resample.is_some() {
                entry.target.set_level(entry.route, true);
            } else {
                entry.asserted = !entry.target.pulse(entry.route);
            }
        }
    }
}

impl Drop for InterruptEvents {
    fn drop(&mut self) {
        for entry in self.state.lock().connections.drain(..) {
            if entry.resample.is_some() {
                entry.target.set_level(entry.route, false);
            }
        }
    }
}

/// VM ownership remains independent of an externally held notification counter.
pub struct VmInterruptTarget(pub Weak<crate::arch::hv::Vm>);

impl InterruptTarget for VmInterruptTarget {
    fn set_level(&self, route: IrqRoute, asserted: bool) {
        if let Some(vm) = self.0.upgrade()
            && let Some(vcpu) = vm.get_vcpu(route.vcpu)
        {
            vcpu.set_irq_line(route.interrupt, asserted);
        }
    }
    fn pulse(&self, route: IrqRoute) -> bool {
        if let Some(vm) = self.0.upgrade()
            && let Some(vcpu) = vm.get_vcpu(route.vcpu)
        {
            vcpu.trigger_irq(route.interrupt);
            true
        } else {
            false
        }
    }
}

/// Native VM control entry point. Counter handles use ordinary StreamOps and
/// Selectable, so U-SHV can integrate them into its existing event loop.
pub fn native_control(vm_id: u32, command: u32, arg: usize) -> Result<i32, &'static str> {
    use crate::hypervisor::vm::{GLOBAL_VM_MANAGER, VmObject};
    use crate::ipc::counter::Counter;
    use crate::library::std::usercopy::{copy_from_user, copy_to_user};
    use crate::object::KernelObject;
    use crate::object::capability::selectable::Selectable;
    use scarlet_abi::hypervisor::*;

    let task = crate::task::mytask().ok_or("No current task")?;
    let mut bytes = [0u8; core::mem::size_of::<VmIrqEvent>()];
    copy_from_user(&task, arg, &mut bytes).map_err(|_| "Invalid IRQ event record")?;
    // SAFETY: this fixed record consists only of u32 fields, with no padding.
    let mut request = unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast::<VmIrqEvent>()) };
    if request.reserved != 0 || request.flags & !(IRQ_EVENT_RESAMPLE | IRQ_EVENT_NONBLOCK) != 0 {
        return Err("Invalid IRQ event flags");
    }
    let vm = GLOBAL_VM_MANAGER
        .get_vm_by_id(vm_id)
        .ok_or("VM no longer exists")?;
    let route = IrqRoute {
        vcpu: request.vcpu,
        interrupt: request.interrupt,
    };
    if request.vcpu != 0 {
        return Err("Only vCPU 0 IRQ events are currently supported");
    }
    #[cfg(target_arch = "aarch64")]
    if !(32..256).contains(&route.interrupt) {
        return Err("Unsupported SPI ID");
    }
    if command == VM_REMOVE_IRQ_EVENT {
        let object = task
            .handle_table
            .get_arc_clone(request.trigger)
            .ok_or("Invalid trigger handle")?;
        let KernelObject::Counter(trigger) = object else {
            return Err("Not a counter");
        };
        vm.irq_events().unbind(route, trigger.as_ref());
        return Ok(0);
    }
    if command != VM_CREATE_IRQ_EVENT || request.trigger != 0 || request.resample != 0 {
        return Err("Invalid IRQ event request");
    }
    let level = request.flags & IRQ_EVENT_RESAMPLE != 0;
    if level && !cfg!(target_arch = "aarch64") {
        return Err("Guest IRQ completion is not supported");
    }
    let trigger = Arc::new(Counter::new(0, false));
    trigger.set_nonblocking(request.flags & IRQ_EVENT_NONBLOCK != 0);
    let resample = level.then(|| Arc::new(Counter::new(0, false)));
    if let Some(counter) = &resample {
        counter.set_nonblocking(request.flags & IRQ_EVENT_NONBLOCK != 0);
    }
    request.trigger = task
        .handle_table
        .insert(KernelObject::from_counter(trigger.clone()))
        .map_err(|_| "No free handle")?;
    if let Some(counter) = &resample {
        match task
            .handle_table
            .insert(KernelObject::from_counter(counter.clone()))
        {
            Ok(handle) => request.resample = handle,
            Err(_) => {
                task.handle_table.remove(request.trigger);
                return Err("No free handle");
            }
        }
    }
    let result = vm.irq_events().bind(
        route,
        Arc::new(VmInterruptTarget(Arc::downgrade(&vm))),
        trigger.clone(),
        resample.map(|counter| counter as Arc<dyn CounterObject>),
    );
    let result = result.and_then(|()| {
        // SAFETY: every byte in this all-u32 record is initialized.
        let output = unsafe {
            core::slice::from_raw_parts(
                (&request as *const VmIrqEvent).cast::<u8>(),
                core::mem::size_of::<VmIrqEvent>(),
            )
        };
        copy_to_user(&task, arg, output).map_err(|_| "Invalid IRQ event output")
    });
    if result.is_err() {
        vm.irq_events().unbind(route, trigger.as_ref());
        task.handle_table.remove(request.trigger);
        if level {
            task.handle_table.remove(request.resample);
        }
    }
    result.map(|()| 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::counter::Counter;
    use crate::object::capability::StreamOps;
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct Target {
        level: AtomicBool,
        pulses: AtomicUsize,
    }
    impl InterruptTarget for Target {
        fn set_level(&self, _: IrqRoute, level: bool) {
            self.level.store(level, Ordering::SeqCst);
        }
        fn pulse(&self, _: IrqRoute) -> bool {
            self.pulses.fetch_add(1, Ordering::SeqCst);
            true
        }
    }
    fn target() -> Arc<Target> {
        Arc::new(Target {
            level: AtomicBool::new(false),
            pulses: AtomicUsize::new(0),
        })
    }
    const ROUTE: IrqRoute = IrqRoute {
        vcpu: 0,
        interrupt: 40,
    };
    fn signal(counter: &Counter) {
        assert_eq!(counter.write(&1u64.to_ne_bytes()).unwrap(), 8);
    }

    #[test_case]
    fn irq_resample_deasserts_then_notifies_and_can_rearm() {
        let events = InterruptEvents::new();
        let target = target();
        let trigger = Arc::new(Counter::new(0, false));
        let resample = Arc::new(Counter::new(0, false));
        // An event written before binding must also assert the interrupt.
        signal(&trigger);
        events
            .bind(
                ROUTE,
                target.clone(),
                trigger.clone(),
                Some(resample.clone()),
            )
            .unwrap();
        assert!(target.level.load(Ordering::SeqCst));
        assert_eq!(trigger.take_notifications(), 0);
        assert_eq!(resample.take_notifications(), 0);
        let generation = events.delivery_generation(ROUTE);
        assert_ne!(generation, 0);
        events.complete(ROUTE, generation);
        assert!(!target.level.load(Ordering::SeqCst));
        assert_eq!(resample.take_notifications(), 1);
        events.complete(ROUTE, generation);
        assert_eq!(resample.take_notifications(), 0);
        signal(&trigger);
        assert!(target.level.load(Ordering::SeqCst));
        events.complete(ROUTE, events.delivery_generation(ROUTE));
        assert_eq!(resample.take_notifications(), 1);
        assert!(!target.level.load(Ordering::SeqCst));
    }

    #[test_case]
    fn irq_resample_shared_line_and_deassign_by_counter_identity() {
        let events = InterruptEvents::new();
        let target = target();
        let first = Arc::new(Counter::new(0, false));
        let second = Arc::new(Counter::new(0, false));
        let first_ack = Arc::new(Counter::new(0, false));
        let second_ack = Arc::new(Counter::new(0, false));
        events
            .bind(
                ROUTE,
                target.clone(),
                first.clone(),
                Some(first_ack.clone()),
            )
            .unwrap();
        events
            .bind(
                ROUTE,
                target.clone(),
                second.clone(),
                Some(second_ack.clone()),
            )
            .unwrap();
        signal(&first);
        let generation = events.delivery_generation(ROUTE);
        // A shared device that asserts after delivery still needs resampling.
        signal(&second);
        events.complete(ROUTE, generation);
        assert!(!target.level.load(Ordering::SeqCst));
        assert_eq!(first_ack.take_notifications(), 1);
        assert_eq!(second_ack.take_notifications(), 1);
        signal(&first);
        signal(&second);
        let duplicate = first.as_ref().clone();
        events.unbind(ROUTE, &duplicate);
        assert!(target.level.load(Ordering::SeqCst));
        events.complete(ROUTE, events.delivery_generation(ROUTE));
        assert_eq!(first_ack.take_notifications(), 0);
        assert_eq!(second_ack.take_notifications(), 1);
        signal(&first);
        assert!(!target.level.load(Ordering::SeqCst));
        assert_eq!(first.take_notifications(), 1);
    }

    #[test_case]
    fn irq_resample_old_completion_and_captured_callback_do_not_reach_new_binding() {
        let events = InterruptEvents::new();
        let target = target();
        let trigger = Arc::new(Counter::new(0, false));
        let ack = Arc::new(Counter::new(0, false));
        events
            .bind(ROUTE, target.clone(), trigger.clone(), Some(ack.clone()))
            .unwrap();
        signal(&trigger);
        let old = events.delivery_generation(ROUTE);
        events.unbind(ROUTE, trigger.as_ref());
        assert!(!target.level.load(Ordering::SeqCst));
        events
            .bind(ROUTE, target.clone(), trigger.clone(), Some(ack.clone()))
            .unwrap();
        // Simulate a writer holding a callback snapshot from before unbind.
        events.trigger(old);
        assert!(!target.level.load(Ordering::SeqCst));
        signal(&trigger);
        events.complete(ROUTE, old);
        assert!(target.level.load(Ordering::SeqCst));
        assert_eq!(ack.take_notifications(), 0);
        events.complete(ROUTE, events.delivery_generation(ROUTE));
        assert_eq!(ack.take_notifications(), 1);
        signal(&trigger);
        drop(events);
        assert!(!target.level.load(Ordering::SeqCst));
        signal(&trigger);
        assert!(!target.level.load(Ordering::SeqCst));
    }

    #[test_case]
    fn irq_events_keep_edge_mode_and_reject_invalid_connections() {
        let events = InterruptEvents::new();
        let target = target();
        let trigger = Arc::new(Counter::new(0, false));
        assert!(
            events
                .bind(
                    ROUTE,
                    target.clone(),
                    trigger.clone(),
                    Some(trigger.clone())
                )
                .is_err()
        );
        events
            .bind(ROUTE, target.clone(), trigger.clone(), None)
            .unwrap();
        assert!(
            events
                .bind(ROUTE, target.clone(), trigger.clone(), None)
                .is_err()
        );
        signal(&trigger);
        assert_eq!(target.pulses.load(Ordering::SeqCst), 1);
        assert_eq!(trigger.take_notifications(), 0);
        assert_eq!(events.delivery_generation(ROUTE), 0);
        events.unbind(ROUTE, trigger.as_ref());
        signal(&trigger);
        assert_eq!(target.pulses.load(Ordering::SeqCst), 1);
    }
}
