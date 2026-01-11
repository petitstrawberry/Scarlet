pub mod device;
pub mod generic;

// Re-export generic implementations for backward compatibility
pub mod cgroup {
    pub use super::generic::cgroup::*;
}

pub mod unshare {
    pub use super::generic::unshare::*;
}

#[cfg(target_arch = "riscv64")]
pub mod riscv64;
