//! One Darwin process owns an installed guest address space.

use crate::{
    run_gate::{RunPermit, TaskId},
    worker_channel::{receive_backing, send_backing},
    worker_context::Registers,
    worker_memory::{Mapping, Protection},
    worker_thread::{GuestStop, Interrupt, Thread},
};
use std::{
    collections::BTreeMap,
    ffi::{CStr, c_char},
    fs::File,
    io::{self, Read, Write},
    ops::Range,
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    path::Path,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, RwLock, Weak},
};

mod lanes;
pub use lanes::WorkerThread;

pub const WORKER_ARG: &str = "--scarlet-address-space-worker";

const MAP: u64 = 1;
const UNMAP: u64 = 2;
const ATTACH: u64 = 3;
const ACK: u64 = 4;
const STOP: u64 = 5;
const FAILURE: u64 = 6;
const INTERRUPT: u64 = 7;
const GUARD: u64 = 8;

// Each scheduled run and stop is a single fixed-size message on a leased
// execution stream. Keep the little-endian layout independent of Rust padding.
const REGISTER_WORDS: usize = 100;
const RUN_HEADER: usize = 10;
const STOP_HEADER: usize = 14;
const EXCLUSIVE_PAYLOAD: usize = 34 + crate::exclusive::MAX_WORDS;

pub struct Worker {
    process: Arc<WorkerProcess>,
    channel: Arc<WorkerControl>,
}

/// Serialize complete wire exchanges, independently of kernel VM admission.
#[derive(Debug)]
pub struct WorkerControl {
    wire: Mutex<UnixStream>,
    lanes: Mutex<lanes::Lanes>,
}

pub struct Stop {
    pub identity: [u64; 7],
    pub guest: GuestStop,
    /// Verified against the immutable instructions of this run permit.
    pub exclusive: Option<crate::exclusive::Window>,
    /// Original guest word at a data-aborting PC in this publication.
    pub fault_instruction: Option<u32>,
}

impl Worker {
    pub fn start() -> io::Result<Self> {
        Self::spawn(&std::env::current_exe()?)
    }

    pub fn spawn(executable: &Path) -> io::Result<Self> {
        let (channel, child) = UnixStream::pair()?;
        let child: OwnedFd = child.into();
        let process = Command::new(executable)
            .arg(WORKER_ARG)
            .stdin(Stdio::null())
            .stdout(Stdio::from(child))
            .spawn()?;
        let mut channel = channel;
        let process = Arc::new(WorkerProcess(Mutex::new(process)));
        if let Err(error) = read_ack(&mut channel) {
            process.terminate()?;
            return Err(error);
        }
        let worker = Self {
            process,
            channel: Arc::new(WorkerControl {
                wire: Mutex::new(channel),
                lanes: Mutex::default(),
            }),
        };
        Ok(worker)
    }

    /// The caller holds the frame lease until the mapping is retired and keeps
    /// the guest parked while replacing ranges.
    pub fn map(
        &mut self,
        address: usize,
        length: usize,
        source: BorrowedFd<'_>,
        offset: usize,
        protection: Protection<'_>,
    ) -> io::Result<()> {
        let (flags, ranges): (_, &[Range<usize>]) = match protection {
            Protection::ReadOnly => (1, &[]),
            Protection::ReadWrite => (3, &[]),
            Protection::ReadExecute(ranges) => (5, ranges),
            Protection::ReadExecuteOnFetch(ranges) => (6, ranges),
            Protection::ExecuteOnFetch(ranges) => (7, ranges),
        };
        let mut control = self.channel.wire.lock().unwrap();
        write_words(
            &mut *control,
            [
                MAP,
                address as u64,
                length as u64,
                offset as u64,
                flags,
                ranges.len() as u64,
            ],
        )?;
        for range in ranges {
            write_words(&mut *control, [range.start as u64, range.end as u64])?;
        }
        send_backing(&control, source)?;
        read_ack(&mut control)
    }

    pub fn unmap(&mut self, address: usize) -> io::Result<()> {
        let mut control = self.channel.wire.lock().unwrap();
        write_words(&mut *control, [UNMAP, address as u64])?;
        read_ack(&mut control)
    }

    pub fn guard(&mut self, address: usize, len: usize) -> io::Result<()> {
        let mut control = self.channel.wire.lock().unwrap();
        write_words(&mut *control, [GUARD, address as u64, len as u64])?;
        read_ack(&mut control)
    }

    pub fn interrupt(&mut self, permit: RunPermit) -> io::Result<()> {
        self.channel.interrupt(permit)
    }

