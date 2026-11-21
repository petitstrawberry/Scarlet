//! Byte-transparent host serial transport for the existing Scarlet TTY.

use scarlet::{
    arch::Trapframe,
    device::{
        Device, DeviceCapability, DeviceType,
        char::CharDevice,
        events::{
            DeviceEvent, DeviceEventEmitter, DeviceEventListener, ErrorEvent, EventCapableDevice,
            HangupEvent, InputEvent,
        },
        manager::DeviceManager,
    },
    interrupt::{
        CpuId, InterruptClaim, InterruptId, InterruptManager, InterruptResult, InterruptSource,
    },
    object::capability::{
        ControlOps, MemoryMappingInfo, MemoryMappingOps, ReadyInterest, ReadySet,
        SelectWaitOutcome, Selectable, StreamError,
    },
    sync::{
        IrqSpinLock, ReadinessRegistration,
        waker::{WaitResult, Waker},
    },
    task::mytask,
    timer::{TimerPrecision, get_time_ns},
};
use std::{
    any::Any,
    collections::VecDeque,
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    marker::PhantomData,
    mem::MaybeUninit,
    os::{
        fd::AsRawFd,
        unix::{fs::OpenOptionsExt, net::UnixStream},
    },
    path::Path,
    sync::{
        Arc, Mutex, TryLockError, Weak,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
};

const RX_IRQ_BATCH: usize = 256;
const RX_CAPACITY: usize = 64 * 1024;
const TX_CAPACITY: usize = 256 * 1024;
const ESCAPE_BYTE: u8 = 0x1d; // Ctrl-]

/// Owns the host terminal session on the bootstrap thread. The host I/O
/// thread restores terminal settings before invoking the host stop callback.
pub struct HostTerminal {
    serial: Arc<SerialDevice>,
    source: Arc<dyn InterruptSource>,
    registration: usize,
    worker: Option<JoinHandle<io::Result<()>>>,
    restored: Arc<(Mutex<bool>, std::sync::Condvar)>,
    _bootstrap_thread: PhantomData<*mut ()>,
}

impl HostTerminal {
    /// Use the terminal device that owns this session. On Darwin the
    /// /dev/tty forwarding device does not implement the PTY's poll interface.
    #[cfg(target_os = "macos")]
    pub fn attach_console(
        cpu: CpuId,
        irq: InterruptId,
        stop: impl FnOnce(io::Result<()>) + Send + 'static,
    ) -> io::Result<Self> {
        use std::os::unix::ffi::OsStrExt;
        let mut name = [0i8; libc::PATH_MAX as usize];
        for fd in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
            if unsafe { libc::ttyname_r(fd, name.as_mut_ptr(), name.len()) } == 0 {
                let path = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) };
                return Self::attach(
                    Path::new(std::ffi::OsStr::from_bytes(path.to_bytes())),
                    cpu,
                    irq,
                    stop,
                );
            }
        }
        // All standard descriptors may have been redirected by the runner.
        // Resolve the controlling device without opening the forwarding vnode.
        let mut info = MaybeUninit::<libc::proc_bsdinfo>::uninit();
        let bytes = size_of::<libc::proc_bsdinfo>() as i32;
        let read = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                bytes,
            )
        };
        if read != bytes {
            return Err(io::Error::other("Cannot resolve controlling terminal"));
        }
        let device = unsafe { info.assume_init() }.e_tdev;
        unsafe extern "C" {
            fn devname_r(
                dev: libc::dev_t,
                kind: libc::mode_t,
                buffer: *mut libc::c_char,
                len: libc::c_int,
            ) -> *mut libc::c_char;
        }
        if device == u32::MAX
            || unsafe {
                devname_r(
                    device as libc::dev_t,
                    libc::S_IFCHR,
                    name.as_mut_ptr(),
                    name.len() as i32,
                )
            }
            .is_null()
        {
            return Self::attach_transport(cpu, irq, RawTerminal::detached, stop);
        }
        let basename = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) };
        let path = Path::new("/dev").join(std::ffi::OsStr::from_bytes(basename.to_bytes()));
        Self::attach(&path, cpu, irq, stop)
    }

    /// Use Linux's pollable controlling terminal.
    #[cfg(target_os = "linux")]
    pub fn attach_console(
        cpu: CpuId,
        irq: InterruptId,
        stop: impl FnOnce(io::Result<()>) + Send + 'static,
    ) -> io::Result<Self> {
        Self::attach(Path::new("/dev/tty"), cpu, irq, stop)
    }

    /// Register a serial device before the TTY late initcall. Ctrl-C remains
    /// a guest byte. Ctrl-] followed by q requests host shutdown even when the guest is
    /// stalled. The callback runs on the host thread after terminal restore
    /// and can stop the instance without entering Scarlet.
    pub fn attach(
        path: &Path,
        cpu: CpuId,
        irq: InterruptId,
        host_stop: impl FnOnce(io::Result<()>) + Send + 'static,
    ) -> io::Result<Self> {
        Self::attach_transport(cpu, irq, || RawTerminal::open(path), host_stop)
    }

    fn attach_transport(
        cpu: CpuId,
        irq: InterruptId,
        open: impl FnOnce() -> io::Result<RawTerminal>,
        host_stop: impl FnOnce(io::Result<()>) + Send + 'static,
    ) -> io::Result<Self> {
        if scarlet::arch::try_get_cpuid() != Some(cpu as usize) {
            return Err(io::Error::other(
                "Terminal IRQ must belong to the attaching vCPU",
            ));
        }
        let manager = DeviceManager::get_manager();
        if manager.get_device_by_name(SerialDevice::NAME).is_some()
            || manager.get_device_by_name("tty0").is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "The instance already initialized its console",
            ));
        }
        let terminal = open()?;
        let (notify, wake) = UnixStream::pair()?;
        notify.set_nonblocking(true)?;
        wake.set_nonblocking(true)?;
        // The terminal reader cannot block on a vCPU holding the ingress lock.
        // Coalesce IRQ requests and let this relay perform the ingress call.
        let (raise_irq, pending_irq) = mpsc::sync_channel(1);
        let relay_done = Arc::new(AtomicBool::new(false));
        let relay_exit = relay_done.clone();
        thread::Builder::new()
            .name("scarlet-terminal-irq".into())
            .spawn(move || {
                while pending_irq.recv().is_ok() {
                    let last = relay_exit.load(Ordering::Acquire);
                    if scarlet::hosted::raise_external_interrupt(cpu, irq).is_err() || last {
                        break;
                    }
                }
            })?;
        let port = Arc::new(Port {
            buffers: Mutex::new(Buffers::default()),
            closed: AtomicBool::new(false),
            notify,
            raise_irq,
            relay_done,
            cpu,
            irq,
        });
        let serial = Arc::new(SerialDevice {
            port: port.clone(),
            raw_input: IrqSpinLock::new(VecDeque::new()),
            events: DeviceEventEmitter::new(),
            readiness: Arc::new(Waker::new_interruptible("host-serial")),
            hangup_published: AtomicBool::new(false),
        });
        let (start, wait) = mpsc::channel();
        let worker_port = port.clone();
        let restored = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let worker_restored = restored.clone();
        let worker = thread::Builder::new()
            .name("scarlet-terminal".into())
            .spawn(move || {
                let mut terminal = terminal;
                let result = if wait.recv().is_ok() {
                    run_terminal(&mut terminal.file, wake, &worker_port)
                } else {
                    Ok(false)
                };
                let restored = terminal.restore();
                drop(terminal);
                *worker_restored.0.lock().unwrap() = true;
                worker_restored.1.notify_all();
                worker_port.closed.store(true, Ordering::Release);
                match result {
                    Ok(true) => {
                        host_stop(restored);
                        worker_port.finish_irq_relay();
                        Ok(())
                    }
                    other => {
                        worker_port.finish_irq_relay();
                        other.and(restored)
                    }
                }
            })?;

        let interrupts = InterruptManager::global();
        let source: Arc<dyn InterruptSource> = Arc::new(SerialIrq(Arc::downgrade(&serial)));
        let registered = interrupts
            .register_interrupt_source(irq, source.clone())
            .and_then(|()| interrupts.enable_external_interrupt(irq, cpu));
        if let Err(error) = registered {
            drop(start);
            let _ = worker.join();
            interrupts.unregister_interrupt_source(irq, &source);
            return Err(io::Error::other(format!(
                "Cannot register terminal IRQ: {error}"
            )));
        }
        let id = manager.register_device_with_name(serial.name().into(), serial.clone());
        let session = Self {
            serial,
            source,
            registration: id,
            worker: Some(worker),
            restored,
            _bootstrap_thread: PhantomData,
        };
        start
            .send(())
            .map_err(|_| io::Error::other("Terminal thread stopped at startup"))?;
        Ok(session)
    }

    /// Window shutdown waits for raw-mode restoration on the serial I/O thread.
    pub fn shutdown_handle(&self) -> Box<dyn FnOnce() + Send> {
        let port = self.serial.port.clone();
        let restored = self.restored.clone();
        Box::new(move || {
            port.closed.store(true, Ordering::Release);
            port.wake();
            drop(
                restored
                    .1
                    .wait_while(restored.0.lock().unwrap(), |done| !*done)
                    .unwrap(),
            );
        })
    }

    /// Stop host I/O and wait for the original terminal settings to be restored.
    pub fn close(mut self) -> io::Result<()> {
        self.stop()
    }

    fn stop(&mut self) -> io::Result<()> {
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        let port = &self.serial.port;
        port.closed.store(true, Ordering::Release);
        port.wake();
        let result = worker
            .join()
            .map_err(|_| io::Error::other("Terminal I/O thread panicked"));
        // This is the bound BSP, so publish hangup even if the relay's pending
        // IRQ has not been handled before the route is removed.
        self.serial.publish_hangup();
        let interrupts = InterruptManager::global();
        let _ = interrupts.disable_external_interrupt(port.irq, port.cpu);
        interrupts.unregister_interrupt_source(port.irq, &self.source);
        DeviceManager::get_manager().unregister_device(self.registration);
        result?
    }
}

