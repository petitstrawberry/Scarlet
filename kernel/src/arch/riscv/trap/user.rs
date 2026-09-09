use core::{arch::asm, mem::transmute};

use super::exception::arch_exception_handler;
use super::interrupt::arch_interrupt_handler;

use crate::arch::trap::prev_mode;
use crate::arch::{
    self, Mode, Trapframe, get_cpu, get_kernel_trapvector_paddr, get_trapvector, set_trapvector,
};
use crate::task::mytask;

#[unsafe(link_section = ".trampoline.text")]
#[unsafe(export_name = "_user_trap_entry")]
#[unsafe(naked)]
pub extern "C" fn _user_trap_entry() {
    unsafe {
        riscv_naked_asm!(
            "
        .option norvc
        .option norelax
        .align 8
                /* Disable the interrupt */
                csrci   sstatus, 0x2

                /* Save a0 to sscratch and load the Riscv struct pointer */
                csrrw   a0, sscratch, a0
                /* Store sp to Riscv.scratch */
                SCARLET_S      sp, 0*SCARLET_WORD_BYTES(a0)

                /* Load the satp for the kernel space from Riscv.satp */
                SCARLET_L      sp, 2*SCARLET_WORD_BYTES(a0) // sp = Riscv.satp
                /* Switch to kernel memory space */
                csrrw   sp, satp, sp
                sfence.vma zero, zero
                /* Store the user memory space */
                SCARLET_S      sp, 2*SCARLET_WORD_BYTES(a0) // Riscv.satp = sp

                /* Load kernel stack pointer from Riscv.kernel_stack */
                SCARLET_L      sp, 3*SCARLET_WORD_BYTES(a0)

                /* Allocate space on the kernel stack for saving user context */
                addi    sp, sp, -SCARLET_TRAPFRAME_SIZE /* aligned sizeof(Trapframe) */

                /* Save the context of the current hart */
                SCARLET_S      x0, 0*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x1, 1*SCARLET_WORD_BYTES(sp)
                // SCARLET_S      x2, 2*SCARLET_WORD_BYTES(sp) (x2 is sp, which we are modifying)
                SCARLET_S      x3, 3*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x4, 4*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x5, 5*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x6, 6*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x7, 7*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x8, 8*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x9, 9*SCARLET_WORD_BYTES(sp)
                // SCARLET_S      x10, 10*SCARLET_WORD_BYTES(sp) (x10 is a0, which we are modifying)
                SCARLET_S      x11, 11*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x12, 12*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x13, 13*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x14, 14*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x15, 15*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x16, 16*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x17, 17*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x18, 18*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x19, 19*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x20, 20*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x21, 21*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x22, 22*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x23, 23*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x24, 24*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x25, 25*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x26, 26*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x27, 27*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x28, 28*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x29, 29*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x30, 30*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x31, 31*SCARLET_WORD_BYTES(sp)
                csrr    t0, sepc
                SCARLET_S      t0, 32*SCARLET_WORD_BYTES(sp)

                // Load sp from Riscv.scratch and store sp to trapframe
                SCARLET_L      t0, 0*SCARLET_WORD_BYTES(a0)  // t0 = Riscv.scratch (old sp)
                SCARLET_S      t0, 2*SCARLET_WORD_BYTES(sp) // trapframe.sp = t0

                // Save original a0 (currently in sscratch) to trapframe
                csrr    t0, sscratch  // t0 = original a0 value
                SCARLET_S      t0, 10*SCARLET_WORD_BYTES(sp)    // trapframe.a0 = original a0

                // Restore sscratch to Riscv pointer
                csrw   sscratch, a0

                /* Call the user trap handler */
                /* Load the function pointer from Riscv.kernel_trap */
                SCARLET_L      t1, 4*SCARLET_WORD_BYTES(a0)

                /* Pass the trapframe pointer as the first argument */
                mv      a0, sp
                jalr    ra, t1, 0 // Riscv.kernel_trap(a0: &mut Trapframe)

                     /* Keep S-mode interrupts masked while sscratch temporarily
                         contains the user a0 during the trampoline return path. */
                     csrci   sstatus, 0x2

                /* Return from Rust handler - restore trapframe and sret */
                mv      a0, sp
                /* epc */
                SCARLET_L     t0, 32*SCARLET_WORD_BYTES(a0)
                csrw   sepc, t0

                /* Register - restore all except sp and a0 */
                SCARLET_L     x0, 0*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x1, 1*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x2, 2*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x3, 3*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x4, 4*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x5, 5*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x6, 6*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x7, 7*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x8, 8*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x9, 9*SCARLET_WORD_BYTES(a0)
                // SCARLET_L     x10, 10*SCARLET_WORD_BYTES(a0) (a0 will be restored last)
                SCARLET_L     x11, 11*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x12, 12*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x13, 13*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x14, 14*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x15, 15*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x16, 16*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x17, 17*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x18, 18*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x19, 19*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x20, 20*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x21, 21*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x22, 22*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x23, 23*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x24, 24*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x25, 25*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x26, 26*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x27, 27*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x28, 28*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x29, 29*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x30, 30*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x31, 31*SCARLET_WORD_BYTES(a0)

                /* Restore a0 from trapframe */
                SCARLET_L     a0, 10*SCARLET_WORD_BYTES(a0)

                /* Swap a0 with sscratch to get Riscv pointer */
                csrrw  a0, sscratch, a0  // a0 = Riscv pointer, sscratch = original a0

                /* Store original t0 in Riscv.scratch temporarily */
                SCARLET_S     t0, 0*SCARLET_WORD_BYTES(a0)        // Riscv.scratch = original t0

                /* Restore the user memory space using t0 as temp */
                SCARLET_L     t0, 2*SCARLET_WORD_BYTES(a0)       // t0 = Riscv.satp (user satp)
                csrrw  t0, satp, t0
                /* Store back the kernel memory space */
                SCARLET_S     t0, 2*SCARLET_WORD_BYTES(a0)       // Riscv.satp = t0
                sfence.vma zero, zero

                /* Restore trapframe t0 from Riscv.scratch */
                SCARLET_L     t0, 0*SCARLET_WORD_BYTES(a0)        // t0 = original t0

                /* Swap back sscratch to original a0 */
                csrrw   a0, sscratch, a0     // a0 = original a0, sscratch = Riscv pointer

                sret
            "
        );
    }
}

