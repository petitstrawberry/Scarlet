use core::arch::naked_asm;
use core::sync::atomic::compiler_fence;
use core::{arch::asm, mem::transmute};

use super::exception::arch_exception_handler;
use super::interrupt::arch_interrupt_handler;

use crate::arch::{get_kernel_trapvector_paddr, set_trapvector, trap, Trapframe};
use crate::initcall::early;

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

                /* Save a0 to sscratch and load the Riscv64 struct pointer */
                csrrw   a0, sscratch, a0
                /* Store sp to Riscv64.scratch */
                sd      sp, 0(a0)

                /* Load the satp for the kernel space from Riscv64.satp */
                ld      sp, 16(a0) // sp = Riscv64.satp
                /* Switch to kernel memory space */
                csrrw   sp, satp, sp
                sfence.vma zero, zero
                /* Store the user memory space */
                sd      sp, 16(a0) // Riscv64.satp = sp
                
                /* Load kernel stack pointer from Riscv64.kernel_stack */
                ld      sp, 24(a0)

                /* Allocate space on the kernel stack for saving user context */
                addi    sp, sp, -264 /* sizeof(Trapframe) = 264 bytes */

                /* Save the context of the current hart */
                sd      x0, 0(sp)
                sd      x1, 8(sp)
                // sd      x2, 16(sp) (x2 is sp, which we are modifying)
                sd      x3, 24(sp)
                sd      x4, 32(sp)
                sd      x5, 40(sp)
                sd      x6, 48(sp)
                sd      x7, 56(sp)
                sd      x8, 64(sp)
                sd      x9, 72(sp)
                // sd      x10, 80(sp) (x10 is a0, which we are modifying)
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

                // Load sp from Riscv64.scratch and store sp to trapframe
                ld      t0, 0(a0)  // t0 = Riscv64.scratch (old sp)
                sd      t0, 16(sp) // trapframe.sp = t0

                // Save original a0 (currently in sscratch) to trapframe
                csrr    t0, sscratch  // t0 = original a0 value
                sd      t0, 80(sp)    // trapframe.a0 = original a0

                // Restore sscratch to Riscv64 pointer
                csrw   sscratch, a0

                /* Call the user trap handler */
                /* Load the function pointer from Riscv64.kernel_trap */
                ld      ra, 32(a0)

                /* Pass the trapframe pointer as the first argument */
                mv      a0, sp
                jr      ra // Riscv64.kernel_trap(a0: &mut Trapframe)
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
                /* Restore the context of the current hart from trapframe first */ 
                /* epc */
                ld     t0, 256(a0)
                csrw   sepc, t0
                
                /* Register - restore all except sp and a0 */
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
                // ld     x10, 80(a0) (a0 will be restored last)
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
                ld     a0, 80(a0)

                /* Swap a0 with sscratch to get Riscv64 pointer */
                csrrw  a0, sscratch, a0  // a0 = Riscv64 pointer, sscratch = original a0

                /* Store original t0 in Riscv64.scratch temporarily */
                sd     t0, 0(a0)        // Riscv64.scratch = original t0

                /* Restore the user memory space using t0 as temp */
                ld     t0, 16(a0)       // t0 = Riscv64.satp (user satp)
                csrrw  t0, satp, t0
                /* Store back the kernel memory space */
                sd     t0, 16(a0)       // Riscv64.satp = t0
                sfence.vma zero, zero

                /* Restore trapframe t0 from Riscv64.scratch */
                ld     t0, 0(a0)        // t0 = original t0

                /* Swap back sscratch to original a0 */
                csrrw   a0, sscratch, a0     // a0 = original a0, sscratch = Riscv64 pointer

                sret
            "
        );
    }
}

#[unsafe(export_name = "arch_user_trap_handler")]
pub extern "C" fn arch_user_trap_handler(addr: usize) -> ! {
    let trapframe: &mut Trapframe = unsafe { transmute(addr) };
    set_trapvector(get_kernel_trapvector_paddr());

    // let cpu = crate::arch::get_cpu();
    // crate::early_println!("CPU: {:#x?}", cpu);

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
    let cpu = crate::arch::get_cpu();

    // crate::early_println!("CPU: {:#x?}", cpu);
    
    // Get the trampoline address for _user_trap_exit
    let trap_exit_offset = _user_trap_exit as usize - _user_trap_entry as usize;
    // crate::early_println!("_user_trap_entry: {:#x}, _user_trap_exit: {:#x}, offset: {:#x}", _user_trap_entry as usize, _user_trap_exit as usize, trap_exit_offset);
    let trampoline_base = crate::vm::get_trampoline_trap_vector();
    let trap_exit_addr = trampoline_base + trap_exit_offset;
    set_trapvector(trampoline_base);

    // crate::early_println!("trap_exit_addr: {:#x}, trapframe: {:#x}", trap_exit_addr, addr);

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