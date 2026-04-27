use crate::runtime::VmContext;
use crate::{RawValue, TrapCode};

pub unsafe extern "C" fn helper_i32_load(ctx: *mut VmContext, addr: u32) -> RawValue {
    unsafe {
        let ctx = &mut *ctx;
        let offset = addr as u64;
        if !ctx.check_memory(offset, 4) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return 0;
        }
        let ptr = ctx.memory_base.add(addr as usize);
        let val = core::ptr::read_unaligned(ptr as *const u32);
        val as RawValue
    }
}

pub unsafe extern "C" fn helper_i32_store(ctx: *mut VmContext, addr: u32, value: u32) {
    unsafe {
        let ctx = &mut *ctx;
        let offset = addr as u64;
        if !ctx.check_memory(offset, 4) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return;
        }
        let ptr = ctx.memory_base.add(addr as usize);
        core::ptr::write_unaligned(ptr as *mut u32, value);
    }
}

pub unsafe extern "C" fn helper_call(
    ctx: *mut VmContext,
    func_index: u32,
    args_ptr: *const RawValue,
) -> RawValue {
    unsafe {
        let ctx = &mut *ctx;
        if func_index as usize >= ctx.function_count {
            ctx.set_trap(TrapCode::BadCall);
            return 0;
        }
        if !ctx.enter_call() {
            return 0;
        }

        let entry = &*ctx.functions.add(func_index as usize);

        let mut frame = alloc::vec![0u64; entry.frame_slots as usize];
        let param_count = entry.param_count as usize;
        for i in 0..param_count.min(core::cmp::min(entry.frame_slots as usize, 16)) {
            frame[i] = *args_ptr.add(i);
        }

        let result = (entry.code)(ctx, frame.as_mut_ptr());

        ctx.leave_call();
        result
    }
}

pub unsafe extern "C" fn helper_trap(ctx: *mut VmContext, trap: TrapCode) -> ! {
    unsafe {
        (*ctx).set_trap(trap);
        core::hint::unreachable_unchecked()
    }
}
