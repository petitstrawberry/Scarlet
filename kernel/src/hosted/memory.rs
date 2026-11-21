//! PMM's host shared-RAM registry and its contiguous kernel alias.

use crate::vm::vmem::PhysicalMemoryArea;
use alloc::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};
use core::sync::atomic::{AtomicU64, Ordering};
use scarlet_hosted_runtime::ram::{RamArena, RamReservation};
use std::sync::{OnceLock, RwLock, mpsc};

const RAM_BASE: u64 = 0x1_0000;
const SHARED_BASE: u64 = 1 << 40;
pub(crate) const SHARED_OBJECT_LIMIT: usize = 1024 * 1024 * 1024;

struct Memory {
    area: PhysicalMemoryArea,
    arena: RamArena,
    retired: mpsc::Sender<RamReservation>,
}

static MEMORY: OnceLock<Memory> = OnceLock::new();
static NEXT_SHARED: AtomicU64 = AtomicU64::new(SHARED_BASE);

struct SharedRegistry {
    frames: BTreeMap<u64, Weak<SharedBacking>>,
    aliases: BTreeMap<usize, Weak<SharedBacking>>,
}

static SHARED: RwLock<SharedRegistry> = RwLock::new(SharedRegistry {
    frames: BTreeMap::new(),
    aliases: BTreeMap::new(),
});

pub(crate) struct SharedBacking {
    base: u64,
    reservation: Option<RamReservation>,
}

impl SharedBacking {
    pub(crate) fn new(size: usize) -> Result<Arc<Self>, &'static str> {
        if MEMORY.get().is_none() {
            return Err("Hosted retirement worker is not initialized");
        }
        if size > SHARED_OBJECT_LIMIT {
            return Err("Hosted shared memory limit exceeded");
        }
        let reservation = RamReservation::reserve(SHARED_OBJECT_LIMIT)
            .map_err(|_| "Cannot reserve a shared memory alias")?;
        reservation
            .commit(size)
            .map_err(|_| "Cannot commit shared memory")?;
        let base = NEXT_SHARED
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |base| {
                base.checked_add(SHARED_OBJECT_LIMIT as u64)
            })
            .map_err(|_| "Shared memory frame addresses exhausted")?;
        let alias = reservation.alias_base();
        let backing = Arc::new(Self {
            base,
            reservation: Some(reservation),
        });
        let mut registry = SHARED.write().unwrap();
        registry
            .frames
            .retain(|_, backing| backing.strong_count() > 0);
        registry
            .aliases
            .retain(|_, backing| backing.strong_count() > 0);
        registry.frames.insert(base, Arc::downgrade(&backing));
        registry.aliases.insert(alias, Arc::downgrade(&backing));
        Ok(backing)
    }

    pub(crate) fn paddr(&self) -> u64 {
        self.base
    }

    pub(crate) fn grow(&self, size: usize) -> Result<(), &'static str> {
        self.reservation()
            .commit(size)
            .map_err(|_| "Cannot grow hosted shared memory")
    }

    pub(crate) fn extent_length(&self, offset: usize, limit: usize) -> Option<usize> {
        self.reservation().extent_length(offset, limit)
    }

    fn reservation(&self) -> &RamReservation {
        self.reservation.as_ref().expect("Shared backing is live")
    }
}

impl Drop for SharedBacking {
    fn drop(&mut self) {
        // A VMA can drop its last owner while holding a short Scarlet lock.
        // The host thread performs munmap and closes extent descriptors.
        MEMORY
            .get()
            .expect("Retirement worker is initialized")
            .retired
            .send(self.reservation.take().expect("Shared backing is live"))
            .expect("Retirement worker must remain live");
    }
}

pub(super) fn initialize(size: usize) -> Result<PhysicalMemoryArea, &'static str> {
    if MEMORY.get().is_some() {
        return Err("Hosted RAM arena is already initialized");
    }
    if RAM_BASE
        .checked_add(size as u64)
        .is_none_or(|end| end > SHARED_BASE)
    {
        return Err("Hosted RAM overlaps reserved shared memory addresses");
    }
    let arena = RamArena::new(RAM_BASE, size).map_err(|_| "Cannot create hosted RAM arena")?;
    let area = PhysicalMemoryArea::new(RAM_BASE, RAM_BASE + size as u64 - 1);
    let (send, receive) = mpsc::channel();
    std::thread::Builder::new()
        .name("scarlet-frame-retirement".into())
        .spawn(move || {
            while let Ok(reservation) = receive.recv() {
                drop(reservation);
            }
        })
        .map_err(|_| "Cannot start hosted frame retirement worker")?;
    MEMORY
        .set(Memory {
            area,
            arena,
            retired: send,
        })
        .map_err(|_| "Hosted RAM arena is already initialized")?;
    Ok(area)
}

pub(crate) fn ram_area() -> Option<PhysicalMemoryArea> {
    MEMORY.get().map(|memory| memory.area)
}

pub(crate) fn alias_for(address: u64) -> Option<usize> {
    if let Some(frame) = MEMORY.get()?.arena.resolve(address, 1) {
        return Some(frame.alias.as_ptr().addr());
    }
    let backing = {
        let registry = SHARED.read().unwrap();
        registry.frames.range(..=address).next_back()?.1.upgrade()?
    };
    let offset = usize::try_from(address - backing.base).ok()?;
    Some(backing.reservation().alias(offset, 1)?.as_ptr().addr())
}

pub(crate) fn logical_address(alias: usize) -> Option<u64> {
    if let Some(address) = MEMORY.get()?.arena.logical_address(alias) {
        return Some(address);
    }
    let (base, backing) = {
        let registry = SHARED.read().unwrap();
        let (&base, backing) = registry.aliases.range(..=alias).next_back()?;
        (base, backing.upgrade()?)
    };
    backing.reservation().alias(alias - base, 1)?;
    Some(backing.base + (alias - base) as u64)
}

pub(super) fn worker_backing(
    address: u64,
    len: usize,
) -> std::io::Result<(std::os::fd::OwnedFd, usize)> {
    use std::os::fd::AsFd;
    if let Some(frame) = MEMORY
        .get()
        .and_then(|memory| memory.arena.resolve(address, len))
    {
        return Ok((frame.backing.try_clone_to_owned()?, frame.offset));
    }
    let registry = SHARED.read().unwrap();
    let frame = registry
        .frames
        .range(..=address)
        .next_back()
        .and_then(|(_, entry)| entry.upgrade())
        .and_then(|entry| {
            entry
                .reservation()
                .resolve((address - entry.base) as usize, len)
        })
        .ok_or_else(|| std::io::Error::other("Missing hosted frame extent"))?;
    Ok((frame.backing.as_fd().try_clone_to_owned()?, frame.offset))
}
