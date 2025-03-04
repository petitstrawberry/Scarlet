use core::panic::PanicInfo;

use crate::arch;
use crate::println;
use crate::print;


#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("[Scarlet Kernel] panic: {}", info);
    println!("[Test Runner] Test failed");
    
    arch::shutdown();
}

#[cfg(test)]
pub fn test_runner(tests: &[&dyn Fn()]) {
    use crate::println;
    use crate::print;

    println!("[Test Runner] Running {} tests", tests.len());
    for test in tests {
        test();
    }

    println!("[Test Runner] All tests passed");
    arch::shutdown();
}
