//! Shared Apple output behind Scarlet's PCM playback interface.

mod completion;
mod unit;

use completion::CompletionSignal;

use scarlet::{
    device::audio::{
        AUDIO_PCM_FORMAT_S16LE, AudioCompletionCallback, AudioPcmBuffer, AudioPcmCapabilities,
        AudioPcmParams, AudioPcmPeriod, AudioPlaybackDevice,
    },
    interrupt::{
        CpuId, InterruptClaim, InterruptId, InterruptManager, InterruptResult, InterruptSource,
    },
    sync::IrqSpinLock,
};
use std::{
    cell::UnsafeCell,
    io,
    mem::MaybeUninit,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const FRAME_BYTES: usize = 4;
const MIN_PERIOD_FRAMES: u32 = 64;
const MAX_PERIOD_FRAMES: u32 = 4096;
const MAX_BUFFER_FRAMES: u32 = 32_768;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionActivation {
    Ready,
    Deferred,
    Failed,
}

/// Session hooks run on a host control path, never on the render callback.
pub struct SessionHooks {
    /// True means lifecycle resume; false means a new guest START request.
    pub activate: fn(bool) -> SessionActivation,
    pub deactivate: fn(),
}

struct Control {
    // Host lifecycle callbacks have no logical CPU binding. Keep their PCM
    // lease here; only guest device operations may acquire `stream`'s IRQ lock.
    queue: Option<Arc<RenderQueue>>,
    unit: Option<unit::OutputUnit>,
    running: bool,
}

pub struct CoreAudioPlayback {
    unit: Mutex<Control>,
    session: Option<SessionHooks>,
    stream: IrqSpinLock<Option<Stream>>,
    completions: Arc<CompletionSource>,
}

struct Stream {
    buffer: AudioPcmBuffer,
    queue: Arc<RenderQueue>,
    reclaimed: usize,
}

struct CompletionSource {
    line: InterruptId,
    signal: Arc<CompletionSignal>,
    callback: IrqSpinLock<Option<AudioCompletionCallback>>,
}

impl InterruptSource for CompletionSource {
    fn interrupt_id(&self) -> Option<InterruptId> {
        Some(self.line)
    }

    fn claim_interrupt(&self) -> InterruptResult<InterruptClaim> {
        let callback = self.callback.lock().clone();
        if let Some(callback) = callback {
            callback();
        }
        Ok(InterruptClaim::Handled)
    }
}

impl CoreAudioPlayback {
    pub fn attach(
        cpu: CpuId,
        line: InterruptId,
        session: Option<SessionHooks>,
    ) -> io::Result<Self> {
        let signal = Arc::new(CompletionSignal::new()?);
        let completions = Arc::new(CompletionSource {
            line,
            signal: signal.clone(),
            callback: IrqSpinLock::new(None),
        });
        let irq = InterruptManager::global();
        let source: Arc<dyn InterruptSource> = completions.clone();
        irq.register_interrupt_source(line, source.clone())
            .map_err(|error| io::Error::other(format!("Cannot register audio IRQ: {error}")))?;
        if let Err(error) = irq.enable_external_interrupt(line, cpu) {
            irq.unregister_interrupt_source(line, &source);
            return Err(io::Error::other(format!(
                "Cannot enable audio IRQ: {error}"
            )));
        }
        let relay = thread::Builder::new()
            .name("scarlet-audio-completions".into())
            .spawn(move || {
                while signal.wait() {
                    scarlet::hosted::raise_external_interrupt(cpu, line)
                        .expect("Registered audio IRQ must remain available");
                }
            });
        match relay {
            Ok(_) => Ok(Self {
                unit: Mutex::new(Control {
                    queue: None,
                    unit: None,
                    running: false,
                }),
                session,
                stream: IrqSpinLock::new(None),
                completions,
            }),
            Err(error) => {
                let _ = irq.disable_external_interrupt(line, cpu);
                irq.unregister_interrupt_source(line, &source);
                Err(error)
            }
        }
    }
}

impl CoreAudioPlayback {
    fn start_host(&self, control: &mut Control, resuming: bool) -> Result<(), &'static str> {
        if control.unit.is_some() {
            return Ok(());
        }
        let queue = control
            .queue
            .as_ref()
            .ok_or("Core Audio output is not configured")?
            .clone();
        if let Some(session) = &self.session {
            match (session.activate)(resuming) {
                SessionActivation::Ready => {}
                // Accept guest START intent without opening a native unit. A
                // lifecycle retry uses the same queue; no completions invented.
                SessionActivation::Deferred => return Ok(()),
                SessionActivation::Failed => return Err("Audio session activation failed"),
            }
        }
        let result = unit::OutputUnit::open(queue).and_then(|mut unit| {
            unit.start()?;
            Ok(unit)
        });
        match result {
            Ok(unit) => {
                control.unit = Some(unit);
                Ok(())
            }
            Err(error) => {
                if let Some(session) = &self.session {
                    (session.deactivate)();
                }
                Err(error)
            }
        }
    }

    fn park_host(&self, control: &mut Control) -> Result<(), &'static str> {
        let result = control.unit.take().map_or(Ok(()), unit::OutputUnit::close);
        if let Some(session) = &self.session {
            (session.deactivate)();
        }
        result
    }

    /// Host interruptions preserve all PCM accounting. Guest STOP still discards.
    pub fn pause_host(&self) {
        let mut control = self.unit.lock().unwrap();
        let _ = self.park_host(&mut control);
    }

    pub fn resume_host(&self) {
        let mut control = self.unit.lock().unwrap();
        if control.running {
            if let Err(error) = self.start_host(&mut control, true) {
                eprintln!("scarlet: audio resume failed: {error}");
            }
        }
    }
}

