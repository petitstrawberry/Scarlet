//! ABI-neutral mmap placement policy. Fixed mappings retain their requested addresses.
use crate::environment::PAGE_SIZE;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayoutPolicy {
    pub native_bits: u8,
    pub low_bits: u8,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Placement {
    pub policy: LayoutPolicy,
    pub seed: u64,
}
impl LayoutPolicy {
    pub const MAX_NATIVE: u8 = if cfg!(target_pointer_width = "64") {
        33
    } else {
        16
    };
    pub fn validate(self) -> Result<Self, &'static str> {
        if self.native_bits > Self::MAX_NATIVE || self.low_bits > 16 {
            return Err("layout entropy outside supported address ranges");
        }
        Ok(self)
    }
    pub fn sample(self) -> Result<Placement, &'static str> {
        self.validate()?;
        if self == Self::default() {
            return Ok(Placement::default());
        }
        let mut bytes = [0u8; 8];
        if crate::random::RandomManager::get_random_bytes(&mut bytes) != bytes.len() {
            return Err("address layout requires an entropy provider");
        }
        Ok(Placement {
            policy: self,
            seed: u64::from_ne_bytes(bytes),
        })
    }
}
impl Placement {
    pub fn offset(self, low_range: bool) -> usize {
        let (bits, seed) = if low_range {
            (self.policy.low_bits, self.seed >> 33)
        } else {
            (self.policy.native_bits, self.seed)
        };
        if bits == 0 {
            0
        } else {
            ((seed & ((1u64 << bits) - 1)) as usize) * PAGE_SIZE
        }
    }
    pub fn base(self, anchor: usize, limit: usize, low_range: bool) -> Result<usize, &'static str> {
        self.policy.validate()?;
        anchor
            .checked_add(self.offset(low_range))
            .filter(|base| *base < limit && *base % PAGE_SIZE == 0)
            .ok_or("randomized mmap anchor exceeds address range")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_pointer_width = "64")]
    #[test_case]
    fn placement_preserves_alignment_entropy_and_both_address_ranges() {
        let policy = LayoutPolicy {
            native_bits: 24,
            low_bits: 16,
        }
        .validate()
        .unwrap();
        let placement = Placement {
            policy,
            seed: u64::MAX,
        };
        let base = placement
            .base(
                0x1_0000_0000,
                crate::environment::USER_LOWER_CANONICAL_END,
                false,
            )
            .unwrap();
        assert_eq!(base, 0x1_0000_0000 + ((1usize << 24) - 1) * PAGE_SIZE);
        assert_eq!(base % PAGE_SIZE, 0);
        assert_eq!(
            placement.base(0x4000_0000, 0x8000_0000, true).unwrap(),
            0x4000_0000 + 65535 * PAGE_SIZE
        );
        assert!(
            placement
                .base(usize::MAX - PAGE_SIZE + 1, usize::MAX, false)
                .is_err()
        );
        assert!(
            LayoutPolicy {
                native_bits: 34,
                low_bits: 0
            }
            .validate()
            .is_err()
        );
        assert!(
            LayoutPolicy {
                native_bits: 0,
                low_bits: 17
            }
            .validate()
            .is_err()
        );
    }
}

pub mod syscall {
    use super::*;
    use crate::{arch::Trapframe, task::mytask};
    pub fn configure(tf: &mut Trapframe) -> usize {
        let task = mytask().unwrap();
        tf.increment_pc_next(&task);
        let raw = tf.get_arg(0);
        if raw & !0xffff != 0
            || !task
                .credentials
                .read()
                .permits(crate::shield::identity::MANAGE_LAYOUT)
        {
            return usize::MAX;
        }
        task.vm_manager
            .configure_layout(LayoutPolicy {
                native_bits: raw as u8,
                low_bits: (raw >> 8) as u8,
            })
            .map_or(usize::MAX, |_| 0)
    }
    pub fn query(tf: &mut Trapframe) -> usize {
        let task = mytask().unwrap();
        tf.increment_pc_next(&task);
        let policy = task.vm_manager.layout_policy();
        let mut raw = [0u8; 16];
        raw[..4].copy_from_slice(&(policy.native_bits as u32).to_ne_bytes());
        raw[4..8].copy_from_slice(&(policy.low_bits as u32).to_ne_bytes());
        raw[8..].copy_from_slice(&(task.vm_manager.get_mmap_base() as u64).to_ne_bytes());
        crate::library::std::usercopy::copy_to_user(&task, tf.get_arg(0), &raw)
            .map_or(usize::MAX, |_| 0)
    }
}
