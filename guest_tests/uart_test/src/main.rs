#![no_std]
#![no_main]

use core::arch::{asm, naked_asm};
use core::ptr::{read_volatile, write_volatile};

const UART_BASE: u64 = 0x10000000;
const UART_RBR: u64 = 0x00;
const UART_IER: u64 = 0x01;
const UART_FCR: u64 = 0x02;
const UART_LSR: u64 = 0x05;

const UART_LSR_RX_READY: u8 = 0x01;
const UART_LSR_TX_EMPTY: u8 = 0x20;

const PLIC_BASE: u64 = 0x0C000000;
const PLIC_PRIORITY_BASE: u64 = 0x0000;
const PLIC_ENABLE_BASE: u64 = 0x2000;
const PLIC_CONTEXT_BASE: u64 = 0x200000;
const PLIC_THRESHOLD_OFFSET: u64 = 0x0000;
const PLIC_CLAIM_OFFSET: u64 = 0x0004;

const UART_IRQ: u32 = 10;
const PLIC_CONTEXT_SMODE: usize = 1;

const SBI_LEGACY_PUTCHAR: u64 = 0x01;
const SBI_SRST: u64 = 0x53525354;

static mut RUNNING: bool = true;

fn plic_write32(offset: u64, val: u32) {
    unsafe {
        write_volatile((PLIC_BASE + offset) as *mut u32, val);
    }
}

fn plic_read32(offset: u64) -> u32 {
    unsafe { read_volatile((PLIC_BASE + offset) as *const u32) }
}

fn plic_set_priority(irq: u32, prio: u32) {
    plic_write32(PLIC_PRIORITY_BASE + (irq as u64 * 4), prio);
}

fn plic_set_enable(context: usize, irq: u32, enable: bool) {
    let word = irq / 32;
    let bit = irq % 32;
    let offset = PLIC_ENABLE_BASE + (context as u64 * 0x80) + (word as u64 * 4);
    let mut val = plic_read32(offset);
    if enable {
        val |= 1 << bit;
    } else {
        val &= !(1 << bit);
    }
    plic_write32(offset, val);
}

fn plic_set_threshold(context: usize, threshold: u32) {
    let offset = PLIC_CONTEXT_BASE + (context as u64 * 0x1000) + PLIC_THRESHOLD_OFFSET;
    plic_write32(offset, threshold);
}

fn plic_claim(context: usize) -> u32 {
    let offset = PLIC_CONTEXT_BASE + (context as u64 * 0x1000) + PLIC_CLAIM_OFFSET;
    plic_read32(offset)
}

fn plic_complete(context: usize, irq: u32) {
    let offset = PLIC_CONTEXT_BASE + (context as u64 * 0x1000) + PLIC_CLAIM_OFFSET;
    plic_write32(offset, irq);
}

fn uart_read_reg(reg: u64) -> u8 {
    unsafe { read_volatile((UART_BASE + reg) as *const u8) }
}

fn uart_write_reg(reg: u64, val: u8) {
    unsafe { write_volatile((UART_BASE + reg) as *mut u8, val) }
}

fn uart_getc() -> Option<u8> {
    let lsr = uart_read_reg(UART_LSR);
    if lsr & UART_LSR_RX_READY != 0 {
        Some(uart_read_reg(UART_RBR))
    } else {
        None
    }
}

fn sbi_putchar(c: u8) {
    unsafe {
        asm!(
            "ecall",
            in("a7") SBI_LEGACY_PUTCHAR,
            in("a0") c as u64,
            lateout("a0") _,
            lateout("a1") _,
            options(nostack)
        );
    }
}

fn sbi_shutdown() -> ! {
    unsafe {
        asm!(
            "ecall",
            in("a7") SBI_SRST,
            in("a6") 0u64,
            in("a0") 0u64,
            in("a1") 0u64,
            options(nostack)
        );
    }
    loop {
        unsafe { asm!("wfi", options(nostack)) };
    }
}

fn print(s: &str) {
    for c in s.bytes() {
        sbi_putchar(c);
    }
}

fn print_char(c: u8) {
    sbi_putchar(c);
}

fn uart_init() {
    uart_write_reg(UART_IER, 0x00);
    uart_write_reg(UART_FCR, 0x07);
    uart_write_reg(UART_IER, 0x01);
}

