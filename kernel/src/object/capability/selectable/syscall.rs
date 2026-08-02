use crate::arch::Trapframe;
use crate::library::std::usercopy::{copy_from_user, copy_to_user};
use crate::object::capability::selectable::{ReadyInterest, Selectable};
use crate::task::mytask;

pub const POLLIN: u16 = 0x0001;
pub const POLLPRI: u16 = 0x0002;
pub const POLLOUT: u16 = 0x0004;
pub const POLLERR: u16 = 0x0008;
pub const POLLHUP: u16 = 0x0010;
pub const POLLNVAL: u16 = 0x0020;

#[repr(C)]
pub struct PollHandle {
    pub handle: u32,
    pub events: u16,
    pub revents: u16,
}

#[repr(C)]
pub struct PollOptions {
    pub timeout_ns: i64,
    pub min_timeout_ns: u64,
}

fn eval_poll_handle(pfd: &mut PollHandle, task: &crate::task::Task) -> (bool, bool) {
    pfd.revents = 0;

    let Some(kobj) = task.handle_table.get(pfd.handle) else {
        pfd.revents |= POLLNVAL;
        return (true, false);
    };

    let want_read = (pfd.events & POLLIN) != 0;
    let want_write = (pfd.events & POLLOUT) != 0;
    let want_except = (pfd.events & POLLPRI) != 0;

    let mut selectable = false;

    if let Some(sel) = kobj.as_selectable() {
        selectable = true;
        let rs = sel.current_ready(ReadyInterest {
            read: want_read,
            write: want_write,
            except: want_except,
        });
        if rs.read && want_read {
            pfd.revents |= POLLIN;
        }
        if rs.write && want_write {
            pfd.revents |= POLLOUT;
        }
        if rs.except && want_except {
            pfd.revents |= POLLPRI;
        }
    } else {
        if want_read {
            pfd.revents |= POLLIN;
        }
        if want_write {
            pfd.revents |= POLLOUT;
        }
    }

    if let Some(pipe) = kobj.as_pipe() {
        if pipe.is_readable() && !pipe.has_writers() {
            pfd.revents |= POLLHUP;
            if want_read && (pfd.revents & POLLIN) == 0 {
                pfd.revents |= POLLIN;
            }
        }
        if pipe.is_writable() && !pipe.has_readers() {
            pfd.revents |= POLLERR | POLLHUP;
        }
    }

    (pfd.revents != 0, selectable)
}

pub fn sys_poll(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let fds_ptr = trapframe.get_arg(0);
    let nfds = trapframe.get_arg(1) as usize;
    let options_ptr = trapframe.get_arg(2);

    trapframe.increment_pc_next(&task);

    let mut options: PollOptions = if options_ptr != 0 {
        let mut opts_bytes = [0u8; core::mem::size_of::<PollOptions>()];
        if copy_from_user(&task, options_ptr, &mut opts_bytes).is_err() {
            return usize::MAX;
        }
        unsafe { core::ptr::read(opts_bytes.as_ptr() as *const PollOptions) }
    } else {
        PollOptions {
            timeout_ns: 0,
            min_timeout_ns: 0,
        }
    };

    if nfds == 0 {
        if options.min_timeout_ns > 0 {
            return usize::MAX;
        }
        return 0;
    }

    let fds_size = nfds * core::mem::size_of::<PollHandle>();
    let mut fds_buf = alloc::vec![0u8; fds_size];
    if copy_from_user(&task, fds_ptr, &mut fds_buf).is_err() {
        return usize::MAX;
    }
    let fds: &mut [PollHandle] =
        unsafe { core::slice::from_raw_parts_mut(fds_buf.as_mut_ptr() as *mut PollHandle, nfds) };

    let timeout_ns: Option<u64> = if options.timeout_ns < 0 {
        None
    } else {
        Some(options.timeout_ns as u64)
    };

    let min_wait_ns = options.min_timeout_ns;
    if min_wait_ns > 0 && timeout_ns.is_some_and(|timeout_ns| min_wait_ns > timeout_ns) {
        return usize::MAX;
    }

    let mut any_ready = false;
    let mut first_selectable_idx: Option<usize> = None;
    let mut selectable_count = 0usize;

    for (idx, pfd) in fds.iter_mut().enumerate() {
        let (ready, selectable) = eval_poll_handle(pfd, &task);
        if ready {
            any_ready = true;
        }
        if selectable {
            selectable_count += 1;
        }
        if first_selectable_idx.is_none() && selectable {
            first_selectable_idx = Some(idx);
        }
    }

    if !any_ready {
        let zero_poll = matches!(timeout_ns, Some(t) if t == 0);
        if !zero_poll {
            if selectable_count > 1 {
                use crate::object::capability::selectable::multi_readiness_recheck_delay;
                use crate::timer::{TimerPrecision, get_time_ns};

                let deadline =
                    timeout_ns.map(|duration_ns| get_time_ns().saturating_add(duration_ns));
                loop {
                    let Some(recheck_delay_ns) =
                        multi_readiness_recheck_delay(deadline, get_time_ns())
                    else {
                        break;
                    };

                    task.sleep_with_precision(trapframe, recheck_delay_ns, TimerPrecision::Exact);

                    any_ready = false;
                    for pfd in fds.iter_mut() {
                        let (ready, _) = eval_poll_handle(pfd, &task);
                        if ready {
                            any_ready = true;
                        }
                    }

                    if any_ready {
                        break;
                    }
                }
            } else if let Some(wait_idx) = first_selectable_idx {
                let pfd = &fds[wait_idx];
                if let Some(kobj) = task.handle_table.get(pfd.handle) {
                    if let Some(sel) = kobj.as_selectable() {
                        let want_read = (pfd.events & POLLIN) != 0;
                        let want_write = (pfd.events & POLLOUT) != 0;
                        let want_except = (pfd.events & POLLPRI) != 0;
                        let _ = sel.wait_until_ready(
                            ReadyInterest {
                                read: want_read,
                                write: want_write,
                                except: want_except,
                            },
                            trapframe,
                            timeout_ns,
                            min_wait_ns,
                        );
                    }
                }

                for pfd in fds.iter_mut() {
                    let _ = eval_poll_handle(pfd, &task);
                }
            }
        }
    }

    let mut count = 0usize;
    for pfd in fds.iter() {
        if pfd.revents != 0 {
            count += 1;
        }
    }
    if copy_to_user(&task, fds_ptr, &fds_buf).is_err() {
        return usize::MAX;
    }
    count
}
