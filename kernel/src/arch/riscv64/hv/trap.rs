use crate::hypervisor::exit::VmExit;
use crate::task::mytask;

use super::vmexit::VmExitInfo;

pub fn guest_trap_handler(trapframe: &mut crate::arch::Trapframe, cause: usize, interrupt: bool) {
    let exit_info = VmExitInfo::capture(trapframe.epc);
    let exit = exit_info.decode();
    match exit {
        VmExit::Hlt | VmExit::Shutdown => {
            if let Some(task) = mytask() {
                task.exit(0);
            }
        }
        _ => {
            crate::early_println!(
                "Guest trap: cause={:#x} interrupt={} exit={:?}",
                cause,
                interrupt,
                exit
            );
            if let Some(task) = mytask() {
                task.exit(1);
            }
        }
    }
}
