use core::panic::PanicInfo;

use crate::arch;
use crate::println;


#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("[Scarlet Kernel] panic: {}", info);
    println!("[Test Runner] Test failed");
    
    arch::shutdown();
}

#[cfg(test)]
pub fn test_runner(tests: &[&dyn Fn()]) {

    use crate::println;

    println!("[Test Runner] Running {} tests", tests.len());
    for test in tests {
        // println!("[Test Runner] Running test: {:?}", test as *const _);
        test();
    }

    println!("[Test Runner] All tests passed");
    arch::shutdown();
}
