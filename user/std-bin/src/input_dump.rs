use std::env;
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

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

fn main() -> ExitCode {
    println!("input-dump: Rust std version");

    let args = env::args().collect::<Vec<_>>();
    let path = args.get(1).map(String::as_str).unwrap_or("/dev/keyboard0");

    println!("input-dump: opening {path}");
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) => {
            println!("input-dump: failed to open {path}: {err}");
            return ExitCode::from(1);
        }
    };

    println!("input-dump: waiting for input events...");
    let mut buffer = [0; InputEvent::SIZE];
    loop {
        match file.read_exact(&mut buffer) {
            Ok(()) => {
                let event =
                    unsafe { core::ptr::read_unaligned(buffer.as_ptr() as *const InputEvent) };
                println!(
                    "input: type={} code={} value={} time={}",
                    event.type_, event.code, event.value, event.time
                );
            }
            Err(err) => {
                println!("input-dump: read error {err}");
                return ExitCode::from(1);
            }
        }
    }
}
