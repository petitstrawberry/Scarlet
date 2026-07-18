//! Private platform hooks used by the AArch64 boot orchestration.

mod apple;

use fdt::node::FdtNode;

use crate::sched::scheduler::CpuCoreClass;

/// Prepare platform-specific state before a direct EL2-to-EL1 transition.
pub(super) fn prepare_el2_drop() {
    apple::prepare_el2_drop();
}

/// Return a recorded platform EL2-control transition, when available.
pub(super) fn el2_drop_diagnostic_transition() -> Option<(u64, u64)> {
    apple::el2_drop_diagnostic_transition()
}

/// Initialize implementation-defined controls for the executing CPU.
pub(super) fn initialize_current_cpu() -> Option<(u64, u64)> {
    apple::initialize_current_cpu()
}

/// Classify a CPU using platform-specific FDT compatibility information.
pub(super) fn classify_cpu_node(cpu: &FdtNode) -> Option<CpuCoreClass> {
    apple::classify_cpu_node(cpu)
}
