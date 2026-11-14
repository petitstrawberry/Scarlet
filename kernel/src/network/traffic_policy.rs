//! Packet-evaluation counters and ordered admission rules. Unlike transmit
//! reservations, these count an evaluation even when a later rule or device
//! rejects the packet. Objects are explicitly shared; there is no global policy.
use super::traffic::{Counter, Direction};
use crate::{
    object::capability::{ReadyInterest, ReadySet, SelectWaitOutcome, Selectable},
    sync::{IrqSpinLock, Waker},
};
use alloc::{collections::VecDeque, sync::Arc, vec::Vec};

const MAX_EVENTS: usize = 64;
pub const MAX_RULES: usize = 128;

/// Whether a packet exactly using the remaining allowance is within it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boundary {
    Inclusive,
    Exclusive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exhaustion {
    pub generation: u64,
    pub direction: Direction,
    pub bytes: u64,
    pub remaining_before: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Exhausted(Exhaustion),
    Overflow,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub generation: u64,
    pub remaining: u64,
    pub received: Counter,
    pub transmitted: Counter,
    pub exhaustions: u64,
}

struct State {
    snapshot: Snapshot,
    events: VecDeque<Event>,
    overflow: bool,
}
pub struct Meter {
    state: IrqSpinLock<State>,
    waker: Waker,
}

impl Meter {
    pub fn new(remaining: u64) -> Arc<Self> {
        Arc::new(Self {
            state: IrqSpinLock::new(State {
                snapshot: Snapshot {
                    generation: 1,
                    remaining,
                    ..Default::default()
                },
                events: VecDeque::with_capacity(MAX_EVENTS),
                overflow: false,
            }),
            waker: Waker::new_interruptible("traffic_meter"),
        })
    }

    /// Re-arm without discarding already generated notifications or accounting.
    /// The generation identifies which setting a delayed event belongs to.
    pub fn reset(&self, remaining: u64) -> Result<(), &'static str> {
        let mut state = self.state.lock();
        let generation = state
            .snapshot
            .generation
            .checked_add(1)
            .ok_or("meter generation exhausted")?;
        state.snapshot.generation = generation;
        state.snapshot.remaining = remaining;
        Ok(())
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().snapshot
    }

    /// Returns whether this nonempty packet is inside the remaining allowance.
    /// An overlarge packet exhausts the counter too; smaller subsequent packets
    /// cannot slip through. Notification wakes happen after releasing the lock.
    fn evaluate(&self, direction: Direction, bytes: usize, boundary: Boundary) -> bool {
        if bytes == 0 {
            return false;
        }
        let bytes = bytes as u64;
        let (within, exhausted) = {
            let mut state = self.state.lock();
            let snapshot = &mut state.snapshot;
            let counter = match direction {
                Direction::Receive => &mut snapshot.received,
                Direction::Transmit => &mut snapshot.transmitted,
            };
            counter.packets = counter.packets.saturating_add(1);
            counter.bytes = counter.bytes.saturating_add(bytes);
            let before = snapshot.remaining;
            let within = match boundary {
                Boundary::Inclusive => before >= bytes,
                Boundary::Exclusive => before > bytes,
            };
            snapshot.remaining = before.saturating_sub(bytes);
            let exhausted = before != 0 && snapshot.remaining == 0;
            if exhausted {
                snapshot.exhaustions = snapshot.exhaustions.saturating_add(1);
                let event = Event::Exhausted(Exhaustion {
                    generation: snapshot.generation,
                    direction,
                    bytes,
                    remaining_before: before,
                });
                if state.events.len() < MAX_EVENTS - 1 {
                    state.events.push_back(event);
                } else if !state.overflow {
                    state.events.push_back(Event::Overflow);
                    state.overflow = true;
                }
            }
            (within, exhausted)
        };
        if exhausted {
            self.waker.wake_all();
        }
        within
    }

    /// Nonblocking callback; refusing a record leaves it queued for retry.
    pub fn drain(&self, mut accept: impl FnMut(Event) -> bool) -> usize {
        let mut state = self.state.lock();
        let mut count = 0;
        while let Some(&event) = state.events.front() {
            if !accept(event) {
                break;
            }
            state.events.pop_front();
            if event == Event::Overflow {
                state.overflow = false;
            }
            count += 1;
        }
        count
    }
    fn ready(&self) -> bool {
        !self.state.lock().events.is_empty()
    }
}

