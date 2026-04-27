#![no_std]

extern "C" {
    fn print(ptr: *const u8, len: usize);
}

#[no_mangle]
pub unsafe extern "C" fn _start() -> i32 {
    let msg = b"Hello, World from Wasm!\n";
    unsafe {
        print(msg.as_ptr(), msg.len());
    }
    0
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
