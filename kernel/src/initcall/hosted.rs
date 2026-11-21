//! Hosted initcall collection using the ranges synthesized by the host linker.

unsafe extern "C" {
    #[cfg_attr(
        target_os = "macos",
        link_name = "\u{1}section$start$__DATA$__sc_init_early"
    )]
    #[cfg_attr(target_os = "linux", link_name = "__start_sc_init_early")]
    static EARLY_START: u8;
    #[cfg_attr(
        target_os = "macos",
        link_name = "\u{1}section$end$__DATA$__sc_init_early"
    )]
    #[cfg_attr(target_os = "linux", link_name = "__stop_sc_init_early")]
    static EARLY_END: u8;
    #[cfg_attr(
        target_os = "macos",
        link_name = "\u{1}section$start$__DATA$__sc_init_driver"
    )]
    #[cfg_attr(target_os = "linux", link_name = "__start_sc_init_driver")]
    static DRIVER_START: u8;
    #[cfg_attr(
        target_os = "macos",
        link_name = "\u{1}section$end$__DATA$__sc_init_driver"
    )]
    #[cfg_attr(target_os = "linux", link_name = "__stop_sc_init_driver")]
    static DRIVER_END: u8;
    #[cfg_attr(
        target_os = "macos",
        link_name = "\u{1}section$start$__DATA$__sc_init_late"
    )]
    #[cfg_attr(target_os = "linux", link_name = "__start_sc_init_late")]
    static LATE_START: u8;
    #[cfg_attr(
        target_os = "macos",
        link_name = "\u{1}section$end$__DATA$__sc_init_late"
    )]
    #[cfg_attr(target_os = "linux", link_name = "__stop_sc_init_late")]
    static LATE_END: u8;
}

fn call_section(start: *const u8, end: *const u8) {
    let size = core::mem::size_of::<fn()>();
    let len = end
        .addr()
        .checked_sub(start.addr())
        .expect("invalid initcall section bounds");
    assert!(
        len.is_multiple_of(size),
        "invalid hosted initcall section size"
    );
    for offset in (0..len).step_by(size) {
        // SAFETY: These sections contain only aligned static function pointers
        // emitted by the corresponding initcall macros.
        let call = unsafe { start.add(offset).cast::<fn()>().read() };
        call();
    }
}

pub fn early_initcall_call() {
    call_section(&raw const EARLY_START, &raw const EARLY_END);
}

pub fn driver_initcall_call() {
    call_section(&raw const DRIVER_START, &raw const DRIVER_END);
}

pub fn call_initcalls() {
    call_section(&raw const LATE_START, &raw const LATE_END);
}
