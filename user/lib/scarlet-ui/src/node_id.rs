use std::sync::atomic::{AtomicU64, Ordering};
use std::println;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

impl NodeId {
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let id = Self(COUNTER.fetch_add(1, Ordering::Relaxed));
        println!("[NodeId] Created new NodeId({})", id.0);
        id
    }
}
