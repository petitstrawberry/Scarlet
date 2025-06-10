#[macro_use]
mod macros;
mod proc;
mod file;
pub mod fs;

// pub mod drivers;

use file::{sys_dup, sys_exec, sys_mknod, sys_open, sys_write};
use proc::{sys_exit, sys_fork, sys_wait, sys_kill};

use crate::{abi::{xv6::riscv64::{file::{sys_close, sys_fstat, sys_read}, proc::{sys_chdir, sys_sbrk}}, AbiModule}, early_initcall, fs::VfsManager, register_abi};


#[derive(Default)]
pub struct Xv6Riscv64Abi;

impl AbiModule for Xv6Riscv64Abi {
    fn name() -> &'static str {
        "xv6-riscv64"
    }
    
    fn handle_syscall(&self, trapframe: &mut crate::arch::Trapframe) -> Result<usize, &'static str> {
        syscall_handler(trapframe)
    }

    fn init(&self) {
        // crate::println!("Xv6Riscv64 ABI initialized");
    }

    fn init_fs(&self, vfs: &mut VfsManager) {
        crate::println!("[Xv6Riscv64 Module] Initializing tmpfs for Xv6Riscv64 ABI");
        // let id = vfs.create_and_register_fs_with_params("tmpfs", &TmpFSParams::default())
        //     .expect("Failed to create tmpfs");
        // let _ = vfs.mount(id, "/");
    }
}

syscall_table! {
    Invalid = 0 => |_: &mut crate::arch::Trapframe| {
        0
    },
    Fork = 1 => sys_fork,
    Exit = 2 => sys_exit,
    Wait = 3 => sys_wait,
    // Pipe = 4 => sys_pipe,
    Read = 5 => sys_read,
    Kill = 6 => sys_kill,
    Exec = 7 => sys_exec,
    Fstat = 8 => sys_fstat,
    Chdir = 9 => sys_chdir,
    Dup = 10 => sys_dup,
    // Getpid = 11 => sys_getpid,
    Sbrk = 12 => sys_sbrk,
    // Sleep = 13 => sys_sleep,
    // Uptime = 14 => sys_uptime,
    Open = 15 => sys_open,
    Write = 16 => sys_write,
    Mknod = 17 => sys_mknod,
    // Unlink = 18 => sys_unlink,
    // Link = 19 => sys_link,
    // Mkdir = 20 => sys_mkdir,
    Close = 21 => sys_close,
}

fn register_xv6_abi() {
    register_abi!(Xv6Riscv64Abi);
}

early_initcall!(register_xv6_abi);