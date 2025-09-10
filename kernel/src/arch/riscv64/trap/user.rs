use core::arch::naked_asm;
use core::sync::atomic::compiler_fence;
use core::{arch::asm, mem::transmute};

use super::exception::arch_exception_handler;
use super::interrupt::arch_interrupt_handler;

use crate::arch::{get_kernel_trapvector_paddr, set_trapvector, trap, Trapframe};

#[unsafe(link_section = ".trampoline.text")]
#[unsafe(export_name = "_user_trap_entry")]
#[unsafe(naked)]
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
                // Restore sscratch from a0
                csrw   sscratch, a0

                sd      t0, 80(a0)

                /* Load the satp for the kernel space from trapframe */
                ld      t0, 272(a0)
                /* Switch to kernel memory space */
                csrrw   t0, satp, t0
                sfence.vma zero, zero
                /* Store the user memory space */
                sd      t0, 272(a0)

                // Load kernel stack pointer
                ld      sp, 280(a0)

                /* Call the user trap handler */
                /* Load the function pointer from the trapframe */
                ld      ra, 288(a0)
                jr      ra
            "
        );
    }
}


#[unsafe(link_section = ".trampoline.text")]
#[unsafe(export_name = "_user_trap_exit")]
#[unsafe(naked)]
pub extern "C" fn _user_trap_exit(trapframe: &mut Trapframe) -> ! {
    unsafe {
        naked_asm!("
        .option norvc
        .option norelax
        .align 8
                /* Restore the user memory space */
                ld     t0, 272(a0)
                csrrw  t0, satp, t0
                sfence.vma zero, zero
                /* Restore the kernel memory space to the trapframe */
                sd     t0, 272(a0)

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
pub extern "C" fn arch_user_trap_handler(addr: usize) -> ! {
    let trapframe: &mut Trapframe = unsafe { transmute(addr) };
    set_trapvector(get_kernel_trapvector_paddr());

    let cause: usize;
    unsafe {
        asm!(
            "csrr {0}, scause",
            out(reg) cause,
        );
    }

    let interrupt = cause & 0x8000000000000000 != 0;
    if interrupt {
        arch_interrupt_handler(trapframe, cause & !0x8000000000000000);
    } else {
        // crate::println!("Entering exception handler for cause: {}", cause);
        arch_exception_handler(trapframe, cause);
        // crate::println!("Exiting exception handler for cause: {}", cause);
    }
    // Jump directly to user trap exit via trampoline
    arch_switch_to_user_space(trapframe);
}

/// Switch to user space using the trampoline mechanism
/// 
/// This function prepares the trapframe for user space execution
/// and jumps to the user trap exit handler using a trampoline.
/// 
/// # Arguments
/// * `trapframe` - A mutable reference to the trapframe that contains the state to switch to user space.
///
/// This function is marked as `noreturn` because it will not return to the caller.
/// It will jump to the user trap exit handler, which will then return to user space.
#[unsafe(export_name = "arch_switch_to_user_space")]
pub fn arch_switch_to_user_space(trapframe: &mut Trapframe) -> ! {
    let addr = trapframe as *mut Trapframe as usize;
    
    // Get the trampoline address for _user_trap_exit
    let trap_exit_offset = _user_trap_exit as usize - _user_trap_entry as usize;
    let trampoline_base = crate::vm::get_trampoline_trap_vector();
    let trap_exit_addr = trampoline_base + trap_exit_offset;
    set_trapvector(trampoline_base);

    unsafe {
        asm!(
            "mv t0, {trap_exit_addr}",    // Load jump target into t0 first
            "mv a0, {trapframe_addr}",    // Load trapframe addr into a0
            "jr t0",                      // Jump using t0 (preserves a0)
            trapframe_addr = in(reg) addr,
            trap_exit_addr = in(reg) trap_exit_addr,
            options(noreturn, nostack)
        );
    }
}