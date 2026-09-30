//! Explicit capability duplication for Linux SCM_RIGHTS clients.
use super::{
    LinuxAbi, errno,
    fs::{FD_CLOEXEC, O_CLOEXEC},
};
use crate::{arch::Trapframe, task::mytask};

pub(super) fn duplicate(abi: &mut LinuxAbi, frame: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let raw = frame.get_arg(0);
    let flags = frame.get_arg(1);
    frame.increment_pc_next(&task);
    if flags & !(O_CLOEXEC as usize) != 0 {
        return errno::to_result(errno::EINVAL);
    }
    let Ok(raw) = u32::try_from(raw) else {
        return errno::to_result(errno::EBADF);
    };
    let Some((object, metadata)) = task.handle_table.clone_for_dup(raw) else {
        return errno::to_result(errno::EBADF);
    };
    let handle = match task.handle_table.insert_with_metadata(object, metadata) {
        Ok(handle) => handle,
        Err(_) => return errno::to_result(errno::ENFILE),
    };
    let fd = match abi.allocate_fd(handle as u32) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = task.handle_table.remove(handle);
            return errno::to_result(errno::EMFILE);
        }
    };
    let _ = abi.set_fd_flags(fd, if flags != 0 { FD_CLOEXEC } else { 0 });
    fd
}
