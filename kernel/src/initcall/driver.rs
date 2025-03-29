use crate::early_println;
use crate::early_print;

/// A macro used to register driver initialization functions to be called during the system boot process.
///
/// This macro places the function pointer into the `.initcall.driver` section of the binary,
/// allowing the kernel to discover and call all driver initialization functions at the appropriate time.
///
/// # Parameters
///
/// * `$func` - A function with the signature `fn()` that register a driver
///
/// # Examples
///
/// ```
/// fn register_my_driver() {
///     // Driver registration logic here
/// }
///
/// driver_initcall!(register_my_driver);
/// ```
///
/// # Safety
///
/// This macro relies on linker sections and should only be used for functions that are
/// safe to call during the kernel's driver initialization phase.
/// 
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