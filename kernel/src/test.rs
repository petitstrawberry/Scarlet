use core::panic::PanicInfo;

use crate::arch;
use crate::early_println;


#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    early_println!("[Scarlet Kernel] panic: {}", info);
    early_println!("[Test Runner] Test failed");

    arch::shutdown();
}

#[cfg(test)]
pub fn test_runner(tests: &[&dyn Fn()]) {

    early_println!("[Test Runner] Running {} tests", tests.len());
    for test in tests {
        test();
    }

    early_println!("[Test Runner] All tests passed");
    arch::shutdown();
}