impl Drop for CoreAudioPlayback {
    fn drop(&mut self) {
        self.completions.signal.close();
    }
}

impl AudioPlaybackDevice for CoreAudioPlayback {
    fn capabilities(&self) -> AudioPcmCapabilities {
        let mut caps = AudioPcmCapabilities {
            formats: 1 << AUDIO_PCM_FORMAT_S16LE,
            rate_count: 1,
            min_channels: CHANNELS,
            max_channels: CHANNELS,
            min_period_frames: MIN_PERIOD_FRAMES,
            max_period_frames: MAX_PERIOD_FRAMES,
            min_buffer_frames: MIN_PERIOD_FRAMES,
            max_buffer_frames: MAX_BUFFER_FRAMES,
            ..Default::default()
        };
        caps.rates[0] = RATE;
        caps
    }

    fn configure(
        &self,
        params: &AudioPcmParams,
        buffer: AudioPcmBuffer,
    ) -> Result<(), &'static str> {
        if params.format != AUDIO_PCM_FORMAT_S16LE
            || params.rate != RATE
            || params.channels != CHANNELS
            || params._reserved != 0
            || !(MIN_PERIOD_FRAMES..=MAX_PERIOD_FRAMES).contains(&params.period_frames)
            || !(params.period_frames..=MAX_BUFFER_FRAMES).contains(&params.buffer_frames)
            || !params.buffer_frames.is_multiple_of(params.period_frames)
            || params.buffer_bytes() != Some(buffer.buffer_bytes)
            || buffer.vaddr == 0
            || buffer.vaddr.checked_add(buffer.buffer_bytes).is_none()
        {
            return Err("Unsupported Core Audio PCM configuration");
        }
        let mut control = self.unit.lock().unwrap();
        if control.queue.is_some() {
            return Err("Core Audio output is already configured");
        }
        let queue = Arc::new(RenderQueue::new(
            params.period_frames as usize * FRAME_BYTES,
            (params.buffer_frames / params.period_frames) as usize,
            self.completions.signal.clone(),
        ));
        control.queue = Some(queue.clone());
        *self.stream.lock() = Some(Stream {
            buffer,
            queue,
            reclaimed: 0,
        });
        drop(control);
        Ok(())
    }

    fn start(&self) -> Result<(), &'static str> {
        let mut control = self.unit.lock().unwrap();
        self.start_host(&mut control, false)?;
        control.running = true;
        Ok(())
    }

    fn stop(&self) -> Result<(), &'static str> {
        let mut control = self.unit.lock().unwrap();
        control.running = false;
        let result = self.park_host(&mut control);
        if let Some(stream) = self.stream.lock().as_mut() {
            stream.queue.discard();
            stream.reclaimed = 0;
        }
        result
    }

    fn release(&self) -> Result<(), &'static str> {
        let mut control = self.unit.lock().unwrap();
        control.running = false;
        let result = self.park_host(&mut control);
        self.stream.lock().take();
        control.queue = None;
        result
    }

    fn submit_period(&self, period: AudioPcmPeriod) -> Result<(), &'static str> {
        let stream = self.stream.lock();
        let stream = stream
            .as_ref()
            .ok_or("Core Audio output is not configured")?;
        let end = period
            .byte_offset
            .checked_add(period.byte_len)
            .ok_or("Core Audio period overflow")?;
        if end > stream.buffer.buffer_bytes || period.byte_len != stream.queue.period_bytes {
            return Err("Core Audio period is outside its PCM buffer or has the wrong length");
        }
        // The AudioCharDevice owns its kernel ring and holds its ring lock
        // throughout this call. The host only ever reads the copied period.
        let source = (stream.buffer.vaddr + period.byte_offset) as *const u8;
        unsafe { stream.queue.submit(source) }
    }

    fn process_completions(&self) -> usize {
        let mut stream = self.stream.lock();
        let Some(stream) = stream.as_mut() else {
            return 0;
        };
        let consumed = stream.queue.consumed.load(Ordering::Acquire);
        let completed = consumed.wrapping_sub(stream.reclaimed);
        stream.reclaimed = consumed;
        completed
    }

    fn set_completion_callback(&self, callback: Option<AudioCompletionCallback>) {
        *self.completions.callback.lock() = callback;
    }

    fn max_in_flight_periods(&self) -> usize {
        self.stream
            .lock()
            .as_ref()
            .map_or(0, |stream| stream.queue.capacity)
    }
}