impl Drop for HostTerminal {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

struct RawTerminal {
    file: File,
    saved: Option<libc::termios>,
}

impl RawTerminal {
    // A GUI launch may have no controlling tty. Keep the normal guest serial
    // device alive and forward its output to the launcher's stdout.
    #[cfg(target_os = "macos")]
    fn detached() -> io::Result<Self> {
        let (port, mut reader) = UnixStream::pair()?;
        port.set_nonblocking(true)?;
        thread::Builder::new()
            .name("scarlet-console-output".into())
            .spawn(move || {
                let mut buf = [0; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => return,
                        Ok(n) => {
                            let _ = io::stdout().write_all(&buf[..n]);
                        }
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => return,
                    }
                }
            })?;
        Ok(Self {
            file: File::from(std::os::fd::OwnedFd::from(port)),
            saved: None,
        })
    }

    fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
            .open(path)?;
        let mut saved = MaybeUninit::uninit();
        // SAFETY: The descriptor lives in file and saved has storage for termios.
        if unsafe { libc::tcgetattr(file.as_raw_fd(), saved.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: Successful tcgetattr initialized the value.
        let saved = unsafe { saved.assume_init() };
        let terminal = Self {
            file,
            saved: Some(saved),
        };
        let mut raw = saved;
        // SAFETY: raw is a fully initialized termios value.
        unsafe { libc::cfmakeraw(&mut raw) };
        terminal.apply(&raw)?;
        Ok(terminal)
    }

    fn apply(&self, settings: &libc::termios) -> io::Result<()> {
        loop {
            // SAFETY: The descriptor and settings stay valid for the call.
            if unsafe { libc::tcsetattr(self.file.as_raw_fd(), libc::TCSANOW, settings) } == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    fn restore(&mut self) -> io::Result<()> {
        if let Some(settings) = self.saved {
            self.apply(&settings)?;
            self.saved = None;
        }
        Ok(())
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[derive(Default)]
struct Buffers {
    input: VecDeque<u8>,
    receive_overrun: bool,
    output: VecDeque<Vec<u8>>,
    // Includes the packet currently being written by the host thread.
    output_bytes: usize,
    readiness_changed: bool,
}

struct Port {
    buffers: Mutex<Buffers>,
    closed: AtomicBool,
    notify: UnixStream,
    raise_irq: mpsc::SyncSender<()>,
    relay_done: Arc<AtomicBool>,
    cpu: CpuId,
    irq: InterruptId,
}

impl Port {
    fn wake(&self) {
        let mut notify = &self.notify;
        while let Err(error) = notify.write(&[1]) {
            if error.kind() != io::ErrorKind::Interrupted {
                // A full socket already has a pending wake, and a closed
                // socket has no worker left to notify.
                break;
            }
        }
    }

    fn signal(&self) {
        // A full channel already represents a pending notification.
        let _ = self.raise_irq.try_send(());
    }

    fn finish_irq_relay(&self) {
        self.relay_done.store(true, Ordering::Release);
        self.signal();
    }

    fn send(&self, bytes: &[u8]) -> Result<usize, &'static str> {
        if bytes.is_empty() {
            return Ok(0);
        }
        {
            let mut buffers = self.buffers.lock().unwrap();
            if self.closed.load(Ordering::Acquire) {
                return Err("Host terminal closed");
            }
            if bytes.len() > TX_CAPACITY - buffers.output_bytes {
                return Err("Host terminal transmit queue is full");
            }
            buffers.output.push_back(bytes.to_vec());
            buffers.output_bytes += bytes.len();
        }
        self.wake();
        Ok(bytes.len())
    }
}

/// Returns true only for an explicit host stop sequence.
fn run_terminal(file: &mut File, mut wake: UnixStream, port: &Port) -> io::Result<bool> {
    let mut output: Option<(Vec<u8>, usize)> = None;
    let mut received = VecDeque::<u8>::new();
    let mut receive_overrun = false;
    let mut escaped = false;
    let mut finished_bytes = 0;
    let mut bytes = [0; 4096];

    loop {
        if port.closed.load(Ordering::Acquire) {
            return Ok(false);
        }
        let waiting_for_queue = match port.buffers.try_lock() {
            Ok(mut buffers) => {
                let new_input = !received.is_empty() || receive_overrun;
                let count = received.len().min(RX_CAPACITY - buffers.input.len());
                buffers.receive_overrun |= receive_overrun || count < received.len();
                buffers.input.extend(received.drain(..count));
                received.clear();
                receive_overrun = false;
                let became_writable = finished_bytes != 0 && buffers.output_bytes == TX_CAPACITY;
                buffers.output_bytes -= std::mem::take(&mut finished_bytes);
                buffers.readiness_changed |= became_writable;
                if output.is_none() {
                    output = buffers.output.pop_front().map(|packet| (packet, 0));
                }
                drop(buffers);
                if new_input || became_writable {
                    port.signal();
                }
                false
            }
            Err(TryLockError::WouldBlock) => true,
            Err(TryLockError::Poisoned(_)) => {
                return Err(io::Error::other("Terminal queue poisoned"));
            }
        };
        let mut fds = [
            libc::pollfd {
                fd: wake.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: file.as_raw_fd(),
                events: libc::POLLIN | if output.is_some() { libc::POLLOUT } else { 0 },
                revents: 0,
            },
        ];
        // SAFETY: fds contains both pollfd entries and both file owners are alive.
        // Retry a contended queue without blocking host control on that queue.
        let timeout_ms = if waiting_for_queue { 5 } else { -1 };
        if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) } < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if fds[0].revents != 0 {
            let _ = wake.read(&mut bytes);
            if port.closed.load(Ordering::Acquire) {
                return Ok(false);
            }
        }
        if fds[1].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
            match file.read(&mut bytes) {
                Ok(0) => return Ok(false),
                Ok(count) => {
                    let mut enqueue = |byte| {
                        if received.len() < RX_CAPACITY {
                            received.push_back(byte);
                        } else {
                            receive_overrun = true;
                        }
                    };
                    for &byte in &bytes[..count] {
                        if escaped {
                            escaped = false;
                            if byte == b'q' {
                                return Ok(true);
                            }
                            if byte != ESCAPE_BYTE {
                                enqueue(ESCAPE_BYTE);
                            }
                        } else if byte == ESCAPE_BYTE {
                            escaped = true;
                            continue;
                        }
                        enqueue(byte);
                    }
                }
                Err(error) if transient(&error) => {}
                Err(error) => return Err(error),
            }
        }
        if fds[1].revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
            return Err(io::Error::other("Host terminal became unavailable"));
        }
        if fds[1].revents & libc::POLLOUT != 0
            && let Some((packet, offset)) = &mut output
        {
            match file.write(&packet[*offset..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => {
                    *offset += count;
                    if *offset == packet.len() {
                        finished_bytes += packet.len();
                        output = None;
                    }
                }
                Err(error) if transient(&error) => {}
                Err(error) => return Err(error),
            }
        }
    }
}

fn transient(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
    )
}

