#[cfg(all(test, feature = "limine"))]
#[unsafe(link_section = ".init")]
#[unsafe(no_mangle)]
pub extern "C" fn arch_start_kernel() -> ! {
    super::limine::limine_entry()
}
