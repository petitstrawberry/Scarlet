use crate::early_println;
use crate::early_print;


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
        let mut func = &__INITCALL_EARLY_START as *const usize as usize;
        let end = &__INITCALL_EARLY_END as *const usize as usize;
        let num = (end - func) / size;

        for i in 0..num {
            early_println!("Early initcalls {} / {}", i + 1, num);
            let initcall = *(func as *const fn());
            initcall();
            func += size;
        }
    }
}