//! AArch64 Linux signal frames and rt_sigreturn.
//!
//! Byte offsets follow the Linux arm64 UAPI (ucontext and sigcontext).
//! The return stub lives in an RX mapping, never on an executable stack.

use crate::abi::linux::aarch64::LinuxAarch64Abi;
use crate::abi::linux::generic::{
    LinuxAbi,
    signal::{LinuxSignal, SignalAction},
};
use crate::arch::{Trapframe, fpu::FpuContext};
use crate::environment::{PAGE_SIZE, USER_LOWER_CANONICAL_END};
use crate::library::std::usercopy::{copy_from_user, copy_to_user};
use crate::task::{Task, mytask};
use alloc::{sync::Arc, vec};

const UCONTEXT: usize = 128;
const MASK: usize = UCONTEXT + 40;
const MCONTEXT: usize = UCONTEXT + 176;
const REGS: usize = MCONTEXT + 8;
const SP: usize = MCONTEXT + 256;
const PC: usize = MCONTEXT + 264;
const PSTATE: usize = MCONTEXT + 272;
const RESERVED: usize = MCONTEXT + 288;
const FRAME_SIZE: usize = RESERVED + 4096;
const FPSIMD_MAGIC: u32 = 0x4650_8001;
const FPSIMD_SIZE: usize = 528;
const SA_ONSTACK: u64 = 0x0800_0000;
const SA_NODEFER: u64 = 0x4000_0000;
const SA_RESETHAND: u64 = 0x8000_0000;
const UNMASKABLE: u64 = (1 << (9 - 1)) | (1 << (19 - 1));

fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_ne_bytes());
}
fn get64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}
fn get32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn encode_context(frame: &mut [u8], regs: &Trapframe, fp: &FpuContext, mask: u64) {
    put64(frame, MASK, mask);
    put64(frame, MCONTEXT, regs.far_el1);
    for index in 0..31 {
        put64(frame, REGS + index * 8, regs.regs.reg[index] as u64);
    }
    put64(frame, SP, regs.sp);
    // ELR already points past SVC. get_current_pc() would subtract four.
    put64(frame, PC, regs.elr);
    put64(frame, PSTATE, regs.spsr);
    put32(frame, RESERVED, FPSIMD_MAGIC);
    put32(frame, RESERVED + 4, FPSIMD_SIZE as u32);
    put32(frame, RESERVED + 8, fp.fpsr as u32);
    put32(frame, RESERVED + 12, fp.fpcr as u32);
    for index in 0..32 {
        put64(frame, RESERVED + 16 + index * 16, fp.v[index][0]);
        put64(frame, RESERVED + 24 + index * 16, fp.v[index][1]);
    }
}

fn decode_context(frame: &[u8], regs: &mut Trapframe) -> Result<FpuContext, &'static str> {
    if frame.len() != FRAME_SIZE {
        return Err("bad signal frame size");
    }
    let pc = get64(frame, PC);
    let sp = get64(frame, SP);
    let pstate = get64(frame, PSTATE);
    if pc >= USER_LOWER_CANONICAL_END as u64
        || pc & 3 != 0
        || sp >= USER_LOWER_CANONICAL_END as u64
        || sp & 15 != 0
        || pstate & 0xf != 0
    {
        return Err("invalid signal return context");
    }
    // Only FPSIMD is currently exposed to Linux applications. Validate the
    // record and terminator before mutating the live register context.
    if get32(frame, RESERVED) != FPSIMD_MAGIC
        || get32(frame, RESERVED + 4) != FPSIMD_SIZE as u32
        || get64(frame, RESERVED + FPSIMD_SIZE) != 0
    {
        return Err("invalid FPSIMD signal context");
    }
    let mut fp = FpuContext::new();
    fp.fpsr = get32(frame, RESERVED + 8) as u64;
    fp.fpcr = get32(frame, RESERVED + 12) as u64;
    for index in 0..32 {
        fp.v[index][0] = get64(frame, RESERVED + 16 + index * 16);
        fp.v[index][1] = get64(frame, RESERVED + 24 + index * 16);
    }
    for index in 0..31 {
        regs.regs.reg[index] = get64(frame, REGS + index * 8) as usize;
    }
    regs.sp = sp;
    regs.elr = pc;
    // Never allow a userspace signal frame to restore privileged mode or DAIF.
    regs.spsr = pstate & 0xf000_0000;
    Ok(fp)
}

