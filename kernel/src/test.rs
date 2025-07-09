use core::panic::PanicInfo;

use crate::arch;
use crate::early_println;
use crate::println;

pub trait TestableFn {
    fn run(&self) -> ();
}

impl<T> TestableFn for T
    where
        T: Fn(),
{
    fn run(&self) {
        early_println!("[Test Runner] test name={}", core::any::type_name::<T>());
        self();
    }
}


#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    early_println!("[Scarlet Kernel] panic: {}", info);
    early_println!("[Test Runner] Test failed");

    arch::shutdown();
}

#[cfg(test)]
pub fn test_runner(tests: &[&dyn TestableFn]) {

    use crate::println;

    println!("[Test Runner] Running {} tests", tests.len());
    for test in tests {
        // println!("[Test Runner] Running test: {:?}", test as *const _);
        test.run();
    }

    println!("[Test Runner] All tests passed");
    arch::shutdown();
}
