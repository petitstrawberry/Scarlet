#![no_std]
#![no_main]

extern crate scarlet_modules;

#[unsafe(link_section = ".init")]
#[unsafe(no_mangle)]
pub extern "C" fn arch_start_kernel() -> ! {
    scarlet_modules::force_link();
    scarlet_modules::scarlet::arch::riscv64::boot::limine::limine_entry()
}
