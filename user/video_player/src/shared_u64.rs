//! Shared playback values retain their full width on RV32.
//!
//! Loads acquire, stores release, and updates acquire/release. RV32 uses the
//! existing sleeping mutex; native 64-bit targets keep their atomic path.

#[cfg(target_has_atomic = "64")]
use core::sync::atomic::{AtomicU64, Ordering};

pub(super) struct SharedU64 {
    #[cfg(target_has_atomic = "64")]
    value: AtomicU64,
    #[cfg(not(target_has_atomic = "64"))]
    value: ProtectedU64,
}

impl SharedU64 {
    pub const fn new(value: u64) -> Self {
        Self {
            #[cfg(target_has_atomic = "64")]
            value: AtomicU64::new(value),
            #[cfg(not(target_has_atomic = "64"))]
            value: ProtectedU64::new(value),
        }
    }

    pub fn load(&self) -> u64 {
        #[cfg(target_has_atomic = "64")]
        {
            self.value.load(Ordering::Acquire)
        }
        #[cfg(not(target_has_atomic = "64"))]
        {
            self.value.load()
        }
    }

    pub fn store(&self, value: u64) {
        #[cfg(target_has_atomic = "64")]
        self.value.store(value, Ordering::Release);
        #[cfg(not(target_has_atomic = "64"))]
        self.value.update(|_| value);
    }

    pub fn fetch_add(&self, value: u64) -> u64 {
        #[cfg(target_has_atomic = "64")]
        {
            self.value.fetch_add(value, Ordering::AcqRel)
        }
        #[cfg(not(target_has_atomic = "64"))]
        {
            self.value.update(|old| old.wrapping_add(value))
        }
    }

    pub fn fetch_max(&self, value: u64) -> u64 {
        #[cfg(target_has_atomic = "64")]
        {
            self.value.fetch_max(value, Ordering::AcqRel)
        }
        #[cfg(not(target_has_atomic = "64"))]
        {
            self.value.update(|old| old.max(value))
        }
    }
}

#[cfg(any(test, not(target_has_atomic = "64")))]
struct ProtectedU64(std::sync::Mutex<u64>);

#[cfg(any(test, not(target_has_atomic = "64")))]
impl ProtectedU64 {
    const fn new(value: u64) -> Self {
        Self(std::sync::Mutex::new(value))
    }

    fn load(&self) -> u64 {
        #[cfg(test)]
        {
            *self.0.lock().unwrap()
        }
        #[cfg(not(test))]
        {
            *self.0.lock()
        }
    }

    fn update(&self, update: impl FnOnce(u64) -> u64) -> u64 {
        #[cfg(test)]
        let mut guard = self.0.lock().unwrap();
        #[cfg(not(test))]
        let mut guard = self.0.lock();
        let old = *guard;
        *guard = update(old);
        old
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_values_keep_high_bits_and_max_updates_do_not_regress() {
        let wide = u32::MAX as u64 + 123;
        let native = SharedU64::new(wide);
        let protected = ProtectedU64::new(wide);
        assert_eq!(native.fetch_max(1), wide);
        assert_eq!(protected.update(|old| old.max(1)), wide);
        assert_eq!(native.load(), wide);
        assert_eq!(protected.load(), wide);
        native.store(u64::MAX);
        protected.update(|_| u64::MAX);
        assert_eq!(native.fetch_add(1), u64::MAX);
        assert_eq!(protected.update(|old| old.wrapping_add(1)), u64::MAX);
        assert_eq!(native.load(), 0);
        assert_eq!(protected.load(), 0);
    }

    #[test]
    fn concurrent_updates_preserve_the_full_count() {
        let start = u32::MAX as u64;
        let native = SharedU64::new(start);
        let protected = ProtectedU64::new(start);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let native = &native;
                let protected = &protected;
                scope.spawn(move || {
                    for _ in 0..1024 {
                        native.fetch_add(1);
                        protected.update(|old| old.wrapping_add(1));
                    }
                });
            }
        });
        assert_eq!(native.load(), start + 4096);
        assert_eq!(protected.load(), start + 4096);
    }
}
