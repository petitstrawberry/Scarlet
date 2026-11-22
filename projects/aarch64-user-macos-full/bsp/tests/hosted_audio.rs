use scarlet::{
    device::{
        audio::{
            AUDIO_PCM_FORMAT_S16LE, AUDIO_STATE_RUNNING, AUDIO_STATE_STOPPED, AudioPcmParams,
            AudioPcmStatus, commands::*,
        },
        manager::DeviceManager,
    },
    hosted,
    interrupt::InterruptManager,
    library::std::usercopy::{copy_from_user, copy_to_user},
    sched::scheduler,
    task::{self, Task},
    timer::get_kernel_timer,
};
use scarlet_aarch64_user_macos_full_bsp::audio;
use std::{
    env,
    mem::{offset_of, size_of},
    process::{self, Command},
    thread,
    time::Duration,
};

const ARG: usize = 0x40000;
const PERIOD_FRAMES: usize = 960;
const BUFFER_FRAMES: usize = 9_600;
const FRAME_BYTES: usize = 4;

#[test]
#[ignore = "requires an active macOS default audio output"]
fn default_output_drains_and_refills_scarlet_pcm_through_irq() {
    const CHILD: &str = "SCARLET_HOSTED_AUDIO_TEST_CHILD";
    if env::var_os(CHILD).is_some() {
        run_kernel();
    }
    let status = Command::new(env::current_exe().unwrap())
        .args([
            "--exact",
            "default_output_drains_and_refills_scarlet_pcm_through_irq",
            "--ignored",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .status()
        .unwrap();
    assert!(status.success(), "Audio test child exited with {status}");
}

fn run_kernel() -> ! {
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(15));
        eprintln!("Hosted audio test did not complete");
        process::exit(1);
    });
    let mut platform = hosted::prepare(2, 256 * 1024 * 1024).unwrap();
    let init = task::new_user_task("init".into(), 0);
    init.init();
    assert_eq!(scheduler::register_task(init), 1);
    let interrupts = InterruptManager::global();
    interrupts.init_controllers();
    interrupts.init_controllers_for_cpu(0);
    audio::link_boot_output();
    scarlet::initcall::call_initcalls();
    scarlet::interrupt::enable_cpu_interrupts();
    get_kernel_timer().init(0);

    let test = task::new_kernel_task("hosted-audio-test".into(), 0, test_pcm);
    test.set_pinned_cpu(Some(0));
    test.init();
    test.allocate_data_pages(ARG, 1).unwrap();
    let id = scheduler::register_task(test);
    scheduler::register_online_cpu(0);
    scheduler::spawn_idle_task(0);
    scheduler::enqueue_task(id, 0);
    let first = scheduler::start_scheduler().unwrap();
    platform.start_secondary_cpus().unwrap();
    scheduler::first_switch_to_kernel_task(first);
}

fn test_pcm() {
    let task = task::mytask().unwrap();
    let audio = DeviceManager::get_manager()
        .get_device_by_name("audio0")
        .unwrap()
        .open()
        .unwrap();
    let pcm = audio.as_char_device().unwrap();
    let mut params = [0; size_of::<AudioPcmParams>()];
    params[..4].copy_from_slice(&AUDIO_PCM_FORMAT_S16LE.to_ne_bytes());
    params[4..8].copy_from_slice(&48_000u32.to_ne_bytes());
    params[8..10].copy_from_slice(&2u16.to_ne_bytes());
    params[12..16].copy_from_slice(&(PERIOD_FRAMES as u32).to_ne_bytes());
    params[16..20].copy_from_slice(&(BUFFER_FRAMES as u32).to_ne_bytes());
    copy_to_user(&task, ARG, &params).unwrap();
    pcm.control(AUDIO_SET_PARAMS, ARG).unwrap();

    // Drain the SAS-sized buffer, then refill the running stream. The host
    // callback must deliver completions through the registered virtual IRQ.
    let buffer = vec![0; BUFFER_FRAMES * FRAME_BYTES];
    assert_eq!(pcm.write(&buffer).unwrap(), buffer.len());
    assert_eq!(irq_deliveries(), 0);
    pcm.control(AUDIO_START, 0).unwrap();
    task.sleep(task.get_trapframe(), 600_000_000);
    assert!(irq_deliveries() > 1);
    assert_status(&task, pcm, AUDIO_STATE_RUNNING, BUFFER_FRAMES);

    let one_period = &buffer[..PERIOD_FRAMES * FRAME_BYTES];
    assert_eq!(pcm.write(one_period).unwrap(), one_period.len());
    task.sleep(task.get_trapframe(), 250_000_000);
    let after_refill = BUFFER_FRAMES + PERIOD_FRAMES;
    assert_status(&task, pcm, AUDIO_STATE_RUNNING, after_refill);

    pcm.control(AUDIO_STOP, 0).unwrap();
    assert_status(&task, pcm, AUDIO_STATE_STOPPED, after_refill);
    assert_eq!(pcm.write(one_period).unwrap(), one_period.len());
    pcm.control(AUDIO_START, 0).unwrap();
    task.sleep(task.get_trapframe(), 250_000_000);
    assert_status(
        &task,
        pcm,
        AUDIO_STATE_RUNNING,
        after_refill + PERIOD_FRAMES,
    );
    pcm.control(AUDIO_STOP, 0).unwrap();
    pcm.control(AUDIO_RELEASE, 0).unwrap();
    drop(audio);
    process::exit(0);
}

fn irq_deliveries() -> u64 {
    InterruptManager::global()
        .statistics()
        .into_iter()
        .find(|irq| irq.interrupt_id == audio::COMPLETION_IRQ)
        .unwrap()
        .deliveries
}

fn assert_status(
    task: &Task,
    pcm: &dyn scarlet::device::char::CharDevice,
    expected_state: u32,
    completed_frames: usize,
) {
    pcm.control(AUDIO_GET_STATUS, ARG).unwrap();
    let mut status = [0; size_of::<AudioPcmStatus>()];
    copy_from_user(task, ARG, &mut status).unwrap();
    let state = offset_of!(AudioPcmStatus, state);
    let hardware = offset_of!(AudioPcmStatus, hw_ptr_frames);
    let xruns = offset_of!(AudioPcmStatus, xruns);
    assert_eq!(
        u32::from_ne_bytes(status[state..state + 4].try_into().unwrap()),
        expected_state
    );
    assert_eq!(
        u64::from_ne_bytes(status[hardware..hardware + 8].try_into().unwrap()),
        completed_frames as u64
    );
    assert_eq!(
        u32::from_ne_bytes(status[xruns..xruns + 4].try_into().unwrap()),
        0
    );
}