struct SerialDevice {
    port: Arc<Port>,
    raw_input: IrqSpinLock<VecDeque<u8>>,
    events: DeviceEventEmitter,
    readiness: Arc<Waker>,
    hangup_published: AtomicBool,
}

impl SerialDevice {
    const NAME: &'static str = "host-serial";

    fn publish_hangup(&self) -> bool {
        if self.hangup_published.swap(true, Ordering::AcqRel) {
            return false;
        }
        self.events.emit(&HangupEvent);
        self.readiness.wake_all();
        true
    }
}

struct SerialIrq(Weak<SerialDevice>);

impl InterruptSource for SerialIrq {
    fn interrupt_id(&self) -> Option<InterruptId> {
        self.0.upgrade().map(|serial| serial.port.irq)
    }

    fn claim_interrupt(&self) -> InterruptResult<InterruptClaim> {
        let Some(serial) = self.0.upgrade() else {
            return Ok(InterruptClaim::NotMine);
        };
        let mut packet = [0; RX_IRQ_BATCH];
        let (count, more, overrun, readiness_changed) = {
            let mut buffers = serial.port.buffers.lock().unwrap();
            let count = packet.len().min(buffers.input.len());
            for (destination, source) in
                packet[..count].iter_mut().zip(buffers.input.drain(..count))
            {
                *destination = source;
            }
            (
                count,
                !buffers.input.is_empty(),
                std::mem::take(&mut buffers.receive_overrun),
                std::mem::take(&mut buffers.readiness_changed),
            )
        };
        if more {
            scarlet::hosted::raise_external_interrupt(serial.port.cpu, serial.port.irq)?;
        }
        if overrun {
            serial.emit_event(&ErrorEvent {
                error_code: libc::ENOBUFS as u32,
            });
        }
        if count != 0 {
            {
                let mut raw = serial.raw_input.lock();
                let count = count.min(RX_CAPACITY - raw.len());
                raw.extend(&packet[..count]);
            }
            for &byte in &packet[..count] {
                serial.emit_event(&InputEvent { data: byte });
            }
        }
        if count != 0 || readiness_changed {
            serial.readiness.wake_all();
        }
        let hung_up =
            !more && serial.port.closed.load(Ordering::Acquire) && serial.publish_hangup();
        if count != 0 || overrun || readiness_changed || hung_up {
            Ok(InterruptClaim::Handled)
        } else {
            Ok(InterruptClaim::NotMine)
        }
    }
}