fn return_stub(abi: &LinuxAbi, task: &Task) -> Result<usize, &'static str> {
    let mut cached = abi.signal_restorer.lock();
    if let Some(address) = *cached {
        return Ok(address);
    }
    use crate::object::capability::memory_mapping::anon_owner::AnonymousPageOwner;
    use crate::vm::vmem::{MemoryArea, PhysicalMemoryArea, VirtualMemoryMap};
    let map = VirtualMemoryMap::new(
        PhysicalMemoryArea::new(0, 0),
        MemoryArea::new(0, PAGE_SIZE - 1),
        0x0d,
        false,
        Some(Arc::new(AnonymousPageOwner::new())),
    );
    let address = task.vm_manager.add_memory_map_anywhere(map)?;
    let initialize = || -> Result<(), &'static str> {
        task.vm_manager.lazy_map_page(address)?;
        let kva = task
            .vm_manager
            .translate_to_kva(address)
            .ok_or("signal restorer mapping failed")?;
        // mov x8, #139; svc #0. Only the kernel writes this read/execute page.
        let code = [0xd280_1168u32, 0xd400_0001u32];
        unsafe {
            core::ptr::copy_nonoverlapping(code.as_ptr(), kva as *mut u32, code.len());
        }
        crate::arch::sync_icache_for_execution(kva, 8);
        Ok(())
    };
    if let Err(error) = initialize() {
        let removed = task.vm_manager.remove_memory_map_range(address, PAGE_SIZE);
        crate::object::capability::memory_mapping::syscall::reclaim_private_removed_mappings(
            task, &removed,
        );
        return Err(error);
    }
    *cached = Some(address);
    Ok(address)
}

pub fn setup_signal_handler(
    abi: &LinuxAbi,
    task: &Task,
    trapframe: &mut Trapframe,
    handler: usize,
    signal: LinuxSignal,
) -> Result<(), &'static str> {
    let (action, old_mask) = {
        let state = abi.signal_state.lock();
        (state.get_sigaction(signal), state.blocked.raw())
    };
    let stack = &abi.thread_state;
    let on_altstack = stack.sigaltstack_size != 0
        && (trapframe.sp as usize).wrapping_sub(stack.sigaltstack_sp) < stack.sigaltstack_size;
    let stack_top = if action.flags & SA_ONSTACK != 0 && stack.sigaltstack_size != 0 && !on_altstack
    {
        stack
            .sigaltstack_sp
            .checked_add(stack.sigaltstack_size)
            .ok_or("signal stack overflow")?
    } else {
        trapframe.sp as usize
    };
    let base = stack_top
        .checked_sub(FRAME_SIZE)
        .ok_or("signal stack underflow")?
        & !15;
    if (on_altstack || stack_top != trapframe.sp as usize) && base < stack.sigaltstack_sp {
        return Err("signal frame exceeds alternate stack");
    }
    let restorer = if action.flags & 0x0400_0000 != 0 && action.restorer != 0 {
        action.restorer
    } else {
        return_stub(abi, task)?
    };
    let mut frame = vec![0u8; FRAME_SIZE];
    put32(&mut frame, 0, signal as u32);
    put32(&mut frame, 8, 128); // SI_KERNEL, for terminal process-control events.
    put64(&mut frame, UCONTEXT + 16, stack.sigaltstack_sp as u64);
    put32(
        &mut frame,
        UCONTEXT + 24,
        if stack.sigaltstack_size == 0 {
            2
        } else if on_altstack {
            1
        } else {
            0
        },
    );
    put64(&mut frame, UCONTEXT + 32, stack.sigaltstack_size as u64);
    let fp = {
        let mut vcpu = task.vcpu.lock();
        if vcpu.fpu_used && crate::arch::user_fpu_enabled() {
            // Delivery happens for the current task while its vector state is live.
            unsafe {
                vcpu.fpu.save();
            }
        }
        vcpu.fpu.clone()
    };
    encode_context(&mut frame, trapframe, &fp, old_mask);
    copy_to_user(task, base, &frame).map_err(|_| "cannot write signal frame")?;
    {
        let mut state = abi.signal_state.lock();
        let mut blocked = old_mask | action.mask;
        if action.flags & SA_NODEFER == 0 {
            blocked |= 1u64 << (signal as u32 - 1);
        }
        state.blocked.set_raw(blocked & !UNMASKABLE);
        if action.flags & SA_RESETHAND != 0 {
            state.set_handler(signal, signal.default_action());
        }
    }
    trapframe.sp = base as u64;
    trapframe.elr = handler as u64;
    trapframe.regs.reg[0] = signal as usize;
    trapframe.regs.reg[1] = base;
    trapframe.regs.reg[2] = base + UCONTEXT;
    trapframe.regs.reg[30] = restorer;
    Ok(())
}

