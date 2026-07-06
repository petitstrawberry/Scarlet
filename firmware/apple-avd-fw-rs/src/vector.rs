//! Cortex-M3 vector table for the AVD firmware.

use core::arch::global_asm;

use crate::abi::MSG_PANIC;
use crate::irq;
use crate::mailbox::send_message;

#[cfg(feature = "v2-t0")]
global_asm!(".equ __avd_stack_top, 0x1000c000");
#[cfg(any(feature = "v3-t0", feature = "v3-t1"))]
global_asm!(".equ __avd_stack_top, 0x10010000");
#[cfg(any(feature = "v4-t0", feature = "v5-t0", feature = "v5-t1"))]
global_asm!(".equ __avd_stack_top, 0x10012000");

global_asm!(
    r#"
    .section .vector_table, "a", %progbits
    .word __avd_stack_top
    .word reset_handler
    .word nmi_handler
    .word hardfault_handler
    .word default_exception_handler
    .word default_exception_handler
    .word default_exception_handler
    .word 0
    .word 0
    .word 0
    .word 0
    .word default_exception_handler
    .word default_exception_handler
    .word 0
    .word default_exception_handler
    .word systick_handler
    .word irq0_handler
    .word irq1_handler
    .word irq2_handler
    .word irq3_handler
    .word irq4_handler
    .word irq5_handler
    .word irq6_handler
    .word irq7_handler
    .word irq8_handler
    .word irq9_handler
    .word irq10_handler
    .word irq11_handler
    .word irq12_handler
    .word irq13_handler
    .word irq14_handler
    .word irq15_handler
    .word irq16_handler
    .word irq17_handler
    .word irq18_handler
    .word irq19_handler
    .word irq20_handler
    .word irq21_handler
    .word irq22_handler
    .word irq23_handler
    .word irq24_handler
    .word irq25_handler
    .word irq26_handler
    .word irq27_handler
    .word irq28_handler
    .word irq29_handler
    .word irq30_handler
    .word irq31_handler
    .word irq32_handler
    .word irq33_handler
    .word irq34_handler
    .word irq35_handler
    .word irq36_handler
    .word irq37_handler
    .word irq38_handler
    .word irq39_handler
    .word irq40_handler
    .word irq41_handler
    .word irq42_handler
    .word irq43_handler
    .word irq44_handler
    .word irq45_handler
    .word irq46_handler
    .word irq47_handler
    .word irq48_handler
    .word irq49_handler
    .word irq50_handler
    .word irq51_handler
    .word irq52_handler
    .word irq53_handler
    .word irq54_handler
    .word irq55_handler
    .word irq56_handler
    .word irq57_handler
    .word irq58_handler
    .word irq59_handler
    .word irq60_handler
    .word irq61_handler
    .word irq62_handler
    .word irq63_handler
    .word irq64_handler
    .word irq65_handler
    .word irq66_handler
    .word irq67_handler
    .word irq68_handler
    .word irq69_handler
    .word irq70_handler
    .word irq71_handler
    .word irq72_handler
    .word irq73_handler
    .word irq74_handler
    .word irq75_handler
    .word irq76_handler
    .word irq77_handler
    .word irq78_handler
    .word irq79_handler
    .word irq80_handler
    .word irq81_handler
    .word irq82_handler
    .word irq83_handler
    .word irq84_handler
    .word irq85_handler
    .word irq86_handler
    .word irq87_handler
    .word irq88_handler
    .word irq89_handler
    .word irq90_handler
    .word irq91_handler
    .word irq92_handler
    .word irq93_handler
    .word irq94_handler
    .word irq95_handler
    .word irq96_handler
    .word irq97_handler
    .word irq98_handler
    .word irq99_handler
    .word irq100_handler
    .word irq101_handler
    .word irq102_handler
    .word irq103_handler
    .word irq104_handler
    .word irq105_handler
    .word irq106_handler
    .word irq107_handler
    .word irq108_handler
    .word irq109_handler
    .word irq110_handler
    .word irq111_handler
    .word irq112_handler
    .word irq113_handler
    .word irq114_handler
    .word irq115_handler
    .word irq116_handler
    .word irq117_handler
    .word irq118_handler
    .word irq119_handler
    .word irq120_handler
    .word irq121_handler
    .word irq122_handler
    .word irq123_handler
    .word irq124_handler
    .word irq125_handler
    .word irq126_handler
    .word irq127_handler
    .word irq128_handler
    .word irq129_handler
    .word irq130_handler
    .word irq131_handler
    .word irq132_handler
    .word irq133_handler
    .word irq134_handler
    .word irq135_handler
    .word irq136_handler
    .word irq137_handler
    .word irq138_handler
    .word irq139_handler
    .word irq140_handler
    "#
);