#[unsafe(link_section = ".trampoline.text")]
#[unsafe(export_name = "_guest_trap_entry")]
#[unsafe(naked)]
pub extern "C" fn _guest_trap_entry() {
    unsafe {
        riscv_naked_asm!(
            "
        .option norvc
        .option norelax
        .align 8
                /* Save a0 to sscratch and load the Riscv struct pointer */
                csrrw   a0, sscratch, a0
                /* Store sp to Riscv.scratch */
                SCARLET_S      sp, 0*SCARLET_WORD_BYTES(a0)

                /* Load kernel guest trapframe pointer from Riscv.guest_trapframe_ptr */
                SCARLET_L      sp, 5*SCARLET_WORD_BYTES(a0)

                /* Save the context of the current hart */
                SCARLET_S      x0, 0*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x1, 1*SCARLET_WORD_BYTES(sp)
                // SCARLET_S      x2, 2*SCARLET_WORD_BYTES(sp) (x2 is sp, which we are modifying)
                SCARLET_S      x3, 3*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x4, 4*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x5, 5*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x6, 6*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x7, 7*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x8, 8*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x9, 9*SCARLET_WORD_BYTES(sp)
                // SCARLET_S      x10, 10*SCARLET_WORD_BYTES(sp) (x10 is a0, which we are modifying)
                SCARLET_S      x11, 11*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x12, 12*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x13, 13*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x14, 14*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x15, 15*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x16, 16*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x17, 17*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x18, 18*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x19, 19*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x20, 20*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x21, 21*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x22, 22*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x23, 23*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x24, 24*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x25, 25*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x26, 26*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x27, 27*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x28, 28*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x29, 29*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x30, 30*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x31, 31*SCARLET_WORD_BYTES(sp)
                csrr    t0, sepc
                SCARLET_S      t0, 32*SCARLET_WORD_BYTES(sp)

                // Load sp from Riscv.scratch and store sp to trapframe
                SCARLET_L      t0, 0*SCARLET_WORD_BYTES(a0)  // t0 = Riscv.scratch (old sp)
                SCARLET_S      t0, 2*SCARLET_WORD_BYTES(sp) // trapframe.sp = t0

                // Save original a0 (currently in sscratch) to trapframe
                csrr    t0, sscratch  // t0 = original a0 value
                SCARLET_S      t0, 10*SCARLET_WORD_BYTES(sp)    // trapframe.a0 = original a0

                // Restore sscratch to Riscv pointer
                csrw   sscratch, a0

                /* Call the user trap handler */
                /* Load the function pointer from Riscv.kernel_trap */
                SCARLET_L      t1, 4*SCARLET_WORD_BYTES(a0)

                /* Save trapframe pointer in t2 before changing sp */
                mv      t2, sp

                /* Load the kernel stack pointer from Riscv.kernel_stack */
                SCARLET_L      sp, 3*SCARLET_WORD_BYTES(a0)

                /* Save a0 (trapframe ptr) on stack */
                addi    sp, sp, -16
                SCARLET_S      t2, 0*SCARLET_WORD_BYTES(sp)

                /* Pass trapframe pointer as first argument */
                mv      a0, t2

                jalr    ra, t1, 0 // Riscv.kernel_trap(a0: &mut Trapframe)

                /* Return from Rust handler - restore trapframe and sret */
                /* Load trapframe pointer from stack */
                SCARLET_L      a0, 0*SCARLET_WORD_BYTES(sp)
                addi    sp, sp, 16

                /* epc */
                SCARLET_L     t0, 32*SCARLET_WORD_BYTES(a0)
                csrw   sepc, t0

                /* Register - restore all except sp and a0 */
                SCARLET_L     x0, 0*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x1, 1*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x2, 2*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x3, 3*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x4, 4*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x5, 5*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x6, 6*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x7, 7*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x8, 8*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x9, 9*SCARLET_WORD_BYTES(a0)
                // SCARLET_L     x10, 10*SCARLET_WORD_BYTES(a0) (a0 will be restored last)
                SCARLET_L     x11, 11*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x12, 12*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x13, 13*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x14, 14*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x15, 15*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x16, 16*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x17, 17*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x18, 18*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x19, 19*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x20, 20*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x21, 21*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x22, 22*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x23, 23*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x24, 24*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x25, 25*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x26, 26*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x27, 27*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x28, 28*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x29, 29*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x30, 30*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x31, 31*SCARLET_WORD_BYTES(a0)

                /* Restore a0 from trapframe */
                SCARLET_L     a0, 10*SCARLET_WORD_BYTES(a0)

                sret
            "
        );
    }
}