impl Device for SerialDevice {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn capabilities(&self) -> &'static [DeviceCapability] {
        &[DeviceCapability::Serial]
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_char_device(&self) -> Option<&dyn CharDevice> {
        Some(self)
    }

    fn as_event_capable(&self) -> Option<&dyn EventCapableDevice> {
        Some(self)
    }
}

impl CharDevice for SerialDevice {
    fn read_byte(&self) -> Option<u8> {
        self.raw_input.lock().pop_front()
    }

    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        self.port.send(&[byte]).map(|_| ())
    }

    fn write(&self, bytes: &[u8]) -> Result<usize, &'static str> {
        self.port.send(bytes)
    }

    fn try_read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let count = self.read(buffer);
        if count == 0 && !buffer.is_empty() && !self.is_hung_up() {
            Err(StreamError::WouldBlock)
        } else {
            Ok(count)
        }
    }

    fn try_read_at(&self, _: u64, buffer: &mut [u8]) -> Result<usize, StreamError> {
        self.try_read(buffer)
    }

    fn can_read(&self) -> bool {
        !self.raw_input.lock().is_empty()
    }

    fn can_write(&self) -> bool {
        let buffers = self.port.buffers.lock().unwrap();
        !self.port.closed.load(Ordering::Acquire) && buffers.output_bytes < TX_CAPACITY
    }
}

