//! Execute trusted, immutable AArch64 text on one Darwin carrier thread.

use crate::{
    ram::host_page_size,
    run_gate::RunPermit,
    worker_context::{Exception, GuestStop, Registers, SYSCALL_BRK},
};
use std::{
    arch::naked_asm,
    cell::UnsafeCell,
    collections::BTreeMap,
    ffi::c_void,
    io, mem,
    ops::Range,
    ptr,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU8, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
};

const ENTER_BRK: u16 = 0x5600;
const READ_TLS_BRK: u16 = 0x5400;
const WRITE_TLS_BRK: u16 = 0x5500;
const HOST: u8 = 0;
const ENTERING: u8 = 1;
const GUEST: u8 = 2;
const EXITING: u8 = 3;
const SIGNALS: [i32; 6] = [
    libc::SIGTRAP,
    libc::SIGUSR1,
    libc::SIGSEGV,
    libc::SIGBUS,
    libc::SIGILL,
    libc::SIGFPE,
];

#[derive(Clone, Copy)]
pub enum Patch {
    Syscall(u16),
    ReadTls(u8),
    WriteTls(u8),
}

impl Patch {
    pub(crate) fn from_guest_word(word: u32) -> Option<Self> {
        if word & 0xffe0_001f == 0xd400_0001 {
            return Some(Self::Syscall(((word >> 5) & 0xffff) as u16));
        }
        let register = (word & 31) as u8;
        match word & !31 {
            0xd53b_d040 => Some(Self::ReadTls(register)),
            0xd51b_d040 => Some(Self::WriteTls(register)),
            _ => None,
        }
    }

    fn breakpoint(self) -> u16 {
        match self {
            Self::Syscall(_) => SYSCALL_BRK,
            Self::ReadTls(_) => READ_TLS_BRK,
            Self::WriteTls(_) => WRITE_TLS_BRK,
        }
    }
}

pub struct SignedText {
    bounds: Range<u64>,
    patches: BTreeMap<u64, Patch>,
    lease: Option<Arc<crate::signed_image::Lease>>,
}

impl SignedText {
    /// Register the signed mapping and its build-time patch sites.
    ///
    /// # Safety
    /// The range must remain readable and executable for this object's
    /// lifetime. Its instructions and any code they can reach must be trusted
    /// to execute in the host process, preserve x18, and trap instead of
    /// issuing host syscalls or changing the host thread pointer.
    pub unsafe fn new(
        bounds: Range<u64>,
        patches: impl IntoIterator<Item = (u64, Patch)>,
    ) -> io::Result<Self> {
        if bounds.start >= bounds.end || (bounds.start | bounds.end) & 3 != 0 {
            return Err(io::Error::other("Invalid signed text bounds"));
        }
        let mut sites = BTreeMap::new();
        for (pc, patch) in patches {
            if !bounds.contains(&pc)
                || pc & 3 != 0
                || matches!(patch, Patch::ReadTls(reg) | Patch::WriteTls(reg) if reg > 31 || reg == 18)
            {
                return Err(io::Error::other("Invalid signed patch site"));
            }
            // SAFETY: The constructor requires the signed range to be readable.
            let word = unsafe { ptr::with_exposed_provenance::<u32>(pc as usize).read() };
            if word != 0xd420_0000 | (u32::from(patch.breakpoint()) << 5)
                || sites.insert(pc, patch).is_some()
            {
                return Err(io::Error::other(
                    "Signed patch site does not match the image",
                ));
            }
        }
        Ok(Self {
            bounds,
            patches: sites,
            lease: None,
        })
    }

    pub(crate) fn retain_mapping(&mut self, lease: Arc<crate::signed_image::Lease>) {
        self.lease = Some(lease);
    }

    pub fn contains(&self, pc: u64) -> bool {
        self.bounds.contains(&pc)
    }
}

struct Request {
    permit: RunPermit,
    serial: u64,
    text: Arc<SignedText>,
    registers: Registers,
    tls: u64,
}

#[derive(Default)]
struct InFlight {
    last_serial: u64,
    active: Option<(RunPermit, u64)>,
}

