#![allow(unsafe_op_in_unsafe_fn)]

use crate::RawValue;
use crate::runtime::VmContext;

const ESUCCESS: u32 = 0;
const EBADF: u32 = 8;
const ENOSYS: u32 = 52;

#[allow(dead_code)]
const FILETYPE_UNKNOWN: u8 = 0;
#[allow(dead_code)]
const FILETYPE_BLOCK_DEVICE: u8 = 1;
const FILETYPE_CHARACTER_DEVICE: u8 = 2;
#[allow(dead_code)]
const FILETYPE_DIRECTORY: u8 = 3;
#[allow(dead_code)]
const FILETYPE_REGULAR_FILE: u8 = 4;

#[allow(dead_code)]
const CLOCK_REALTIME: u32 = 0;
#[allow(dead_code)]
const CLOCK_MONOTONIC: u32 = 1;

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
    let _clock_id = *args.add(0) as u32;
    let _precision = *args.add(1) as u64;
    let time_ptr = *args.add(2) as u32;
    write_u64_le(ctx, time_ptr, 0);
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

unsafe fn wasi_fd_close(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    ESUCCESS as RawValue
}

unsafe fn wasi_fd_datasync(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    ESUCCESS as RawValue
}

unsafe fn wasi_fd_fdstat_get(ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let fd = *args.add(0) as u32;
    let buf_ptr = *args.add(1) as u32;

    if fd > 2 {
        return EBADF as RawValue;
    }

    write_u32_le(ctx, buf_ptr, FILETYPE_CHARACTER_DEVICE as u32);
    write_u64_le(ctx, buf_ptr + 8, u64::MAX);
    write_u64_le(ctx, buf_ptr + 16, u64::MAX);
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

unsafe fn wasi_fd_filestat_get(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _buf_ptr = *args.add(1) as u32;
    ENOSYS as RawValue
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

unsafe fn wasi_fd_prestat_get(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _buf_ptr = *args.add(1) as u32;
    EBADF as RawValue
}

unsafe fn wasi_fd_prestat_dir_name(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _name_ptr = *args.add(1) as u32;
    let _name_len = *args.add(2) as u32;
    EBADF as RawValue
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

    if fd != 1 && fd != 2 {
        return EBADF as RawValue;
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

        if let Some(host_write) = ctx.host_write {
            host_write(bytes.as_ptr(), bytes.len());
        }

        total_written += buf_len;
    }

    write_u32_le(ctx, nwritten_ptr, total_written);
    ESUCCESS as RawValue
}

unsafe fn wasi_fd_read(ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let fd = *args.add(0) as u32;
    let _iovs_ptr = *args.add(1) as u32;
    let _iovs_len = *args.add(2) as u32;
    let nread_ptr = *args.add(3) as u32;

    if fd != 0 {
        return EBADF as RawValue;
    }

    write_u32_le(ctx, nread_ptr, 0);
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

unsafe fn wasi_fd_seek(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _offset = *args.add(1) as u64;
    let _whence = *args.add(2) as u32;
    let _newoffset_ptr = *args.add(3) as u32;
    ENOSYS as RawValue
}

unsafe fn wasi_fd_sync(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    ESUCCESS as RawValue
}

unsafe fn wasi_fd_tell(_ctx: &mut VmContext, args: *const RawValue) -> RawValue {
    let _fd = *args.add(0) as u32;
    let _newoffset_ptr = *args.add(1) as u32;
    ENOSYS as RawValue
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
    let _fd = *args.add(0) as u32;
    let _dirflags = *args.add(1) as u32;
    let _path_ptr = *args.add(2) as u32;
    let _path_len = *args.add(3) as u32;
    let _oflags = *args.add(4) as u32;
    let _fs_rights_base = *args.add(5) as u64;
    let _fs_rights_inheriting = *args.add(6) as u64;
    let _fdflags = *args.add(7) as u32;
    let fd_out_ptr = *args.add(8) as u32;
    write_u32_le(ctx, fd_out_ptr, u32::MAX);
    ENOSYS as RawValue
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
    let _in_ptr = *args.add(0) as u32;
    let _out_ptr = *args.add(1) as u32;
    let _nsubscriptions = *args.add(2) as u32;
    let nevents_ptr = *args.add(3) as u32;
    write_u32_le(ctx, nevents_ptr, 0);
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

    core::ptr::write_bytes(ctx.memory_base.add(buf_ptr as usize), 0, buf_len as usize);
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

unsafe fn read_u32_le(ctx: &VmContext, addr: u32) -> u32 {
    unsafe {
        if !ctx.check_memory(addr as u64, 4) {
            return 0;
        }
        let ptr = ctx.memory_base.add(addr as usize);
        u32::from_le(core::ptr::read_unaligned(ptr as *const u32))
    }
}

unsafe fn write_u32_le(ctx: &mut VmContext, addr: u32, value: u32) {
    unsafe {
        if ctx.check_memory(addr as u64, 4) {
            let ptr = ctx.memory_base.add(addr as usize);
            core::ptr::write_unaligned(ptr as *mut u32, value.to_le());
        }
    }
}

#[allow(dead_code)]
unsafe fn read_u64_le(ctx: &VmContext, addr: u32) -> u64 {
    unsafe {
        if !ctx.check_memory(addr as u64, 8) {
            return 0;
        }
        let ptr = ctx.memory_base.add(addr as usize);
        u64::from_le(core::ptr::read_unaligned(ptr as *const u64))
    }
}

unsafe fn write_u64_le(ctx: &mut VmContext, addr: u32, value: u64) {
    unsafe {
        if ctx.check_memory(addr as u64, 8) {
            let ptr = ctx.memory_base.add(addr as usize);
            core::ptr::write_unaligned(ptr as *mut u64, value.to_le());
        }
    }
}
