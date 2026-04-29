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
        ctx.debug_store_count = ctx.debug_store_count.wrapping_add(1);
        ctx.debug_last_store_addr = addr;
        ctx.debug_last_store_value = value as u64;
        let offset = addr as u64;
        if !ctx.check_memory(offset, 4) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return;
        }
        let ptr = ctx.memory_base.add(addr as usize);
        core::ptr::write_unaligned(ptr as *mut u32, value);
    }
}

pub unsafe extern "C" fn helper_i32_load8_u(ctx: *mut VmContext, addr: u32) -> RawValue {
    unsafe {
        let ctx = &mut *ctx;
        let offset = addr as u64;
        if !ctx.check_memory(offset, 1) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return 0;
        }
        u64::from(*ctx.memory_base.add(addr as usize))
    }
}

pub unsafe extern "C" fn helper_i32_load8_s(ctx: *mut VmContext, addr: u32) -> RawValue {
    unsafe {
        let ctx = &mut *ctx;
        let offset = addr as u64;
        if !ctx.check_memory(offset, 1) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return 0;
        }
        let value = *(ctx.memory_base.add(addr as usize) as *const i8);
        (value as i32) as u32 as RawValue
    }
}

pub unsafe extern "C" fn helper_i32_load16_u(ctx: *mut VmContext, addr: u32) -> RawValue {
    unsafe {
        let ctx = &mut *ctx;
        let offset = addr as u64;
        if !ctx.check_memory(offset, 2) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return 0;
        }
        let ptr = ctx.memory_base.add(addr as usize) as *const u16;
        u64::from(core::ptr::read_unaligned(ptr))
    }
}

pub unsafe extern "C" fn helper_i32_load16_s(ctx: *mut VmContext, addr: u32) -> RawValue {
    unsafe {
        let ctx = &mut *ctx;
        let offset = addr as u64;
        if !ctx.check_memory(offset, 2) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return 0;
        }
        let ptr = ctx.memory_base.add(addr as usize) as *const i16;
        let value = core::ptr::read_unaligned(ptr);
        (value as i32) as u32 as RawValue
    }
}

pub unsafe extern "C" fn helper_i64_load8_u(ctx: *mut VmContext, addr: u32) -> RawValue {
    unsafe {
        let ctx = &mut *ctx;
        let offset = addr as u64;
        if !ctx.check_memory(offset, 1) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return 0;
        }
        u64::from(*ctx.memory_base.add(addr as usize))
    }
}

pub unsafe extern "C" fn helper_i64_load8_s(ctx: *mut VmContext, addr: u32) -> RawValue {
    unsafe {
        let ctx = &mut *ctx;
        let offset = addr as u64;
        if !ctx.check_memory(offset, 1) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return 0;
        }
        let value = *(ctx.memory_base.add(addr as usize) as *const i8);
        value as i64 as u64
    }
}

pub unsafe extern "C" fn helper_i64_load16_u(ctx: *mut VmContext, addr: u32) -> RawValue {
    unsafe {
        let ctx = &mut *ctx;
        let offset = addr as u64;
        if !ctx.check_memory(offset, 2) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return 0;
        }
        let ptr = ctx.memory_base.add(addr as usize) as *const u16;
        u64::from(core::ptr::read_unaligned(ptr))
    }
}

pub unsafe extern "C" fn helper_i64_load16_s(ctx: *mut VmContext, addr: u32) -> RawValue {
    unsafe {
        let ctx = &mut *ctx;
        let offset = addr as u64;
        if !ctx.check_memory(offset, 2) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return 0;
        }
        let ptr = ctx.memory_base.add(addr as usize) as *const i16;
        let value = core::ptr::read_unaligned(ptr);
        value as i64 as u64
    }
}

pub unsafe extern "C" fn helper_i64_load32_u(ctx: *mut VmContext, addr: u32) -> RawValue {
    unsafe {
        let ctx = &mut *ctx;
        let offset = addr as u64;
        if !ctx.check_memory(offset, 4) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return 0;
        }
        let ptr = ctx.memory_base.add(addr as usize) as *const u32;
        u64::from(core::ptr::read_unaligned(ptr))
    }
}

pub unsafe extern "C" fn helper_i64_load32_s(ctx: *mut VmContext, addr: u32) -> RawValue {
    unsafe {
        let ctx = &mut *ctx;
        let offset = addr as u64;
        if !ctx.check_memory(offset, 4) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return 0;
        }
        let ptr = ctx.memory_base.add(addr as usize) as *const i32;
        let value = core::ptr::read_unaligned(ptr);
        value as i64 as u64
    }
}

pub unsafe extern "C" fn helper_i32_store8(ctx: *mut VmContext, addr: u32, value: u32) {
    unsafe {
        let ctx = &mut *ctx;
        let offset = addr as u64;
        if !ctx.check_memory(offset, 1) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return;
        }
        *ctx.memory_base.add(addr as usize) = value as u8;
    }
}

pub unsafe extern "C" fn helper_i32_store16(ctx: *mut VmContext, addr: u32, value: u32) {
    unsafe {
        let ctx = &mut *ctx;
        let offset = addr as u64;
        if !ctx.check_memory(offset, 2) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return;
        }
        let ptr = ctx.memory_base.add(addr as usize) as *mut u16;
        core::ptr::write_unaligned(ptr, value as u16);
    }
}

pub unsafe extern "C" fn helper_i64_store8(ctx: *mut VmContext, addr: u32, value: u64) {
    unsafe {
        let ctx = &mut *ctx;
        let offset = addr as u64;
        if !ctx.check_memory(offset, 1) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return;
        }
        *ctx.memory_base.add(addr as usize) = value as u8;
    }
}

