#[macro_export]
macro_rules! late_initcall {
    ($func:ident) => {
        #[cfg_attr(
            target_vendor = "apple",
            unsafe(link_section = "__DATA,__sc_init_late")
        )]
        #[cfg_attr(target_os = "linux", unsafe(link_section = "sc_init_late"))]
        #[cfg_attr(
            not(any(target_os = "linux", target_vendor = "apple")),
            unsafe(link_section = ".initcall.late")
        )]
        #[used]
        static __LATE_INITCALL__: fn() = $func;
    };
}