struct RenderQueue {
    samples: Box<[UnsafeCell<u8>]>,
    period_bytes: usize,
    capacity: usize,
    slot_mask: usize,
    submitted: AtomicUsize,
    consumed: AtomicUsize,
    offset: AtomicUsize,
    enabled: AtomicBool,
    rendering: AtomicBool,
    signal: Arc<CompletionSignal>,
}

// The stream lock serializes producers. A release of submitted transfers
// each period to the render callback, and a release of consumed returns it.
// The rendering flag serializes callbacks and protects the partial offset.
unsafe impl Sync for RenderQueue {}

impl RenderQueue {
    fn new(period_bytes: usize, capacity: usize, signal: Arc<CompletionSignal>) -> Self {
        // Make the entire committed Scarlet ring available even when a
        // host render slice spans many short periods. Power-of-two slots
        // preserve indexing when the sequence counter wraps.
        let slots = capacity.next_power_of_two();
        Self {
            samples: (0..slots * period_bytes)
                .map(|_| UnsafeCell::new(0))
                .collect(),
            period_bytes,
            capacity,
            slot_mask: slots - 1,
            submitted: AtomicUsize::new(0),
            consumed: AtomicUsize::new(0),
            offset: AtomicUsize::new(0),
            enabled: AtomicBool::new(false),
            rendering: AtomicBool::new(false),
            signal,
        }
    }

