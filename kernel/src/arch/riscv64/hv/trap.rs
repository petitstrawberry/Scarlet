use crate::hypervisor::exit::VmExit;

use super::vmexit::VmExitInfo;

pub(super) fn guest_trap_handler(info: &VmExitInfo) -> VmExit {
    info.decode()
}
