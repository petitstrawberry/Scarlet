#[unsafe(link_section = ".lsm_symbols")]
#[used]
static _FORCE_SECTION: usize = 0;
static KERNEL_SYMBOLS: [(&'static str, usize); 0] = [];
pub fn get_kernel_symbols() -> &'static [(&'static str, usize)] { &KERNEL_SYMBOLS }