    pub fn control(&self) -> Arc<WorkerControl> {
        self.channel.clone()
    }

    pub fn process(&self) -> Arc<WorkerProcess> {
        self.process.clone()
    }

    pub fn terminate(&mut self) -> io::Result<()> {
        self.process.terminate()
    }
}

// The execution stream carries only bytes. Use read/write on the socket fd
// and report a disconnected peer as EPIPE even with SIGPIPE enabled.
fn execution_stream(fd: OwnedFd) -> io::Result<File> {
    let enabled: libc::c_int = 1;
    let status = unsafe {
        libc::setsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            (&raw const enabled).cast(),
            size_of_val(&enabled) as libc::socklen_t,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd.into())
}

fn read_ack(channel: &mut UnixStream) -> io::Result<()> {
    let [reply] = read_words::<1>(channel)?;
    if reply == FAILURE {
        let [len] = read_words::<1>(channel)?;
        if len > 1024 {
            return Err(invalid("Invalid worker failure length"));
        }
        let mut message = vec![0; len as usize];
        channel.read_exact(&mut message)?;
        return Err(io::Error::other(
            String::from_utf8_lossy(&message).into_owned(),
        ));
    }
    if reply != ACK {
        return Err(invalid("Expected a worker acknowledgement"));
    }
    Ok(())
}

/// Process death can be confirmed independently of the mapping coordinator.
#[derive(Debug)]
pub struct WorkerProcess(Mutex<Child>);

impl WorkerProcess {
    pub fn terminate(&self) -> io::Result<()> {
        let mut process = self.0.lock().unwrap();
        if process.try_wait()?.is_none() {
            let _ = process.kill();
            process.wait()?;
        }
        Ok(())
    }
}

