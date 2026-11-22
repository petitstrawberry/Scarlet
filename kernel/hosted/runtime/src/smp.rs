//! Host thread launch for secondary CPUs.

use std::{
    ops::Range,
    sync::mpsc,
    thread::{self, JoinHandle},
};

struct Secondary {
    command: mpsc::Sender<()>,
    status: mpsc::Receiver<Result<(), &'static str>>,
    thread: JoinHandle<()>,
}

/// Bind every thread before preparing any CPU, and prepare every CPU before
/// entering its scheduler. A bind or spawn failure can be retried. Starting
/// preparation commits the boot: the caller may have published CPU and Task
/// state, so losing a host thread from that point is fatal.
pub fn start_secondaries<B: 'static, P: 'static>(
    cpus: Range<usize>,
    bind: fn(usize) -> Result<B, &'static str>,
    prepare: fn(usize) -> Result<P, &'static str>,
    enter: fn(P),
) -> Result<(), &'static str> {
    let mut secondaries = Vec::with_capacity(cpus.len());
    let bound = (|| {
        for cpu in cpus {
            let (release, command) = mpsc::channel();
            let (report, status) = mpsc::channel();
            let thread = thread::Builder::new()
                .name(format!("scarlet-vcpu-{cpu}"))
                .spawn(move || {
                    let binding = bind(cpu);
                    if report
                        .send(binding.as_ref().map(|_| ()).map_err(|&error| error))
                        .is_err()
                    {
                        return;
                    }
                    let Ok(_binding) = binding else {
                        return;
                    };
                    if command.recv().is_err() {
                        return;
                    }
                    let prepared = prepare(cpu);
                    if report
                        .send(prepared.as_ref().map(|_| ()).map_err(|&error| error))
                        .is_err()
                    {
                        return;
                    }
                    if let Ok(prepared) = prepared && command.recv().is_ok() {
                        enter(prepared);
                    }
                })
                .map_err(|_| "Cannot spawn a secondary vCPU host thread")?;
            secondaries.push(Secondary {
                command: release,
                status,
                thread,
            });
        }
        for secondary in &secondaries {
            secondary
                .status
                .recv()
                .map_err(|_| "Secondary vCPU failed to bind")??;
        }
        Ok(())
    })();

    if let Err(error) = bound {
        for Secondary { command, thread, .. } in secondaries {
            drop(command);
            // Join even if this was the thread that failed to bind or panicked.
            let _ = thread.join();
        }
        return Err(error);
    }

    for secondary in &secondaries {
        if secondary.command.send(()).is_err() {
            abort_boot("Secondary vCPU stopped before preparation");
        }
    }
    for secondary in &secondaries {
        match secondary.status.recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => abort_boot(error),
            Err(_) => abort_boot("Secondary vCPU stopped during preparation"),
        }
    }
    for secondary in &secondaries {
        if secondary.command.send(()).is_err() {
            abort_boot("Secondary vCPU stopped before scheduler entry");
        }
    }
    Ok(())
}

fn abort_boot(message: &str) -> ! {
    eprintln!("scarlet: {message}");
    std::process::abort();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        os::unix::process::ExitStatusExt,
        process::{self, Command},
        sync::{
            Barrier, OnceLock,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::Duration,
    };

    #[test]
    fn failed_binding_joins_prepared_threads_before_retry() {
        static FAIL_BIND: AtomicBool = AtomicBool::new(true);
        static BOUND: AtomicUsize = AtomicUsize::new(0);
        static PREPARED: AtomicUsize = AtomicUsize::new(0);
        struct Binding(usize);
        impl Drop for Binding {
            fn drop(&mut self) {
                BOUND.fetch_and(!self.0, Ordering::SeqCst);
            }
        }
        fn bind(cpu: usize) -> Result<Binding, &'static str> {
            if cpu == 2 && FAIL_BIND.load(Ordering::SeqCst) {
                return Err("Injected bind failure");
            }
            let bit = 1 << cpu;
            assert_eq!(BOUND.fetch_or(bit, Ordering::SeqCst) & bit, 0);
            Ok(Binding(bit))
        }
        fn prepare(cpu: usize) -> Result<(), &'static str> {
            PREPARED.fetch_or(1 << cpu, Ordering::SeqCst);
            Ok(())
        }

        assert_eq!(
            start_secondaries(1..4, bind, prepare, |_| {}),
            Err("Injected bind failure")
        );
        assert_eq!(BOUND.load(Ordering::SeqCst), 0);
        assert_eq!(PREPARED.load(Ordering::SeqCst), 0);
        FAIL_BIND.store(false, Ordering::SeqCst);
        start_secondaries(1..4, bind, prepare, |_| {}).unwrap();
        assert_eq!(PREPARED.load(Ordering::SeqCst), 0b1110);
    }

    #[test]
    fn secondary_entry_waits_for_the_last_cpu() {
        static HOLD: Barrier = Barrier::new(2);
        static READY: OnceLock<mpsc::Sender<usize>> = OnceLock::new();
        static ENTERED: OnceLock<mpsc::Sender<usize>> = OnceLock::new();
        fn prepare(cpu: usize) -> Result<usize, &'static str> {
            READY.get().unwrap().send(cpu).unwrap();
            if cpu == 2 {
                HOLD.wait();
            }
            Ok(cpu)
        }
        fn enter(cpu: usize) {
            ENTERED.get().unwrap().send(cpu).unwrap();
        }

        let (ready, readiness) = mpsc::channel();
        READY.set(ready).unwrap();
        let (entered, arrivals) = mpsc::channel();
        ENTERED.set(entered).unwrap();
        let boot = thread::spawn(|| start_secondaries(1..3, |_| Ok(()), prepare, enter));
        let mut prepared: Vec<_> = (0..2)
            .map(|_| readiness.recv_timeout(Duration::from_secs(1)).unwrap())
            .collect();
        prepared.sort_unstable();
        assert_eq!(prepared, [1, 2]);
        let before_release = arrivals.recv_timeout(Duration::from_millis(35));
        HOLD.wait();
        assert_eq!(before_release, Err(mpsc::RecvTimeoutError::Timeout));
        boot.join().unwrap().unwrap();
        let mut entered: Vec<_> = (0..2)
            .map(|_| arrivals.recv_timeout(Duration::from_secs(1)).unwrap())
            .collect();
        entered.sort_unstable();
        assert_eq!(entered, [1, 2]);
    }

    #[test]
    fn secondary_preparation_failure_aborts_boot() {
        const CHILD: &str = "SCARLET_TEST_ABORT_AP_STARTUP";
        if env::var_os(CHILD).is_some() {
            fn prepare(cpu: usize) -> Result<(), &'static str> {
                if cpu == 2 {
                    return Err("Injected secondary CPU preparation failure");
                }
                Ok(())
            }
            let _ = start_secondaries(1..3, |_| Ok(()), prepare, |_| {});
            process::exit(42);
        }
        let child = Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "smp::tests::secondary_preparation_failure_aborts_boot",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert_eq!(child.status.signal(), Some(libc::SIGABRT));
        assert!(
            String::from_utf8_lossy(&child.stderr)
                .contains("Injected secondary CPU preparation failure")
        );
    }
}
