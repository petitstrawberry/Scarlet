#[macro_export]
macro_rules! driver_initcall {
    ($func:ident) => {
        #[unsafe(link_section = ".initcall.driver")]
        #[used(linker)]
        static __INITCALL__ : fn() = $func;
    };
}
