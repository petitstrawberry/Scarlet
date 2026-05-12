#[macro_use]
mod macros;
pub mod errno;
pub mod fs;
pub mod futex;
pub mod mm;
pub mod pipe;
pub mod proc;
pub mod signal;
pub mod socket;
pub mod time;

use alloc::{collections::BTreeMap, sync::Arc, vec, vec::Vec};

use self::time::PosixTimer;
use crate::arch::Trapframe;

const MAX_FDS: usize = 1024;

#[derive(Clone, Default)]
pub struct LinuxThreadState {
    pub parent_tid_ptr: Option<usize>,
    pub child_tid_ptr: Option<usize>,
    pub clear_child_tid_ptr: Option<usize>,
    pub robust_list_head: Option<usize>,
    pub robust_list_len: usize,
    pub tls_pointer: Option<usize>,
    pub tgid: usize,
    pub pending_clone_is_thread: bool,
}

#[derive(Clone)]
pub struct LinuxAbi {
    pub namespace: Arc<crate::task::namespace::TaskNamespace>,
    pub fd_to_handle: Vec<Option<u32>>,
    pub fd_flags: Vec<u32>,
    pub file_status_flags: Vec<u32>,
    pub free_fds: Vec<usize>,
    pub signal_state: Arc<spin::Mutex<signal::SignalState>>,
    pub thread_state: LinuxThreadState,
    pub posix_timers: BTreeMap<u64, PosixTimer>,
    pub next_timer_id: u64,
}

impl Default for LinuxAbi {
    fn default() -> Self {
        let mut free_fds: Vec<usize> = (0..MAX_FDS).collect();
        free_fds.reverse();

        let namespace = crate::task::namespace::get_root_namespace().clone();

        Self {
            namespace,
            fd_to_handle: vec![None; MAX_FDS],
            fd_flags: vec![0; MAX_FDS],
            file_status_flags: vec![0; MAX_FDS],
            free_fds,
            signal_state: Arc::new(spin::Mutex::new(signal::SignalState::new())),
            thread_state: LinuxThreadState::default(),
            posix_timers: BTreeMap::new(),
            next_timer_id: 1,
        }
    }
}

impl LinuxAbi {
    pub fn thread_state(&self) -> &LinuxThreadState {
        &self.thread_state
    }
    pub fn thread_state_mut(&mut self) -> &mut LinuxThreadState {
        &mut self.thread_state
    }

