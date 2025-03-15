use crate::{arch::instruction::idle, println, print};

pub mod early;
pub mod driver;
pub mod late;

#[allow(improper_ctypes)]
unsafe extern "C" {
    static mut __INITCALL_START: usize;
    static mut __INITCALL_END: usize;
}

#[allow(static_mut_refs)]
pub fn initcall_task() {
    let size = core::mem::size_of::<fn()>();

    println!("Running initcalls... ");
    let mut func = unsafe { &__INITCALL_START as *const usize as usize };
    let end = unsafe { &__INITCALL_END as *const usize as usize };
    let num = (end - func) / size;

    for i in 0..num {
        println!("Initcalls {} / {}", i + 1, num);
        let initcall = unsafe { *(func as *const fn()) };
        initcall();
        func += size;
    }

    idle();
}