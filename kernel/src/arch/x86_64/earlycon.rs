//! x86_64 early console support using serial port

use core::arch::asm;

const SERIAL_LSR: u16 = 0x3FD;
const SERIAL_THR: u16 = 0x3F8;
const SERIAL_LSR_THRE: u8 = 0x20;

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

fn serial_put_byte(byte: u8) {
    unsafe {
        while (inb(SERIAL_LSR) & SERIAL_LSR_THRE) == 0 {
            core::hint::spin_loop();
        }
        outb(SERIAL_THR, byte);
    }
}

pub fn early_putc(c: u8) {
    serial_put_byte(c);
}

pub fn init_earlycon() {}

pub fn is_earlycon_available() -> bool {
    true
}
