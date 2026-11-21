use core::ptr::read_volatile;

use crate::println;

#[macro_export]
macro_rules! early_initcall {
    ($func:ident) => {
        #[cfg_attr(target_os = "macos", unsafe(link_section = "__DATA,__sc_init_early"))]
        #[cfg_attr(not(target_os = "macos"), unsafe(link_section = ".initcall.early"))]
        #[used]
        static __EARLY_INITCALL__: fn() = $func;
    };
}

#[cfg(not(feature = "hosted"))]
unsafe extern "C" {
    static __INITCALL_EARLY_START: usize;
    static __INITCALL_EARLY_END: usize;
}

#[cfg(not(feature = "hosted"))]
pub fn early_initcall_call() {
    unsafe {
        let size = core::mem::size_of::<fn()>();

        println!("Running early initcalls... ");
        let mut func_addr = &__INITCALL_EARLY_START as *const usize as usize;
        let end_addr = &__INITCALL_EARLY_END as *const usize as usize;

        while func_addr < end_addr {
            let initcall = read_volatile(func_addr as *const fn());

            initcall();

            func_addr += size;
        }
    }
}

#[cfg(feature = "hosted")]
pub use super::macho::early_initcall_call;
