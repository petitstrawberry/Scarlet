use core::{ptr::read_volatile, sync::atomic::compiler_fence};

use crate::early_println;


#[macro_export]
macro_rules! early_initcall {
    ($func:ident) => {
        #[unsafe(link_section = ".initcall.early")]
        #[used(linker)]
        static __EARLY_INITCALL__ : fn() = $func;
    };
}

unsafe extern "C" {
    static __INITCALL_EARLY_START: usize;
    static __INITCALL_EARLY_END: usize;
}

pub fn early_initcall_call() {
     unsafe {
         let size = core::mem::size_of::<fn()>();

         early_println!("Running early initcalls... ");
         let mut func_addr = &__INITCALL_EARLY_START as *const usize as usize;
         let end_addr = &__INITCALL_EARLY_END as *const usize as usize;

         while func_addr < end_addr {
             let initcall = read_volatile(func_addr as *const fn());

             initcall();

             func_addr += size;
         }
     }
 }