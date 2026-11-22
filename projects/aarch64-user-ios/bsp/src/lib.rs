//! Exercise the hosted iOS scheduler from the probe application's CPU0 thread.

use scarlet::{
    hosted, interrupt::InterruptManager, sched::scheduler, task, timer::get_kernel_timer,
};
use scarlet_abi::Syscall;
use scarlet_hosted_runtime::in_process::{Patch, SignedText};
use std::{
    arch::global_asm,
    ffi::{CString, c_char},
    io,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

global_asm!(
    include_str!("../../tests/scheduler_fixture.S"),
    sleep = const Syscall::Sleep as usize,
    time = const Syscall::MonotonicTime as usize,
    exit = const Syscall::Exit as usize,
);

unsafe extern "C" {
    static scarlet_scheduler_start: u8;
    static scarlet_scheduler_end: u8;
    static scarlet_scheduler_write_tls: u8;
    static scarlet_scheduler_read_tls: u8;
    static scarlet_scheduler_before_sleep: u8;
    static scarlet_scheduler_sleep: u8;
    static scarlet_scheduler_after_sleep: u8;
    static scarlet_scheduler_exit: u8;
}

#[repr(C)]
struct Progress {
    loops: AtomicU64,
    outcome: AtomicU64,
    rendezvous: AtomicU64,
}

static PROGRESS: [Progress; 2] = [const {
    Progress {
        loops: AtomicU64::new(0),
        outcome: AtomicU64::new(0),
        rendezvous: AtomicU64::new(0),
    }
}; 2];

type Log = extern "C" fn(*const c_char);

fn log(callback: Log, message: &str) {
    callback(CString::new(message).unwrap().as_ptr());
}

fn watch(callback: Log) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let outcomes = PROGRESS
            .each_ref()
            .map(|p| p.outcome.load(Ordering::Acquire));
        if outcomes == [1, 1] {
            let loops = PROGRESS.each_ref().map(|p| p.loops.load(Ordering::Acquire));
            log(
                callback,
                &format!("Both CPU-bound Tasks ran on CPU0 (loop counts {loops:?})"),
            );
            log(
                callback,
                "Scarlet Sleep blocked one Task while the peer ran. TLS and registers survived resumption.",
            );
            log(callback, "SCARLET_P1_PASS");
            return;
        }
        if outcomes.iter().any(|&outcome| outcome > 1) || Instant::now() >= deadline {
            let loops = PROGRESS.each_ref().map(|p| p.loops.load(Ordering::Acquire));
            log(
                callback,
                &format!("Guest fixture did not complete (outcomes {outcomes:?}, loops {loops:?})"),
            );
            log(callback, "SCARLET_P1_FAIL");
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn boot(callback: Log) -> io::Result<()> {
    let mut platform =
        hosted::prepare(hosted::DEFAULT_VCPUS, 64 * 1024 * 1024).map_err(io::Error::other)?;
    let interrupts = InterruptManager::global();
    interrupts.init_controllers();
    interrupts.init_controllers_for_cpu(0);
    scarlet::interrupt::enable_cpu_interrupts();
    get_kernel_timer().init(0);
    scheduler::register_boot_cpu(0);
    scheduler::register_online_cpu(0);
    scheduler::spawn_idle_task(0);

    let entry = (&raw const scarlet_scheduler_start).addr() as u64;
    // SAFETY: These immutable, signed routines are linked into the app.
    let text = Arc::new(unsafe {
        SignedText::new(
            entry..(&raw const scarlet_scheduler_end).addr() as u64,
            [
                (
                    (&raw const scarlet_scheduler_write_tls).addr() as u64,
                    Patch::WriteTls(23),
                ),
                (
                    (&raw const scarlet_scheduler_read_tls).addr() as u64,
                    Patch::ReadTls(14),
                ),
                (
                    (&raw const scarlet_scheduler_before_sleep).addr() as u64,
                    Patch::Syscall(0),
                ),
                (
                    (&raw const scarlet_scheduler_sleep).addr() as u64,
                    Patch::Syscall(0),
                ),
                (
                    (&raw const scarlet_scheduler_after_sleep).addr() as u64,
                    Patch::Syscall(0),
                ),
                (
                    (&raw const scarlet_scheduler_exit).addr() as u64,
                    Patch::Syscall(0),
                ),
            ],
        )?
    });

    for (role, progress) in PROGRESS.iter().enumerate() {
        let task = task::new_user_task(format!("ios-fixture-{role}"), 0);
        task.init();
        let stack = Box::leak(Box::new([0u128; 4096]));
        {
            let mut vcpu = task.vcpu.lock();
            vcpu.set_pc(entry);
            vcpu.set_sp(stack.as_ptr_range().end.addr());
            vcpu.iregs.reg[0] = std::ptr::from_ref(progress).addr();
            vcpu.iregs.reg[1] = std::ptr::from_ref(&PROGRESS[1 - role]).addr();
            vcpu.iregs.reg[2] = role;
            vcpu.iregs.reg[3] = 100_000_000;
            vcpu.iregs.reg[4] = 0x731a + role;
        }
        hosted::install_signed_text(&task, text.clone())?;
        let id = scheduler::register_task(task);
        scheduler::enqueue_task(id, 0);
    }
    let first = scheduler::start_scheduler()
        .ok_or_else(|| io::Error::other("CPU0 has no runnable Task"))?;
    platform.start_secondary_cpus().map_err(io::Error::other)?;
    log(
        callback,
        "Scarlet CPU0 online. Running two signed guest Tasks.",
    );
    thread::Builder::new()
        .name("scarlet-ios-report".into())
        .spawn(move || watch(callback))?;
    scheduler::first_switch_to_kernel_task(first)
}

/// Run once from the app's dedicated CPU0 pthread.
#[unsafe(no_mangle)]
pub extern "C" fn scarlet_ios_run(callback: Log) {
    if let Err(error) = boot(callback) {
        log(callback, &format!("Scarlet boot failed: {error}"));
        log(callback, "SCARLET_P1_FAIL");
    }
}
