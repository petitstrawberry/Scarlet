#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;

use std::task::exit;

#[unsafe(no_mangle)]
pub extern "C" fn main(_argc: isize, _argv: *const *const u8) -> isize {
    let code = scarlet_youtube_net::run_cli();
    exit(code as i32);
}
