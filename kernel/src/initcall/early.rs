#[macro_export]
macro_rules! early_initcall {
    ($func:ident) => {
        #[unsafe(link_section = ".initcall.early")]
        #[used(linker)]
        static __INITCALL__ : fn() = $func;
    };
}