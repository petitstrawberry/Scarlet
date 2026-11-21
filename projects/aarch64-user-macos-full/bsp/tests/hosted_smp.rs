use scarlet::{
    arch::{self, instruction},
    device::block::{
        BlockDevice,
        request::{BlockIORequest, BlockIORequestType},
    },
    environment::MAX_NUM_CPUS,
    hosted::{self, DEFAULT_VCPUS},
    interrupt::{InterruptManager, controllers::LocalInterruptType},
    sched::scheduler,
    task,
    timer::{TimerPrecision, get_kernel_timer, get_time_ns},
};
use scarlet_aarch64_user_macos_full_bsp::{
    block::RootBlock,
    root_image::{RootImage, SECTOR_SIZE},
};
use std::{
    env, fs,
    process::{self, Command},
    sync::{OnceLock, mpsc},
    thread,
    time::{Duration, Instant},
};

static REPORTS: OnceLock<mpsc::Sender<(usize, u64)>> = OnceLock::new();
static CPU_COUNT: OnceLock<usize> = OnceLock::new();
static ROOT: OnceLock<RootBlock> = OnceLock::new();

#[test]
fn hosted_schedulers_enter_tasks_after_all_cpus_are_ready() {
    const CHILD: &str = "SCARLET_HOSTED_SMP_TEST_CPUS";
    if let Some(count) = env::var_os(CHILD) {
        run_kernel(count.to_str().unwrap().parse().unwrap());
    }

    // Each boot owns its scheduler and vCPU slots for the process lifetime.
    for count in [2, DEFAULT_VCPUS, MAX_NUM_CPUS] {
        let status = Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "hosted_schedulers_enter_tasks_after_all_cpus_are_ready",
                "--nocapture",
            ])
            .env(CHILD, count.to_string())
            .status()
            .unwrap();
        assert!(status.success(), "{count} vCPUs: {status}");
    }
}

fn run_kernel(count: usize) -> ! {
    CPU_COUNT.set(count).unwrap();
    let (tx, rx) = mpsc::channel();
    REPORTS.set(tx).unwrap();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut entered = 0u64;
        let expected = (1u64 << count) - 1;
        while entered != expected {
            let Ok((cpu, online)) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            else {
                eprintln!("Expected {expected:#x}; scheduler Tasks entered on {entered:#x}");
                process::exit(1);
            };
            if cpu >= count || online != expected || entered & (1 << cpu) != 0 {
                eprintln!("Unexpected scheduler Task report: CPU {cpu}, online {online:#x}");
                process::exit(1);
            }
            entered |= 1 << cpu;
        }
        process::exit(0);
    });

    let mut platform = hosted::prepare(count, 1024 * 1024 * 1024).unwrap();
    assert!(platform.start_secondary_cpus().is_err());
    let interrupts = InterruptManager::global();
    interrupts.init_controllers();
    interrupts.init_controllers_for_cpu(0);
    scarlet::interrupt::enable_cpu_interrupts();
    get_kernel_timer().init(0);
    let path = env::temp_dir().join(format!("scarlet-smp-root-{}", process::id()));
    let file = fs::File::create_new(&path).unwrap();
    let length = file.set_len((count * SECTOR_SIZE) as u64);
    let image = RootImage::open(&path);
    fs::remove_file(path).unwrap();
    length.unwrap();
    assert!(ROOT.set(RootBlock::attach(image.unwrap(), 0, 16).unwrap()).is_ok());

    let first = spawn_task(0, bootstrap_task);
    scheduler::register_online_cpu(0);
    scheduler::spawn_idle_task(0);
    assert_eq!(scheduler::start_scheduler(), Some(first));
    platform.start_secondary_cpus().unwrap();
    scheduler::first_switch_to_kernel_task(first);
}

fn spawn_task(cpu: usize, entry: fn()) -> usize {
    let task = task::new_kernel_task(format!("smp-test-{cpu}"), 0, entry);
    task.set_pinned_cpu(Some(cpu));
    task.init();
    let id = scheduler::register_task(task);
    scheduler::enqueue_task(id, cpu);
    id
}

fn bootstrap_task() {
    for cpu in 1..*CPU_COUNT.get().unwrap() {
        spawn_task(cpu, report_current_cpu);
    }
    report_current_cpu();
}

fn report_current_cpu() {
    let cpu = arch::get_cpu().get_cpuid();
    let online = scheduler::online_cpu_mask();
    let data = vec![cpu as u8 + 0x40; SECTOR_SIZE];
    let request = |request_type, buffer| {
        Box::new(BlockIORequest {
            request_type,
            sector: cpu,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer,
        })
    };
    // The scheduler is active, so both requests complete through IRQ ingress
    // and a Waker on the submitting Task's continuation.
    let results = ROOT.get().unwrap().submit_requests(vec![
        request(BlockIORequestType::Write, data.clone()),
        request(BlockIORequestType::Read, vec![0; SECTOR_SIZE]),
    ]);
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|result| result.result.is_ok()));
    assert_eq!(results[1].request.buffer, data);
    let current = task::mytask().unwrap();
    let interrupts = InterruptManager::global();
    let id = cpu as u32;
    for _ in 0..2 {
        interrupts.send_software_interrupt(id).unwrap();
        assert!(interrupts.is_local_interrupt_pending(id, LocalInterruptType::Software));
        let started = get_time_ns();
        let sleep_ns = 20_000_000;
        current.sleep_with_precision(current.get_trapframe(), sleep_ns, TimerPrecision::Exact);
        assert!(get_time_ns() >= started + sleep_ns);
        assert_eq!(arch::get_cpu().get_cpuid(), cpu);
        assert!(!interrupts.is_local_interrupt_pending(id, LocalInterruptType::Software));
    }

    REPORTS
        .get()
        .unwrap()
        .send((cpu, online))
        .unwrap();
    loop {
        instruction::idle();
    }
}