macro_rules! default_irq_handler {
    ($name:ident, $irq:expr) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name() {
            irq::unknown_irq($irq);
        }
    };
}

macro_rules! pipe_irq_handlers {
    ($unk:ident, $err:ident, $done:ident, $pipe:expr) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $unk() {
            irq::video_pipe_unknown($pipe);
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn $err() {
            irq::video_pipe_error($pipe);
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn $done() {
            irq::video_pipe_done($pipe);
        }
    };
}

macro_rules! submit_unknown_irq_handler {
    ($name:ident) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name() {
            irq::submit_unknown();
        }
    };
}

macro_rules! post_process_irq_handler {
    ($name:ident) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name() {
            irq::post_process_done();
        }
    };
}

macro_rules! h264_status_irq_handler {
    ($name:ident) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name() {
            irq::h264_status_irq1();
        }
    };
}

/// Non-maskable interrupt handler.
#[unsafe(no_mangle)]
pub extern "C" fn nmi_handler() -> ! {
    send_message(MSG_PANIC | 0x10);
    loop {
        core::hint::spin_loop();
    }
}

/// HardFault handler.
#[unsafe(no_mangle)]
pub extern "C" fn hardfault_handler() -> ! {
    send_message(MSG_PANIC | 0x20);
    loop {
        core::hint::spin_loop();
    }
}

/// Default exception handler.
#[unsafe(no_mangle)]
pub extern "C" fn default_exception_handler() {
    send_message(MSG_PANIC | 0x30);
}

/// SysTick handler.
#[unsafe(no_mangle)]
pub extern "C" fn systick_handler() {
    irq::unknown_irq(15);
}

pipe_irq_handlers!(irq18_handler, irq19_handler, irq20_handler, 0);
pipe_irq_handlers!(irq23_handler, irq24_handler, irq25_handler, 1);
pipe_irq_handlers!(irq28_handler, irq29_handler, irq30_handler, 2);
pipe_irq_handlers!(irq33_handler, irq34_handler, irq35_handler, 3);
submit_unknown_irq_handler!(irq38_handler);
post_process_irq_handler!(irq40_handler);

h264_status_irq_handler!(irq1_handler);

submit_unknown_irq_handler!(irq62_handler);
post_process_irq_handler!(irq64_handler);
pipe_irq_handlers!(irq78_handler, irq79_handler, irq80_handler, 0);
pipe_irq_handlers!(irq83_handler, irq84_handler, irq85_handler, 1);
pipe_irq_handlers!(irq88_handler, irq89_handler, irq90_handler, 2);
pipe_irq_handlers!(irq93_handler, irq94_handler, irq95_handler, 3);
pipe_irq_handlers!(irq98_handler, irq99_handler, irq100_handler, 4);
pipe_irq_handlers!(irq103_handler, irq104_handler, irq105_handler, 5);
pipe_irq_handlers!(irq108_handler, irq109_handler, irq110_handler, 6);
pipe_irq_handlers!(irq113_handler, irq114_handler, irq115_handler, 7);
pipe_irq_handlers!(irq118_handler, irq119_handler, irq120_handler, 8);
pipe_irq_handlers!(irq123_handler, irq124_handler, irq125_handler, 9);
pipe_irq_handlers!(irq128_handler, irq129_handler, irq130_handler, 10);
pipe_irq_handlers!(irq133_handler, irq134_handler, irq135_handler, 11);