impl EventCapableDevice for SerialDevice {
    fn register_event_listener(&self, listener: Weak<dyn DeviceEventListener>) {
        self.events.register_listener(listener);
    }

    fn unregister_event_listener(&self, _: &str) {
        // Weak registrations are pruned by the common emitter.
    }

    fn emit_event(&self, event: &dyn DeviceEvent) {
        self.events.emit(event);
    }
}

impl ControlOps for SerialDevice {}

impl MemoryMappingOps for SerialDevice {
    fn get_mapping_info(&self, _: usize, _: usize) -> Result<MemoryMappingInfo, &'static str> {
        Err("Host serial is a byte stream")
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for SerialDevice {
    fn is_hung_up(&self) -> bool {
        self.hangup_published.load(Ordering::Acquire)
    }

    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let closed = self.is_hung_up();
        ReadySet {
            read: interest.read && (closed || self.can_read()),
            write: interest.write && self.can_write(),
            except: interest.except && closed,
        }
    }

    fn subscribe_readiness(
        &self,
        _: ReadyInterest,
        waiter: &Arc<Waker>,
    ) -> Option<Vec<ReadinessRegistration>> {
        Some(vec![self.readiness.subscribe_readiness(waiter)])
    }

    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        trapframe: &mut Trapframe,
        timeout_ns: Option<u64>,
        min_wait_ns: u64,
    ) -> SelectWaitOutcome {
        let ready = || {
            let set = self.current_ready(interest);
            set.read || set.write || set.except || self.is_hung_up()
        };
        if min_wait_ns == 0 && ready() {
            return SelectWaitOutcome::Ready;
        }
        let Some(task) = mytask() else {
            return SelectWaitOutcome::TimedOut;
        };
        let deadline = timeout_ns.map(|ns| get_time_ns().saturating_add(ns.max(min_wait_ns)));
        if min_wait_ns != 0
            && !task.sleep_interruptible_with_precision(
                trapframe,
                min_wait_ns,
                TimerPrecision::Exact,
            )
        {
            return SelectWaitOutcome::Interrupted;
        }

        loop {
            if ready() {
                return SelectWaitOutcome::Ready;
            }
            let result = match deadline {
                Some(deadline) => {
                    let remaining = deadline.saturating_sub(get_time_ns());
                    if remaining == 0 {
                        return SelectWaitOutcome::TimedOut;
                    }
                    self.readiness.clone().wait_with_timeout_result_until_owned(
                        task.get_id(),
                        trapframe,
                        remaining,
                        ready,
                    )
                }
                None => {
                    self.readiness
                        .clone()
                        .wait_result_until_owned(task.get_id(), trapframe, ready)
                }
            };
            match result {
                WaitResult::Interrupted => return SelectWaitOutcome::Interrupted,
                WaitResult::TimedOut if !ready() => return SelectWaitOutcome::TimedOut,
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::fd::OwnedFd, time::Duration};

    #[test]
    fn host_control_keeps_reading_with_contended_buffers_and_a_full_irq_relay() {
        let (mut input, terminal) = UnixStream::pair().unwrap();
        terminal.set_nonblocking(true).unwrap();
        let mut terminal = File::from(OwnedFd::from(terminal));
        let (notify, wake) = UnixStream::pair().unwrap();
        notify.set_nonblocking(true).unwrap();
        wake.set_nonblocking(true).unwrap();
        let (raise_irq, _pending_irq) = mpsc::sync_channel(1);
        raise_irq.send(()).unwrap();
        let port = Arc::new(Port {
            buffers: Mutex::new(Buffers::default()),
            closed: AtomicBool::new(false),
            notify,
            raise_irq,
            relay_done: Arc::new(AtomicBool::new(false)),
            cpu: 0,
            irq: 0,
        });
        let pending = port.buffers.lock().unwrap();
        let worker_port = port.clone();
        let restored = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let worker_restored = restored.clone();
        let (report, stopped) = mpsc::channel();
        let worker = thread::spawn(move || {
            report
                .send(run_terminal(&mut terminal, wake, &worker_port))
                .unwrap();
        });

        input.write_all(b"ordinary input").unwrap();
        thread::sleep(Duration::from_millis(20));
        input.write_all(b"more input\x1dq").unwrap();
        let result = stopped.recv_timeout(Duration::from_secs(1));
        drop(pending);
        worker.join().unwrap();
        assert!(result.unwrap().unwrap());
    }
}