pub struct Carrier {
    commands: SyncSender<Request>,
    stops: Mutex<Receiver<(RunPermit, GuestStop)>>,
    position: Mutex<InFlight>,
    control: &'static Control,
    pthread: usize,
}

impl Carrier {
    pub fn start() -> io::Result<Self> {
        let (commands, work) = mpsc::sync_channel(1);
        let (report, stops) = mpsc::sync_channel(1);
        let (ready, startup) = mpsc::sync_channel(0);
        thread::Builder::new()
            .name("scarlet-guest".into())
            .spawn(move || {
                let control = match install_control() {
                    Ok(control) => control,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                if ready
                    .send(Ok((control, unsafe { libc::pthread_self() } as usize)))
                    .is_err()
                {
                    return;
                }
                for request in work {
                    let stop = control.execute(&request);
                    if report.send((request.permit, stop)).is_err() {
                        break;
                    }
                }
            })?;
        let (control, pthread) = startup
            .recv()
            .map_err(|_| io::Error::other("Carrier exited during setup"))??;
        Ok(Self {
            commands,
            stops: Mutex::new(stops),
            position: Mutex::default(),
            control,
            pthread,
        })
    }

    pub fn issue(
        &self,
        permit: RunPermit,
        text: Arc<SignedText>,
        registers: Registers,
        tls: u64,
    ) -> io::Result<()> {
        if !text.contains(registers.pc) || registers.pc & 3 != 0 {
            return Err(io::Error::other("Guest PC is outside the signed image"));
        }
        let mut position = self.position.lock().unwrap();
        if position.active.is_some() {
            return Err(io::Error::other("Carrier already owns a run permit"));
        }
        let serial = position
            .last_serial
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Carrier run serial exhausted"))?;
        position.last_serial = serial;
        position.active = Some((permit, serial));
        if self
            .commands
            .send(Request {
                permit,
                serial,
                text,
                registers,
                tls,
            })
            .is_err()
        {
            position.active = None;
            return Err(io::Error::other("Carrier command channel closed"));
        }
        Ok(())
    }

    pub fn receive(&self, permit: RunPermit) -> io::Result<GuestStop> {
        let received = self
            .stops
            .lock()
            .unwrap()
            .recv()
            .map_err(|_| io::Error::other("Carrier stop channel closed"))?;
        let mut position = self.position.lock().unwrap();
        if received.0 != permit || !position.active.is_some_and(|(active, _)| active == permit) {
            std::process::abort();
        }
        position.active = None;
        Ok(received.1)
    }

    pub fn interrupt(&self, permit: RunPermit) -> io::Result<()> {
        let position = self.position.lock().unwrap();
        let Some((active, serial)) = position.active else {
            return Ok(());
        };
        if active != permit || self.control.interrupt.swap(serial, Ordering::SeqCst) == serial {
            return Ok(());
        }
        // The position lock spans the notification. Receiving the stop and
        // admitting another request cannot overtake the send.
        let error = unsafe { libc::pthread_kill(self.pthread as libc::pthread_t, libc::SIGUSR1) };
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error));
        }
        Ok(())
    }
}

#[derive(Default)]
struct Frame {
    host: Registers,
    guest: Registers,
    text_start: u64,
    text_end: u64,
    serial: u64,
    interrupted: bool,
    esr: u32,
    far: u64,
}

struct Control {
    altstack: Range<usize>,
    mode: AtomicU8,
    interrupt: AtomicU64,
    previous: [libc::sigaction; SIGNALS.len()],
    frame: UnsafeCell<Frame>,
}

// Only the carrier and its own handler access the frame, on opposite sides
// of the entry/return trampoline. Other threads only touch the interrupt.
unsafe impl Sync for Control {}

static CONTROL: OnceLock<Control> = OnceLock::new();

