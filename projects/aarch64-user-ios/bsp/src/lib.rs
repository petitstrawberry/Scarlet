//! Exercise the hosted iOS scheduler from the probe application's CPU0 thread.

use scarlet::{
    hosted, interrupt::InterruptManager, sched::scheduler, task, timer::get_kernel_timer,
};
use scarlet_abi::Syscall;
use scarlet_hosted_runtime::signed_image::Image;
use std::{
    ffi::{CStr, CString, OsStr, c_char},
    io,
    os::unix::ffi::OsStrExt,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

const _: () = {
    assert!(Syscall::Sleep as usize == 20);
    assert!(Syscall::MonotonicTime as usize == 35);
    assert!(Syscall::Exit as usize == 1);
};

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
            log(callback, "SCARLET_AOT_PASS");
            return;
        }
        if outcomes.iter().any(|&outcome| outcome > 1) || Instant::now() >= deadline {
            let loops = PROGRESS.each_ref().map(|p| p.loops.load(Ordering::Acquire));
            log(
                callback,
                &format!("Guest fixture did not complete (outcomes {outcomes:?}, loops {loops:?})"),
            );
            log(callback, "SCARLET_P1_FAIL");
            log(callback, "SCARLET_AOT_FAIL");
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn boot(path: &Path, callback: Log) -> io::Result<()> {
    // SAFETY: The UIKit entry supplies the immutable framework in this app bundle.
    let image = unsafe { Image::open(path)? };
    log(
        callback,
        &format!("Loaded signed ELF image {}", image.elf_sha256()),
    );
    let retired = image.instantiate()?;
    let reused_entry = retired.entry();
    let retired_tls = retired.tls();
    let marker =
        std::ptr::with_exposed_provenance_mut::<scarlet_abi::tls::NativeTlsHeader>(retired_tls);
    // No guest owns this instance yet.
    unsafe { (*marker).errno = 123 };
    drop(retired);
    let first = image.instantiate()?;
    let second = image.instantiate()?;
    let errno = unsafe {
        (*std::ptr::with_exposed_provenance::<scarlet_abi::tls::NativeTlsHeader>(first.tls())).errno
    };
    if first.entry() != reused_entry
        || first.tls() != retired_tls
        || errno != 0
        || first.entry() == second.entry()
        || first.tls() == second.tls()
    {
        return Err(io::Error::other(
            "Signed image instances did not reset or use distinct VA",
        ));
    }
    log(
        callback,
        &format!(
            "Signed RX aliases at {:#x} and {:#x}",
            first.entry(),
            second.entry()
        ),
    );
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

    for (role, (progress, instance)) in PROGRESS.iter().zip([first, second]).enumerate() {
        let task = task::new_user_task(format!("ios-fixture-{role}"), 0);
        task.init();
        {
            let mut vcpu = task.vcpu.lock();
            vcpu.set_pc(instance.entry() as u64);
            vcpu.set_sp(instance.stack_top());
            vcpu.set_tls_pointer(instance.tls());
            vcpu.iregs.reg[0] = std::ptr::from_ref(progress).addr();
            vcpu.iregs.reg[1] = std::ptr::from_ref(&PROGRESS[1 - role]).addr();
            vcpu.iregs.reg[2] = role;
            vcpu.iregs.reg[3] = 100_000_000;
            vcpu.iregs.reg[4] = 0x731a + role;
        }
        hosted::install_signed_text(&task, instance.text())?;
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
///
/// # Safety
/// `path` is a NUL-terminated path to the app-bundled signed guest framework.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scarlet_ios_run(path: *const c_char, callback: Log) {
    let path = Path::new(OsStr::from_bytes(
        unsafe { CStr::from_ptr(path) }.to_bytes(),
    ));
    if let Err(error) = boot(path, callback) {
        log(callback, &format!("Scarlet boot failed: {error}"));
        log(callback, "SCARLET_AOT_FAIL");
    }
}
