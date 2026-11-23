//! Hosted Scarlet on the iOS application's CPU0 thread.

#[allow(dead_code)]
#[path = "../../../shared/hosted/block.rs"]
mod block;
mod boot;
mod buttons;
mod display;
mod metal;
#[allow(dead_code)]
#[path = "../../../shared/hosted/root_image.rs"]
mod root_image;
#[allow(dead_code, unused_imports)]
#[path = "../../../shared/hosted/sgfx/mod.rs"]
mod sgfx;
#[allow(dead_code)]
#[path = "../../../shared/hosted/terminal.rs"]
mod terminal;
mod touch;

use scarlet::{
    abi::{AbiRegistry, scarlet::ScarletAbi},
    arch::Trapframe,
    executor::{TransparentExecutor, environment::Environment},
    fs::{FileType, VfsManager},
    hosted,
    interrupt::InterruptManager,
    object::KernelObject,
    sched::scheduler,
    task,
    timer::get_kernel_timer,
};
use scarlet_abi::Syscall;
use scarlet_hosted_runtime::signed_image::Image;
use std::{
    ffi::{CStr, CString, OsStr, c_char},
    io,
    os::unix::ffi::OsStrExt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const _: () = {
    assert!(Syscall::Sleep as usize == 20);
    assert!(Syscall::MonotonicTime as usize == 35);
    assert!(Syscall::Exit as usize == 1);
    assert!(Syscall::ExecveABI as usize == 4);
    assert!(Syscall::GetKernelInfo as usize == 51);
    assert!(Syscall::Clone as usize == 2);
    assert!(Syscall::Waitpid as usize == 5);
    assert!(Syscall::Getpid as usize == 7);
    assert!(Syscall::Getppid as usize == 8);
    assert!(Syscall::Sbrk as usize == 13);
    assert!(Syscall::GetSessionId as usize == 27);
    assert!(Syscall::GetProcessGroupId as usize == 28);
    assert!(Syscall::MemoryMap as usize == 700);
    assert!(Syscall::MemoryUnmap as usize == 701);
    assert!(Syscall::MemoryProtect as usize == 702);
    assert!(Syscall::EnvironmentCurrent as usize == 1304);
    assert!(Syscall::EnvironmentSpawn as usize == 1306);
    assert!(size_of::<scarlet_abi::RawEnvironmentExec>() == 48);
    assert!(size_of::<scarlet_abi::RawEnvironmentSpawn>() == 56);
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
            log(
                callback,
                "Signed ELF images used the exec stack, rejected an unsigned executable and kept running.",
            );
            log(
                callback,
                "EnvironmentSpawn applied process group and session placement before entry, and Waitpid returned each child's exit status.",
            );
            log(
                callback,
                "Signed guest heaps grew, released their pages and returned zero-filled pages on regrowth.",
            );
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

fn add_executable(vfs: &VfsManager, path: &str, bytes: &[u8]) -> io::Result<KernelObject> {
    let describe = |error: scarlet::fs::FileSystemError| io::Error::other(error.message);
    vfs.create_file_with_mode(path, FileType::RegularFile, 0o755)
        .map_err(describe)?;
    let writable = vfs.open(path, 2).map_err(describe)?;
    let file = writable
        .as_file()
        .ok_or_else(|| io::Error::other("Expected writable ELF file"))?;
    let mut offset = 0;
    while offset < bytes.len() {
        let count = file
            .write_at(offset as u64, &bytes[offset..])
            .map_err(|error| io::Error::other(format!("Cannot write guest ELF: {error:?}")))?;
        if count == 0 || count > bytes.len() - offset {
            return Err(io::Error::other("Short write of guest ELF"));
        }
        offset += count;
    }
    vfs.open(path, 0).map_err(describe)
}

fn boot(path: &Path, elf: &Path, callback: Log) -> io::Result<()> {
    // SAFETY: The UIKit entry supplies the immutable framework in this app bundle.
    let image = Arc::new(unsafe { Image::open(path)? });
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
    hosted::register_signed_image(image).map_err(io::Error::other)?;
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

    AbiRegistry::register::<ScarletAbi>();
    let vfs = VfsManager::new();
    let mut bytes = std::fs::read(elf)?;
    let executable = add_executable(&vfs, "/scheduler", &bytes)?;
    bytes.push(1);
    add_executable(&vfs, "/unsigned", &bytes)?;
    let environment = Environment::new();
    environment
        .set_root("scarlet".into(), vfs.view())
        .map_err(io::Error::other)?;
    environment.seal().map_err(io::Error::other)?;

    let mut entries = [0; 2];
    for (role, progress) in PROGRESS.iter().enumerate() {
        let task = task::new_user_task(format!("ios-fixture-{role}"), 0);
        task.init();
        let prepare = |argv: &[&str], frame: &mut Trapframe| {
            TransparentExecutor::execute_in_environment_with_abi(
                environment.clone(),
                &executable,
                argv,
                &[],
                "/",
                &[],
                &task,
                &task,
                frame,
                Some("scarlet"),
            )
        };
        let mut frame = Trapframe::new();
        prepare(&["/scheduler"], &mut frame)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let (entry, tls, old_asid) = (frame.elr, frame.tpidr_el0, task.vm_manager.get_asid());
        let failed = prepare(&["/scheduler", "invalid\0arg"], &mut frame);
        if !failed.is_err_and(|error| {
            error
                .to_string()
                .contains("Invalid or oversized process arguments")
        }) || frame.elr != entry
            || frame.tpidr_el0 != tls
            || task.vm_manager.get_asid() != old_asid
        {
            return Err(io::Error::other("Failed exec changed the previous image"));
        }
        if role == 0 {
            let errno = unsafe {
                (*std::ptr::with_exposed_provenance::<scarlet_abi::tls::NativeTlsHeader>(
                    tls as usize,
                ))
                .errno
            };
            if entry as usize != reused_entry || tls as usize != retired_tls || errno != 0 {
                return Err(io::Error::other(
                    "Signed image did not reset its reused slot",
                ));
            }
        }
        let file_handle = task
            .handle_table
            .insert(executable.clone())
            .map_err(io::Error::other)?;
        {
            let mut vcpu = task.vcpu.lock();
            vcpu.iregs.reg[3] = file_handle as usize;
            vcpu.iregs.reg[4] = 0x731a + role;
            vcpu.iregs.reg[5] = std::ptr::from_ref(progress).addr();
            vcpu.iregs.reg[6] = std::ptr::from_ref(&PROGRESS[1 - role]).addr();
            vcpu.iregs.reg[7] = role;
            vcpu.iregs.reg[9] = 100_000_000;
        }
        entries[role] = entry;
        let id = scheduler::register_task(task);
        scheduler::enqueue_task(id, 0);
    }
    if entries[0] == entries[1] {
        return Err(io::Error::other("Signed guest instances share an RX alias"));
    }
    log(
        callback,
        &format!(
            "Signed RX aliases at {:#x} and {:#x}",
            entries[0], entries[1]
        ),
    );
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
/// Both pointers are NUL-terminated paths inside the app bundle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scarlet_ios_run(path: *const c_char, elf: *const c_char, callback: Log) {
    let path = Path::new(OsStr::from_bytes(
        unsafe { CStr::from_ptr(path) }.to_bytes(),
    ));
    let elf = Path::new(OsStr::from_bytes(unsafe { CStr::from_ptr(elf) }.to_bytes()));
    if let Err(error) = boot(path, elf, callback) {
        log(callback, &format!("Scarlet boot failed: {error}"));
        log(callback, "SCARLET_AOT_FAIL");
    }
}
