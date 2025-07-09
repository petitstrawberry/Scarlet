use core::ptr::read_volatile;

use crate::early_println;

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
        static __DRIVER_INITCALL__ : fn() = $func;
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
        let mut func_addr = &__INITCALL_DRIVER_START as *const usize as usize;
        let end_addr = &__INITCALL_DRIVER_END as *const usize as usize;

        while func_addr < end_addr {
            let initcall = read_volatile(func_addr as *const fn());

            initcall();

            func_addr += size;
        }
    }
}