default_irq_handler!(irq0_handler, 0);
default_irq_handler!(irq2_handler, 2);
default_irq_handler!(irq3_handler, 3);
default_irq_handler!(irq4_handler, 4);
default_irq_handler!(irq5_handler, 5);
default_irq_handler!(irq6_handler, 6);
default_irq_handler!(irq7_handler, 7);
default_irq_handler!(irq8_handler, 8);
default_irq_handler!(irq9_handler, 9);
default_irq_handler!(irq10_handler, 10);
default_irq_handler!(irq11_handler, 11);
default_irq_handler!(irq12_handler, 12);
default_irq_handler!(irq13_handler, 13);
default_irq_handler!(irq14_handler, 14);
default_irq_handler!(irq15_handler, 15);
default_irq_handler!(irq16_handler, 16);
default_irq_handler!(irq17_handler, 17);
default_irq_handler!(irq21_handler, 21);
default_irq_handler!(irq22_handler, 22);
default_irq_handler!(irq26_handler, 26);
default_irq_handler!(irq27_handler, 27);
default_irq_handler!(irq31_handler, 31);
default_irq_handler!(irq32_handler, 32);
default_irq_handler!(irq36_handler, 36);
default_irq_handler!(irq37_handler, 37);
default_irq_handler!(irq39_handler, 39);
default_irq_handler!(irq41_handler, 41);
default_irq_handler!(irq42_handler, 42);
default_irq_handler!(irq43_handler, 43);
default_irq_handler!(irq44_handler, 44);
default_irq_handler!(irq45_handler, 45);
default_irq_handler!(irq46_handler, 46);
default_irq_handler!(irq47_handler, 47);
default_irq_handler!(irq48_handler, 48);
default_irq_handler!(irq49_handler, 49);
default_irq_handler!(irq50_handler, 50);
default_irq_handler!(irq51_handler, 51);
default_irq_handler!(irq52_handler, 52);
default_irq_handler!(irq53_handler, 53);
default_irq_handler!(irq54_handler, 54);
default_irq_handler!(irq55_handler, 55);
default_irq_handler!(irq56_handler, 56);
default_irq_handler!(irq57_handler, 57);
default_irq_handler!(irq58_handler, 58);
default_irq_handler!(irq59_handler, 59);
default_irq_handler!(irq60_handler, 60);
default_irq_handler!(irq61_handler, 61);
default_irq_handler!(irq63_handler, 63);
default_irq_handler!(irq65_handler, 65);
default_irq_handler!(irq66_handler, 66);
default_irq_handler!(irq67_handler, 67);
default_irq_handler!(irq68_handler, 68);
default_irq_handler!(irq69_handler, 69);
default_irq_handler!(irq70_handler, 70);
default_irq_handler!(irq71_handler, 71);
default_irq_handler!(irq72_handler, 72);
default_irq_handler!(irq73_handler, 73);
default_irq_handler!(irq74_handler, 74);
default_irq_handler!(irq75_handler, 75);
default_irq_handler!(irq76_handler, 76);
default_irq_handler!(irq77_handler, 77);
default_irq_handler!(irq81_handler, 81);
default_irq_handler!(irq82_handler, 82);
default_irq_handler!(irq86_handler, 86);
default_irq_handler!(irq87_handler, 87);
default_irq_handler!(irq91_handler, 91);
default_irq_handler!(irq92_handler, 92);
default_irq_handler!(irq96_handler, 96);
default_irq_handler!(irq97_handler, 97);
default_irq_handler!(irq101_handler, 101);
default_irq_handler!(irq102_handler, 102);
default_irq_handler!(irq106_handler, 106);
default_irq_handler!(irq107_handler, 107);
default_irq_handler!(irq111_handler, 111);
default_irq_handler!(irq112_handler, 112);
default_irq_handler!(irq116_handler, 116);
default_irq_handler!(irq117_handler, 117);
default_irq_handler!(irq121_handler, 121);
default_irq_handler!(irq122_handler, 122);
default_irq_handler!(irq126_handler, 126);
default_irq_handler!(irq127_handler, 127);
default_irq_handler!(irq131_handler, 131);
default_irq_handler!(irq132_handler, 132);
default_irq_handler!(irq136_handler, 136);
default_irq_handler!(irq137_handler, 137);
default_irq_handler!(irq138_handler, 138);
default_irq_handler!(irq139_handler, 139);
default_irq_handler!(irq140_handler, 140);
