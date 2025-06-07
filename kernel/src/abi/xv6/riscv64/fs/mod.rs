use crate::{abi::xv6::riscv64::fs::xv6fs::Stat, fs::FileType, task::mytask};

pub mod xv6fs;

pub fn sys_fstat(trapframe: &mut crate::arch::Trapframe) -> usize {
    let fd = trapframe.get_arg(0) as usize;

    let task = mytask()
        .expect("sys_fstat: No current task found");
    trapframe.increment_pc_next(task); // Increment the program counter

    let stat_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1) as usize)
        .expect("sys_fstat: Failed to translate stat pointer") as *mut Stat;
    let file = match task.get_file(fd) {
        Some(file) => file,
        None => return usize::MAX, // Return -1 on error
    };
    let metadata = file.metadata()
        .expect("sys_fstat: Failed to get file metadata");

    if stat_ptr.is_null() {
        return usize::MAX; // Return -1 if stat pointer is null
    }
    
    let stat = unsafe { &mut *stat_ptr };

    *stat = Stat {
        dev: 0,
        ino: 0,
        file_type: match metadata.file_type {
            FileType::Directory => 1, // T_DIR
            FileType::RegularFile => 2,      // T_FILE
            FileType::CharDevice(_) => 3, // T_DEVICE
            FileType::BlockDevice(_) => 3, // T_DEVICE
            _ => 0, // Unknown type
        },
        nlink: 1,
        size: metadata.size as u64,
    };

    0
}