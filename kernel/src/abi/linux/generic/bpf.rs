//! BPF program loading, object pinning and packet attachment are not provided.
//! Android's hosted network provider uses native traffic facilities in
//! userspace; this ABI must not manufacture pinned programs or detached maps.
use crate::{arch::Trapframe, task::mytask};
use super::{LinuxAbi, errno};

fn unsupported(command: usize, size: usize) -> usize {
    if command > 49 || !(4..=4096).contains(&size) {
        errno::to_result(errno::EINVAL)
    } else {
        errno::to_result(errno::EOPNOTSUPP)
    }
}

pub fn sys_bpf(_abi: &mut LinuxAbi, tf: &mut Trapframe) -> usize {
    let Some(task) = mytask() else { return errno::to_result(errno::ESRCH); };
    let result = unsupported(tf.get_arg(0), tf.get_arg(2));
    tf.increment_pc_next(&task);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn unprovided_bpf_operations_never_report_success() {
        // Includes map creation/update, program load, object pin/get and attach.
        for command in 0..=49 {
            assert_eq!(unsupported(command, 144), errno::to_result(errno::EOPNOTSUPP));
        }
    }

    #[test_case]
    fn invalid_bpf_command_headers_remain_errors() {
        for (command, size) in [(50, 144), (usize::MAX, 144), (5, 0), (5, 4097)] {
            assert_eq!(unsupported(command, size), errno::to_result(errno::EINVAL));
        }
    }
}