impl Control {
    fn execute(&self, request: &Request) -> GuestStop {
        let mut registers = request.registers;
        let mut tls = request.tls;
        loop {
            // SAFETY: The carrier is parked. No borrow spans entry or exit.
            unsafe {
                self.frame.get().write(Frame {
                    guest: registers,
                    serial: request.serial,
                    text_start: request.text.bounds.start,
                    text_end: request.text.bounds.end,
                    ..Frame::default()
                });
            }
            self.mode.store(ENTERING, Ordering::SeqCst);
            unsafe { enter() };
            self.mode.store(HOST, Ordering::SeqCst);
            let frame = unsafe { &*self.frame.get() };
            let exception = Exception {
                registers: frame.guest,
                esr: frame.esr,
                far: frame.far,
            };
            registers = exception.registers;
            if frame.interrupted {
                return GuestStop {
                    registers,
                    tls,
                    esr: 0,
                    far: 0,
                    interrupted: true,
                };
            }
            if let Some(patch) = request.text.patches.get(&registers.pc)
                && exception.is_breakpoint(patch.breakpoint())
            {
                match *patch {
                    Patch::Syscall(immediate) => {
                        let trap = exception
                            .translated_syscall(registers.pc, immediate)
                            .unwrap();
                        registers.pc = trap.elr;
                        return GuestStop {
                            registers,
                            tls,
                            esr: trap.esr,
                            far: 0,
                            interrupted: false,
                        };
                    }
                    Patch::ReadTls(register) => {
                        if let Some(destination) = registers.x.get_mut(usize::from(register)) {
                            *destination = tls;
                        }
                    }
                    Patch::WriteTls(register) => {
                        tls = registers.x.get(usize::from(register)).copied().unwrap_or(0);
                    }
                }
                registers.pc += 4;
                if request.text.contains(registers.pc) {
                    continue;
                }
            }
            return GuestStop {
                registers,
                tls,
                esr: exception.esr.into(),
                far: exception.far,
                interrupted: false,
            };
        }
    }
}

#[unsafe(naked)]
unsafe extern "C" fn enter() {
    naked_asm!("brk #{entry}", "ret", entry = const ENTER_BRK);
}

fn install_control() -> io::Result<&'static Control> {
    let page = host_page_size()?;
    let stack_size = 128 * 1024;
    let size = page + stack_size;
    let mapping = unsafe {
        libc::mmap(
            ptr::null_mut(),
            size,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        )
    };
    if mapping == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    let stack = unsafe { mapping.byte_add(page) };
    let result = enable_handlers(stack, stack_size);
    if result.is_err() {
        unsafe { libc::munmap(mapping, size) };
    }
    result
}