    pub fn allocate_fd(&mut self, handle: u32) -> Result<usize, &'static str> {
        let fd = if let Some(freed_fd) = self.free_fds.pop() {
            freed_fd
        } else {
            return Err("Too many open files");
        };
        self.fd_to_handle[fd] = Some(handle);
        Ok(fd)
    }

    pub fn allocate_specific_fd(&mut self, fd: usize, handle: u32) -> Result<(), &'static str> {
        if fd >= MAX_FDS {
            return Err("File descriptor out of range");
        }
        if self.fd_to_handle[fd].is_some() {
            return Err("File descriptor already in use");
        }
        if let Some(pos) = self.free_fds.iter().position(|&x| x == fd) {
            self.free_fds.remove(pos);
        }
        self.fd_to_handle[fd] = Some(handle);
        Ok(())
    }

    pub fn get_handle(&self, fd: usize) -> Option<u32> {
        if fd < MAX_FDS {
            self.fd_to_handle[fd]
        } else {
            None
        }
    }

    pub fn remove_fd(&mut self, fd: usize) -> Option<u32> {
        if fd < MAX_FDS {
            if let Some(handle) = self.fd_to_handle[fd].take() {
                self.fd_flags[fd] = 0;
                self.file_status_flags[fd] = 0;
                self.free_fds.push(fd);
                Some(handle)
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn find_fd_by_handle(&self, handle: u32) -> Option<usize> {
        for (fd, &mapped_handle) in self.fd_to_handle.iter().enumerate() {
            if let Some(h) = mapped_handle {
                if h == handle {
                    return Some(fd);
                }
            }
        }
        None
    }

    pub fn init_std_fds(&mut self, stdin_handle: u32, stdout_handle: u32, stderr_handle: u32) {
        self.fd_to_handle[0] = Some(stdin_handle);
        self.fd_to_handle[1] = Some(stdout_handle);
        self.fd_to_handle[2] = Some(stderr_handle);
        self.free_fds.retain(|&fd| fd != 0 && fd != 1 && fd != 2);
    }

    pub fn get_fd_flags(&self, fd: usize) -> Option<u32> {
        if fd < MAX_FDS && self.fd_to_handle[fd].is_some() {
            Some(self.fd_flags[fd])
        } else {
            None
        }
    }

    pub fn set_fd_flags(&mut self, fd: usize, flags: u32) -> Result<(), &'static str> {
        use crate::{object::handle::SpecialSemantics, task::mytask};

        if fd < MAX_FDS && self.fd_to_handle[fd].is_some() {
            let handle = self.fd_to_handle[fd].unwrap();
            self.fd_flags[fd] = flags;

            if let Some(task) = mytask() {
                if let Some(current_metadata) = task.handle_table.get_metadata(handle) {
                    let mut new_metadata = current_metadata.clone();

                    if flags & fs::FD_CLOEXEC != 0 {
                        new_metadata.special_semantics = Some(SpecialSemantics::CloseOnExec);
                    } else {
                        if matches!(
                            new_metadata.special_semantics,
                            Some(SpecialSemantics::CloseOnExec)
                        ) {
                            new_metadata.special_semantics = None;
                        }
                    }

                    let _ = task.handle_table.update_metadata(handle, new_metadata);
                }
            }

            Ok(())
        } else {
            Err("Invalid file descriptor")
        }
    }

    pub fn get_file_status_flags(&self, fd: usize) -> Option<u32> {
        if fd < MAX_FDS && self.fd_to_handle[fd].is_some() {
            Some(self.file_status_flags[fd])
        } else {
            None
        }
    }

    pub fn set_file_status_flags(&mut self, fd: usize, flags: u32) -> Result<(), &'static str> {
        if fd < MAX_FDS && self.fd_to_handle[fd].is_some() {
            self.file_status_flags[fd] = flags;
            Ok(())
        } else {
            Err("Invalid file descriptor")
        }
    }

    pub fn fd_count(&self) -> usize {
        self.fd_to_handle.iter().filter(|&&h| h.is_some()).count()
    }

    pub fn allocated_fds(&self) -> Vec<usize> {
        self.fd_to_handle
            .iter()
            .enumerate()
            .filter_map(|(fd, &handle)| if handle.is_some() { Some(fd) } else { None })
            .collect()
    }

    pub fn process_signals(&self, trapframe: &mut Trapframe) -> bool {
        let mut signal_state = self.signal_state.lock();
        signal::process_pending_signals_with_state(&mut *signal_state, trapframe)
    }

    pub fn handle_event_direct(&self, event: &crate::ipc::event::Event) {
        if let Some(signal) = signal::handle_event_to_signal(event) {
            let mut signal_state = self.signal_state.lock();
            signal_state.add_pending(signal);
        }
    }

    pub fn has_pending_signals(&self) -> bool {
        let signal_state = self.signal_state.lock();
        signal_state.next_deliverable_signal().is_some()
    }

    pub fn allocate_posix_timer_id(&mut self) -> u64 {
        let mut id = self.next_timer_id;
        if id == 0 {
            id = 1;
        }
        self.next_timer_id = id.wrapping_add(1);
        if self.next_timer_id == 0 {
            self.next_timer_id = 1;
        }
        id
    }

    pub fn store_posix_timer(&mut self, timer: PosixTimer) {
        self.posix_timers.insert(timer.id, timer);
    }

    pub fn get_posix_timer(&self, id: u64) -> Option<&PosixTimer> {
        self.posix_timers.get(&id)
    }

    pub fn remove_posix_timer(&mut self, id: u64) -> Option<PosixTimer> {
        self.posix_timers.remove(&id)
    }
}