fn read_stop(channel: &mut File) -> io::Result<Stop> {
    let frame = read_words::<{ STOP_HEADER + REGISTER_WORDS }>(channel)?;
    if frame[0] != STOP {
        return Err(invalid("Expected a worker stop"));
    }
    let identity = frame[1..8].try_into().unwrap();
    let [esr, far, tls, interrupted, window_len, fault_word] =
        frame[8..STOP_HEADER].try_into().unwrap();
    let registers = decode_registers(&frame[STOP_HEADER..]);
    Ok(Stop {
        identity,
        fault_instruction: fault_word
            .checked_sub(1)
            .map(u32::try_from)
            .transpose()
            .map_err(|_| invalid("Invalid fault instruction"))?,
        exclusive: if window_len == 0 {
            None
        } else {
            if window_len > crate::exclusive::MAX_WORDS as u64 {
                return Err(invalid("Bad exclusive window length"));
            }
            let payload = read_words::<EXCLUSIVE_PAYLOAD>(channel)?;
            let mut initial = Registers {
                sp: payload[31],
                pc: payload[32],
                nzcv: payload[33] as u32,
                ..Default::default()
            };
            initial.x.copy_from_slice(&payload[..31]);
            let words = &payload[34..34 + window_len as usize];
            let window = crate::exclusive::Window::capture(initial, |pc| {
                let index = pc.checked_sub(initial.pc as usize)? / 4;
                u32::try_from(*words.get(index)?).ok()
            })
            .filter(|w| w.words.len() == window_len as usize)
            .ok_or_else(|| invalid("Invalid exclusive window"))?;
            Some(window)
        },
        guest: GuestStop {
            registers,
            interrupted: interrupted != 0,
            esr,
            far,
            tls,
        },
    })
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

pub fn identity(permit: RunPermit) -> [u64; 7] {
    [
        permit.worker.address_space as u64,
        permit.worker.image_generation,
        permit.task.id as u64,
        permit.task.generation,
        permit.vcpu as u64,
        permit.mapping_generation,
        permit.epoch,
    ]
}

/// Invoked only in the spawned copy of the BSP executable, before platform initialization.
pub fn run_worker() -> io::Result<()> {
    crate::worker_thread::mask_interrupt(libc::SIG_BLOCK)?;
    // SAFETY: worker mode owns its inherited control endpoint as stdout.
    let mut channel = unsafe { UnixStream::from_raw_fd(libc::STDOUT_FILENO) };
    let descriptors = std::fs::read_dir("/dev/fd")?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().parse::<i32>().ok()))
        .collect::<io::Result<Vec<_>>>()?;
    for fd in descriptors
        .into_iter()
        .flatten()
        .filter(|&fd| fd != 1 && fd != 2)
    {
        unsafe { libc::close(fd) };
    }
    confine()?;
    write_words(&mut channel, [ACK])?;
    // No diagnostics descriptor while running guest code.
    unsafe { libc::close(libc::STDERR_FILENO) };
    let mut threads = BTreeMap::<u64, Weak<Interrupt>>::new();
    let mappings = Arc::new(RwLock::new(BTreeMap::<usize, Mapping>::new()));
    while let Some(command) = read_command(&mut channel)? {
        match command {
            MAP => {
                let [address, len, offset, flags, count] = read_words(&mut channel)?;
                let mut mappings = mappings.write().unwrap();
                if mappings.contains_key(&(address as usize))
                    || count > len / 4
                    || count > 1_048_576
                {
                    return Err(invalid("Bad worker mapping"));
                }
                let mut ranges = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    let [start, end] = read_words(&mut channel)?;
                    ranges.push(start as usize..end as usize);
                }
                let protection = match flags {
                    1 if ranges.is_empty() => Protection::ReadOnly,
                    3 if ranges.is_empty() => Protection::ReadWrite,
                    5 => Protection::ReadExecute(&ranges),
                    6 => Protection::ReadExecuteOnFetch(&ranges),
                    7 => Protection::ExecuteOnFetch(&ranges),
                    _ => return Err(invalid("Bad worker protection")),
                };
                let backing = receive_backing(&channel)?;
                let mapping = unsafe {
                    Mapping::install(
                        address as usize,
                        len as usize,
                        backing.as_fd(),
                        offset as usize,
                        protection,
                    )
                };
                drop(backing);
                let mapping = match mapping {
                    Ok(mapping) => mapping,
                    Err(error) => {
                        let message = error.to_string();
                        let bytes = message.as_bytes();
                        let length = bytes.len().min(1024);
                        write_words(&mut channel, [FAILURE, length as u64])?;
                        channel.write_all(&bytes[..length])?;
                        return Err(error);
                    }
                };
                mappings.insert(address as usize, mapping);
                write_words(&mut channel, [ACK])?;
            }
            GUARD => {
                let [address, len] = read_words(&mut channel)?;
                let mut mappings = mappings.write().unwrap();
                let mapping = Mapping::guard(address as usize, len as usize)?;
                mappings.insert(address as usize, mapping);
                write_words(&mut channel, [ACK])?;
            }
            UNMAP => {
                let [address] = read_words(&mut channel)?;
                mappings
                    .write()
                    .unwrap()
                    .remove(&(address as usize))
                    .ok_or_else(|| invalid("Missing worker mapping"))?;
                write_words(&mut channel, [ACK])?;
            }
            INTERRUPT => {
                let [lane, sequence] = read_words(&mut channel)?;
                if let Some(thread) = threads.get(&lane).and_then(Weak::upgrade) {
                    thread.request(sequence)?;
                }
                write_words(&mut channel, [ACK])?;
            }
            ATTACH => {
                let [lane] = read_words(&mut channel)?;
                threads.retain(|_, owner| owner.strong_count() != 0);
                if threads.contains_key(&lane) {
                    return Err(invalid("Duplicate worker thread"));
                }
                let interrupt = Arc::new(Interrupt::default());
                threads.insert(lane, Arc::downgrade(&interrupt));
                let endpoint = execution_stream(receive_backing(&channel)?)?;
                let mappings = mappings.clone();
                let (ready, started) = std::sync::mpsc::sync_channel(1);
                std::thread::Builder::new().spawn(move || {
                    let mut thread = match Thread::new(interrupt) {
                        Ok(thread) => thread,
                        Err(error) => {
                            let _ = ready.send(Err(error));
                            return;
                        }
                    };
                    if ready.send(Ok(())).is_ok() {
                        let _ = serve_thread(endpoint, &mut thread, &mappings);
                    }
                })?;
                started.recv().map_err(io::Error::other)??;
                write_words(&mut channel, [ACK])?;
            }
            _ => return Err(invalid("Unknown worker command")),
        }
    }
    Ok(())
}

// EOF between commands means the broker closed its control channel. EOF
// within a command still reports a truncated protocol message.
fn read_command(channel: &mut UnixStream) -> io::Result<Option<u64>> {
    let mut bytes = [0; 8];
    let count = loop {
        match channel.read(&mut bytes) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            result => break result?,
        }
    };
    if count == 0 {
        return Ok(None);
    }
    channel.read_exact(&mut bytes[count..])?;
    Ok(Some(u64::from_le_bytes(bytes)))
}

