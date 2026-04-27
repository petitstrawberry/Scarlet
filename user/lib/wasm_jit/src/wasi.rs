use crate::RawValue;
use crate::runtime::VmContext;

/// Dispatch an imported function call by index.
/// Currently supports only a single import convention:
///   index 0: write(ptr: u32, len: u32) -> ()
/// Calls host_write if set on the VmContext.
pub unsafe fn dispatch_imported(
    ctx: *mut VmContext,
    _import_index: u32,
    args: *const RawValue,
) -> RawValue {
    unsafe {
        let ptr = *args.add(0) as u32;
        let len = *args.add(1) as u32;

        let ctx_ref = &mut *ctx;
        if let Some(host_write) = ctx_ref.host_write {
            if ctx_ref.check_memory(ptr as u64, len as u64) {
                let buf = core::slice::from_raw_parts(
                    ctx_ref.memory_base.add(ptr as usize),
                    len as usize,
                );
                host_write(buf.as_ptr(), buf.len());
            }
        }
        0
    }
}

pub unsafe extern "C" fn wasi_fd_write(
    ctx: *mut VmContext,
    fd: u32,
    iovs_ptr: u32,
    iovs_len: u32,
    nwritten_ptr: u32,
) -> RawValue {
    unsafe {
        let ctx = &mut *ctx;
        if fd != 1 && fd != 2 {
            ctx.set_trap(crate::TrapCode::BadCall);
            return u32::MAX as RawValue;
        }

        let mut total_written: u32 = 0;
        for i in 0..iovs_len {
            let iov_offset = iovs_ptr + i * 8;
            let buf_ptr = read_u32_le(ctx, iov_offset);
            let buf_len = read_u32_le(ctx, iov_offset + 4);

            if !ctx.check_memory(buf_ptr as u64, buf_len as u64) {
                ctx.set_trap(crate::TrapCode::MemoryOutOfBounds);
                return u32::MAX as RawValue;
            }

            let src = ctx.memory_base.add(buf_ptr as usize);
            let bytes = core::slice::from_raw_parts(src, buf_len as usize);

            total_written += buf_len;
            let _ = bytes;
        }

        write_u32_le(ctx, nwritten_ptr, total_written);
        0
    }
}

pub unsafe extern "C" fn wasi_fd_read(
    ctx: *mut VmContext,
    fd: u32,
    iovs_ptr: u32,
    iovs_len: u32,
    nread_ptr: u32,
) -> RawValue {
    unsafe {
        let ctx = &mut *ctx;
        if fd != 0 {
            ctx.set_trap(crate::TrapCode::BadCall);
            return u32::MAX as RawValue;
        }

        write_u32_le(ctx, nread_ptr, 0);
        0
    }
}

pub unsafe extern "C" fn wasi_path_open(
    ctx: *mut VmContext,
    _dirfd: u32,
    _dirflags: u32,
    _path_ptr: u32,
    _path_len: u32,
    _oflags: u32,
    _fs_rights_base: u64,
    _fs_rights_inheriting: u64,
    _fdflags: u32,
    fd_out_ptr: u32,
) -> RawValue {
    unsafe {
        let ctx = &mut *ctx;
        write_u32_le(ctx, fd_out_ptr, u32::MAX);
        0
    }
}

pub unsafe extern "C" fn wasi_proc_exit(_ctx: *mut VmContext, _rval: u32) -> RawValue {
    0
}

pub unsafe extern "C" fn wasi_environ_sizes_get(
    ctx: *mut VmContext,
    count_ptr: u32,
    buf_size_ptr: u32,
) -> RawValue {
    unsafe {
        write_u32_le(&mut *ctx, count_ptr, 0);
        write_u32_le(&mut *ctx, buf_size_ptr, 0);
    }
    0
}

pub unsafe extern "C" fn wasi_environ_get(
    _ctx: *mut VmContext,
    _environ_ptr: u32,
    _environ_buf_ptr: u32,
) -> RawValue {
    0
}

pub unsafe extern "C" fn wasi_args_sizes_get(
    ctx: *mut VmContext,
    count_ptr: u32,
    buf_size_ptr: u32,
) -> RawValue {
    unsafe {
        write_u32_le(&mut *ctx, count_ptr, 0);
        write_u32_le(&mut *ctx, buf_size_ptr, 0);
    }
    0
}

pub unsafe extern "C" fn wasi_args_get(
    _ctx: *mut VmContext,
    _argv_ptr: u32,
    _argv_buf_ptr: u32,
) -> RawValue {
    0
}

unsafe fn read_u32_le(ctx: &VmContext, addr: u32) -> u32 {
    unsafe {
        if !ctx.check_memory(addr as u64, 4) {
            return 0;
        }
        let ptr = ctx.memory_base.add(addr as usize);
        core::ptr::read_unaligned(ptr as *const u32)
    }
}

unsafe fn write_u32_le(ctx: &mut VmContext, addr: u32, value: u32) {
    unsafe {
        if ctx.check_memory(addr as u64, 4) {
            let ptr = ctx.memory_base.add(addr as usize);
            core::ptr::write_unaligned(ptr as *mut u32, value);
        }
    }
}
