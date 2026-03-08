use core::panic::PanicInfo;

use crate::arch;
use crate::println;

pub trait TestableFn {
    fn run(&self) -> ();
}

impl<T> TestableFn for T
where
    T: Fn(),
{
    fn run(&self) {
        println!("[Test Runner] test name={}", core::any::type_name::<T>());
        self();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("[Scarlet Kernel] panic: {}", info);
    println!("[Test Runner] Test failed");

    #[cfg(feature = "profiler")]
    {
        use crate::profiler;
        profiler::print_profiling_results();
    }

    crate::arch::shutdown_with_code(1);
}

#[cfg(test)]
pub fn test_runner(tests: &[&dyn TestableFn]) {
    println!("[Test Runner] Running {} tests", tests.len());
    for test in tests {
        // println!("[Test Runner] Running test: {:?}", test as *const _);
        test.run();
    }

    println!("[Test Runner] All {} tests passed", tests.len());

    #[cfg(feature = "profiler")]
    {
        use crate::profiler;
        crate::println!("[Profiler] Printing profiling results:");
        profiler::print_profiling_results();
    }
    crate::arch::shutdown_with_code(0);
}
