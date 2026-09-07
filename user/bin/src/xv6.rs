#![no_std]
#![no_main]

extern crate scarlet_std as std;
mod abi_exec;

use std::println;

#[unsafe(no_mangle)]
fn main() -> i32 {
    let result = abi_exec::exec("xv6-riscv64", "/init", &["/init"], &[], "/", None);
    println!("xv6: cannot start /init in xv6-riscv64 view ({:?})", result);
    127
}