#[unsafe(link_section = ".trampoline.text")]
#[unsafe(export_name = "_switch_to_user")]
#[unsafe(naked)]
pub extern "C" fn _switch_to_user(trapframe: &mut Trapframe) -> ! {
    unsafe {
        riscv_naked_asm!(
            "
        .option norvc
        .option norelax
        .align 8
            /* Keep S-mode interrupts masked while sscratch temporarily
               contains the user a0 during the first user return path. */
            csrci   sstatus, 0x2

                /* Restore the context of the current hart from trapframe first */
                /* epc */
                SCARLET_L     t0, 32*SCARLET_WORD_BYTES(a0)
                csrw   sepc, t0

                /* Register - restore all except sp and a0 */
                SCARLET_L     x0, 0*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x1, 1*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x2, 2*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x3, 3*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x4, 4*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x5, 5*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x6, 6*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x7, 7*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x8, 8*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x9, 9*SCARLET_WORD_BYTES(a0)
                // SCARLET_L     x10, 10*SCARLET_WORD_BYTES(a0) (a0 will be restored last)
                SCARLET_L     x11, 11*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x12, 12*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x13, 13*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x14, 14*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x15, 15*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x16, 16*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x17, 17*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x18, 18*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x19, 19*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x20, 20*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x21, 21*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x22, 22*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x23, 23*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x24, 24*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x25, 25*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x26, 26*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x27, 27*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x28, 28*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x29, 29*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x30, 30*SCARLET_WORD_BYTES(a0)
                SCARLET_L     x31, 31*SCARLET_WORD_BYTES(a0)

                /* Restore a0 from trapframe */
                SCARLET_L     a0, 10*SCARLET_WORD_BYTES(a0)

                /* Swap a0 with sscratch to get Riscv pointer */
                csrrw  a0, sscratch, a0  // a0 = Riscv pointer, sscratch = original a0

                /* Store original t0 in Riscv.scratch temporarily */
                SCARLET_S     t0, 0*SCARLET_WORD_BYTES(a0)        // Riscv.scratch = original t0

                /* Restore the user memory space using t0 as temp */
                SCARLET_L     t0, 2*SCARLET_WORD_BYTES(a0)       // t0 = Riscv.satp (user satp)
                csrrw  t0, satp, t0
                /* Store back the kernel memory space */
                SCARLET_S     t0, 2*SCARLET_WORD_BYTES(a0)       // Riscv.satp = t0
                sfence.vma zero, zero

                /* Restore trapframe t0 from Riscv.scratch */
                SCARLET_L     t0, 0*SCARLET_WORD_BYTES(a0)        // t0 = original t0

                /* Swap back sscratch to original a0 */
                csrrw   a0, sscratch, a0     // a0 = original a0, sscratch = Riscv pointer

                sret
            "
        );
    }
}

