use scarlet_sys::{Syscall, syscall2};
use std::process::ExitCode;

const ENTRY_SIZE: usize = 264;
const MAX_MODULES: usize = 16;

fn main() -> ExitCode {
    println!("lsm-list: Rust std version");

    let mut buf = [0; ENTRY_SIZE * MAX_MODULES];
    let count = syscall2(Syscall::LsmList, buf.as_mut_ptr() as usize, buf.len());

    if count == 0 {
        println!("no modules loaded");
        return ExitCode::SUCCESS;
    }

    println!("{count} module(s) loaded:");
    println!("{:<5} {:<40}", "ID", "NAME");

    for i in 0..count.min(MAX_MODULES) {
        let offset = i * ENTRY_SIZE;
        let id = u64::from_le_bytes(buf[offset..offset + 8].try_into().unwrap());
        let name_bytes = &buf[offset + 8..offset + 8 + 256];
        let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(256);
        let name = std::str::from_utf8(&name_bytes[..name_len]).unwrap_or("<invalid>");
        println!("{id:<5} {name}");
    }

    ExitCode::SUCCESS
}
