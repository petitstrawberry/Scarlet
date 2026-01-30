pub mod cpu;
pub mod smp;

pub use smp::{get_cpus_started, mark_cpu_started, start_secondary_cpus, wait_for_cpus};
