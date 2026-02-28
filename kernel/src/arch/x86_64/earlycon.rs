//! x86_64 early console support using VGA text mode and serial port

use core::arch::asm;
use core::ptr::{read_volatile, write_volatile};

const VGA_BUFFER: usize = 0xB8000;
const VGA_WIDTH: usize = 80;
const VGA_HEIGHT: usize = 25;

const SERIAL_LSR: u16 = 0x3FD;
const SERIAL_THR: u16 = 0x3F8;
const SERIAL_LSR_THRE: u8 = 0x20;

static mut CURSOR_ROW: usize = 0;
static mut CURSOR_COL: usize = 0;

unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    asm!(
        "in al, dx",
        in("dx") port,
        out("al") value,
        options(nostack, nomem)
    );
    value
}

unsafe fn outb(port: u16, value: u8) {
    asm!(
        "out dx, al",
        in("dx") port,
        in("al") value,
        options(nostack, nomem)
    );
}

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
        write_volatile(vga_buffer.add(offset), vga_entry(c, color));
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
                let val = read_volatile(vga_buffer.add(src));
                write_volatile(vga_buffer.add(dst), val);
            }
        }
        // Clear last row
        let color = vga_entry_color(VgaColor::LightGrey, VgaColor::Black);
        for col in 0..VGA_WIDTH {
            write_volatile(
                vga_buffer.add((VGA_HEIGHT - 1) * VGA_WIDTH + col),
                vga_entry(b' ', color),
            );
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

/// Write a byte to serial port (using port I/O)
fn serial_put_byte(byte: u8) {
    unsafe {
        // Wait for transmit holding register to be empty
        while (inb(SERIAL_LSR) & SERIAL_LSR_THRE) == 0 {
            core::hint::spin_loop();
        }
        // Write byte to transmit holding register
        outb(SERIAL_THR, byte);
    }
}

/// Early console putchar function for x86_64
///
/// This function provides character output during early boot before
/// the full driver is initialized. It outputs to both VGA and serial.
///
/// # Arguments
/// * `c` - Character to output
pub fn early_putc(c: u8) {
    // Output to VGA
    vga_put_byte(c);
    // Also output to serial for QEMU -debug mode
    serial_put_byte(c);
}

/// Initialize early console
pub fn init_earlycon() {
    // Clear VGA screen
    unsafe {
        let vga_buffer = VGA_BUFFER as *mut u16;
        let color = vga_entry_color(VgaColor::LightGrey, VgaColor::Black);
        for i in 0..(VGA_WIDTH * VGA_HEIGHT) {
            write_volatile(vga_buffer.add(i), vga_entry(b' ', color));
        }
        CURSOR_ROW = 0;
        CURSOR_COL = 0;
    }
}

/// Check if early console is available
pub fn is_earlycon_available() -> bool {
    true // VGA is always available on x86_64
}
