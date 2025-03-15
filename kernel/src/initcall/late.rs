#[macro_export]
macro_rules! late_initcall {
    ($func:ident) => {
        #[unsafe(link_section = ".initcall.late")]
        #[used(linker)]
        static __INITCALL__ : fn() = $func;
    };
}