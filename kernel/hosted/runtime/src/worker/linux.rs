//! Restrict the Linux worker to operations on its inherited channels,
//! mappings, and its own threads. The filter is installed before attachment.

use std::io;

fn statement(code: u16, k: u32) -> libc::sock_filter {
    libc::sock_filter {
        code,
        jt: 0,
        jf: 0,
        k,
    }
}

fn equal(k: u32, jt: u8, jf: u8) -> libc::sock_filter {
    libc::sock_filter {
        code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
        jt,
        jf,
        k,
    }
}

pub(super) fn confine() -> io::Result<()> {
    const LOAD: u16 = (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16;
    const RET: u16 = (libc::BPF_RET | libc::BPF_K) as u16;
    const AUDIT_ARCH_AARCH64: u32 = 0xc000_00b7;
    const ALLOW: u32 = libc::SECCOMP_RET_ALLOW;
    const DENY: u32 = libc::SECCOMP_RET_ERRNO | libc::EPERM as u32;
    const NR: u32 = 0;
    const ARCH: u32 = 4;
    const ARG0: u32 = 16;

    let pid = unsafe { libc::getpid() } as u32;
    let thread_flags = libc::CLONE_VM
        | libc::CLONE_FS
        | libc::CLONE_FILES
        | libc::CLONE_SIGHAND
        | libc::CLONE_THREAD
        | libc::CLONE_SYSVSEM
        | libc::CLONE_SETTLS
        | libc::CLONE_PARENT_SETTID
        | libc::CLONE_CHILD_CLEARTID;
    let mut filter = vec![
        statement(LOAD, ARCH),
        equal(AUDIT_ARCH_AARCH64, 1, 0),
        statement(RET, libc::SECCOMP_RET_KILL_PROCESS),
        statement(LOAD, NR),
        // glibc retries with pthread clone when clone3 is unavailable.
        equal(libc::SYS_clone3 as u32, 0, 1),
        statement(RET, libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32),
        equal(libc::SYS_clone as u32, 0, 4),
        statement(LOAD, ARG0),
        equal(thread_flags as u32, 0, 1),
        statement(RET, ALLOW),
        statement(RET, DENY),
        equal(libc::SYS_tgkill as u32, 0, 4),
        statement(LOAD, ARG0),
        equal(pid, 0, 1),
        statement(RET, ALLOW),
        statement(RET, DENY),
    ];

    for syscall in [
        libc::SYS_read,
        libc::SYS_write,
        libc::SYS_close,
        libc::SYS_sendto,
        libc::SYS_recvfrom,
        libc::SYS_sendmsg,
        libc::SYS_recvmsg,
        libc::SYS_fcntl,
        libc::SYS_ppoll,
        libc::SYS_mmap,
        libc::SYS_mprotect,
        libc::SYS_munmap,
        libc::SYS_madvise,
        libc::SYS_brk,
        libc::SYS_futex,
        libc::SYS_rseq,
        libc::SYS_set_robust_list,
        libc::SYS_sched_yield,
        libc::SYS_sched_getaffinity,
        libc::SYS_getrandom,
        libc::SYS_getpid,
        libc::SYS_gettid,
        libc::SYS_clock_gettime,
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigreturn,
        libc::SYS_sigaltstack,
        libc::SYS_exit,
        libc::SYS_exit_group,
    ] {
        filter.push(equal(syscall as u32, 0, 1));
        filter.push(statement(RET, ALLOW));
    }
    filter.push(statement(RET, DENY));

    let program = libc::sock_fprog {
        len: filter.len().try_into().unwrap(),
        filter: filter.as_mut_ptr(),
    };
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0
        || unsafe {
            libc::prctl(
                libc::PR_SET_SECCOMP,
                libc::SECCOMP_MODE_FILTER,
                &raw const program,
            )
        } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
