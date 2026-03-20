#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::fs::File;
use std::println;
use std::string::String;
use std::vec::Vec;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct InputEvent {
    time: u64,
    type_: u16,
    code: u16,
    value: i32,
}

impl InputEvent {
    const SIZE: usize = core::mem::size_of::<Self>();
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).map(|s| s.as_str()).unwrap_or("/dev/keyboard0");

    println!("input_dump: opening {}", path);

    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) => {
            println!("input_dump: failed to open {}: {:?}", path, err);
            return 1;
        }
    };

    println!("input_dump: waiting for input events...");

    let mut buffer = [0u8; InputEvent::SIZE];
    loop {
        match file.read(&mut buffer) {
            Ok(bytes_read) if bytes_read == InputEvent::SIZE => {
                let event =
                    unsafe { core::ptr::read_unaligned(buffer.as_ptr() as *const InputEvent) };
                println!(
                    "input: type={} code={} value={} time={}",
                    event.type_, event.code, event.value, event.time
                );
            }
            Ok(bytes_read) => {
                println!("input_dump: short read {}", bytes_read);
            }
            Err(err) => {
                println!("input_dump: read error {:?}", err);
                return 1;
            }
        }
    }
}
