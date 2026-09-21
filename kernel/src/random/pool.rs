//! Cryptographic state independent of devices, scheduling and wall time.
//!
//! A complete seed from a trusted provider is required before any output.
//! Timing samples are mixed without an entropy estimate. ChaCha20 uses a new
//! key for every request: the first 32 stream bytes replace the key, and only
//! subsequent bytes are released. No nonce is reused with a retained key.

use chacha20::{ChaCha20, cipher::{KeyIvInit, StreamCipher}};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

pub(super) const SEED_BYTES: usize = 32;
pub(super) const OUTPUT_CHUNK: usize = 256;
const RESEED_BYTES: usize = 1024 * 1024;
const RESEED_NS: u64 = 60_000_000_000;
const RETRY_NS: u64 = 1_000_000_000;

pub(super) struct Pool {
    key: [u8; SEED_BYTES],
    auxiliary: Sha256,
    auxiliary_pending: bool,
    seeded: bool,
    generated: usize,
    reseed_at: u64,
    retry_at: u64,
}

impl Pool {
    pub fn new() -> Self {
        Self {
            key: [0; SEED_BYTES], auxiliary: Sha256::new(),
            auxiliary_pending: false, seeded: false, generated: 0,
            reseed_at: 0, retry_at: 0,
        }
    }

    pub fn is_seeded(&self) -> bool { self.seeded }

    /// Domain and length framing prevents one source from impersonating a
    /// differently partitioned contribution. These samples never grant credit.
    pub fn mix_auxiliary(&mut self, domain: u32, sample: &[u8]) {
        self.auxiliary.update(domain.to_le_bytes());
        self.auxiliary.update((sample.len() as u64).to_le_bytes());
        self.auxiliary.update(sample);
        self.auxiliary_pending = true;
    }

    /// A short/failed read cannot bootstrap the generator, even after many
    /// such reads. Providers must satisfy EntropySource's trust contract.
    pub fn mix_source(&mut self, name: &str, sample: &[u8], now: u64) -> bool {
        let mut hash = Sha256::new();
        hash.update(b"Scarlet RNG source v1\0");
        hash.update(self.key);
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update((sample.len() as u64).to_le_bytes());
        hash.update(sample);
        let mut auxiliary = self.auxiliary.finalize_reset();
        hash.update(&auxiliary);
        self.key.copy_from_slice(&hash.finalize());
        auxiliary.zeroize();
        self.auxiliary_pending = false;
        let credited = sample.len() == SEED_BYTES;
        if credited {
            self.seeded = true;
            self.generated = 0;
            self.reseed_at = now.saturating_add(RESEED_NS);
        }
        credited
    }

    pub fn needs_reseed(&self, now: u64) -> bool {
        now >= self.retry_at &&
            (!self.seeded || now >= self.reseed_at || self.generated >= RESEED_BYTES)
    }

    pub fn attempted_reseed(&mut self, now: u64) {
        self.retry_at = now.saturating_add(RETRY_NS);
    }

    pub fn source_added(&mut self) { self.retry_at = 0; self.reseed_at = 0; }

    /// The caller bounds each lock hold to OUTPUT_CHUNK bytes.
    pub fn generate(&mut self, output: &mut [u8]) -> bool {
        if !self.seeded || output.len() > OUTPUT_CHUNK { return false; }
        if output.is_empty() { return true; }
        if self.auxiliary_pending {
            let mut auxiliary = self.auxiliary.finalize_reset();
            let mut hash = Sha256::new();
            hash.update(b"Scarlet RNG auxiliary v1\0");
            hash.update(self.key);
            hash.update(&auxiliary);
            self.key.copy_from_slice(&hash.finalize());
            auxiliary.zeroize();
            self.auxiliary_pending = false;
        }
        let mut cipher = ChaCha20::new((&self.key).into(), (&[0u8; 12]).into());
        self.key.zeroize();
        cipher.apply_keystream(&mut self.key);
        output.fill(0);
        cipher.apply_keystream(output);
        self.generated = self.generated.saturating_add(output.len());
        true
    }
}

impl Drop for Pool {
    fn drop(&mut self) { self.key.zeroize(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn auxiliary_and_partial_sources_never_bootstrap() {
        let mut pool = Pool::new();
        let mut out = [0xa5; 32];
        for n in 0..128 {
            pool.mix_auxiliary(1, &u64::to_le_bytes(n));
            assert!(!pool.mix_source("short", &[42; 31], n));
        }
        assert!(!pool.generate(&mut out));
        assert_eq!(out, [0xa5; 32]);
        assert!(!pool.is_seeded());
    }

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn mixes_multiple_sources_and_preserves_seed_on_failure() {
        let mut a = Pool::new();
        let mut b = Pool::new();
        assert!(a.mix_source("hardware", &[1; 32], 10));
        assert!(b.mix_source("hardware", &[1; 32], 10));
        assert!(a.mix_source("virtio", &[2; 32], 10));
        let mut x = [0; 64];
        let mut y = [0; 64];
        assert!(a.generate(&mut x));
        assert!(b.generate(&mut y));
        assert_ne!(x, y);
        assert!(!a.mix_source("failed", &[], 11));
        assert!(a.generate(&mut y));
        assert_ne!(x, y);
    }

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn erases_key_and_never_releases_rekey_bytes() {
        let mut pool = Pool::new();
        pool.mix_source("test", &[3; 32], 0);
        let old_key = pool.key;
        let mut expected = [0; 96];
        ChaCha20::new((&old_key).into(), (&[0u8; 12]).into())
            .apply_keystream(&mut expected);
        let mut out = [0; 64];
        assert!(pool.generate(&mut out));
        assert_eq!(pool.key, expected[..32]);
        assert_eq!(out, expected[32..]);
        assert_ne!(pool.key, old_key);
        let first = out;
        assert!(pool.generate(&mut out));
        assert_ne!(out, first);
    }

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn retry_and_reseed_limits_do_not_stop_seeded_output() {
        let mut pool = Pool::new();
        assert!(pool.needs_reseed(0));
        pool.attempted_reseed(0);
        assert!(!pool.needs_reseed(RETRY_NS - 1));
        pool.source_added();
        assert!(pool.needs_reseed(0));
        pool.mix_source("test", &[5; 32], 0);
        assert!(!pool.needs_reseed(RESEED_NS - 1));
        assert!(pool.needs_reseed(RESEED_NS));
        let mut out = [0; OUTPUT_CHUNK];
        for _ in 0..RESEED_BYTES / OUTPUT_CHUNK { assert!(pool.generate(&mut out)); }
        assert!(pool.needs_reseed(1));
        pool.attempted_reseed(1);
        assert!(pool.generate(&mut out));
    }

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn auxiliary_framing_changes_output_without_credit() {
        let mut a = Pool::new();
        let mut b = Pool::new();
        a.mix_auxiliary(1, b"ab"); a.mix_auxiliary(1, b"c");
        b.mix_auxiliary(1, b"a"); b.mix_auxiliary(1, b"bc");
        a.mix_source("test", &[8; 32], 0);
        b.mix_source("test", &[8; 32], 0);
        let mut x = [0; 32]; let mut y = [0; 32];
        assert!(a.generate(&mut x)); assert!(b.generate(&mut y));
        assert_ne!(x, y);
        assert!(!a.generate(&mut [0; OUTPUT_CHUNK + 1]));
    }
}
