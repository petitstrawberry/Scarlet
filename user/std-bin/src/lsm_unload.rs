use scarlet_sys::{Syscall, syscall1, syscall2};
use std::env;
use std::process::ExitCode;

const LSM_LIST_ENTRY_SIZE: usize = 264;
const LSM_LIST_MAX_MODULES: usize = 128;

fn main() -> ExitCode {
    println!("lsm-unload: Rust std version");

    let args = env::args().collect::<Vec<_>>();
    if args.len() < 2 {
        println!("usage: lsm-unload <module_name>");
        return ExitCode::from(1);
    }

    let name = &args[1];
    let module_id = match find_module_id_by_name(name) {
        Some(id) => id,
        None => {
            println!("module '{name}' not found");
            return ExitCode::from(1);
        }
    };

    let ret = syscall1(Syscall::LsmUnload, module_id as usize);
    if ret != 0 {
        println!("failed to unload '{name}' (id={module_id}, error: {ret})");
        return ExitCode::from(1);
    }

    println!("module '{name}' (id={module_id}) unloaded successfully");
    ExitCode::SUCCESS
}

fn find_module_id_by_name(name: &str) -> Option<u64> {
    let mut buf = [0; LSM_LIST_ENTRY_SIZE * LSM_LIST_MAX_MODULES];
    let count = syscall2(Syscall::LsmList, buf.as_mut_ptr() as usize, buf.len());
    if count == 0 {
        return None;
    }

    for i in 0..count.min(LSM_LIST_MAX_MODULES) {
        let offset = i * LSM_LIST_ENTRY_SIZE;
        let id = u64::from_le_bytes(buf[offset..offset + 8].try_into().ok()?);
        let name_bytes = &buf[offset + 8..offset + 8 + 256];
        let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(256);
        let entry_name = std::str::from_utf8(&name_bytes[..name_len]).ok()?;
        if entry_name == name {
            return Some(id);
        }
    }

    None
}