#[unsafe(export_name = "arch_user_trap_handler")]
pub extern "C" fn arch_user_trap_handler(addr: usize) {
    let trapframe: &mut Trapframe = unsafe { transmute(addr) };
    let saved_stvec = get_trapvector();
    set_trapvector(get_kernel_trapvector_paddr());

    #[cfg(feature = "hypervisor")]
    {
        let from_guest = crate::arch::hv::trap::is_from_guest();
        // crate::println!("[trap_handler] from_guest={}", from_guest);

        if from_guest {
            if let Some(task) = mytask() {
                let mode = match prev_mode() {
                    // from VU-mode
                    arch::riscv::trap::PRIV_U_MODE => Mode::GuestUser,
                    // from VS-mode
                    arch::riscv::trap::PRIV_S_MODE => Mode::GuestKernel,
                    _ => {
                        panic!("Invalid previous mode in guest trap: {}", prev_mode());
                    }
                };

                task.vcpu.lock().set_mode(mode);
            }
        } else {
            if let Some(task) = mytask() {
                let mode = match prev_mode() {
                    arch::riscv::trap::PRIV_U_MODE => Mode::User,
                    arch::riscv::trap::PRIV_S_MODE => Mode::Kernel,
                    _ => panic!("Invalid previous mode in user trap: {}", prev_mode()),
                };
                task.vcpu.lock().set_mode(mode);
            }
        }
    }
    #[cfg(not(all(feature = "hypervisor", target_arch = "riscv64")))]
    {
        if let Some(task) = mytask() {
            let mode = match prev_mode() {
                arch::riscv::trap::PRIV_U_MODE => Mode::User,
                arch::riscv::trap::PRIV_S_MODE => Mode::Kernel,
                _ => panic!("Invalid previous mode in user trap: {}", prev_mode()),
            };
        }
    }

    let cause: usize;
    unsafe {
        asm!(
            "csrr {0}, scause",
            out(reg) cause,
        );
    }

    let interrupt = cause & (1usize << (usize::BITS - 1)) != 0;
    if interrupt {
        arch_interrupt_handler(trapframe, cause & !(1usize << (usize::BITS - 1)));
    } else {
        arch_exception_handler(trapframe, cause);
    }

    let cpu_id = get_cpu().get_cpuid();
    if crate::sched::scheduler::may_schedule_from_interrupt(cpu_id)
        && crate::sched::scheduler::take_deferred_reschedule(cpu_id)
    {
        crate::sched::scheduler::schedule(trapframe);
    }

    // The Rust trap handler is the final task-context frame before `sret`.
    // Do not consume events from `schedule()` itself: it may resume inside a
    // blocking operation while owned values are still live on its stack.
    crate::sched::scheduler::process_pending_events_before_user_return(trapframe);

    // Scheduling (including pending-event delivery) installs the kernel trap
    // vector when this task resumes. Restore the user/guest vector only after
    // all such work; it must be mapped when the trampoline restores user satp.
    // Keep interrupts masked until sret, including the Rust epilogue.
    crate::arch::interrupt::disable_interrupts();
    set_trapvector(saved_stvec);
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
#[unsafe(export_name = "arch_switch_to_user")]
pub fn arch_switch_to_user(trapframe: &mut Trapframe) -> ! {
    // First entry and explicit task switches bypass the returning trap handler
    // above, but still pass through this true userspace boundary.
    crate::sched::scheduler::process_pending_events_before_user_return(trapframe);
    let addr = trapframe as *mut Trapframe as usize;

    // Do not take a kernel interrupt after installing the trampoline vector.
    crate::arch::interrupt::disable_interrupts();

    // Configure the upcoming user return. This affects sstatus.SPIE, not the current kernel SIE.
    crate::arch::configure_user_entry(
        trapframe,
        crate::arch::UserEntryOptions {
            irq_policy: crate::arch::UserReturnIrqPolicy::Enable,
        },
    );

    // Get the trampoline address for _switch_to_user
    let switch_to_user_offset = (_switch_to_user as *const () as usize)
        .wrapping_sub(_user_trap_entry as *const () as usize);
    let trampoline_base = crate::vm::get_trampoline_trap_vector();
    let switch_to_user_addr = trampoline_base.wrapping_add(switch_to_user_offset);
    set_trapvector(trampoline_base);

    // crate::println!(
    //     "switch_to_user_addr: {:#x}, trapframe: {:#x}",
    //     switch_to_user_addr,
    //     addr
    // );

    unsafe {
        asm!(
            "mv t0, {switch_to_user_addr}",    // Load jump target into t0 first
            "mv a0, {trapframe_addr}",    // Load trapframe addr into a0
            "jr t0",                      // Jump using t0 (preserves a0)
            trapframe_addr = in(reg) addr,
            switch_to_user_addr = in(reg) switch_to_user_addr,
            options(noreturn, nostack)
        );
    }
}
