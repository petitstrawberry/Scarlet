#![allow(unsafe_op_in_unsafe_fn)]

use crate::RawValue;
use crate::runtime::VmContext;

const ESUCCESS: u32 = 0;
const EBADF: u32 = 8;
const ENOSYS: u32 = 52;

macro_rules! get_ops {
    ($ctx:expr) => {
        match $ctx.host_ops.as_ref() {
            Some(ops) => ops,
            None => return ENOSYS as RawValue,
        }
    };
}

pub unsafe fn dispatch_imported(
    ctx: *mut VmContext,
    import_index: u32,
    args: *const RawValue,
) -> RawValue {
    unsafe {
        let ctx_ref = &mut *ctx;
        let (module, name) = match ctx_ref.imported_func_name(import_index) {
            Some(pair) => pair,
            None => return u32::MAX as RawValue,
        };

        if module != "wasi_snapshot_preview1" {
            return ENOSYS as RawValue;
        }

        match name {
            "args_get" => wasi_args_get(ctx_ref, args),
            "args_sizes_get" => wasi_args_sizes_get(ctx_ref, args),
            "clock_res_get" => wasi_clock_res_get(ctx_ref, args),
            "clock_time_get" => wasi_clock_time_get(ctx_ref, args),
            "environ_get" => wasi_environ_get(ctx_ref, args),
            "environ_sizes_get" => wasi_environ_sizes_get(ctx_ref, args),
            "fd_advise" => wasi_fd_advise(ctx_ref, args),
            "fd_allocate" => wasi_fd_allocate(ctx_ref, args),
            "fd_close" => wasi_fd_close(ctx_ref, args),
            "fd_datasync" => wasi_fd_datasync(ctx_ref, args),
            "fd_fdstat_get" => wasi_fd_fdstat_get(ctx_ref, args),
            "fd_fdstat_set_flags" => wasi_fd_fdstat_set_flags(ctx_ref, args),
            "fd_fdstat_set_rights" => wasi_fd_fdstat_set_rights(ctx_ref, args),
            "fd_filestat_get" => wasi_fd_filestat_get(ctx_ref, args),
            "fd_filestat_set_size" => wasi_fd_filestat_set_size(ctx_ref, args),
            "fd_filestat_set_times" => wasi_fd_filestat_set_times(ctx_ref, args),
            "fd_pread" => wasi_fd_pread(ctx_ref, args),
            "fd_prestat_get" => wasi_fd_prestat_get(ctx_ref, args),
            "fd_prestat_dir_name" => wasi_fd_prestat_dir_name(ctx_ref, args),
            "fd_pwrite" => wasi_fd_pwrite(ctx_ref, args),
            "fd_read" => wasi_fd_read(ctx_ref, args),
            "fd_readdir" => wasi_fd_readdir(ctx_ref, args),
            "fd_renumber" => wasi_fd_renumber(ctx_ref, args),
            "fd_seek" => wasi_fd_seek(ctx_ref, args),
            "fd_sync" => wasi_fd_sync(ctx_ref, args),
            "fd_tell" => wasi_fd_tell(ctx_ref, args),
            "fd_write" => wasi_fd_write(ctx_ref, args),
            "path_create_directory" => wasi_path_create_directory(ctx_ref, args),
            "path_filestat_get" => wasi_path_filestat_get(ctx_ref, args),
            "path_filestat_set_times" => wasi_path_filestat_set_times(ctx_ref, args),
            "path_link" => wasi_path_link(ctx_ref, args),
            "path_open" => wasi_path_open(ctx_ref, args),
            "path_readlink" => wasi_path_readlink(ctx_ref, args),
            "path_remove_directory" => wasi_path_remove_directory(ctx_ref, args),
            "path_rename" => wasi_path_rename(ctx_ref, args),
            "path_symlink" => wasi_path_symlink(ctx_ref, args),
            "path_unlink_file" => wasi_path_unlink_file(ctx_ref, args),
            "poll_oneoff" => wasi_poll_oneoff(ctx_ref, args),
            "proc_exit" => wasi_proc_exit(ctx_ref, args),
            "proc_raise" => wasi_proc_raise(ctx_ref, args),
            "random_get" => wasi_random_get(ctx_ref, args),
            "sched_yield" => wasi_sched_yield(),
            "sock_accept" => wasi_sock_accept(ctx_ref, args),
            "sock_recv" => wasi_sock_recv(ctx_ref, args),
            "sock_send" => wasi_sock_send(ctx_ref, args),
            "sock_shutdown" => wasi_sock_shutdown(ctx_ref, args),
            _ => ENOSYS as RawValue,
        }
    }
}

