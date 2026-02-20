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
        "la t0, {trap_handler}",
        "csrw stvec, t0",
        "j {main}",
        trap_handler = sym trap_handler,
        main = sym main,
    )
}

#[unsafe(no_mangle)]
extern "C" fn main() -> ! {
    print("Timer test start\n");
    
    // Set timer to 3 seconds (30M ticks at 10MHz)
    // Start from a reasonable base since we can't read time directly
    sbi_set_timer(30_000_000);
    
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

#[unsafe(no_mangle)]
extern "C" fn trap_handler() {
    let scause: u64;
    unsafe {
        asm!("csrr {0}, scause", out(reg) scause, options(nostack));
    }
    
    if scause == (1u64 << 63) | 5 {
        print("Tick\n");
        
        unsafe {
            TICK_COUNT += 1;
            if TICK_COUNT >= 3 {
                print("Timer test done!\n");
                sbi_shutdown();
            }
        }
        
        // Set next timer
        sbi_set_timer(30_000_000);
    } else {
        // Unknown trap, print scause
        let c = match scause {
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
    
    unsafe { asm!("sret", options(nostack)) };
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop { unsafe { asm!("wfi", options(nostack)) } }
}