pub unsafe extern "C" fn helper_i64_store16(ctx: *mut VmContext, addr: u32, value: u64) {
    unsafe {
        let ctx = &mut *ctx;
        let offset = addr as u64;
        if !ctx.check_memory(offset, 2) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return;
        }
        let ptr = ctx.memory_base.add(addr as usize) as *mut u16;
        core::ptr::write_unaligned(ptr, value as u16);
    }
}

pub unsafe extern "C" fn helper_i64_store32(ctx: *mut VmContext, addr: u32, value: u64) {
    unsafe {
        let ctx = &mut *ctx;
        let offset = addr as u64;
        if !ctx.check_memory(offset, 4) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return;
        }
        let ptr = ctx.memory_base.add(addr as usize) as *mut u32;
        core::ptr::write_unaligned(ptr, value as u32);
    }
}

pub unsafe extern "C" fn helper_memory_copy(ctx: *mut VmContext, dst: u32, src: u32, len: u32) {
    unsafe {
        let ctx = &mut *ctx;
        let len64 = u64::from(len);
        if !ctx.check_memory(u64::from(dst), len64) || !ctx.check_memory(u64::from(src), len64) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return;
        }
        core::ptr::copy(
            ctx.memory_base.add(src as usize),
            ctx.memory_base.add(dst as usize),
            len as usize,
        );
    }
}

pub unsafe extern "C" fn helper_memory_fill(ctx: *mut VmContext, dst: u32, value: u32, len: u32) {
    unsafe {
        let ctx = &mut *ctx;
        if !ctx.check_memory(u64::from(dst), u64::from(len)) {
            ctx.set_trap(TrapCode::MemoryOutOfBounds);
            return;
        }
        core::ptr::write_bytes(ctx.memory_base.add(dst as usize), value as u8, len as usize);
    }
}

pub unsafe extern "C" fn helper_memory_grow(ctx: *mut VmContext, delta: u32) -> RawValue {
    unsafe {
        let ctx = &mut *ctx;
        let prev_pages = ctx.memory_len / 65536;
        let delta_bytes = delta as usize * 65536;
        let result = if delta_bytes == 0 {
            (prev_pages as u32) as RawValue
        } else {
            let new_len = match ctx.memory_len.checked_add(delta_bytes) {
                Some(n) => n,
                None => return u32::MAX as RawValue,
            };
            if new_len > ctx.memory_cap {
                return u32::MAX as RawValue;
            }
            ctx.memory_len = new_len;
            (prev_pages as u32) as RawValue
        };
        result
    }
}

pub unsafe extern "C" fn helper_memory_size(ctx: *mut VmContext) -> RawValue {
    unsafe { ((*ctx).memory_len / 65536) as u32 as RawValue }
}

pub unsafe extern "C" fn helper_global_get(ctx: *mut VmContext, index: u32) -> RawValue {
    unsafe {
        let ctx = &mut *ctx;
        if index as usize >= ctx.global_count {
            ctx.set_trap(TrapCode::BadCall);
            return 0;
        }
        (*ctx.globals.add(index as usize)).value
    }
}

pub unsafe extern "C" fn helper_global_set(ctx: *mut VmContext, index: u32, value: RawValue) {
    unsafe {
        let ctx = &mut *ctx;
        ctx.debug_global_set_count = ctx.debug_global_set_count.wrapping_add(1);
        ctx.debug_last_global_idx = index;
        ctx.debug_last_global_val = value;
        if index as usize >= ctx.global_count {
            ctx.set_trap(TrapCode::BadCall);
            return;
        }
        let global = &mut *ctx.globals.add(index as usize);
        if !global.mutable || (index as usize) < ctx.imported_global_count {
            ctx.set_trap(TrapCode::BadCall);
            return;
        }
        global.value = value;
    }
}

pub unsafe extern "C" fn helper_call(
    ctx: *mut VmContext,
    func_index: u32,
    args_ptr: *const RawValue,
) -> RawValue {
    unsafe {
        let ctx = &mut *ctx;
        ctx.debug_last_func = func_index;
        ctx.debug_call_count = ctx.debug_call_count.wrapping_add(1);
        let ti = ctx.debug_trace_idx;
        if ti < 64 {
            ctx.debug_trace[ti] = func_index;
            ctx.debug_trace_idx = ti + 1;
        }
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
        for i in 0..param_count.min(entry.frame_slots as usize) {
            frame[i] = *args_ptr.add(i);
        }

        let result = (entry.code)(ctx, frame.as_mut_ptr());

        ctx.leave_call();
        result
    }
}

pub unsafe extern "C" fn helper_call_indirect(
    ctx: *mut VmContext,
    table_index: u32,
    args_ptr: *const RawValue,
) -> RawValue {
    unsafe {
        let ctx = &mut *ctx;
        if table_index as usize >= ctx.table_count {
            ctx.set_trap(TrapCode::BadCall);
            return 0;
        }
        let func_index = *ctx.table.add(table_index as usize);
        if func_index == u32::MAX {
            ctx.set_trap(TrapCode::BadCall);
            return 0;
        }
        if func_index < ctx.imported_count as u32 {
            crate::wasi::dispatch_imported(ctx, func_index, args_ptr)
        } else {
            helper_call(ctx, func_index - ctx.imported_count as u32, args_ptr)
        }
    }
}

pub unsafe extern "C" fn helper_trap(ctx: *mut VmContext, trap: TrapCode) -> RawValue {
    unsafe {
        (*ctx).set_trap(trap);
    }
    0
}