unsafe fn wasi_args_get(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _argv_ptr = *args.add(0) as u32;
    let _argv_buf_ptr = *args.add(1) as u32;
    ESUCCESS as RawValue
}

unsafe fn wasi_args_sizes_get(ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    write_u32_le(ctx, *args.add(0) as u32, 0);
    write_u32_le(ctx, *args.add(1) as u32, 0);
    ESUCCESS as RawValue
}

unsafe fn wasi_clock_res_get(ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _clock_id = *args.add(0) as u32;
    let res_ptr = *args.add(1) as u32;
    write_u64_le(ctx, res_ptr, 1_000_000);
    ESUCCESS as RawValue
}

unsafe fn wasi_clock_time_get(ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let clock_id = *args.add(0) as u32;
    let _precision = *args.add(1) as u64;
    let time_ptr = *args.add(2) as u32;

    let ops = match ctx.host_ops.as_ref() {
        Some(ops) => ops,
        None => {
            write_u64_le(ctx, time_ptr, 0);
            return ESUCCESS as RawValue;
        }
    };

    let mut time: u64 = 0;
    (ops.clock_time_get)(clock_id, &mut time);
    write_u64_le(ctx, time_ptr, time);
    ESUCCESS as RawValue
}

unsafe fn wasi_environ_get(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _environ_ptr = *args.add(0) as u32;
    let _environ_buf_ptr = *args.add(1) as u32;
    ESUCCESS as RawValue
}

unsafe fn wasi_environ_sizes_get(ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    write_u32_le(ctx, *args.add(0) as u32, 0);
    write_u32_le(ctx, *args.add(1) as u32, 0);
    ESUCCESS as RawValue
}

