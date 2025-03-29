use crate::early_println;
use crate::early_print;

#[macro_export]
macro_rules! driver_initcall {
    ($func:ident) => {
        #[unsafe(link_section = ".initcall.driver")]
        #[used(linker)]
        static __INITCALL__ : fn() = $func;
    };
}


unsafe extern "C" {
    static __INITCALL_DRIVER_START: usize;
    static __INITCALL_DRIVER_END: usize;
}

pub fn driver_initcall_call() {
    unsafe {
        let size = core::mem::size_of::<fn()>();

        early_println!("Running driver initcalls... ");
        let mut func = &__INITCALL_DRIVER_START as *const usize as usize;
        let end = &__INITCALL_DRIVER_END as *const usize as usize;
        let num = (end - func) / size;

        for i in 0..num {
            early_println!("Driver initcalls {} / {}", i + 1, num);
            let initcall = *(func as *const fn());
            initcall();
            func += size;
        }
    }
}