//! # Initcall System
//! 
//! The initcall module manages the kernel's initialization sequence by providing
//! a structured way to execute initialization functions at different stages of boot.
//! 
//! ## Submodules
//! 
//! - `early`: Initialization functions that need to run early in the boot process
//! - `driver`: Driver initialization routines
//! - `late`: Initialization functions that should run late in the boot process
//! 
//! ## Initcall Mechanism
//! 
//! The initcall system works by collecting function pointers between special linker
//! sections (`__INITCALL_START` and `__INITCALL_END`). Each function pointer
//! represents an initialization function that needs to be called during boot.
//! 
//! The `initcall_task()` function iterates through these function pointers and
//! executes each initialization routine in sequence, providing progress updates
//! to the console. After all initialization routines have been executed, the
//! processor enters an idle state.

use crate::println;

pub mod early;
pub mod driver;
pub mod late;

#[allow(improper_ctypes)]
unsafe extern "C" {
    static mut __INITCALL_DRIVER_END: usize;
    static mut __INITCALL_END: usize;
}

#[allow(static_mut_refs)]
pub fn call_initcalls() {
    unsafe {
        let size = core::mem::size_of::<fn()>();
        
        println!("Running initcalls... ");
        let mut func_addr = &__INITCALL_DRIVER_END as *const usize as usize;
        let end_addr = &__INITCALL_END as *const usize as usize;

        while func_addr < end_addr {
            let initcall = core::ptr::read_volatile(func_addr as *const fn());
            initcall();
            func_addr += size;
        }
    }
}