pub fn sys_rt_sigreturn(abi: &mut LinuxAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let Some(task) = mytask() else {
        return usize::MAX;
    };
    let mut frame = vec![0u8; FRAME_SIZE];
    let result = if trapframe.sp & 15 != 0
        || copy_from_user(&task, trapframe.sp as usize, &mut frame).is_err()
    {
        Err("cannot read signal frame")
    } else {
        decode_context(&frame, trapframe)
    };
    match result {
        Ok(fp) => {
            abi.0
                .signal_state
                .lock()
                .blocked
                .set_raw(get64(&frame, MASK) & !UNMASKABLE);
            let mut vcpu = task.vcpu.lock();
            vcpu.fpu = fp;
            if crate::arch::user_fpu_enabled() {
                vcpu.fpu_used = true;
                crate::arch::fpu::set_user_fpu_enabled(true);
                unsafe {
                    vcpu.fpu.restore();
                }
            }
            // The syscall dispatcher writes this value to x0. Preserve the
            // interrupted result rather than replacing it with sigreturn's 0.
            trapframe.get_return_value()
        }
        Err(error) => {
            crate::println!("[linux] sigreturn failed: {}", error);
            task.mark_signal_termination(11);
            task.request_deferred_exit_group(139);
            usize::MAX
        }
    }
}

/// Install one newly unblocked handler after the syscall result has been saved.
pub fn deliver_pending_handler(abi: &LinuxAbi, frame: &mut Trapframe, result: usize) -> usize {
    frame.set_return_value(result);
    let pending = {
        let mut state = abi.signal_state.lock();
        state.next_deliverable_signal().and_then(|signal| {
            if let SignalAction::Custom(handler) = state.get_handler(signal) {
                state.remove_pending(signal);
                Some((signal, handler))
            } else {
                None
            }
        })
    };
    if let Some((signal, handler)) = pending {
        if let Some(task) = mytask() {
            if setup_signal_handler(abi, &task, frame, handler, signal).is_err() {
                task.mark_signal_termination(11);
                task.request_deferred_exit_group(139);
            }
        }
    }
    frame.get_return_value()
}

pub fn dispatch_arch_syscall(
    abi: &mut LinuxAarch64Abi,
    frame: &mut Trapframe,
    syscall: usize,
) -> Option<usize> {
    match syscall {
        139 => Some(sys_rt_sigreturn(abi, frame)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn signal_frame_restores_interrupted_registers_and_fpsimd() {
        assert_eq!(FRAME_SIZE, 4688);
        assert_eq!(RESERVED % 16, 0);
        let mut original = Trapframe::new();
        for index in 0..31 {
            original.regs.reg[index] = index * 17 + 3;
        }
        original.regs.reg[0] = (-4isize) as usize;
        original.elr = 0x400080;
        original.sp = 0x800000;
        original.spsr = 0xa0000000;
        let mut fp = FpuContext::new();
        fp.v[0] = [0x1234, 0x5678];
        fp.v[31] = [0x8765, 0x4321];
        fp.fpcr = 0x400000;
        fp.fpsr = 1;
        let mut bytes = vec![0; FRAME_SIZE];
        encode_context(&mut bytes, &original, &fp, 0x1234);
        let mut restored = Trapframe::new();
        let restored_fp = decode_context(&bytes, &mut restored).unwrap();
        assert_eq!(restored.regs.reg, original.regs.reg);
        assert_eq!(restored.elr, original.elr);
        assert_eq!(restored.sp, original.sp);
        assert_eq!(restored.spsr, original.spsr);
        assert_eq!(restored_fp.v, fp.v);
        assert_eq!(restored_fp.fpcr, fp.fpcr);
        assert_eq!(restored_fp.fpsr, fp.fpsr);
        assert_eq!(get64(&bytes, MASK), 0x1234);
    }

    #[test_case]
    fn invalid_signal_return_cannot_change_privileged_context() {
        let mut original = Trapframe::new();
        original.sp = 0x800000;
        original.elr = 0x400000;
        let mut frame = vec![0; FRAME_SIZE];
        encode_context(&mut frame, &original, &FpuContext::new(), 0);
        for (offset, value) in [(PSTATE, 5), (PC, u64::MAX), (SP, 0x800001), (RESERVED, 0)] {
            let saved = get64(&frame, offset);
            put64(&mut frame, offset, value);
            let mut target = original.clone();
            assert!(decode_context(&frame, &mut target).is_err());
            assert_eq!(target.elr, original.elr);
            assert_eq!(target.sp, original.sp);
            assert_eq!(target.regs.reg, original.regs.reg);
            put64(&mut frame, offset, saved);
        }
    }
}
