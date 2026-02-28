//! x86_64 early console support using VGA text mode
//!
//! x86_64 QEMU typically supports VGA text mode at 0xB8000
//! This provides early debugging output before the full driver system is ready.

use core::arch::asm;
use core::fmt;

const VGA_BUFFER: usize = 0xB8000;
const VGA_WIDTH: usize = 80;
const VGA_HEIGHT: usize = 25;

static mut CURSOR_ROW: usize = 0;
static mut CURSOR_COL: usize = 0;

/// Color codes for VGA text mode
#[allow(dead_code)]
#[repr(u8)]
enum VgaColor {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGrey = 7,
    DarkGrey = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    LightMagenta = 13,
    Yellow = 14,
    White = 15,
}

const fn vga_entry_color(fg: VgaColor, bg: VgaColor) -> u8 {
    (fg as u8) | ((bg as u8) << 4)
}

const fn vga_entry(ascii: u8, color: u8) -> u16 {
    (ascii as u16) | ((color as u16) << 8)
}

fn vga_write_char(c: u8, color: u8) {
    let offset = (unsafe { CURSOR_ROW } * VGA_WIDTH) + unsafe { CURSOR_COL };
    unsafe {
        let vga_buffer = VGA_BUFFER as *mut u16;
        *vga_buffer.add(offset) = vga_entry(c, color);
    }
}

fn vga_scroll_up() {
    unsafe {
        let vga_buffer = VGA_BUFFER as *mut u16;
        // Copy each row up
        for row in 1..VGA_HEIGHT {
            for col in 0..VGA_WIDTH {
                let src = row * VGA_WIDTH + col;
                let dst = (row - 1) * VGA_WIDTH + col;
                *vga_buffer.add(dst) = *vga_buffer.add(src);
            }
        }
        // Clear last row
        let color = vga_entry_color(VgaColor::LightGrey, VgaColor::Black);
        for col in 0..VGA_WIDTH {
            *vga_buffer.add((VGA_HEIGHT - 1) * VGA_WIDTH + col) = vga_entry(b' ', color);
        }
        // Move cursor up
        if CURSOR_ROW > 0 {
            CURSOR_ROW -= 1;
        }
    }
}

fn vga_put_byte(byte: u8) {
    let color = vga_entry_color(VgaColor::LightGrey, VgaColor::Black);

    match byte {
        b'\n' => unsafe {
            CURSOR_COL = 0;
            if CURSOR_ROW < VGA_HEIGHT - 1 {
                CURSOR_ROW += 1;
            } else {
                vga_scroll_up();
            }
        },
        b'\r' => unsafe {
            CURSOR_COL = 0;
        },
        b'\t' => unsafe {
            CURSOR_COL = (CURSOR_COL + 8) & !7;
            if CURSOR_COL >= VGA_WIDTH {
                CURSOR_COL = 0;
                if CURSOR_ROW < VGA_HEIGHT - 1 {
                    CURSOR_ROW += 1;
                } else {
                    vga_scroll_up();
                }
            }
        },
        0x20..=0x7E | 0xA0.. => {
            vga_write_char(byte, color);
            unsafe {
                CURSOR_COL += 1;
                if CURSOR_COL >= VGA_WIDTH {
                    CURSOR_COL = 0;
                    if CURSOR_ROW < VGA_HEIGHT - 1 {
                        CURSOR_ROW += 1;
                    } else {
                        vga_scroll_up();
                    }
                }
            }
        }
        _ => {
            // Print replacement character for non-printable
            vga_write_char(0xFE, color);
            unsafe {
                CURSOR_COL += 1;
                if CURSOR_COL >= VGA_WIDTH {
                    CURSOR_COL = 0;
                    if CURSOR_ROW < VGA_HEIGHT - 1 {
                        CURSOR_ROW += 1;
                    } else {
                        vga_scroll_up();
                    }
                }
            }
        }
    }
}

pub struct EarlyWriter;

impl fmt::Write for EarlyWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            vga_put_byte(byte);
        }
        Ok(())
    }
}

/// Print to the early console (VGA text mode)
#[macro_export]
macro_rules! early_print {
    ($($arg:tt)*) => ({
        use core::fmt::Write;
        let _ = write!($crate::arch::x86_64::earlycon::EarlyWriter, $($arg)*);
    });
}

/// Print to the early console with newline
#[macro_export]
macro_rules! early_println {
    () => ($crate::early_print!("\n"));
    ($($arg:tt)*) => ($crate::early_print!("{}\n", format_args!($($arg)*)));
}

pub fn init_earlycon() {
    // Clear screen
    unsafe {
        let vga_buffer = VGA_BUFFER as *mut u16;
        let color = vga_entry_color(VgaColor::LightGrey, VgaColor::Black);
        for i in 0..(VGA_WIDTH * VGA_HEIGHT) {
            *vga_buffer.add(i) = vga_entry(b' ', color);
        }
        CURSOR_ROW = 0;
        CURSOR_COL = 0;
    }
}

/// Check if early console is available
pub fn is_earlycon_available() -> bool {
    true // VGA is always available on x86_64
}

/// Serial port output for QEMU (optional, more portable)
const SERIAL_PORT: u16 = 0x3F8;

fn serial_put_byte(byte: u8) {
    unsafe {
        // Wait for transmit buffer empty
        asm!(
            "1: in al, dx",
            "test al, 0x20",
            "jz 1b",
            "2: out dx, al",
            in("dx") SERIAL_PORT + 5u16, // LSR
            inlateout("al") 0u8 => _,
            in("dx") SERIAL_PORT, // THR
            in("al") byte,
            options(nostack)
        );
    }
}

/// Output byte to both VGA and serial
pub fn earlycon_put_byte(byte: u8) {
    vga_put_byte(byte);
    serial_put_byte(byte);
}
