use scarlet::{
    device::{
        char::{CharDevice, TtyControl, tty::TtyDevice},
        manager::DeviceManager,
    },
    interrupt::InterruptManager,
    ipc::event::{EventContent, EventManager, ProcessControlType},
    object::capability::{ReadyInterest, Selectable},
    sched::scheduler,
    sync::waker::Waker,
    task::{self, TaskState},
    timer::{TimerPrecision, get_kernel_timer},
};
use scarlet_aarch64_user_macos_full_bsp::terminal::HostTerminal;
use std::{
    env,
    ffi::CStr,
    fs::File,
    io::{self, Read, Write},
    mem::MaybeUninit,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    },
    path::PathBuf,
    process::{self, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const CPU: u32 = 0;
const TERMINAL_IRQ: u32 = 20;
static READER: AtomicUsize = AtomicUsize::new(0);

#[test]
fn disconnect_wakes_a_blocked_scarlet_tty_reader() {
    const CHILD_TERMINAL: &str = "SCARLET_DISCONNECTED_TERMINAL_TEST";
    if let Some(path) = env::var_os(CHILD_TERMINAL) {
        let _platform = scarlet::hosted::prepare(2, 256 * 1024 * 1024).unwrap();
        let interrupts = InterruptManager::global();
        interrupts.init_controllers();
        interrupts.init_controllers_for_cpu(CPU);
        scarlet::interrupt::enable_cpu_interrupts();
        get_kernel_timer().init(CPU as usize);
        scarlet::network::NetworkManager::init();
        let _terminal =
            HostTerminal::attach(&PathBuf::from(path), CPU, TERMINAL_IRQ, |_| {}).unwrap();
        scarlet::initcall::call_initcalls();

        scheduler::register_online_cpu(CPU as usize);
        for (name, entry) in [
            ("tty-waiting-reader", read_until_disconnect as fn()),
            ("tty-watch-reader", report_blocked_reader as fn()),
        ] {
            let task = task::new_kernel_task(name.into(), 0, entry);
            task.set_pinned_cpu(Some(CPU as usize));
            task.init();
            let id = scheduler::register_task(task);
            if name == "tty-waiting-reader" {
                READER.store(id, Ordering::Release);
            }
            scheduler::enqueue_task(id, CPU as usize);
        }
        scheduler::spawn_idle_task(CPU as usize);
        let first = scheduler::start_scheduler().unwrap();
        scheduler::first_switch_to_kernel_task(first);
    }

    let HostPty {
        master,
        slave: _slave,
        path,
    } = HostPty::new();
    let mut child = Command::new(env::current_exe().unwrap())
        .args([
            "--exact",
            "disconnect_wakes_a_blocked_scarlet_tty_reader",
            "--nocapture",
        ])
        .env(CHILD_TERMINAL, path)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    nonblocking(&stdout);
    let result = (|| {
        let expires = Instant::now() + Duration::from_secs(5);
        let mut progress = Vec::new();
        while !progress
            .windows(b"tty-reader-blocked".len())
            .any(|w| w == b"tty-reader-blocked")
        {
            let mut buffer = [0; 128];
            match stdout.read(&mut buffer) {
                Ok(count) => progress.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
            if child.try_wait()?.is_some() || Instant::now() >= expires {
                return Err(io::Error::other(
                    "Guest reader did not block before disconnect",
                ));
            }
            thread::sleep(Duration::from_millis(1));
        }
        drop(master);
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= expires {
                return Err(io::Error::other(
                    "Guest reader did not resume after disconnect",
                ));
            }
            thread::sleep(Duration::from_millis(1));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    assert!(result.unwrap().success());
}

fn read_until_disconnect() {
    let tty = DeviceManager::get_manager()
        .get_device_by_name("tty0")
        .unwrap();
    assert_eq!(
        tty.as_char_device().unwrap().try_read(&mut [0; 1]).unwrap(),
        0
    );
    assert!(
        tty.as_any()
            .downcast_ref::<TtyDevice>()
            .unwrap()
            .is_hung_up()
    );
    process::exit(0);
}

fn report_blocked_reader() {
    let current = task::mytask().unwrap();
    let reader = scheduler::get_task_by_id(READER.load(Ordering::Acquire)).unwrap();
    loop {
        if matches!(reader.get_state(), TaskState::Blocked(_)) {
            println!("tty-reader-blocked");
            io::stdout().flush().unwrap();
            return;
        }
        current.sleep_with_precision(current.get_trapframe(), 20_000_000, TimerPrecision::Exact);
    }
}

#[test]
fn host_stop_can_exit_without_dispatching_any_guest_interrupt() {
    const CHILD_TERMINAL: &str = "SCARLET_STALLED_TERMINAL_TEST";
    if let Some(path) = env::var_os(CHILD_TERMINAL) {
        let _platform = scarlet::hosted::prepare(2, 64 * 1024 * 1024).unwrap();
        let interrupts = InterruptManager::global();
        interrupts.init_controllers();
        interrupts.init_controllers_for_cpu(CPU);
        let _session = HostTerminal::attach(&PathBuf::from(path), CPU, TERMINAL_IRQ, |restored| {
            process::exit(if restored.is_ok() { 0 } else { 1 });
        })
        .unwrap();
        loop {
            thread::park();
        }
    }

    let mut pty = HostPty::new();
    let before = settings(&pty.slave);
    let mut child = Command::new(env::current_exe().unwrap())
        .args([
            "--exact",
            "host_stop_can_exit_without_dispatching_any_guest_interrupt",
        ])
        .env(CHILD_TERMINAL, &pty.path)
        .spawn()
        .unwrap();
    let result = (|| {
        let expires = Instant::now() + Duration::from_secs(5);
        while settings(&pty.slave).c_lflag & libc::ICANON != 0 {
            if child.try_wait()?.is_some() || Instant::now() >= expires {
                return Err(io::Error::other(
                    "Stalled instance did not open its terminal",
                ));
            }
            thread::sleep(Duration::from_millis(1));
        }
        pty.master.write_all(b"\x1dq")?;
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= expires {
                return Err(io::Error::other(
                    "Stalled instance did not exit after Ctrl-] q",
                ));
            }
            thread::sleep(Duration::from_millis(1));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    assert!(result.unwrap().success());
    assert_settings(&pty.slave, &before);
}

#[test]
fn host_serial_uses_scarlet_tty_and_restores_the_terminal_on_host_stop() {
    let mut pty = HostPty::new();
    let before = settings(&pty.slave);
    let _platform = scarlet::hosted::prepare(2, 64 * 1024 * 1024).unwrap();
    let foreground = register_user_task("tty-foreground");
    let background = register_user_task("tty-background");
    let interrupts = InterruptManager::global();
    interrupts.init_controllers();
    interrupts.init_controllers_for_cpu(CPU);
    scarlet::interrupt::enable_cpu_interrupts();
    scarlet::network::NetworkManager::init();
    // The existing foreground-group routing enumerates scheduler-visible Tasks.
    scheduler::register_online_cpu(CPU as usize);
    for id in [foreground, background] {
        scheduler::enqueue_task(id, CPU as usize);
    }

    let manager = DeviceManager::get_manager();
    assert!(HostTerminal::attach(&pty.path, CPU, u32::MAX, |_| {}).is_err());
    assert_settings(&pty.slave, &before);
    assert!(manager.get_device_by_name("host-serial").is_none());

    let (send, stopped) = mpsc::channel();
    let terminal = HostTerminal::attach(&pty.path, CPU, TERMINAL_IRQ, move |restore| {
        send.send(restore).unwrap();
    })
    .unwrap();
    let raw = settings(&pty.slave);
    assert_eq!(raw.c_lflag & (libc::ICANON | libc::ISIG | libc::ECHO), 0);
    assert_eq!(raw.c_oflag & libc::OPOST, 0);

    scarlet::initcall::call_initcalls();
    let device = manager
        .get_device_by_name("tty0")
        .expect("TTY late initcall");
    let tty = device.as_any().downcast_ref::<TtyDevice>().unwrap();
    tty.set_echo(false);
    tty.set_foreground_task_group_id(foreground);

    pty.master.write_all(b"hello\r").unwrap();
    let foreground_task = scheduler::get_task_by_id(foreground).unwrap();
    pump_serial_until("first input line", || tty.can_read());
    pty.master.write_all(b"\x03").unwrap();
    pump_serial_until("Ctrl-C", || foreground_task.has_pending_process_control());
    assert!(
        !scheduler::get_task_by_id(background)
            .unwrap()
            .has_pending_process_control()
    );
    #[allow(deprecated)]
    let event = EventManager::get_manager()
        .dequeue_event_for_task(foreground as u32)
        .unwrap();
    assert!(matches!(
        event.content,
        EventContent::ProcessControl(ProcessControlType::Interrupt)
    ));
    let mut input = [0; 6];
    assert_eq!(tty.try_read(&mut input).unwrap(), input.len());
    assert_eq!(&input, b"hello\n");
    assert_eq!(tty.input_len(), 0);

    pty.master.write_all(b"q\x1d\x1d\x1dx\r").unwrap();
    pump_serial_until("literal escape and ordinary q", || tty.can_read());
    let mut escaped_line = [0; 5];
    assert_eq!(tty.try_read(&mut escaped_line).unwrap(), escaped_line.len());
    assert_eq!(&escaped_line, b"q\x1d\x1dx\n");

    let mut long_line = vec![b'x'; 700];
    long_line.push(b'\r');
    pty.master.write_all(&long_line).unwrap();
    pump_serial_until("multi-IRQ input line", || {
        tty.input_len() == long_line.len()
    });
    let mut received = vec![0; long_line.len()];
    assert_eq!(tty.try_read(&mut received).unwrap(), received.len());
    *long_line.last_mut().unwrap() = b'\n';
    assert_eq!(received, long_line);

    tty.write(b"ready\n").unwrap();
    assert_eq!(pty.read_exact_until(7), b"ready\r\n");
    pty.master.write_all(b"partial").unwrap();
    pump_serial_until("partial input before hangup", || tty.input_len() == 7);

    // Leave a large host output packet undrained and stop consuming Scarlet
    // interrupts. The escape key must still restore the host terminal.
    let output = vec![b'x'; 256 * 1024];
    let expires = Instant::now() + Duration::from_secs(2);
    while tty.write(&output).is_err() {
        assert!(
            Instant::now() < expires,
            "Could not fill the terminal send queue"
        );
        thread::sleep(Duration::from_millis(1));
    }
    thread::sleep(Duration::from_millis(20));
    assert!(!tty.can_write(), "PTY output must be backpressured");
    let serial = manager.get_device_by_name("host-serial").unwrap();
    let selectable = serial.as_ref() as &dyn Selectable;
    let subscriber = Arc::new(Waker::new_interruptible("terminal-test"));
    let subscription = selectable
        .subscribe_readiness(ReadyInterest::read(), &subscriber)
        .unwrap()
        .pop()
        .unwrap();
    let generation = subscription.generation();
    pty.master.write_all(&[0x1d]).unwrap();
    assert!(matches!(
        stopped.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    pty.master.write_all(b"q").unwrap();
    stopped
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert_settings(&pty.slave, &before);
    terminal.close().unwrap();
    assert!(subscription.generation() > generation);
    assert!(tty.is_hung_up());
    let mut buffered = [0; 7];
    assert_eq!(tty.try_read(&mut buffered).unwrap(), buffered.len());
    assert_eq!(&buffered, b"partial");
    assert_eq!(tty.try_read(&mut [0; 1]).unwrap(), 0);
    assert!(manager.get_device_by_name("host-serial").is_none());
    assert!(
        HostTerminal::attach(&pty.path, CPU, TERMINAL_IRQ, |_| {})
            .is_err_and(|error| error.kind() == io::ErrorKind::AlreadyExists)
    );
}

fn register_user_task(name: &str) -> usize {
    let task = task::new_user_task(name.into(), 0);
    task.init();
    task.enable_events();
    scheduler::register_task(task)
}

fn pump_serial_until(expected: &str, mut ready: impl FnMut() -> bool) {
    let expires = Instant::now() + Duration::from_secs(2);
    while !ready() {
        let handled = InterruptManager::global()
            .claim_and_handle_pending_external_interrupt(CPU)
            .unwrap();
        assert!(Instant::now() < expires, "Timed out waiting for {expected}");
        if handled.is_none() {
            thread::sleep(Duration::from_millis(1));
        }
    }
}

struct HostPty {
    master: File,
    slave: File,
    path: PathBuf,
}

impl HostPty {
    fn new() -> Self {
        let mut master = -1;
        let mut slave = -1;
        // SAFETY: openpty writes two descriptors. The optional name and
        // initial settings pointers are null.
        let opened = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(opened, 0);
        // SAFETY: openpty returned two distinct newly owned descriptors.
        let (master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
        for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
            // SAFETY: F_SETFD updates a live descriptor owned by the test.
            assert_eq!(
                unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
                0
            );
        }
        let mut name = [0; 1024];
        // SAFETY: name is a writable buffer and slave owns the descriptor.
        assert_eq!(
            unsafe { libc::ttyname_r(slave.as_raw_fd(), name.as_mut_ptr(), name.len()) },
            0
        );
        // SAFETY: successful ttyname_r produced a null-terminated name.
        let path = unsafe { CStr::from_ptr(name.as_ptr()) };
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes()));
        nonblocking(&master);
        Self {
            master,
            slave,
            path,
        }
    }

    fn read_exact_until(&mut self, length: usize) -> Vec<u8> {
        let mut bytes = vec![0; length];
        let mut read = 0;
        let expires = Instant::now() + Duration::from_secs(2);
        while read < bytes.len() {
            match self.master.read(&mut bytes[read..]) {
                Ok(0) => panic!("PTY master reached EOF"),
                Ok(count) => read += count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < expires,
                        "Timed out waiting for host output"
                    );
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("{error}"),
            }
        }
        bytes
    }
}

fn nonblocking(file: &impl AsRawFd) {
    let fd = file.as_raw_fd();
    // SAFETY: F_GETFL/F_SETFL operate on a live descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    assert_ne!(flags, -1);
    assert_eq!(
        unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) },
        0
    );
}

fn settings(slave: &File) -> libc::termios {
    let mut value = MaybeUninit::uninit();
    // SAFETY: tcgetattr initializes value on success and the descriptor is live.
    assert_eq!(
        unsafe { libc::tcgetattr(slave.as_raw_fd(), value.as_mut_ptr()) },
        0
    );
    unsafe { value.assume_init() }
}

fn assert_settings(slave: &File, before: &libc::termios) {
    let current = settings(slave);
    assert_eq!(current.c_iflag, before.c_iflag);
    assert_eq!(current.c_oflag, before.c_oflag);
    assert_eq!(current.c_cflag, before.c_cflag);
    // Darwin sets the pending-input state when switching back to canonical mode.
    assert_eq!(
        current.c_lflag & !libc::PENDIN,
        before.c_lflag & !libc::PENDIN
    );
    assert_eq!(current.c_cc, before.c_cc);
    assert_eq!(current.c_ispeed, before.c_ispeed);
    assert_eq!(current.c_ospeed, before.c_ospeed);
}