fn serve_thread(
    mut channel: File,
    thread: &mut Thread,
    mappings: &RwLock<BTreeMap<usize, Mapping>>,
) -> io::Result<()> {
    let mut last_sequence = 0;
    loop {
        let frame = read_words::<{ RUN_HEADER + REGISTER_WORDS }>(&mut channel)?;
        let token = &frame[..7];
        let tls = frame[7];
        let registers = decode_registers(&frame[RUN_HEADER..]);
        if frame[9] <= last_sequence || frame[8] > 1 {
            return Err(invalid("Stale or misrouted worker run"));
        }
        last_sequence = frame[9];
        thread.retry = (frame[8] != 0).then_some(registers.pc);
        let (stopped, fault_word, window) = {
            let mappings = mappings.read().unwrap();
            let stopped = thread.run(last_sequence, registers, tls, &mappings)?;
            let read = |pc| mappings.range(..=pc).next_back()?.1.published_word(pc);
            let pc = stopped.registers.pc as usize;
            let word = (stopped.esr >> 26 == 0x24).then(|| read(pc)).flatten();
            let access = word.and_then(crate::exclusive::Access::decode);
            let initial = thread
                .exclusive_initial
                .take()
                .or_else(|| access.filter(|a| a.load).map(|_| stopped.registers));
            let window = initial.and_then(|r| crate::exclusive::Window::capture(r, read));
            (stopped, word, window)
        };
        let mut reply = [0; STOP_HEADER + REGISTER_WORDS];
        reply[0] = STOP;
        reply[1..8].copy_from_slice(token);
        reply[8..STOP_HEADER].copy_from_slice(&[
            stopped.esr,
            stopped.far,
            stopped.tls,
            stopped.interrupted as u64,
            window.as_ref().map_or(0, |w| w.words.len() as u64),
            fault_word.map_or(0, |word| u64::from(word) + 1),
        ]);
        encode_registers(&mut reply[STOP_HEADER..], &stopped.registers);
        write_words(&mut channel, reply)?;
        if let Some(window) = window {
            let mut payload = [0; EXCLUSIVE_PAYLOAD];
            payload[..31].copy_from_slice(&window.initial.x);
            payload[31..34].copy_from_slice(&[
                window.initial.sp,
                window.initial.pc,
                u64::from(window.initial.nzcv),
            ]);
            for (dst, word) in payload[34..].iter_mut().zip(window.words) {
                *dst = u64::from(word);
            }
            write_words(&mut channel, payload)?;
        }
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn write_words<const N: usize>(stream: &mut impl Write, words: [u64; N]) -> io::Result<()> {
    // A fixed little-endian wire format with no structure padding.
    let words = words.map(u64::to_le);
    stream.write_all(unsafe { std::slice::from_raw_parts(words.as_ptr().cast(), N * 8) })
}

fn read_words<const N: usize>(stream: &mut impl Read) -> io::Result<[u64; N]> {
    let mut words = [0u64; N];
    stream
        .read_exact(unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr().cast(), N * 8) })?;
    Ok(words.map(u64::from_le))
}

fn encode_registers(words: &mut [u64], regs: &Registers) {
    words[..31].copy_from_slice(&regs.x);
    words[31..36].copy_from_slice(&[
        regs.pc,
        regs.sp,
        regs.nzcv.into(),
        regs.fpcr.into(),
        regs.fpsr.into(),
    ]);
    for (&q, pair) in regs.q.iter().zip(words[36..].chunks_exact_mut(2)) {
        pair[0] = q as u64;
        pair[1] = (q >> 64) as u64;
    }
}

fn decode_registers(words: &[u64]) -> Registers {
    let mut q = [0u128; 32];
    for (q, pair) in q.iter_mut().zip(words[36..].chunks_exact(2)) {
        *q = u128::from(pair[0]) | (u128::from(pair[1]) << 64);
    }
    Registers {
        x: words[..31].try_into().unwrap(),
        pc: words[31],
        sp: words[32],
        nzcv: words[33] as u32,
        fpcr: words[34] as u32,
        fpsr: words[35] as u32,
        q,
    }
}

fn confine() -> io::Result<()> {
    unsafe extern "C" {
        static kSBXProfilePureComputation: c_char;
        fn sandbox_init(profile: *const c_char, flags: u64, error: *mut *mut c_char)
        -> libc::c_int;
        fn sandbox_free_error(error: *mut c_char);
    }
    let mut error = std::ptr::null_mut();
    if unsafe { sandbox_init(&raw const kSBXProfilePureComputation, 1, &mut error) } == 0 {
        return Ok(());
    }
    if error.is_null() {
        return Err(io::Error::last_os_error());
    }
    let message = unsafe { CStr::from_ptr(error).to_string_lossy().into_owned() };
    unsafe { sandbox_free_error(error) };
    Err(io::Error::other(message))
}
