//! Readiness notifications for multi-object waits.
//!
//! A registration keeps only a weakly held link back to its target [`Waker`].
//! Event sources likewise retain registrations weakly and prune dead entries
//! when they next publish readiness.

extern crate alloc;

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::sync::IrqSpinLock;
use crate::sync::waker::Waker;

/// A persistent readiness subscription returned by [`Waker::subscribe_readiness`].
///
/// Clones share the same notification counter. Callers retain a registration
/// across polling or `epoll_wait` iterations and compare [`Self::generation`]
/// with an earlier sample to detect a readiness notification.
#[derive(Clone)]
pub struct ReadinessRegistration {
    listener: Arc<ReadinessListener>,
}

impl ReadinessRegistration {
    /// Return the number of readiness notifications published for this
    /// subscription. The counter is monotonically increasing in practice and
    /// starts at zero when the subscription is created.
    pub fn generation(&self) -> u64 {
        *self.listener.generation.lock()
    }
}

struct ReadinessListener {
    generation: IrqSpinLock<u64>,
    target: Weak<Waker>,
}

impl ReadinessListener {
    fn new(target: Weak<Waker>) -> Self {
        Self {
            generation: IrqSpinLock::new(0),
            target,
        }
    }

    fn notify(&self) -> Option<Arc<Waker>> {
        let target = self.target.upgrade()?;
        let mut generation = self.generation.lock();
        *generation = generation.wrapping_add(1);
        Some(target)
    }
}

/// Weak registration registry embedded in each event-source [`Waker`].
pub(crate) struct ReadinessSource {
    subscriber_count: AtomicUsize,
    subscribers: IrqSpinLock<Vec<Weak<ReadinessListener>>>,
}

impl ReadinessSource {
    pub(crate) const fn new() -> Self {
        Self {
            subscriber_count: AtomicUsize::new(0),
            subscribers: IrqSpinLock::new(Vec::new()),
        }
    }

    pub(crate) fn subscribe(&self, target: &Arc<Waker>) -> ReadinessRegistration {
        let listener = Arc::new(ReadinessListener::new(Arc::downgrade(target)));
        {
            let mut subscribers = self.subscribers.lock();
            subscribers.push(Arc::downgrade(&listener));
            self.subscriber_count
                .store(subscribers.len(), Ordering::Release);
        }
        ReadinessRegistration { listener }
    }

    /// Publish one readiness notification to every live subscription and
    /// return the target wakers that must be woken by the caller.
    ///
    /// The generation updates happen after releasing the source registry lock.
    /// The count check keeps the no-subscriber path free of allocation and
    /// generation writes.
    pub(crate) fn notify(&self) -> Vec<Arc<Waker>> {
        if self.subscriber_count.load(Ordering::Acquire) == 0 {
            return Vec::new();
        }

        let listeners = {
            let mut subscribers = self.subscribers.lock();
            let mut listeners = Vec::with_capacity(subscribers.len());
            subscribers.retain(|subscriber| match subscriber.upgrade() {
                Some(listener) if listener.target.strong_count() != 0 => {
                    listeners.push(listener);
                    true
                }
                Some(_) | None => false,
            });
            self.subscriber_count
                .store(subscribers.len(), Ordering::Release);
            listeners
        };

        let mut targets = Vec::with_capacity(listeners.len());
        for listener in listeners {
            if let Some(target) = listener.notify() {
                targets.push(target);
            }
        }
        targets
    }

    #[cfg(test)]
    pub(crate) fn subscriber_count_for_test(&self) -> usize {
        self.subscriber_count.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn prewait_wake_advances_the_generation_without_a_task() {
        let source = Arc::new(Waker::new_interruptible("readiness-source"));
        let target = Arc::new(Waker::new_interruptible("readiness-target"));
        let registration = source.subscribe_readiness(&target);
        assert_eq!(registration.generation(), 0);

        assert!(!source.wake_one());
        assert_eq!(registration.generation(), 1);
        assert_eq!(target.pending_wake_count_for_test(), 1);
        assert_eq!(source.wake_all(), 0);
        assert_eq!(registration.generation(), 2);
    }

    #[test_case]
    fn readiness_notification_fans_out_to_all_subscribers() {
        let source = Arc::new(Waker::new_interruptible("readiness-fanout"));
        let first_target = Arc::new(Waker::new_interruptible("readiness-first"));
        let second_target = Arc::new(Waker::new_interruptible("readiness-second"));
        let first = source.subscribe_readiness(&first_target);
        let second = source.subscribe_readiness(&second_target);
        let first_clone = first.clone();
        assert_eq!(source.readiness_subscriber_count_for_test(), 2);

        assert_eq!(source.wake_all(), 0);
        assert_eq!(first.generation(), 1);
        assert_eq!(first_clone.generation(), 1);
        assert_eq!(second.generation(), 1);
        assert_eq!(first_target.pending_wake_count_for_test(), 1);
        assert_eq!(second_target.pending_wake_count_for_test(), 1);
    }

    #[test_case]
    fn dropped_subscriptions_are_pruned_by_the_next_notification() {
        let source = Arc::new(Waker::new_interruptible("readiness-prune"));
        let target = Arc::new(Waker::new_interruptible("readiness-prune-target"));
        let registration = source.subscribe_readiness(&target);
        assert_eq!(source.readiness_subscriber_count_for_test(), 1);

        drop(registration);
        assert_eq!(source.readiness_subscriber_count_for_test(), 1);
        assert_eq!(source.wake_all(), 0);
        assert_eq!(source.readiness_subscriber_count_for_test(), 0);
    }

    #[test_case]
    fn subscriptions_with_dropped_targets_are_pruned() {
        let source = Arc::new(Waker::new_interruptible("readiness-prune-target"));
        let target = Arc::new(Waker::new_interruptible("readiness-dropped-target"));
        let registration = source.subscribe_readiness(&target);

        drop(target);
        assert_eq!(source.wake_all(), 0);
        assert_eq!(source.readiness_subscriber_count_for_test(), 0);
        assert_eq!(registration.generation(), 0);
    }

    #[test_case]
    fn subscriptions_do_not_form_a_retain_cycle() {
        let source = Arc::new(Waker::new_interruptible("readiness-no-cycle-source"));
        let target = Arc::new(Waker::new_interruptible("readiness-no-cycle-target"));
        let source_weak = Arc::downgrade(&source);
        let target_weak = Arc::downgrade(&target);
        let registration = source.subscribe_readiness(&target);

        assert_eq!(Arc::strong_count(&source), 1);
        assert_eq!(Arc::strong_count(&target), 1);
        drop(source);
        assert!(source_weak.upgrade().is_none());
        drop(target);
        assert!(target_weak.upgrade().is_none());
        drop(registration);
    }

    #[test_case]
    fn readiness_dispatch_terminates_for_a_subscription_cycle() {
        let first = Arc::new(Waker::new_interruptible("readiness-cycle-first"));
        let second = Arc::new(Waker::new_interruptible("readiness-cycle-second"));
        let first_to_second = first.subscribe_readiness(&second);
        let second_to_first = second.subscribe_readiness(&first);

        assert!(!first.wake_one());
        assert_eq!(first_to_second.generation(), 1);
        assert_eq!(second_to_first.generation(), 1);
        assert_eq!(first.pending_wake_count_for_test(), 1);
        assert_eq!(second.pending_wake_count_for_test(), 1);
    }
}