syscall_table! {
    dispatch_common_syscall,
    Invalid = 0 => |_abi: &mut crate::abi::linux::generic::LinuxAbi, _trapframe: &mut crate::arch::Trapframe| {
        0
    },
    Getcwd = 17 => fs::sys_getcwd,
    Eventfd2 = 19 => fs::sys_eventfd2,
    EpollCtl = 21 => fs::sys_epoll_ctl,
    EpollPwait = 22 => fs::sys_epoll_pwait,
    EpollCreate1 = 20 => fs::sys_epoll_create1,
    Flock = 32 => fs::sys_flock,
    Dup = 23 => fs::sys_dup,
    Dup3 = 24 => fs::sys_dup3,
    Fcntl = 25 => fs::sys_fcntl,
    Ioctl = 29 => fs::sys_ioctl,
    MkdirAt = 34 => fs::sys_mkdirat,
    UnlinkAt = 35 => fs::sys_unlinkat,
    Ftruncate = 46 => fs::sys_ftruncate,
    Fallocate = 47 => fs::sys_fallocate,
    LinkAt = 37 => fs::sys_linkat,
    FaccessAt = 48 => fs::sys_faccessat,
    Chdir = 49 => fs::sys_chdir,
    Fchmod = 52 => fs::sys_fchmod,
    OpenAt = 56 => fs::sys_openat,
    Close = 57 => fs::sys_close,
    Pipe2 = 59 => pipe::sys_pipe2,
    GetDents64 = 61 => fs::sys_getdents64,
    Lseek = 62 => fs::sys_lseek,
    Read = 63 => fs::sys_read,
    Write = 64 => fs::sys_write,
    Readv = 65 => fs::sys_readv,
    Writev = 66 => fs::sys_writev,
    Pread64 = 67 => fs::sys_pread64,
    Pwrite64 = 68 => fs::sys_pwrite64,
    Pselect6 = 72 => fs::sys_pselect6,
    Ppoll = 73 => fs::sys_ppoll,
    NewFstAtAt = 79 => fs::sys_newfstatat,
    NewFstat = 80 => fs::sys_newfstat,
    ReadLinkAt = 78 => fs::sys_readlinkat,
    Fsync = 82 => fs::sys_fsync,
    Exit = 93 => proc::sys_exit,
    ExitGroup = 94 => proc::sys_exit_group,
    SetTidAddress = 96 => proc::sys_set_tid_address,
    Futex = 98 => futex::sys_futex,
    SetRobustList = 99 => proc::sys_set_robust_list,
    Nanosleep = 101 => time::sys_nanosleep,
    TimerCreate = 107 => time::sys_timer_create,
    TimerGettime = 108 => time::sys_timer_gettime,
    TimerGetoverrun = 109 => time::sys_timer_getoverrun,
    TimerSettime = 110 => time::sys_timer_settime,
    TimerDelete = 111 => time::sys_timer_delete,
    ClockGettime = 113 => time::sys_clock_gettime,
    ClockGetres = 114 => time::sys_clock_getres,
    RtSigaction = 134 => signal::sys_rt_sigaction,
    RtSigprocmask = 135 => signal::sys_rt_sigprocmask,
    SetGid = 144 => proc::sys_setgid,
    SetUid = 146 => proc::sys_setuid,
    SetPgid = 154 => proc::sys_setpgid,
    GetPgid = 155 => proc::sys_getpgid,
    Uname = 160 => proc::sys_uname,
    Umask = 166 => fs::sys_umask,
    Prctl = 167 => proc::sys_prctl,
    GetPid = 172 => proc::sys_getpid,
    GetPpid = 173 => proc::sys_getppid,
    GetUid = 174 => proc::sys_getuid,
    GetEuid = 175 => proc::sys_geteuid,
    GetGid = 176 => proc::sys_getgid,
    GetEgid = 177 => proc::sys_getegid,
    GetTid = 178 => proc::sys_gettid,
    Kill = 129 => signal::sys_tkill,
    Tkill = 130 => signal::sys_tkill,
    Brk = 214 => proc::sys_brk,
    Munmap = 215 => mm::sys_munmap,
    Clone = 220 => proc::sys_clone,
    Execve = 221 => fs::sys_execve,
    Mmap = 222 => mm::sys_mmap,
    Mprotect = 226 => mm::sys_mprotect,
    EpollWait = 232 => fs::sys_epoll_wait,
    Getrandom = 278 => fs::sys_getrandom,
    MemfdCreate = 279 => proc::sys_memfd_create,
    Wait4 = 260 => proc::sys_wait4,
    Prlimit64 = 261 => proc::sys_prlimit64,
    Socket = 198 => socket::sys_socket,
    Socketpair = 199 => socket::sys_socketpair,
    Bind = 200 => socket::sys_bind,
    Listen = 201 => socket::sys_listen,
    Accept = 202 => socket::sys_accept,
    Connect = 203 => socket::sys_connect,
    GetSockname = 204 => socket::sys_getsockname,
    GetPeerName = 205 => socket::sys_getpeername,
    Sendto = 206 => socket::sys_sendto,
    Recvfrom = 207 => socket::sys_recvfrom,
    SetSockopt = 208 => socket::sys_setsockopt,
    GetSockopt = 209 => socket::sys_getsockopt,
    Shutdown = 210 => socket::sys_shutdown,
    Sendmsg = 211 => socket::sys_sendmsg,
    Recvmsg = 212 => socket::sys_recvmsg,
    Statx = 291 => fs::sys_statx,
    RenameAt2 = 276 => fs::sys_renameat2,
    Membarrier = 283 => proc::sys_membarrier,
    FaccessAt2 = 439 => fs::sys_faccessat2,
}
