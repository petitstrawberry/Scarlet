//! Guest entry and exception delivery through Linux signal return.

use super::{Control, STACK_SIZE};
use crate::{
    worker_context::{Exception, linux::SignalContext},
    worker_memory::PatchedInstruction,
};
use std::{ffi::c_void, io, ptr, sync::atomic::Ordering};

pub(super) fn signal_stack() -> io::Result<*mut c_void> {
    // Align the reservation so the signal trampoline can find Control
    // from SP without consulting a guest register or the guest stack.
    let mapping = unsafe {
        libc::mmap(
            ptr::null_mut(),
            2 * STACK_SIZE,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        )
    };
    if mapping == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    let start = mapping.addr();
    let aligned = start.next_multiple_of(STACK_SIZE);
    let prefix = aligned - start;
    let suffix = STACK_SIZE - prefix;
    unsafe {
        if prefix != 0 {
            libc::munmap(mapping, prefix);
        }
        libc::munmap(mapping.byte_add(prefix + STACK_SIZE), suffix);
    }
    Ok(mapping.with_addr(aligned))
}

pub(super) unsafe extern "C" fn handle_signal(
    signal: i32,
    info: *mut libc::siginfo_t,
    raw: *mut c_void,
) {
    let stack: usize;
    unsafe {
        core::arch::asm!("mov {}, sp", out(reg) stack, options(nomem, nostack, preserves_flags))
    };
    let stack_base = stack & !(STACK_SIZE - 1);
    let control = unsafe { &mut *ptr::with_exposed_provenance_mut::<Control>(stack_base) };
    let cancelled =
        control.epoch != 0 && control.interrupt.epoch.load(Ordering::SeqCst) >= control.epoch;
    if signal == libc::SIGUSR1 && (!cancelled || control.host.is_none()) {
        return;
    }

    let mut context = unsafe { SignalContext::new(raw) };
    let mut exception = unsafe { context.exception(signal, info) };
    let breakpoint = exception.esr >> 26 == 0x3c;
    if let Some(host) = control.host {
        let mut interrupted = signal == libc::SIGUSR1;
        if breakpoint && matches!(exception.esr & 0xffff, 0x5400 | 0x5500) {
            let pc = exception.registers.pc as usize;
            let mappings = unsafe { &*control.mappings };
            if let Some((_, mapping)) = mappings.range(..=pc).next_back()
                && let Some(word) = mapping.instruction_at(pc)
                && let Some(instruction) = mapping.patched_instruction(pc, word)
            {
                if cancelled {
                    interrupted = true;
                } else {
                    let regs = &mut exception.registers;
                    match instruction {
                        PatchedInstruction::ReadThreadPointer { register } => {
                            if let Some(dest) = regs.x.get_mut(register as usize) {
                                *dest = control.tls;
                            }
                        }
                        PatchedInstruction::WriteThreadPointer { register } => {
                            control.tls = regs.x.get(register as usize).copied().unwrap_or(0);
                        }
                        _ => unreachable!(),
                    }
                    regs.pc += 4;
                    context.restore(*regs);
                    return;
                }
            }
        }
        control.interrupted = interrupted;
        control.stop = Some(exception);
        control.host = None;
        context.restore(host);
    } else if breakpoint
        && exception.esr & 0xffff == 0x5600
        && let Some(guest) = control.pending.take()
    {
        let mut host = exception.registers;
        host.pc += 4;
        if cancelled {
            control.interrupted = true;
            control.stop = Some(Exception {
                registers: guest,
                esr: 0,
                far: 0,
            });
            context.restore(host);
        } else {
            control.host = Some(host);
            context.restore(guest);
        }
    } else {
        unsafe { libc::_exit(128 + signal) };
    }
}