fn enable_handlers(stack: *mut c_void, size: usize) -> io::Result<&'static Control> {
    let mut previous: [libc::sigaction; SIGNALS.len()] = unsafe { mem::zeroed() };
    for (signal, action) in SIGNALS.into_iter().zip(&mut previous) {
        if unsafe { libc::sigaction(signal, ptr::null(), action) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    let start = stack.addr();
    let mut mask = 0;
    let mut action: libc::sigaction = unsafe { mem::zeroed() };
    unsafe {
        libc::sigemptyset(&mut mask);
        libc::sigaddset(&mut mask, libc::SIGUSR1);
        libc::sigfillset(&mut action.sa_mask);
    }
    let error = unsafe { libc::pthread_sigmask(libc::SIG_UNBLOCK, &mask, ptr::null_mut()) };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error));
    }
    if unsafe { libc::mprotect(stack, size, libc::PROT_READ | libc::PROT_WRITE) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let alt = libc::stack_t {
        ss_sp: stack,
        ss_size: size,
        ss_flags: 0,
    };
    if unsafe { libc::sigaltstack(&alt, ptr::null_mut()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let control = match CONTROL.set(Control {
        altstack: start..start + size,
        mode: AtomicU8::new(HOST),
        interrupt: AtomicU64::new(0),
        previous,
        frame: Default::default(),
    }) {
        Ok(()) => CONTROL.get().unwrap(),
        Err(_) => {
            disable_altstack();
            return Err(io::Error::other(
                "Carrier signal actions are already installed",
            ));
        }
    };
    action.sa_sigaction = handle_signal as *const () as usize;
    action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK | libc::SA_RESTART;
    for (index, signal) in SIGNALS.into_iter().enumerate() {
        if unsafe { libc::sigaction(signal, &action, ptr::null_mut()) } != 0 {
            let error = io::Error::last_os_error();
            for i in 0..index {
                unsafe { libc::sigaction(SIGNALS[i], &control.previous[i], ptr::null_mut()) };
            }
            disable_altstack();
            return Err(error);
        }
    }
    Ok(control)
}

fn disable_altstack() {
    let off = libc::stack_t {
        ss_sp: ptr::null_mut(),
        ss_size: 0,
        ss_flags: libc::SS_DISABLE,
    };
    unsafe { libc::sigaltstack(&off, ptr::null_mut()) };
}

unsafe extern "C" fn handle_signal(signal: i32, info: *mut libc::siginfo_t, raw: *mut c_void) {
    let Some(control) = CONTROL.get() else { return };
    let marker = &raw as *const _ as usize;
    if !control.altstack.contains(&marker) {
        forward_signal(control, signal, info, raw);
        return;
    }
    let mode = control.mode.load(Ordering::SeqCst);
    let requested = control.interrupt.load(Ordering::SeqCst);
    if signal == libc::SIGUSR1 && requested != 0 && mode != GUEST {
        // The entry trampoline checks for notifications issued while parked.
        return;
    }
    if mode != ENTERING && mode != GUEST {
        forward_signal(control, signal, info, raw);
        return;
    }
    // SAFETY: SA_ONSTACK selected the carrier's own alternate stack, and
    // the action masks every signal until the current frame returns.
    let frame = unsafe { &mut *control.frame.get() };
    let context = unsafe { &mut *(*raw.cast::<libc::ucontext_t>()).uc_mcontext };
    let pc = context.__ss.__pc;
    let esr = context.__es.__esr;
    let stop_requested = requested == frame.serial;
    if mode == ENTERING
        && signal == libc::SIGTRAP
        && pc == enter as *const () as u64
        && esr >> 26 == 0x3c
        && esr & 0xffff == u32::from(ENTER_BRK)
    {
        frame.host = Registers::capture(context);
        frame.host.pc += 4;
        if stop_requested {
            frame.interrupted = true;
            control.mode.store(EXITING, Ordering::SeqCst);
            frame.host.restore(context);
        } else {
            let mut guest = frame.guest;
            guest.x[18] = frame.host.x[18];
            guest.restore(context);
            control.mode.store(GUEST, Ordering::SeqCst);
        }
        return;
    }
    if mode == GUEST {
        let instruction_fault =
            matches!(signal, libc::SIGBUS | libc::SIGSEGV) && matches!(esr >> 26, 0x20 | 0x21);
        let timer = signal == libc::SIGUSR1;
        if timer && requested != 0 && !stop_requested {
            return;
        }
        if (timer && stop_requested)
            || (!timer && ((frame.text_start..frame.text_end).contains(&pc) || instruction_fault))
        {
            control.mode.store(EXITING, Ordering::SeqCst);
            let mut guest = Registers::capture(context);
            guest.x[18] = frame.guest.x[18];
            frame.guest = guest;
            frame.interrupted = timer;
            frame.esr = esr;
            frame.far = context.__es.__far;
            frame.host.restore(context);
            return;
        }
    }
    forward_signal(control, signal, info, raw);
}

fn forward_signal(control: &Control, signal: i32, info: *mut libc::siginfo_t, raw: *mut c_void) {
    for (number, previous) in SIGNALS.into_iter().zip(&control.previous) {
        if signal != number {
            continue;
        }
        let handler = previous.sa_sigaction;
        // SAFETY: This is the native sigaction ABI of the previous handler.
        unsafe {
            if handler == libc::SIG_DFL {
                libc::sigaction(signal, previous, ptr::null_mut());
                libc::raise(signal);
            } else if handler != libc::SIG_IGN {
                if previous.sa_flags & libc::SA_SIGINFO != 0 {
                    let handler: unsafe extern "C" fn(i32, *mut libc::siginfo_t, *mut c_void) =
                        mem::transmute(handler);
                    handler(signal, info, raw);
                } else {
                    let handler: unsafe extern "C" fn(i32) = mem::transmute(handler);
                    handler(signal);
                }
            }
        }
        return;
    }
}