unsafe fn wasi_fd_advise(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _offset = *args.add(1) as u64;
    let _len = *args.add(2) as u64;
    let _advice = *args.add(3) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_fd_allocate(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _offset = *args.add(1) as u64;
    let _len = *args.add(2) as u64;
    ENOSYS as RawValue
}

unsafe fn wasi_fd_close(ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let fd = *args.add(0) as u32;
    let ops = match ctx.host_ops.as_ref() {
        Some(ops) => ops,
        None => return EBADF as RawValue,
    };
    let result = (ops.fd_close)(fd);
    errno_or_success_i32(result)
}

unsafe fn wasi_fd_datasync(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    ESUCCESS as RawValue
}

unsafe fn wasi_fd_fdstat_get(ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let fd = *args.add(0) as u32;
    let buf_ptr = *args.add(1) as u32;

    if !ctx.check_memory(buf_ptr as u64, 24) {
        return EBADF as RawValue;
    }

    let ops = match ctx.host_ops.as_ref() {
        Some(ops) => ops,
        None => return EBADF as RawValue,
    };

    let mut buf = [0u8; 24];
    let result = (ops.fd_fdstat_get)(fd, buf.as_mut_ptr());
    if result < 0 {
        return (-result) as RawValue;
    }

    core::ptr::copy_nonoverlapping(buf.as_ptr(), ctx.memory_base.add(buf_ptr as usize), 24);
    ESUCCESS as RawValue
}

unsafe fn wasi_fd_fdstat_set_flags(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _flags = *args.add(1) as u32;
    ESUCCESS as RawValue
}

unsafe fn wasi_fd_fdstat_set_rights(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _rights_base = *args.add(1) as u64;
    let _rights_inheriting = *args.add(2) as u64;
    ESUCCESS as RawValue
}

unsafe fn wasi_fd_filestat_get(ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let fd = *args.add(0) as u32;
    let buf_ptr = *args.add(1) as u32;

    if !ctx.check_memory(buf_ptr as u64, 64) {
        return EBADF as RawValue;
    }

    let ops = match ctx.host_ops.as_ref() {
        Some(ops) => ops,
        None => return EBADF as RawValue,
    };

    let mut buf = [0u8; 64];
    let result = (ops.fd_filestat_get)(fd, buf.as_mut_ptr());
    if result < 0 {
        return (-result) as RawValue;
    }

    core::ptr::copy_nonoverlapping(buf.as_ptr(), ctx.memory_base.add(buf_ptr as usize), 64);
    ESUCCESS as RawValue
}

unsafe fn wasi_fd_filestat_set_size(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _size = *args.add(1) as u64;
    ENOSYS as RawValue
}

unsafe fn wasi_fd_filestat_set_times(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _atim = *args.add(1) as u64;
    let _mtim = *args.add(2) as u64;
    let _fst_flags = *args.add(3) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_fd_pread(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _iovs_ptr = *args.add(1) as u32;
    let _iovs_len = *args.add(2) as u32;
    let _offset = *args.add(3) as u64;
    let _nread_ptr = *args.add(4) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_fd_prestat_get(ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let fd = *args.add(0) as u32;
    let buf_ptr = *args.add(1) as u32;

    if !ctx.check_memory(buf_ptr as u64, 8) {
        return EBADF as RawValue;
    }

    let ops = match ctx.host_ops.as_ref() {
        Some(ops) => ops,
        None => return EBADF as RawValue,
    };

    let mut buf = [0u8; 8];
    let result = (ops.fd_prestat_get)(fd, buf.as_mut_ptr());
    if result < 0 {
        return (-result) as RawValue;
    }

    core::ptr::copy_nonoverlapping(buf.as_ptr(), ctx.memory_base.add(buf_ptr as usize), 8);
    ESUCCESS as RawValue
}

unsafe fn wasi_fd_prestat_dir_name(ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let fd = *args.add(0) as u32;
    let name_ptr = *args.add(1) as u32;
    let name_len = *args.add(2) as u32;

    if !ctx.check_memory(name_ptr as u64, name_len as u64) {
        return EBADF as RawValue;
    }

    let ops = match ctx.host_ops.as_ref() {
        Some(ops) => ops,
        None => return EBADF as RawValue,
    };

    let dst = ctx.memory_base.add(name_ptr as usize);
    let result = (ops.fd_prestat_dir_name)(fd, dst, name_len);
    if result < 0 {
        return (-result) as RawValue;
    }

    ESUCCESS as RawValue
}

unsafe fn wasi_fd_pwrite(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _iovs_ptr = *args.add(1) as u32;
    let _iovs_len = *args.add(2) as u32;
    let _offset = *args.add(3) as u64;
    let _nwritten_ptr = *args.add(4) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_fd_write(ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let fd = *args.add(0) as u32;
    let iovs_ptr = *args.add(1) as u32;
    let iovs_len = *args.add(2) as u32;
    let nwritten_ptr = *args.add(3) as u32;

    let ops = match ctx.host_ops.as_ref() {
        Some(ops) => ops,
        None => return EBADF as RawValue,
    };

    let mut total: u32 = 0;
    for i in 0..iovs_len {
        let iov_offset = iovs_ptr + i * 8;
        if !ctx.check_memory(iov_offset as u64, 8) {
            ctx.set_trap(crate::TrapCode::MemoryOutOfBounds);
            return u32::MAX as RawValue;
        }

        let buf_ptr = read_u32_le(ctx, iov_offset);
        let buf_len = read_u32_le(ctx, iov_offset + 4);
        if !ctx.check_memory(buf_ptr as u64, buf_len as u64) {
            ctx.set_trap(crate::TrapCode::MemoryOutOfBounds);
            return u32::MAX as RawValue;
        }

        let data_ptr = ctx.memory_base.add(buf_ptr as usize);
        let result = (ops.fd_write)(fd, data_ptr, buf_len as usize);
        if result < 0 {
            return (-result) as RawValue;
        }
        total = total.saturating_add(result as u32);
    }

    write_u32_le(ctx, nwritten_ptr, total);
    ESUCCESS as RawValue
}

unsafe fn wasi_fd_read(ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let fd = *args.add(0) as u32;
    let iovs_ptr = *args.add(1) as u32;
    let iovs_len = *args.add(2) as u32;
    let nread_ptr = *args.add(3) as u32;

    let ops = match ctx.host_ops.as_ref() {
        Some(ops) => ops,
        None => return EBADF as RawValue,
    };

    let mut total: u32 = 0;
    for i in 0..iovs_len {
        let iov_offset = iovs_ptr + i * 8;
        if !ctx.check_memory(iov_offset as u64, 8) {
            ctx.set_trap(crate::TrapCode::MemoryOutOfBounds);
            return u32::MAX as RawValue;
        }

        let buf_ptr = read_u32_le(ctx, iov_offset);
        let buf_len = read_u32_le(ctx, iov_offset + 4);
        if !ctx.check_memory(buf_ptr as u64, buf_len as u64) {
            ctx.set_trap(crate::TrapCode::MemoryOutOfBounds);
            return u32::MAX as RawValue;
        }

        let data_ptr = ctx.memory_base.add(buf_ptr as usize);
        let result = (ops.fd_read)(fd, data_ptr, buf_len as usize);
        if result < 0 {
            return (-result) as RawValue;
        }
        total = total.saturating_add(result as u32);
    }

    write_u32_le(ctx, nread_ptr, total);
    ESUCCESS as RawValue
}

unsafe fn wasi_fd_readdir(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _buf_ptr = *args.add(1) as u32;
    let _buf_len = *args.add(2) as u32;
    let _cookie = *args.add(3) as u64;
    let _bufused_ptr = *args.add(4) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_fd_renumber(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _to = *args.add(1) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_fd_seek(ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let fd = *args.add(0) as u32;
    let offset = *args.add(1) as i64;
    let whence = *args.add(2) as u32;
    let newoffset_ptr = *args.add(3) as u32;
    let ops = get_ops!(ctx);

    let mut new_offset: i64 = 0;
    let result = (ops.fd_seek)(fd, offset, whence, &mut new_offset);
    if result < 0 {
        return (-result) as RawValue;
    }

    write_u64_le(ctx, newoffset_ptr, new_offset as u64);
    ESUCCESS as RawValue
}

unsafe fn wasi_fd_sync(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    ESUCCESS as RawValue
}

unsafe fn wasi_fd_tell(ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let fd = *args.add(0) as u32;
    let offset_ptr = *args.add(1) as u32;
    let ops = get_ops!(ctx);

    let mut offset: i64 = 0;
    let result = (ops.fd_tell)(fd, &mut offset);
    if result < 0 {
        return (-result) as RawValue;
    }

    write_u64_le(ctx, offset_ptr, offset as u64);
    ESUCCESS as RawValue
}

unsafe fn wasi_path_create_directory(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _path_ptr = *args.add(1) as u32;
    let _path_len = *args.add(2) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_path_filestat_get(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _flags = *args.add(1) as u32;
    let _path_ptr = *args.add(2) as u32;
    let _path_len = *args.add(3) as u32;
    let _buf_ptr = *args.add(4) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_path_filestat_set_times(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _flags = *args.add(1) as u32;
    let _path_ptr = *args.add(2) as u32;
    let _path_len = *args.add(3) as u32;
    let _atim = *args.add(4) as u64;
    let _mtim = *args.add(5) as u64;
    let _fst_flags = *args.add(6) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_path_link(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _old_fd = *args.add(0) as u32;
    let _old_flags = *args.add(1) as u32;
    let _old_path_ptr = *args.add(2) as u32;
    let _old_path_len = *args.add(3) as u32;
    let _new_fd = *args.add(4) as u32;
    let _new_path_ptr = *args.add(5) as u32;
    let _new_path_len = *args.add(6) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_path_open(ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let dirfd = *args.add(0) as u32;
    let _dirflags = *args.add(1) as u32;
    let path_ptr = *args.add(2) as u32;
    let path_len = *args.add(3) as u32;
    let oflags = *args.add(4) as u32;
    let _fs_rights_base = *args.add(5) as u64;
    let _fs_rights_inheriting = *args.add(6) as u64;
    let fdflags = *args.add(7) as u32;
    let fd_out_ptr = *args.add(8) as u32;

    if !ctx.check_memory(path_ptr as u64, path_len as u64) {
        return EBADF as RawValue;
    }

    let ops = match ctx.host_ops.as_ref() {
        Some(ops) => ops,
        None => {
            write_u32_le(ctx, fd_out_ptr, u32::MAX);
            return ENOSYS as RawValue;
        }
    };

    let path_ptr_host = ctx.memory_base.add(path_ptr as usize);
    let result = (ops.path_open)(dirfd, path_ptr_host, path_len, oflags, fdflags);
    if result < 0 {
        write_u32_le(ctx, fd_out_ptr, u32::MAX);
        return (-result) as RawValue;
    }

    write_u32_le(ctx, fd_out_ptr, result as u32);
    ESUCCESS as RawValue
}

unsafe fn wasi_path_readlink(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _path_ptr = *args.add(1) as u32;
    let _path_len = *args.add(2) as u32;
    let _buf_ptr = *args.add(3) as u32;
    let _buf_len = *args.add(4) as u32;
    let _nread_ptr = *args.add(5) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_path_remove_directory(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _path_ptr = *args.add(1) as u32;
    let _path_len = *args.add(2) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_path_rename(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _old_path_ptr = *args.add(1) as u32;
    let _old_path_len = *args.add(2) as u32;
    let _new_fd = *args.add(3) as u32;
    let _new_path_ptr = *args.add(4) as u32;
    let _new_path_len = *args.add(5) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_path_symlink(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _old_path_ptr = *args.add(0) as u32;
    let _old_path_len = *args.add(1) as u32;
    let _fd = *args.add(2) as u32;
    let _new_path_ptr = *args.add(3) as u32;
    let _new_path_len = *args.add(4) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_path_unlink_file(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _path_ptr = *args.add(1) as u32;
    let _path_len = *args.add(2) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_poll_oneoff(ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let in_ptr = *args.add(0) as u32;
    let out_ptr = *args.add(1) as u32;
    let nsubscriptions = *args.add(2) as u32;
    let nevents_ptr = *args.add(3) as u32;

    const SUBSCRIPTION_SIZE: u32 = 48;
    const EVENT_SIZE: u32 = 32;

    let in_bytes = nsubscriptions as u64 * SUBSCRIPTION_SIZE as u64;
    let out_bytes = nsubscriptions as u64 * EVENT_SIZE as u64;

    if !ctx.check_memory(in_ptr as u64, in_bytes) {
        ctx.set_trap(crate::TrapCode::MemoryOutOfBounds);
        return u32::MAX as RawValue;
    }
    if !ctx.check_memory(out_ptr as u64, out_bytes) {
        ctx.set_trap(crate::TrapCode::MemoryOutOfBounds);
        return u32::MAX as RawValue;
    }
    if !ctx.check_memory(nevents_ptr as u64, 4) {
        ctx.set_trap(crate::TrapCode::MemoryOutOfBounds);
        return u32::MAX as RawValue;
    }

    let mut nevents: u32 = 0;
    for i in 0..nsubscriptions {
        let sub_base = in_ptr + i * SUBSCRIPTION_SIZE;
        let evt_base = out_ptr + i * EVENT_SIZE;

        // Copy userdata (8 bytes) from subscription to event
        let src = ctx.memory_base.add(sub_base as usize);
        let dst = ctx.memory_base.add(evt_base as usize);
        core::ptr::copy_nonoverlapping(src, dst, 8);

        // Event: bytes 0..7 = userdata, byte 8..9 = error (0=none), byte 10 = type
        // Set error = 0 (no error)
        core::ptr::write_bytes(dst.add(8), 0, 2);

        // Copy subscription type (byte 8 of subscription) to event type (byte 10)
        let sub_type = *ctx.memory_base.add(sub_base as usize + 8);
        *dst.add(10) = sub_type;

        // Zero out rest of event
        core::ptr::write_bytes(dst.add(11), 0, EVENT_SIZE as usize - 11);

        // For clock type (0): write result (u16) at offset 12 = 0 (no error)
        // For fd type: set to 0 (no error)
        nevents += 1;
    }

    write_u32_le(ctx, nevents_ptr, nevents);
    ESUCCESS as RawValue
}

unsafe fn wasi_proc_exit(ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let rval = *args.add(0) as u32;
    ctx.exit_code = rval;
    ctx.exited = true;
    ctx.set_trap(crate::TrapCode::ProcExit);
    ESUCCESS as RawValue
}

unsafe fn wasi_proc_raise(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _sig = *args.add(0) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_random_get(ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let buf_ptr = *args.add(0) as u32;
    let buf_len = *args.add(1) as u32;

    if !ctx.check_memory(buf_ptr as u64, buf_len as u64) {
        ctx.set_trap(crate::TrapCode::MemoryOutOfBounds);
        return u32::MAX as RawValue;
    }

    let ops = match ctx.host_ops.as_ref() {
        Some(ops) => ops,
        None => {
            core::ptr::write_bytes(ctx.memory_base.add(buf_ptr as usize), 0, buf_len as usize);
            return ESUCCESS as RawValue;
        }
    };

    (ops.random_get)(ctx.memory_base.add(buf_ptr as usize), buf_len as usize);
    ESUCCESS as RawValue
}

unsafe fn wasi_sched_yield() -> RawValue {
    ESUCCESS as RawValue
}

unsafe fn wasi_sock_accept(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _flags = *args.add(1) as u32;
    let _fd_out_ptr = *args.add(2) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_sock_recv(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _ri_data_ptr = *args.add(1) as u32;
    let _ri_data_len = *args.add(2) as u32;
    let _ri_flags = *args.add(3) as u32;
    let _ro_data_ptr = *args.add(4) as u32;
    let _ro_flags_ptr = *args.add(5) as u32;
    let _ro_size_ptr = *args.add(6) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_sock_send(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _si_data_ptr = *args.add(1) as u32;
    let _si_data_len = *args.add(2) as u32;
    let _si_flags = *args.add(3) as u32;
    let _so_size_ptr = *args.add(4) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_sock_shutdown(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _how = *args.add(1) as u32;
    ENOSYS as RawValue
}

unsafe fn errno_or_success_i32(result: i32) -> RawValue {
    if result < 0 {
        (-result) as RawValue
    } else {
        ESUCCESS as RawValue
    }
}

unsafe fn read_u32_le(ctx: &VmContext, addr: u32) -> u32 {
    if !ctx.check_memory(addr as u64, 4) {
        return 0;
    }
    let ptr = ctx.memory_base.add(addr as usize);
    u32::from_le(core::ptr::read_unaligned(ptr as *const u32))
}

unsafe fn write_u32_le(ctx: &mut VmContext, addr: u32, value: u32) {
    if ctx.check_memory(addr as u64, 4) {
        let ptr = ctx.memory_base.add(addr as usize);
        core::ptr::write_unaligned(ptr as *mut u32, value.to_le());
    }
}

#[allow(dead_code)]
unsafe fn read_u64_le(ctx: &VmContext, addr: u32) -> u64 {
    if !ctx.check_memory(addr as u64, 8) {
        return 0;
    }
    let ptr = ctx.memory_base.add(addr as usize);
    u64::from_le(core::ptr::read_unaligned(ptr as *const u64))
}

unsafe fn write_u64_le(ctx: &mut VmContext, addr: u32, value: u64) {
    if ctx.check_memory(addr as u64, 8) {
        let ptr = ctx.memory_base.add(addr as usize);
        core::ptr::write_unaligned(ptr as *mut u64, value.to_le());
    }
}