impl Selectable for Meter {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let mut ready = ReadySet::none();
        ready.read = interest.read && self.ready();
        ready
    }
    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        tf: &mut crate::arch::Trapframe,
        timeout: Option<u64>,
        minimum: u64,
    ) -> SelectWaitOutcome {
        let Some(task) = crate::task::mytask() else {
            return SelectWaitOutcome::TimedOut;
        };
        match self
            .waker
            .wait_with_condition(task.get_id(), tf, timeout, minimum, || {
                interest.read && self.ready()
            }) {
            crate::sync::waker::WaitResult::TimedOut => SelectWaitOutcome::TimedOut,
            crate::sync::waker::WaitResult::Interrupted => SelectWaitOutcome::Interrupted,
            crate::sync::waker::WaitResult::Woken => SelectWaitOutcome::Ready,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Observe,
    DenyExhausted,
}

pub struct Rule {
    pub meter: Arc<Meter>,
    pub direction: Option<Direction>,
    pub boundary: Boundary,
    pub action: Action,
}

/// Immutable rule topology; mutable counters can be shared across interfaces.
/// Replacement publishes a complete new policy without partially edited rules.
pub struct Policy {
    rules: Vec<Rule>,
}
impl Policy {
    pub fn new(rules: Vec<Rule>) -> Result<Arc<Self>, &'static str> {
        if rules.len() > MAX_RULES {
            return Err("traffic rule limit");
        }
        Ok(Arc::new(Self { rules }))
    }
    pub(crate) fn admit(&self, direction: Direction, bytes: usize) -> bool {
        if bytes == 0 {
            return false;
        }
        for rule in &self.rules {
            if rule.direction.is_some_and(|selected| selected != direction) {
                continue;
            }
            let within = rule.meter.evaluate(direction, bytes, rule.boundary);
            if rule.action == Action::DenyExhausted && !within {
                return false;
            }
        }
        true
    }
    pub(crate) fn uses_meter(&self, meter: &Arc<Meter>) -> bool {
        self.rules
            .iter()
            .any(|rule| Arc::ptr_eq(&rule.meter, meter))
    }
    pub fn deny_when_exhausted(meter: Arc<Meter>) -> Arc<Self> {
        Arc::new(Self {
            rules: alloc::vec![Rule {
                meter,
                direction: None,
                boundary: Boundary::Exclusive,
                action: Action::DenyExhausted,
            }],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rule(meter: &Arc<Meter>, action: Action, boundary: Boundary) -> Rule {
        Rule {
            meter: meter.clone(),
            direction: None,
            boundary,
            action,
        }
    }
    #[test_case]
    fn exact_boundary_overrun_and_rearm_have_single_generation_tagged_events() {
        let meter = Meter::new(60);
        let policy = Policy::new(alloc::vec![rule(
            &meter,
            Action::DenyExhausted,
            Boundary::Exclusive
        )])
        .unwrap();
        assert!(policy.admit(Direction::Transmit, 40));
        assert!(!policy.admit(Direction::Receive, 20));
        assert!(!policy.admit(Direction::Receive, 1));
        assert_eq!(meter.snapshot().exhaustions, 1);
        assert_eq!(meter.drain(|_| false), 0);
        assert!(meter.ready());
        meter.reset(10).unwrap();
        assert!(!policy.admit(Direction::Transmit, 11));
        let mut events = Vec::new();
        meter.drain(|event| {
            events.push(event);
            true
        });
        assert_eq!(
            events,
            alloc::vec![
                Event::Exhausted(Exhaustion {
                    generation: 1,
                    direction: Direction::Receive,
                    bytes: 20,
                    remaining_before: 20
                }),
                Event::Exhausted(Exhaustion {
                    generation: 2,
                    direction: Direction::Transmit,
                    bytes: 11,
                    remaining_before: 10
                }),
            ]
        );
        assert_eq!(
            meter.snapshot().received,
            Counter {
                packets: 2,
                bytes: 21
            }
        );
        assert_eq!(
            meter.snapshot().transmitted,
            Counter {
                packets: 2,
                bytes: 51
            }
        );
        meter.reset(10).unwrap();
        assert!(meter.evaluate(Direction::Receive, 10, Boundary::Inclusive));
        assert!(!meter.evaluate(Direction::Receive, 1, Boundary::Inclusive));
    }
    #[test_case]
    fn ordered_rules_share_only_explicit_meters_and_ignore_other_directions() {
        let global = Meter::new(5);
        let limit = Meter::new(20);
        let later = Meter::new(100);
        let policy = Policy::new(alloc::vec![
            rule(&global, Action::Observe, Boundary::Exclusive),
            rule(&limit, Action::DenyExhausted, Boundary::Exclusive),
            rule(&later, Action::Observe, Boundary::Exclusive)
        ])
        .unwrap();
        let shared = Policy::new(alloc::vec![Rule {
            meter: global.clone(),
            direction: Some(Direction::Transmit),
            boundary: Boundary::Exclusive,
            action: Action::Observe
        }])
        .unwrap();
        assert!(shared.admit(Direction::Receive, 100));
        assert_eq!(global.snapshot().remaining, 5);
        assert!(policy.admit(Direction::Receive, 10)); // alert never drops
        assert!(!policy.admit(Direction::Transmit, 10)); // equality exhausts
        assert_eq!(later.snapshot().received.bytes, 10);
        assert_eq!(later.snapshot().transmitted.bytes, 0); // reject short circuits
        assert!(shared.admit(Direction::Transmit, 1));
        assert_eq!(global.snapshot().transmitted.bytes, 11);
        assert_eq!(global.snapshot().exhaustions, 1);
        assert_eq!(Meter::new(5).snapshot().remaining, 5);
        assert!(!policy.admit(Direction::Transmit, 0));
    }
    #[test_case]
    fn event_overflow_is_bounded_and_recoverable() {
        let meter = Meter::new(1);
        for _ in 0..MAX_EVENTS + 20 {
            meter.reset(1).unwrap();
            meter.evaluate(Direction::Receive, 2, Boundary::Exclusive);
        }
        let mut overflow = 0;
        assert_eq!(
            meter.drain(|event| {
                overflow += usize::from(event == Event::Overflow);
                true
            }),
            MAX_EVENTS
        );
        assert_eq!(overflow, 1);
        assert!(!meter.ready());
        meter.reset(1).unwrap();
        meter.evaluate(Direction::Receive, 2, Boundary::Exclusive);
        assert_eq!(meter.drain(|event| event != Event::Overflow), 1);
        let generation = meter.snapshot().generation;
        meter.state.lock().snapshot.generation = u64::MAX;
        assert!(meter.reset(99).is_err());
        assert_eq!(meter.snapshot().remaining, 0);
        assert!(generation > 1);
    }

    struct Interface(IrqSpinLock<Option<crate::device::network::DevicePacket>>);
    impl crate::network::NetworkInterface for Interface {
        fn name(&self) -> &str {
            "traffic-policy-test"
        }
        fn mac_address(&self) -> crate::device::network::MacAddress {
            crate::device::network::MacAddress::new([2, 1, 2, 3, 4, 7])
        }
        fn ip_address(&self) -> Option<crate::network::ipv4::Ipv4Address> {
            None
        }
        fn set_ip_address(&self, _: crate::network::ipv4::Ipv4Address) {}
        fn clear_ip_address(&self) {}
        fn send(&self, packet: crate::device::network::DevicePacket) -> Result<(), &'static str> {
            let mut slot = self.0.lock();
            if slot.is_some() {
                return Err(crate::network::TX_QUEUE_FULL);
            }
            *slot = Some(packet);
            Ok(())
        }
        fn poll(&self) -> Result<Vec<crate::device::network::DevicePacket>, &'static str> {
            Ok(Vec::new())
        }
        fn stats(&self) -> crate::network::InterfaceStats {
            Default::default()
        }
    }
    struct Receiver(core::sync::atomic::AtomicUsize);
    impl crate::network::NetworkLayer for Receiver {
        fn name(&self) -> &'static str {
            "traffic-test-receiver"
        }
        fn as_any(&self) -> &dyn core::any::Any {
            self
        }
        fn stats(&self) -> crate::network::NetworkLayerStats {
            Default::default()
        }
        fn register_protocol(&self, _: u16, _: Arc<dyn crate::network::NetworkLayer>) {}
        fn send(
            &self,
            _: &[u8],
            _: &crate::network::LayerContext,
            _: &[Arc<dyn crate::network::NetworkLayer>],
        ) -> Result<(), crate::network::SocketError> {
            unreachable!()
        }
        fn receive(
            &self,
            data: &[u8],
            _: Option<&crate::network::LayerContext>,
        ) -> Result<(), crate::network::SocketError> {
            assert_eq!(data, &[1, 2, 3, 4]);
            self.0.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            Ok(())
        }
    }

    #[test_case]
    fn real_ingress_and_egress_enforce_link_policy_and_removal_retires_binding() {
        use crate::network::{
            LayerContext, NetworkInterface, NetworkLayer, NetworkManager, SocketError,
            ethernet::EthernetLayer,
            ipv4::{Ipv4Address, Ipv4AddressInfo, Ipv4Header, Ipv4Layer},
        };
        use core::sync::atomic::{AtomicUsize, Ordering};
        let manager = NetworkManager::new();
        let ip = Ipv4Layer::new();
        let eth = EthernetLayer::new();
        manager.register_layer("ip", ip.clone());
        manager.register_layer("ethernet", eth.clone());
        let device = Arc::new(Interface(IrqSpinLock::new(None)));
        manager
            .register_interface(device.name(), device.clone())
            .unwrap();
        eth.register_interface(device.name(), device.mac_address(), device.clone());
        ip.add_address(
            device.name(),
            Ipv4AddressInfo {
                address: Ipv4Address::new(192, 168, 1, 100),
                netmask: Ipv4Address::new(255, 255, 255, 0),
                broadcast: None,
                is_primary: true,
            },
        );
        let receiver = Arc::new(Receiver(AtomicUsize::new(0)));
        ip.register_protocol(253, receiver.clone());
        let link = manager.link_snapshot()[0];
        let meter = Meter::new(70);
        let policy = Policy::new(alloc::vec![rule(
            &meter,
            Action::DenyExhausted,
            Boundary::Exclusive
        )])
        .unwrap();
        assert!(
            manager
                .set_link_traffic_policy(link.id, link.generation + 1, Some(policy.clone()))
                .is_err()
        );
        manager
            .set_link_traffic_policy(link.id, link.generation, Some(policy))
            .unwrap();
        let mut context = LayerContext::new();
        context.set("interface", device.name().as_bytes());
        context.set("ip_src", &[192, 168, 1, 100]);
        context.set("ip_dst", &[192, 168, 1, 1]);
        context.set("eth_dst_mac", &[2, 9, 8, 7, 6, 5]);
        let send = || ip.send(&[1, 2, 3, 4], &context, &[eth.clone()]);
        send().unwrap();
        assert_eq!(meter.snapshot().remaining, 46);
        assert_eq!(send(), Err(SocketError::TransmitBusy));
        assert_eq!(meter.snapshot().remaining, 22); // evaluation, not driver acceptance

        let mut header = Ipv4Header::new();
        header.total_length = 24;
        header.protocol = 253;
        header.source_ip = [192, 168, 1, 1];
        header.dest_ip = [192, 168, 1, 100];
        header.checksum = header.calculate_checksum();
        let mut frame = alloc::vec![0; 60];
        frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        frame[14..34].copy_from_slice(&header.to_bytes());
        frame[34..38].copy_from_slice(&[1, 2, 3, 4]);
        let packet = crate::device::network::DevicePacket::with_data(frame);
        manager.deliver_received_packet(device.name(), &packet);
        assert_eq!(receiver.0.load(Ordering::Relaxed), 0);
        assert_eq!(meter.snapshot().remaining, 0);
        assert_eq!(meter.snapshot().received.bytes, 24); // ignores Ethernet padding
        meter.reset(49).unwrap();
        manager.deliver_received_packet(device.name(), &packet);
        assert_eq!(receiver.0.load(Ordering::Relaxed), 1);
        device.0.lock().take();
        send().unwrap();
        assert_eq!(meter.snapshot().remaining, 1);
        assert_eq!(send(), Err(SocketError::PermissionDenied));
        assert_eq!(meter.snapshot().exhaustions, 2);
        meter.reset(24).unwrap();
        assert_eq!(
            ip.receive(&packet.data[14..], Some(&context)),
            Err(SocketError::PermissionDenied)
        );
        assert_eq!(receiver.0.load(Ordering::Relaxed), 1);
        assert!(ip.receive(&packet.data[14..], None).is_err()); // no unattributed bypass

        manager.unregister_interface(device.name()).unwrap();
        manager
            .register_interface(device.name(), device.clone())
            .unwrap();
        assert_ne!(manager.link_snapshot()[0].id, link.id);
        assert!(
            manager
                .set_link_traffic_policy(link.id, link.generation, None)
                .is_err()
        );
        let before = meter.snapshot();
        manager.deliver_received_packet(device.name(), &packet);
        assert_eq!(receiver.0.load(Ordering::Relaxed), 2);
        assert_eq!(meter.snapshot(), before);
    }
}