fn plic_init() {
    plic_set_priority(UART_IRQ, 1);
    plic_set_threshold(PLIC_CONTEXT_SMODE, 0);
    plic_set_enable(PLIC_CONTEXT_SMODE, UART_IRQ, true);
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.init")]
unsafe extern "C" fn _start() -> ! {
    naked_asm!(
        "li sp, 0x80100000",
        "la t0, {trap_entry}",
        "csrw stvec, t0",
        "j {main}",
        trap_entry = sym trap_entry,
        main = sym main,
    )
}

#[unsafe(no_mangle)]
extern "C" fn main() -> ! {
    print("UART test start\n");

    plic_init();
    print("PLIC initialized\n");

    uart_init();
    print("UART initialized\n");

    unsafe {
        asm!("csrw sie, {0}", in(reg) (1u64 << 9) | (1u64 << 5), options(nostack));
        asm!("csrs sstatus, {0}", in(reg) 1u64 << 1, options(nostack));
    }

    print("Interrupts enabled (SIE=1, SEIE=1, STIE=1)\n");
    print("Type characters. Press 'q' to quit.\n");

    loop {
        unsafe {
            asm!("wfi", options(nostack));
            if !RUNNING {
                break;
            }
        }
    }

    print("UART test done\n");
    sbi_shutdown();
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
unsafe extern "C" fn trap_entry() {
    naked_asm!(
        ".align 8",
        "addi sp, sp, -256",
        "sd x1, 8(sp)",
        "sd x3, 24(sp)",
        "sd x4, 32(sp)",
        "sd x5, 40(sp)",
        "sd x6, 48(sp)",
        "sd x7, 56(sp)",
        "sd x8, 64(sp)",
        "sd x9, 72(sp)",
        "sd x10, 80(sp)",
        "sd x11, 88(sp)",
        "sd x12, 96(sp)",
        "sd x13, 104(sp)",
        "sd x14, 112(sp)",
        "sd x15, 120(sp)",
        "sd x16, 128(sp)",
        "sd x17, 136(sp)",
        "sd x18, 144(sp)",
        "sd x19, 152(sp)",
        "sd x20, 160(sp)",
        "sd x21, 168(sp)",
        "sd x22, 176(sp)",
        "sd x23, 184(sp)",
        "sd x24, 192(sp)",
        "sd x25, 200(sp)",
        "sd x26, 208(sp)",
        "sd x27, 216(sp)",
        "sd x28, 224(sp)",
        "sd x29, 232(sp)",
        "sd x30, 240(sp)",
        "sd x31, 248(sp)",
        "csrr a0, scause",
        "csrr a1, sepc",
        "jal {trap_handler}",
        "ld x1, 8(sp)",
        "ld x3, 24(sp)",
        "ld x4, 32(sp)",
        "ld x5, 40(sp)",
        "ld x6, 48(sp)",
        "ld x7, 56(sp)",
        "ld x8, 64(sp)",
        "ld x9, 72(sp)",
        "ld x10, 80(sp)",
        "ld x11, 88(sp)",
        "ld x12, 96(sp)",
        "ld x13, 104(sp)",
        "ld x14, 112(sp)",
        "ld x15, 120(sp)",
        "ld x16, 128(sp)",
        "ld x17, 136(sp)",
        "ld x18, 144(sp)",
        "ld x19, 152(sp)",
        "ld x20, 160(sp)",
        "ld x21, 168(sp)",
        "ld x22, 176(sp)",
        "ld x23, 184(sp)",
        "ld x24, 192(sp)",
        "ld x25, 200(sp)",
        "ld x26, 208(sp)",
        "ld x27, 216(sp)",
        "ld x28, 224(sp)",
        "ld x29, 232(sp)",
        "ld x30, 240(sp)",
        "ld x31, 248(sp)",
        "addi sp, sp, 256",
        "sret",
        trap_handler = sym trap_handler,
    )
}

#[unsafe(no_mangle)]
extern "C" fn trap_handler(scause: u64, _sepc: u64) {
    let is_interrupt = (scause >> 63) & 1 == 1;
    let cause = scause & 0x7FFF_FFFF_FFFF_FFFF;

    if is_interrupt {
        match cause {
            9 => {
                let irq = plic_claim(PLIC_CONTEXT_SMODE);
                if irq == UART_IRQ {
                    if let Some(c) = uart_getc() {
                        print_char(c);
                        if c == b'q' {
                            print("\nExiting from interrupt...\n");
                            unsafe {
                                RUNNING = false;
                            }
                        }
                    }
                    plic_complete(PLIC_CONTEXT_SMODE, irq);
                }
            }
            5 => {}
            _ => {}
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        unsafe { asm!("wfi", options(nostack)) }
    }
}
