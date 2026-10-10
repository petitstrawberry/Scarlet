#![no_std]
#![no_main]

// Direct Image boot has no firmware PCI allocator. This fixture owns PCI slot
// 00:01.0 (qemu-xhci) and reserves its 16KiB BAR at 0x10000000. Configure it
// before the standard entry installs mappings and enumerates devices.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".network_perf_head")]
pub extern "C" fn network_perf_head() -> ! {
    core::arch::naked_asm!(
        "b 2f",
        ".word 0",
        ".quad 0x200000",
        ".quad __KERNEL_IMAGE_SIZE",
        ".quad 0x2",
        ".quad 0", ".quad 0", ".quad 0",
        ".word 0x644d5241", ".word 0",
        "2:",
        "mov x4, #0x8010",
        "movk x4, #0x1000, lsl #16",
        "movk x4, #0x40, lsl #32",
        "mov w5, #4",
        "movk w5, #0x1000, lsl #16",
        "str w5, [x4]",
        "dsb sy",
        "b {entry}",
        entry = sym scarlet::arch::aarch64::boot::linux::image_entry,
    );
}