    /// The caller owns producer access and supplies one readable period.
    unsafe fn submit(&self, source: *const u8) -> Result<(), &'static str> {
        let submitted = self.submitted.load(Ordering::Relaxed);
        let consumed = self.consumed.load(Ordering::Acquire);
        if submitted.wrapping_sub(consumed) >= self.capacity {
            return Err("Core Audio period queue is full");
        }
        let start = (submitted & self.slot_mask) * self.period_bytes;
        unsafe {
            let destination = UnsafeCell::raw_get(self.samples.as_ptr().add(start));
            std::ptr::copy_nonoverlapping(source, destination, self.period_bytes);
        }
        self.submitted
            .store(submitted.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Called only with submissions excluded and playback disabled and parked.
    fn discard(&self) {
        self.offset.store(0, Ordering::Relaxed);
        self.consumed.store(0, Ordering::Relaxed);
        self.submitted.store(0, Ordering::Relaxed);
    }

    #[cfg(test)]
    fn render(&self, output: &mut [MaybeUninit<u8>]) -> bool {
        self.render_gated(output, &self.enabled)
    }

    fn render_gated(&self, output: &mut [MaybeUninit<u8>], gate: &AtomicBool) -> bool {
        // Serial even if the host attempts overlapping callbacks. Return
        // silence on the competing callback without waiting or allocating.
        if self.rendering.swap(true, Ordering::AcqRel) {
            output.fill(MaybeUninit::new(0));
            return false;
        }
        let mut written = 0;
        if self.enabled.load(Ordering::Acquire) && gate.load(Ordering::Acquire) {
            let mut consumed = self.consumed.load(Ordering::Relaxed);
            let mut offset = self.offset.load(Ordering::Relaxed);
            while written < output.len() && consumed != self.submitted.load(Ordering::Acquire) {
                let start = (consumed & self.slot_mask) * self.period_bytes;
                let count = (output.len() - written).min(self.period_bytes - offset);
                unsafe {
                    let source = UnsafeCell::raw_get(self.samples.as_ptr().add(start + offset));
                    std::ptr::copy_nonoverlapping(
                        source,
                        output[written..].as_mut_ptr().cast::<u8>(),
                        count,
                    );
                }
                written += count;
                offset += count;
                if offset == self.period_bytes {
                    offset = 0;
                    consumed = consumed.wrapping_add(1);
                    self.consumed.store(consumed, Ordering::Release);
                    self.signal.notify();
                }
            }
            self.offset.store(offset, Ordering::Relaxed);
        }
        self.rendering.store(false, Ordering::Release);
        output[written..].fill(MaybeUninit::new(0));
        written != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_start_preserves_pcm_and_explicit_retry_reports_real_failure() {
        thread::spawn(|| {
            use std::cell::{Cell, RefCell};
            thread_local! {
                static STATUS: Cell<SessionActivation> = const { Cell::new(SessionActivation::Deferred) };
                static REQUESTS: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
            }
            let signal = Arc::new(CompletionSignal::new().unwrap());
            let queue = Arc::new(RenderQueue::new(8, 2, signal.clone()));
            unsafe { queue.submit([9; 8].as_ptr()) }.unwrap();
            let output = CoreAudioPlayback {
                unit: Mutex::new(Control { queue: Some(queue.clone()), unit: None, running: false }),
                session: Some(SessionHooks {
                    activate: |resuming| {
                        REQUESTS.with(|requests| requests.borrow_mut().push(resuming));
                        STATUS.with(Cell::get)
                    },
                    deactivate: || {},
                }),
                stream: IrqSpinLock::new(None),
                completions: Arc::new(CompletionSource { line: 18, signal, callback: IrqSpinLock::new(None) }),
            };
            assert_eq!(output.start(), Ok(()));
            assert!(output.unit.lock().unwrap().running);
            assert!(output.unit.lock().unwrap().unit.is_none());
            output.pause_host();
            output.resume_host();
            assert_eq!(output.start(), Ok(())); // Explicit retry, still deferred.
            STATUS.with(|status| status.set(SessionActivation::Failed));
            assert_eq!(output.start(), Err("Audio session activation failed"));
            REQUESTS.with(|requests| assert_eq!(*requests.borrow(), [false, true, false, false]));
            assert_eq!(queue.submitted.load(Ordering::Acquire), 1);
            assert_eq!(queue.consumed.load(Ordering::Acquire), 0);
            assert_eq!(queue.offset.load(Ordering::Acquire), 0);
        }).join().unwrap();
    }

    #[test]
    fn lifecycle_resume_on_unbound_host_thread_never_takes_guest_irq_lock() {
        thread::spawn(|| {
            assert_eq!(scarlet::arch::try_get_cpuid(), None);
            static ACTIVATED: AtomicBool = AtomicBool::new(false);
            let signal = Arc::new(CompletionSignal::new().unwrap());
            let queue = Arc::new(RenderQueue::new(8, 2, signal.clone()));
            unsafe { queue.submit([7; 8].as_ptr()) }.unwrap();
            let output = CoreAudioPlayback {
                unit: Mutex::new(Control {
                    queue: Some(queue.clone()),
                    unit: None,
                    running: true,
                }),
                session: Some(SessionHooks {
                    activate: |resuming| {
                        assert!(resuming);
                        assert_eq!(scarlet::arch::try_get_cpuid(), None);
                        ACTIVATED.store(true, Ordering::Release);
                        SessionActivation::Deferred // No audio hardware required.
                    },
                    deactivate: || assert_eq!(scarlet::arch::try_get_cpuid(), None),
                }),
                stream: IrqSpinLock::new(None),
                completions: Arc::new(CompletionSource {
                    line: 18,
                    signal,
                    callback: IrqSpinLock::new(None),
                }),
            };
            output.pause_host();
            output.resume_host();
            assert!(ACTIVATED.load(Ordering::Acquire));
            assert_eq!(queue.submitted.load(Ordering::Acquire), 1);
            assert_eq!(queue.consumed.load(Ordering::Acquire), 0);
            assert_eq!(scarlet::arch::try_get_cpuid(), None);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn host_pause_preserves_partial_period_and_rejects_old_unit_callbacks() {
        let signal = Arc::new(CompletionSignal::new().unwrap());
        let queue = RenderQueue::new(8, 2, signal.clone());
        let old_unit = AtomicBool::new(true);
        let new_unit = AtomicBool::new(true);
        queue.enabled.store(true, Ordering::Release);
        unsafe { queue.submit([7; 8].as_ptr()) }.unwrap();
        let mut half = [MaybeUninit::uninit(); 4];
        assert!(queue.render_gated(&mut half, &old_unit));
        assert_eq!(queue.offset.load(Ordering::Acquire), 4);
        assert_eq!(queue.consumed.load(Ordering::Acquire), 0);
        old_unit.store(false, Ordering::Release);
        queue.enabled.store(false, Ordering::Release);
        assert!(!queue.render_gated(&mut half, &old_unit));
        assert_eq!(queue.offset.load(Ordering::Acquire), 4);
        assert!(!signal.is_pending());
        queue.enabled.store(true, Ordering::Release);
        // A callback retained by a failed dispose must not consume new audio.
        assert!(!queue.render_gated(&mut half, &old_unit));
        assert_eq!(half.map(|v| unsafe { v.assume_init() }), [0; 4]);
        assert!(queue.render_gated(&mut half, &new_unit));
        assert_eq!(half.map(|v| unsafe { v.assume_init() }), [7; 4]);
        assert_eq!(queue.consumed.load(Ordering::Acquire), 1);
        assert!(signal.wait());
    }

    #[test]
    fn guest_stop_discards_paused_tail_without_completing_it() {
        let signal = Arc::new(CompletionSignal::new().unwrap());
        let queue = RenderQueue::new(8, 2, signal.clone());
        queue.enabled.store(true, Ordering::Release);
        unsafe { queue.submit([7; 8].as_ptr()) }.unwrap();
        assert!(queue.render(&mut [MaybeUninit::uninit(); 4]));
        queue.enabled.store(false, Ordering::Release);
        queue.discard();
        queue.enabled.store(true, Ordering::Release);
        unsafe { queue.submit([9; 8].as_ptr()) }.unwrap();
        let mut output = [MaybeUninit::uninit(); 8];
        assert!(queue.render(&mut output));
        assert_eq!(output.map(|v| unsafe { v.assume_init() }), [9; 8]);
        assert_eq!(queue.consumed.load(Ordering::Acquire), 1);
        assert!(signal.wait());
    }

    #[test]
    fn render_reclaims_only_whole_periods_and_zero_fills_underruns() {
        const PERIOD_SLOTS: usize = 5;
        let signal = Arc::new(CompletionSignal::new().unwrap());
        let queue = RenderQueue::new(8, PERIOD_SLOTS, signal.clone());
        for value in 1..=PERIOD_SLOTS as u8 {
            unsafe { queue.submit([value; 8].as_ptr()) }.unwrap();
        }
        assert!(unsafe { queue.submit([9; 8].as_ptr()) }.is_err());
        let mut half = [MaybeUninit::uninit(); 4];
        assert!(!queue.render(&mut half));
        assert_eq!(half.map(|byte| unsafe { byte.assume_init() }), [0; 4]);
        queue.enabled.store(true, Ordering::Release);
        assert!(queue.render(&mut half));
        assert_eq!(half.map(|byte| unsafe { byte.assume_init() }), [1; 4]);
        assert_eq!(queue.consumed.load(Ordering::Acquire), 0);
        assert!(!signal.is_pending());
        assert!(unsafe { queue.submit([9; 8].as_ptr()) }.is_err());

        let mut tail = [MaybeUninit::uninit(); 48];
        assert!(queue.render(&mut tail));
        assert_eq!(queue.consumed.load(Ordering::Acquire), PERIOD_SLOTS);
        assert!(signal.is_pending());
        assert!(signal.wait());
        assert!(!signal.is_pending());
        let tail = tail.map(|byte| unsafe { byte.assume_init() });
        assert_eq!(&tail[..4], &[1; 4]);
        for value in 2..=PERIOD_SLOTS {
            let start = 4 + (value - 2) * 8;
            assert_eq!(&tail[start..start + 8], &[value as u8; 8]);
        }
        assert_eq!(&tail[36..], &[0; 12]);

        unsafe { queue.submit([9; 8].as_ptr()) }.unwrap();
        queue.enabled.store(false, Ordering::Release);
        queue.discard();
        queue.enabled.store(true, Ordering::Release);
        assert!(!queue.render(&mut half));
        assert_eq!(half.map(|byte| unsafe { byte.assume_init() }), [0; 4]);
        assert_eq!(queue.consumed.load(Ordering::Acquire), 0);
        assert!(!signal.is_pending());
        // Submitted silence has the same PCM clock and completion as sound.
        unsafe { queue.submit([0; 8].as_ptr()) }.unwrap();
        let mut silent = [MaybeUninit::uninit(); 8];
        assert!(queue.render(&mut silent));
        assert_eq!(silent.map(|byte| unsafe { byte.assume_init() }), [0; 8]);
        assert!(signal.wait());
        assert_eq!(queue.consumed.load(Ordering::Acquire), 1);
        signal.close();
        assert!(!signal.wait());
    }
}
