//! VFS v2 Page Cache integration helpers
//!
//! Defines the `CacheId` newtype and the `PageCacheCapable` trait that
//! provides a common interface for FileObjects supporting the global page cache.

use core::hash::Hash;

/// Opaque identifier for a cacheable file object
/// Drivers are responsible for returning a stable, globally unique id
/// for the lifetime of the filesystem instance.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CacheId(pub u64);

impl CacheId {
    #[inline]
    pub const fn new(v: u64) -> Self {
        Self(v)
    }
    #[inline]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// FileObjects that support the global page cache must implement this trait.
///
/// Implementors should compose the id from (filesystem identity, file identity).
/// The concrete composition is left to the filesystem driver for now.
pub trait PageCacheCapable {
    /// Returns a stable `CacheId` for this file for the lifetime of the
    /// underlying filesystem instance.
    fn cache_id(&self) -> CacheId;
}
