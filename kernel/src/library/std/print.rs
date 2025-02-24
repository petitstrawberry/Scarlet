use core::fmt::{self, Write};

use crate::driver::uart::virt::Uart;
use crate::traits::serial::Serial;

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::library::std::print::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    ($fmt:expr) => (print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => (print!(concat!($fmt, "\n"), $($arg)*));
}

pub fn _print(args: fmt::Arguments) {
    unsafe {
        match UART_WRITER {
            Some(ref mut writer) => writer.write_fmt(args).unwrap(),
            None => {
                UART_WRITER = Some(UartWriter {
                    serial: Uart::new(0x1000_0000),
                });
                if let Some(ref mut writer) = UART_WRITER {
                    writer.serial.init();
                }
                _print(args);
            }
            
        }
    }
}

static mut UART_WRITER: Option<UartWriter> = None;

#[derive(Clone)]
struct UartWriter {
    serial: Uart,
}

impl Write for UartWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.bytes() {
            if c == b'\n' {
                self.serial.write_byte(b'\r');
            }
            self.serial.write_byte(c);
        }
        Ok(())
    }
}
