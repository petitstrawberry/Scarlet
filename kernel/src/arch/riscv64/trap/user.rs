use core::arch::naked_asm;
use core::{arch::asm, mem::transmute};

use super::exception::arch_exception_handler;
use super::interrupt::arch_interrupt_handler;

use crate::arch::Trapframe;
use crate::vm::{switch_to_kernel_vm, switch_to_user_vm};

#[unsafe(link_section = ".trampoline.text")]
#[unsafe(export_name = "_user_trap_entry")]
#[naked]
pub extern "C" fn _user_trap_entry() {
    unsafe {
        naked_asm!("
        .option norvc
        .option norelax
        .align 8
                /* Disable the interrupt */
                csrci   sstatus, 0x2
                /* Save the current a0 to sscratch and load the trapframe pointer */
                csrrw   a0, sscratch, a0
                /* Save the context of the current hart */
                sd      x0, 0(a0)
                sd      x1, 8(a0)
                sd      x2, 16(a0)
                sd      x3, 24(a0)
                sd      x4, 32(a0)
                sd      x5, 40(a0)
                sd      x6, 48(a0)
                sd      x7, 56(a0)
                sd      x8, 64(a0)
                sd      x9, 72(a0)
                // sd      x10, 80(a0)
                sd      x11, 88(a0)
                sd      x12, 96(a0)
                sd      x13, 104(a0)
                sd      x14, 112(a0)
                sd      x15, 120(a0)
                sd      x16, 128(a0)
                sd      x17, 136(a0)
                sd      x18, 144(a0)
                sd      x19, 152(a0)
                sd      x20, 160(a0)
                sd      x21, 168(a0)
                sd      x22, 176(a0)
                sd      x23, 184(a0)
                sd      x24, 192(a0)
                sd      x25, 200(a0)
                sd      x26, 208(a0)
                sd      x27, 216(a0)
                sd      x28, 224(a0)
                sd      x29, 232(a0)
                sd      x30, 240(a0)
                sd      x31, 248(a0)
                csrr    t0, sepc
                sd      t0, 256(a0)

                // Load and store a0 to trapframe
                csrr    t0, sscratch
                sd      t0, 80(a0)

                // Load kernel stack pointer
                ld      sp, 272(a0)

                /* Call the user trap handler */
                /* Load the function pointer from the trapframe */
                ld      ra, 280(a0)
                jalr    ra, 0(ra)

                /* Restore the context of the current hart */ 
                /* epc */
                ld     t0, 256(a0)
                csrw   sepc, t0
                /* Register */
                ld     x0, 0(a0)
                ld     x1, 8(a0)
                ld     x2, 16(a0)
                ld     x3, 24(a0)
                ld     x4, 32(a0)
                ld     x5, 40(a0)
                ld     x6, 48(a0)
                ld     x7, 56(a0)
                ld     x8, 64(a0)
                ld     x9, 72(a0)
                // ld     x10, 80(a0)
                ld     x11, 88(a0)
                ld     x12, 96(a0)
                ld     x13, 104(a0)
                ld     x14, 112(a0)
                ld     x15, 120(a0)
                ld     x16, 128(a0)
                ld     x17, 136(a0)
                ld     x18, 144(a0)
                ld     x19, 152(a0)
                ld     x20, 160(a0)
                ld     x21, 168(a0)
                ld     x22, 176(a0)
                ld     x23, 184(a0)
                ld     x24, 192(a0)
                ld     x25, 200(a0)
                ld     x26, 208(a0)
                ld     x27, 216(a0)
                ld     x28, 224(a0)
                ld     x29, 232(a0)
                ld     x30, 240(a0)
                ld     x31, 248(a0)

                /* Restore a0 from trapframe */
                csrrw  zero, sscratch, a0
                ld     a0, 80(a0)

                sret
            "
        );
    }
}


#[unsafe(export_name = "arch_user_trap_handler")]
pub extern "C" fn arch_user_trap_handler(addr: usize) -> usize {
    let trapframe: &mut Trapframe = unsafe { transmute(addr) };

    let cause: usize;
    unsafe {
        asm!(
            "csrr {0}, scause",
            out(reg) cause,
        );
    }

    /* Switch to kernel memory space */
    switch_to_kernel_vm();

    let interrupt = cause & 0x8000000000000000 != 0;
    if interrupt {
        arch_interrupt_handler(trapframe, cause & !0x8000000000000000);
    } else {
        arch_exception_handler(trapframe, cause);
    }

    /* Switch to user memory space */
    switch_to_user_vm(trapframe);

    addr
}