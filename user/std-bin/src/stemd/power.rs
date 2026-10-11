//! Bounded userspace process shutdown. Kernel workers and all init threads stay
//! alive so process exit can close files through the ordinary I/O paths.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Process {
    pub pid: usize,
    pub tgid: usize,
    pub user: bool,
    pub exited: bool,
}

pub trait ProcessControl {
    fn snapshot(&mut self) -> Result<Vec<Process>, &'static str>;
    fn signal(&mut self, pid: usize, signal: usize);
    fn elapsed_ms(&self) -> u64;
    fn wait(&mut self);
}

/// Give processes two seconds to exit on TERM, then one second after KILL.
/// Refuse the final hardware request if any user task is still alive. New
/// descendants observed during either phase receive that phase's signal too.
pub fn stop_processes(io: &mut impl ProcessControl) -> Result<(), &'static str> {
    let start = io.elapsed_ms();
    let mut term_sent = Vec::new();
    let mut kill_sent = Vec::new();
    loop {
        let processes = io.snapshot()?;
        let mut active = Vec::new();
        for process in processes {
            if process.user
                && !process.exited
                && process.tgid != 1
                && process.pid != 1
                && !active.iter().any(|p: &Process| p.tgid == process.tgid)
            {
                active.push(process);
            }
        }
        if active.is_empty() {
            return Ok(());
        }
        let elapsed = io.elapsed_ms().saturating_sub(start);
        if elapsed >= 3_000 {
            return Err("user processes did not finish exiting; power request cancelled");
        }
        let (signal, sent) = if elapsed < 2_000 {
            (15, &mut term_sent)
        } else {
            (9, &mut kill_sent)
        };
        for process in active {
            if !sent.contains(&process.pid) {
                io.signal(process.pid, signal);
                sent.push(process.pid);
            }
        }
        io.wait();
    }
}

#[cfg(target_os = "scarlet")]
mod native {
    use super::*;
    use scarlet_sys::{Syscall, syscall0, syscall2};
    use std::time::{Duration, Instant};

    // GetTaskInfoList's existing native ABI (also used by ps/top).
    #[derive(Clone, Copy)]
    #[repr(C)]
    struct RawTaskInfo {
        pid: usize,
        ppid: usize,
        state: u8,
        task_type: u8,
        cpu_id: u8,
        _reserved: u8,
        exit_status: i32,
        tgid: usize,
        name: [u8; 64],
        cpu_time_ns: u64,
        sched_util_avg: u32,
        sched_util_min: u32,
        sched_required_capacity: u32,
        core_preference: u8,
        _reserved2: [u8; 3],
        sched_migration_count: u64,
        sched_nice: i32,
        sched_weight: u32,
        sched_vruntime: u64,
        sched_deadline: u64,
    }

    pub struct Native {
        started: Instant,
    }
    impl Native {
        pub fn new() -> Self {
            Self {
                started: Instant::now(),
            }
        }
    }
    impl ProcessControl for Native {
        fn snapshot(&mut self) -> Result<Vec<Process>, &'static str> {
            // SAFETY: Scalar query with no pointers.
            let count = unsafe { syscall0(Syscall::GetTaskInfoCount) };
            let mut capacity = count
                .checked_add(32)
                .filter(|&n| n <= 65_536)
                .ok_or("cannot enumerate processes for shutdown")?;
            for _ in 0..3 {
                // SAFETY: This ABI record contains only integers and byte arrays.
                let mut raw = vec![unsafe { core::mem::zeroed::<RawTaskInfo>() }; capacity];
                // SAFETY: The writable array has capacity records in the kernel's native layout.
                let count = unsafe {
                    syscall2(
                        Syscall::GetTaskInfoList,
                        raw.as_mut_ptr() as usize,
                        raw.len(),
                    )
                };
                if count == usize::MAX || count > raw.len() {
                    return Err("process snapshot failed");
                }
                if count < raw.len() {
                    return Ok(raw[..count]
                        .iter()
                        .map(|p| Process {
                            pid: p.pid,
                            tgid: p.tgid,
                            user: p.task_type == 1,
                            exited: matches!(p.state, 5 | 6),
                        })
                        .collect());
                }
                capacity = capacity
                    .checked_mul(2)
                    .filter(|&n| n <= 65_536)
                    .ok_or("process snapshot keeps growing")?;
            }
            Err("process snapshot keeps growing")
        }
        fn signal(&mut self, pid: usize, signal: usize) {
            // SAFETY: Positive PID from the snapshot, fixed TERM/KILL scalar.
            // A racing exit is harmless; the next snapshot confirms it.
            unsafe {
                syscall2(Syscall::Kill, pid, signal);
            }
        }
        fn elapsed_ms(&self) -> u64 {
            self.started.elapsed().as_millis() as u64
        }
        fn wait(&mut self) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
#[cfg(target_os = "scarlet")]
pub use native::Native;

#[cfg(test)]
mod tests {
    use super::*;
    struct Model {
        now: u64,
        tasks: Vec<Process>,
        signals: Vec<(usize, usize)>,
        ignores_term: bool,
        stuck: bool,
        new_child: bool,
    }
    impl ProcessControl for Model {
        fn snapshot(&mut self) -> Result<Vec<Process>, &'static str> {
            Ok(self.tasks.clone())
        }
        fn signal(&mut self, pid: usize, signal: usize) {
            self.signals.push((pid, signal));
            if !self.stuck && (signal == 9 || !self.ignores_term) {
                let tgid = self.tasks.iter().find(|p| p.pid == pid).unwrap().tgid;
                for p in &mut self.tasks {
                    if p.tgid == tgid {
                        p.exited = true;
                    }
                }
            }
        }
        fn elapsed_ms(&self) -> u64 {
            self.now
        }
        fn wait(&mut self) {
            self.now += 20;
            if self.new_child {
                self.tasks.push(user(40, 40));
                self.new_child = false;
            }
        }
    }
    fn user(pid: usize, tgid: usize) -> Process {
        Process {
            pid,
            tgid,
            user: true,
            exited: false,
        }
    }
    fn model() -> Model {
        Model {
            now: 0,
            tasks: vec![
                user(1, 1),
                user(2, 1),
                user(20, 20),
                user(21, 20),
                Process {
                    pid: 3,
                    tgid: 3,
                    user: false,
                    exited: false,
                },
            ],
            signals: vec![],
            ignores_term: false,
            stuck: false,
            new_child: false,
        }
    }
    #[test]
    fn init_and_kernel_workers_survive_and_user_thread_groups_exit_once() {
        let mut m = model();
        assert_eq!(stop_processes(&mut m), Ok(()));
        assert_eq!(m.signals, [(20, 15)]);
        assert!(
            m.tasks
                .iter()
                .filter(|p| p.tgid == 1 || !p.user)
                .all(|p| !p.exited)
        );
    }
    #[test]
    fn ignoring_term_escalates_after_grace_period() {
        let mut m = model();
        m.ignores_term = true;
        assert_eq!(stop_processes(&mut m), Ok(()));
        assert_eq!(m.signals, [(20, 15), (20, 9)]);
        assert!(m.now >= 2_000 && m.now < 3_000);
    }
    #[test]
    fn late_descendants_are_included() {
        let mut m = model();
        m.new_child = true;
        assert_eq!(stop_processes(&mut m), Ok(()));
        assert_eq!(m.signals, [(20, 15), (40, 15)]);
    }
    #[test]
    fn stuck_process_cancels_hardware_transition() {
        let mut m = model();
        m.stuck = true;
        assert!(stop_processes(&mut m).is_err());
        assert_eq!(m.now, 3_000);
    }
}
