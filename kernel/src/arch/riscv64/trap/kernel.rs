use core::arch::naked_asm;

#[unsafe(export_name = "_kernel_trap_entry")]
#[naked]
pub extern "C" fn _kernel_trap_entry() {
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

                // Load kernel stack pointer
                ld      sp, 272(a0)

                call   arch_trap_handler

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

                /* Restore the a0 and kernel stack pointer */
                csrrw  a0, sscratch, a0

                sret
            "
        );
    }
}
