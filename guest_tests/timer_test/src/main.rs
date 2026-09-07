#![no_std]
#![no_main]

use core::arch::{asm, naked_asm};

const SBI_LEGACY_PUTCHAR: u64 = 0x01;
const SBI_TIMER: u64 = 0x54494D45;
const SBI_SRST: u64 = 0x53525354;

static mut TICK_COUNT: u32 = 0;

fn sbi_putchar(c: char) {
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

fn sbi_set_timer(time: u64) {
    unsafe {
        asm!(
            "ecall",
            in("a7") SBI_TIMER,
            in("a6") 0u64,
            in("a0") time,
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
    for c in s.chars() {
        sbi_putchar(c);
    }
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
    print("Timer test start\n");
    
    // Set timer to 3 seconds (30M ticks at 10MHz)
    // Start from a reasonable base since we can't read time directly
    let time: u64;
    unsafe {
        asm!("rdtime {0}", out(reg) time, options(nostack));
    }

    sbi_set_timer(30_000_000 + time);
    
    // Enable timer interrupt (sie.stie = bit 5)
    unsafe {
        asm!("csrw sie, {0}", in(reg) 1u64 << 5, options(nostack));
        asm!("csrs sstatus, {0}", in(reg) 1u64 << 1, options(nostack));
    }
    
    print("Timer set, waiting...\n");
    
    loop {
        unsafe { asm!("wfi", options(nostack)) };
    }
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
unsafe extern "C" fn trap_entry() {
    naked_asm!(
        ".align 8",
        "addi sp, sp, -256",
        // "sd x0, 0(sp)",
        "sd x1, 8(sp)",
        // "sd x2, 16(sp)",
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
        "jal {trap_handler}",
        "ld x1, 8(sp)",
        // "ld x2, 16(sp)",
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
    );
}

#[unsafe(no_mangle)]
extern "C" fn trap_handler(cause: u64) {
    if cause == (1u64 << 63) | 5 {
        print("Tick\n");
        
        unsafe {
            TICK_COUNT += 1;
            if TICK_COUNT >= 3 {
                print("Timer test done!\n");
                sbi_shutdown();
            }
        }
        
        // Set next timer
        let time: u64;
        unsafe {
            asm!("rdtime {0}", out(reg) time, options(nostack));
        }
        sbi_set_timer(30_000_000 + time);
    } else {
        // Unknown trap, print scause
        let c = match cause {
            2 => '2',
            8 => 'I',  // ECALL from U-mode
            10 => 'E', // ECALL from VS-mode  
            12 => 'F', // Instruction page fault
            13 => 'G', // Load page fault
            22 => 'V', // Virtual instruction
            _ => '?',
        };
        sbi_putchar(c);
        sbi_putchar('\n');
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop { unsafe { asm!("wfi", options(nostack)) } }
}
