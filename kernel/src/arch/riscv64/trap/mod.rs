use core::arch::asm;
use core::arch::naked_asm;
use core::mem::transmute;

use crate::arch::instruction::sbi::sbi_set_timer;
use crate::println;
use crate::print;

use super::Riscv64;

#[unsafe(export_name = "_trap_entry")]
#[naked]
pub extern "C" fn _trap_entry() {
    unsafe {
        naked_asm!("
        .option norvc
        .option norelax
        .align 8
                /* Save the context of the current hart */
                /* Save the current sp to sscratch and load the trap stack pointer */
                csrci   sstatus, 0x2
                csrrw   sp, sscratch, sp
                addi    sp, sp, -272
                sd      x0, 0(sp)
                sd      x1, 8(sp)
                // sd      x2, 16(sp)
                sd      x3, 24(sp)
                sd      x4, 32(sp)
                sd      x5, 40(sp)
                sd      x6, 48(sp)
                sd      x7, 56(sp)
                sd      x8, 64(sp)
                sd      x9, 72(sp)
                sd      x10, 80(sp)
                sd      x11, 88(sp)
                sd      x12, 96(sp)
                sd      x13, 104(sp)
                sd      x14, 112(sp)
                sd      x15, 120(sp)
                sd      x16, 128(sp)
                sd      x17, 136(sp)
                sd      x18, 144(sp)
                sd      x19, 152(sp)
                sd      x20, 160(sp)
                sd      x21, 168(sp)
                sd      x22, 176(sp)
                sd      x23, 184(sp)
                sd      x24, 192(sp)
                sd      x25, 200(sp)
                sd      x26, 208(sp)
                sd      x27, 216(sp)
                sd      x28, 224(sp)
                sd      x29, 232(sp)
                sd      x30, 240(sp)
                sd      x31, 248(sp)
                csrr    t0, sepc
                sd      t0, 256(sp)
                mv      a0, sp

                call    trap_handler

                /* Restore the context of the current hart */
                ld     t0, 256(sp)
                csrw    sepc, t0                
                ld     x0, 0(sp)
                ld     x1, 8(sp)
                // ld     x2, 16(sp)
                ld     x3, 24(sp)
                ld     x4, 32(sp)
                ld     x5, 40(sp)
                ld     x6, 48(sp)
                ld     x7, 56(sp)
                ld     x8, 64(sp)
                ld     x9, 72(sp)
                ld     x10, 80(sp)
                ld     x11, 88(sp)
                ld     x12, 96(sp)
                ld     x13, 104(sp)
                ld     x14, 112(sp)
                ld     x15, 120(sp)
                ld     x16, 128(sp)
                ld     x17, 136(sp)
                ld     x18, 144(sp)
                ld     x19, 152(sp)
                ld     x20, 160(sp)
                ld     x21, 168(sp)
                ld     x22, 176(sp)
                ld     x23, 184(sp)
                ld     x24, 192(sp)
                ld     x25, 200(sp)
                ld     x26, 208(sp)
                ld     x27, 216(sp)
                ld     x28, 224(sp)
                ld     x29, 232(sp)
                ld     x30, 240(sp)
                ld     x31, 248(sp)
                addi   sp, sp, 272

                /* restore the sp to the previous value */
                csrrw   sp, sscratch, sp

                sret
            "
        );
    }
}

#[unsafe(export_name = "trap_handler")]
pub extern "C" fn trap_handler(addr: usize) {
    let riscv: &mut Riscv64 = unsafe { transmute(addr) };
    let sp: usize;
    unsafe { asm!("csrr {0}, sscratch", out(reg) sp) }; 
    riscv.regs[2] = sp as u64;

    println!("[riscv64] Hart {}: Trap handler called", riscv.hartid);

    let cause: usize;
    unsafe {
        asm!(
            "csrr {0}, scause",
            out(reg) cause,
        );
    }
    println!("cause: {:#x}", cause);
    println!("epc: {:#x}", riscv.epc);

    match cause {
        0x8000000000000005 => {
            println!("[riscv64] Hart {}: timer interrupt", riscv.hartid);
            sbi_set_timer(usize::MAX as u64);
        }
        _ => {        
            // print regs
            for i in 0..32 {
                print!("x{}: {:#x} ", i, riscv.regs[i]);
                if i % 4 == 3 {
                    println!("");
                }
            }
            panic!("Unknown trap cause");
        }
    }
}
