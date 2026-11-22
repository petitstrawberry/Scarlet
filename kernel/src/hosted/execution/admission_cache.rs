//! Publication caching independent of worker transport. Unknown owner versions
//! are rescanned on every admission; equal extents do not rebuild publication.
use alloc::vec::Vec;

/// Snapshots arrive in strictly increasing key order. Equality must include
/// every descriptor field and owner identity affecting publication.
pub(super) trait Snapshot {
    fn key(&self) -> usize;
    fn same(&self, other: &Self) -> bool;
    /// Can the published extents change without replacing the descriptor?
    fn needs_owner_check(&self) -> bool {
        true
    }
}

pub(super) struct Cache<M, V> {
    generation: Option<u64>,
    entries: Vec<Entry<M, V>>,
    owner_checks: Vec<usize>,
}

struct Entry<M, V> {
    map: M,
    version: Option<u64>,
    value: Option<V>,
}

impl<M, V> Default for Cache<M, V> {
    fn default() -> Self {
        Self {
            generation: None,
            entries: Vec::new(),
            owner_checks: Vec::new(),
        }
    }
}

impl<M: Snapshot, V: PartialEq> Cache<M, V> {
    pub(super) fn update(
        &mut self,
        generation: u64,
        maps: impl FnOnce() -> Vec<M>,
        mut version: impl FnMut(&M) -> Option<u64>,
        mut build: impl FnMut(&M) -> V,
    ) -> bool {
        let mut changed = self.generation != Some(generation);
        if changed {
            let mut previous = core::mem::take(&mut self.entries).into_iter().peekable();
            self.entries = maps()
                .into_iter()
                .map(|map| {
                    while previous.peek().is_some_and(|old| old.map.key() < map.key()) {
                        previous.next();
                    }
                    if previous
                        .peek()
                        .is_some_and(|old| old.map.key() == map.key())
                    {
                        let old = previous.next().unwrap();
                        if map.same(&old.map) {
                            return old;
                        }
                    }
                    Entry {
                        map,
                        version: None,
                        value: None,
                    }
                })
                .collect();
            self.owner_checks.clear();
            for (index, entry) in self.entries.iter_mut().enumerate() {
                if entry.map.needs_owner_check() {
                    self.owner_checks.push(index);
                } else if entry.value.is_none() {
                    // Descriptor replacement is the only invalidation for these extents.
                    entry.value = Some(build(&entry.map));
                }
            }
            self.generation = Some(generation);
        }
        for &index in &self.owner_checks {
            let entry = &mut self.entries[index];
            let before = version(&entry.map);
            if before.is_some() && entry.version == before && entry.value.is_some() {
                continue;
            }
            let value = build(&entry.map);
            if entry.value.as_ref() != Some(&value) {
                entry.value = Some(value);
                changed = true;
            }
            // Another address space may populate the same owner while we
            // inspect it. Never tag that snapshot with a later generation.
            entry.version = before.filter(|_| version(&entry.map) == before);
        }
        changed
    }

    pub(super) fn values(&self) -> impl Iterator<Item = &V> {
        self.entries
            .iter()
            .map(|entry| entry.value.as_ref().unwrap())
    }
}
