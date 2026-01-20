use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::vec::Vec;

// Small helper to optimize unsubscribe lookups
// Since we're in no_std without HashSet, we use Vec but optimize access patterns
fn contains_id(ids: &[SubscriptionId], target: SubscriptionId) -> bool {
    ids.iter().any(|&id| id == target)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(u64);

impl SubscriptionId {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

pub struct State<T: Clone> {
    inner: Arc<StateInner<T>>,
}

struct StateInner<T> {
    value: Mutex<T>,
    version: AtomicU64,
    subscribers: Mutex<Vec<(SubscriptionId, SubscriberCallback)>>,
    pending_unsubscribe: Mutex<Vec<SubscriptionId>>,
}

type SubscriberCallback = Arc<dyn Fn() + Send + Sync>;

impl<T: Clone> State<T> {
    pub fn new(initial: T) -> Self {
        Self {
            inner: Arc::new(StateInner {
                value: Mutex::new(initial),
                version: AtomicU64::new(0),
                subscribers: Mutex::new(Vec::new()),
                pending_unsubscribe: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn get(&self) -> T {
        self.inner.value.lock().clone()
    }

    pub fn set(&self, value: T)
    where
        T: PartialEq,
    {
        let mut guard = self.inner.value.lock();
        if *guard == value {
            return;
        }
        *guard = value;
        drop(guard);
        self.notify();
    }

    pub fn update<F>(&self, f: F)
    where
        F: FnOnce(&mut T),
    {
        let mut guard = self.inner.value.lock();
        f(&mut guard);
        drop(guard);
        self.inner.version.fetch_add(1, Ordering::Relaxed);
        self.notify();
    }

    pub fn version(&self) -> u64 {
        self.inner.version.load(Ordering::Relaxed)
    }

    pub fn subscribe(&self, callback: std::boxed::Box<dyn Fn() + Send + Sync>) -> SubscriptionId {
        let id = SubscriptionId::new();
        self.inner.subscribers.lock().push((id, Arc::from(callback)));
        id
    }

    pub fn unsubscribe(&self, id: SubscriptionId) {
        self.inner.pending_unsubscribe.lock().push(id);
    }

    fn notify(&self) {
        // Take pending unsubscriptions and clear the list (O(1))
        let pending_unsubscribes = {
            let mut pending = self.inner.pending_unsubscribe.lock();
            let list: Vec<_> = pending.drain(..).collect();
            list
        };

        if pending_unsubscribes.is_empty() {
            // Fast path: no unsubscriptions pending
            // Copy callbacks to avoid holding lock during iteration
            let callbacks = self
                .inner
                .subscribers
                .lock()
                .iter()
                .map(|(_, cb)| cb.clone())
                .collect::<Vec<_>>();

            // Process callbacks outside lock (re-entry safe)
            for callback in callbacks {
                callback();
            }
        } else {
            // Slow path: filter out unsubscribed callbacks
            // Copy callbacks to avoid holding lock during iteration
            let callbacks = self
                .inner
                .subscribers
                .lock()
                .iter()
                .filter(|(id, _)| !contains_id(&pending_unsubscribes, *id))
                .map(|(_, cb)| cb.clone())
                .collect::<Vec<_>>();

            // Process callbacks outside lock (re-entry safe)
            for callback in callbacks {
                callback();
            }

            // Remove unsubscribed entries
            let mut subscribers = self.inner.subscribers.lock();
            subscribers.retain(|(id, _)| !contains_id(&pending_unsubscribes, *id));
        }
    }
}

impl<T: Clone> Clone for State<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}
