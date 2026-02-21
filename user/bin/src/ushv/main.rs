#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std;

#[cfg(target_arch = "riscv64")]
mod riscv64;

#[cfg(target_arch = "riscv64")]
mod device;
#[cfg(target_arch = "riscv64")]
mod devices;
#[cfg(target_arch = "riscv64")]
mod machine;

#[cfg(target_arch = "riscv64")]
#[unsafe(no_mangle)]
fn main() -> i32 {
    riscv64::run()
}

#[cfg(not(target_arch = "riscv64"))]
#[unsafe(no_mangle)]
fn main() -> i32 {
    use scarlet_std::println;
    println!("[ushv] Unsupported architecture");
    1
}
