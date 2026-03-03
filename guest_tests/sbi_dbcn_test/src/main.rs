#![no_std]
#![no_main]

use core::arch::{asm, naked_asm};

const SBI_LEGACY_PUTCHAR: u64 = 0x01;
const SBI_DBCN: u64 = 0x4442434E;
const SBI_DBCN_WRITE: u64 = 0;
const SBI_DBCN_WRITE_BYTE: u64 = 2;
const SBI_SRST: u64 = 0x53525354;

#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
pub unsafe extern "C" fn _start() -> ! {
    naked_asm!("csrw sie, zero", "la sp, _stack_top", "call main", "j .",)
}

fn sbi_call(eid: u64, fid: u64, a0: u64, a1: u64, a2: u64) -> (i64, u64) {
    let error: i64;
    let value: u64;
    unsafe {
        asm!(
            "ecall",
            inout("a0") a0 as i64 => error,
            inout("a1") a1 => value,
            inout("a2") a2 => _,
            inout("a6") fid => _,
            inout("a7") eid => _,
            clobber_abi("C"),
            options(nostack),
        );
    }
    (error, value)
}

fn sbi_putchar(ch: u8) {
    let _ = sbi_call(SBI_LEGACY_PUTCHAR, 0, ch as u64, 0, 0);
}

fn sbi_dbcn_write_byte(ch: u8) {
    let _ = sbi_call(SBI_DBCN, SBI_DBCN_WRITE_BYTE, ch as u64, 0, 0);
}

fn sbi_dbcn_write(buf: &[u8]) -> usize {
    let (error, value) = sbi_call(
        SBI_DBCN,
        SBI_DBCN_WRITE,
        buf.len() as u64,
        buf.as_ptr() as u64,
        0,
    );
    if error == 0 {
        value as usize
    } else {
        0
    }
}

fn sbi_shutdown() {
    let _ = sbi_call(SBI_SRST, 0, 0, 0, 0);
    loop {}
}

fn print(s: &str) {
    for ch in s.bytes() {
        sbi_dbcn_write_byte(ch);
    }
}

fn println(s: &str) {
    print(s);
    sbi_dbcn_write_byte(b'\n');
}

#[unsafe(no_mangle)]
pub fn main() {
    println("SBI DBCN Test Start");

    print("Testing DBCN WRITE_BYTE: ");
    sbi_dbcn_write_byte(b'O');
    sbi_dbcn_write_byte(b'K');
    sbi_dbcn_write_byte(b'\n');

    print("Testing DBCN WRITE with buffer: ");
    let msg = b"Hello from DBCN WRITE!\n";
    let written = sbi_dbcn_write(msg);

    if written == msg.len() {
        println("OK (wrote all bytes)");
    } else {
        println("FAILED");
    }

    println("Testing legacy putchar: ");
    sbi_putchar(b'H');
    sbi_putchar(b'i');
    sbi_putchar(b'\n');

    println("SBI DBCN Test Done");
    sbi_shutdown();
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
