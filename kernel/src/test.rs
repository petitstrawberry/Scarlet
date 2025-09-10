use core::panic::PanicInfo;

use crate::arch;
use crate::early_println;

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

    #[cfg(feature = "profiler")]
    {
        use crate::profiler;
        profiler::print_profiling_results();
    }

    crate::arch::shutdown_with_code(1);
}

#[cfg(test)]
pub fn test_runner(tests: &[&dyn TestableFn]) {

    early_println!("[Test Runner] Running {} tests", tests.len());
    for test in tests {
        // println!("[Test Runner] Running test: {:?}", test as *const _);
        test.run();
    }

    early_println!("[Test Runner] All {} tests passed", tests.len());

    #[cfg(feature = "profiler")]
    {
        use crate::profiler;
        crate::early_println!("[Profiler] Printing profiling results:");
        profiler::print_profiling_results();
    }
    crate::arch::shutdown_with_code(0);
}
