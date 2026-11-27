//! Bounded display handoff, independent of GPU rendering completion.
//!
//! The worker calls the existing synchronous display ABI. Its reply therefore
//! proves that the old front has retired; queue admission alone never does.

#[cfg(target_os = "scarlet")]
use framebuffer::{DisplayPresentRegion, DisplaySurface};
#[cfg(target_os = "scarlet")]
use scarlet_os::handle::Handle;
use std::io::{BufWriter, Write};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use super::PresentationProgress;

/// A target keeps this shared hook so a presentation worker created before
/// completion preparation also receives the compositor's eventual wake callback.
#[derive(Clone, Default)]
pub(super) struct CompletionWake(Arc<Mutex<Option<fn()>>>);
impl CompletionWake {
    pub(super) fn set(&self, wake: fn()) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(wake);
    }
    fn signal(&self) {
        let wake = *self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(wake) = wake {
            wake();
        }
    }
}

struct Present<R> {
    sequence: u64,
    slot: usize,
    region: Option<R>,
    queued_at: Option<Instant>,
}

pub(super) struct PresentationQueue<R> {
    sender: Option<SyncSender<Present<R>>>,
    retired: Receiver<Result<u64, &'static str>>,
    worker: Option<JoinHandle<()>>,
    progress: PresentationProgress,
    profiling: bool,
}

#[cfg(target_os = "scarlet")]
impl PresentationQueue<DisplayPresentRegion> {
    pub(super) fn new(
        display: &DisplaySurface,
        images: Vec<Handle>,
        wake: CompletionWake,
    ) -> Result<Self, &'static str> {
        let presenter = display
            .presenter()
            .map_err(|_| "Failed to retain display presenter")?;
        let profiling = std::env::var("SWS_FRAME_PROFILE").is_ok_and(|value| value == "1")
            || std::env::args().any(|arg| arg == "--profile-frames");
        Self::with_presenter(wake, profiling, move |slot, region| {
            let image = images.get(slot).ok_or("Invalid presentation slot")?;
            presenter
                .present_completed_swapchain_image(image, region)
                .map_err(|_| "Display page flip failed")
        })
    }
}

impl<R: Send + 'static> PresentationQueue<R> {
    fn with_presenter(
        wake: CompletionWake,
        profiling: bool,
        mut present: impl FnMut(usize, Option<R>) -> Result<(), &'static str> + Send + 'static,
    ) -> Result<Self, &'static str> {
        // One armed flip plus one queued image. The third swapchain slot may
        // be rendered only after a later completed flip released its old use.
        // The two-entry channel also holds both admitted frames before the
        // worker first runs. Progress, rather than thread scheduling, bounds
        // the total armed + queued count to two.
        let (sender, jobs) = mpsc::sync_channel::<Present<R>>(PresentationProgress::CAPACITY);
        let (responses, retired) = mpsc::sync_channel(3);
        let worker = thread::Builder::new()
            .name("sws-present".into())
            .spawn(move || {
                let mut profile = if profiling {
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(format!("/tmp/sws-display-{}.tsv", std::process::id()))
                        .ok()
                        .map(|file| BufWriter::with_capacity(32768, file))
                } else {
                    None
                };
                if let Some(file) = &mut profile {
                    let _ = writeln!(file, "sequence\tslot\tqueue_ns\tdisplay_ns\tsuccess");
                }
                loop {
                    let job_result = jobs.recv();
                    let Ok(job) = job_result else {
                        break;
                    };
                    let started = job.queued_at.map(|_| Instant::now());
                    let result = present(job.slot, job.region).map(|()| job.sequence);
                    let failed = result.is_err();
                    let finished = started.map(|_| Instant::now());
                    // Publish the completion/failure before waking. The owner
                    // must poll exact progress; a wake itself never retires an image.
                    let reply = responses.send(result);
                    if reply.is_ok() {
                        wake.signal();
                    }
                    if let (Some(file), Some(queued), Some(start), Some(end)) =
                        (&mut profile, job.queued_at, started, finished)
                    {
                        let written = writeln!(
                            file,
                            "{}\t{}\t{}\t{}\t{}",
                            job.sequence,
                            job.slot,
                            start.duration_since(queued).as_nanos(),
                            end.duration_since(start).as_nanos(),
                            u8::from(!failed)
                        );
                        if written.is_err() || (job.sequence % 60 == 0 && file.flush().is_err()) {
                            profile = None;
                        }
                    }
                    if reply.is_err() || failed {
                        break;
                    }
                }
            })
            .map_err(|_| "Failed to start display presentation worker")?;
        Ok(Self {
            sender: Some(sender),
            retired,
            worker: Some(worker),
            progress: PresentationProgress::default(),
            profiling,
        })
    }

    pub(super) fn present(
        &mut self,
        sequence: u64,
        slot: usize,
        region: Option<R>,
    ) -> Result<(), &'static str> {
        self.poll()?;
        let sender = self.sender.as_ref().ok_or("Display presentation stopped")?;
        self.progress.submit(sequence)?;
        let result = sender
            .try_send(Present {
                sequence,
                slot,
                region,
                queued_at: self.profiling.then(Instant::now),
            })
            .map_err(|_| "Display presentation stopped");
        result
    }

    fn poll(&mut self) -> Result<(), &'static str> {
        loop {
            let result = self.retired.try_recv();
            match result {
                Ok(result) => self.progress.retire(result?)?,
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => return Err("Display presentation stopped"),
            }
        }
    }

    /// Wait until every accepted flip has finished on the physical display.
    pub(super) fn drain(&mut self) -> Result<(), &'static str> {
        while self.progress.completed < self.progress.submitted {
            let result = self
                .retired
                .recv()
                .map_err(|_| "Display presentation stopped")?;
            self.progress.retire(result?)?;
        }
        Ok(())
    }

    /// An image last presented at N is writable only after a later completed
    /// flip. Queue admission and that image's own flip are not retirement.
    pub(super) fn ready(&mut self, last_present: Option<u64>) -> Result<bool, &'static str> {
        self.poll()?;
        Ok(self.progress.ready(last_present))
    }
}

impl<R> Drop for PresentationQueue<R> {
    fn drop(&mut self) {
        // Drain already accepted flips before releasing session images or
        // installing a replacement compositor. At most two replies remain,
        // so the three-entry reply queue cannot block teardown.
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Only owned ABI receipts cross threads. Session, resource table and leases
/// remain on the compositor thread until this owner reports proven retirement.
pub(super) struct GpuCompletionQueue<S: sgfx::backend::Completion + Send + 'static> {
    sender: Option<SyncSender<(u64, Vec<S>, Instant)>>,
    completed: Receiver<(u64, Result<std::time::Duration, &'static str>)>,
    worker: Option<JoinHandle<()>>,
    sequence: u64,
    pending: Option<u64>,
}

impl<S: sgfx::backend::Completion + Send + 'static> GpuCompletionQueue<S> {
    pub(super) fn new(wake: fn()) -> Result<Self, &'static str> {
        let (sender, jobs) = mpsc::sync_channel::<(u64, Vec<S>, Instant)>(1);
        let (results, completed) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("sws-gpu-wait".into())
            .spawn(move || {
                use sgfx::backend::{Completion, CompletionStatus};
                while let Ok((sequence, receipts, started)) = jobs.recv() {
                    let mut result = Ok(());
                    for receipt in receipts {
                        match receipt.wait(None) {
                            Ok(CompletionStatus::Complete) => {}
                            Ok(_) => {
                                result = Err("GPU completion did not prove retirement");
                                break;
                            }
                            Err(_) => {
                                result = Err("GPU composition completion failed");
                                break;
                            }
                        }
                    }
                    let failed = result.is_err();
                    // Publish before waking: a final static frame must retire even
                    // if no more input or client commits ever arrive.
                    if results
                        .send((sequence, result.map(|()| started.elapsed())))
                        .is_err()
                    {
                        break;
                    }
                    wake();
                    if failed {
                        break;
                    }
                }
            })
            .map_err(|_| "Failed to start GPU completion worker")?;
        Ok(Self {
            sender: Some(sender),
            completed,
            worker: Some(worker),
            sequence: 0,
            pending: None,
        })
    }

    pub(super) fn submit(&mut self, receipts: Vec<S>) -> Result<(), &'static str> {
        if self.pending.is_some() {
            return Self::reject_handoff(receipts, "GPU composition already pending");
        }
        let Some(sequence) = self.sequence.checked_add(1) else {
            return Self::reject_handoff(receipts, "GPU frame sequence exhausted");
        };
        let Some(sender) = self.sender.as_ref() else {
            return Self::reject_handoff(receipts, "GPU completion worker stopped");
        };
        if let Err(error) = sender.try_send((sequence, receipts, Instant::now())) {
            let (_, receipts, _) = match error {
                mpsc::TrySendError::Full(job) | mpsc::TrySendError::Disconnected(job) => job,
            };
            return Self::reject_handoff(receipts, "GPU completion worker handoff failed");
        }

        self.sequence = sequence;
        self.pending = Some(sequence);
        Ok(())
    }

    pub(super) fn submit_to(
        worker: Option<&mut Self>,
        receipts: Vec<S>,
    ) -> Result<(), &'static str> {
        match worker {
            Some(worker) => worker.submit(receipts),
            None => Self::reject_handoff(receipts, "GPU completion worker unavailable"),
        }
    }

    fn reject_handoff(receipts: Vec<S>, reason: &'static str) -> Result<(), &'static str> {
        // These receipts already own accepted GPU work. Even an invariant or
        // shutdown error cannot turn that work into a side-effect-free Busy.
        use sgfx::backend::CompletionStatus;
        for receipt in receipts {
            if !matches!(receipt.wait(None), Ok(CompletionStatus::Complete)) {
                return Err("GPU handoff failure did not prove retirement");
            }
        }
        Err(reason)
    }

    pub(super) fn poll(&mut self) -> Result<Option<std::time::Duration>, &'static str> {
        let Some(expected) = self.pending else {
            return Ok(None);
        };
        match self.completed.try_recv() {
            Ok((sequence, result)) => {
                if sequence != expected {
                    return Err("Invalid GPU completion sequence");
                }
                let elapsed = result?;
                self.pending = None;
                Ok(Some(elapsed))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err("GPU completion worker stopped"),
        }
    }
}

impl<S: sgfx::backend::Completion + Send + 'static> Drop for GpuCompletionQueue<S> {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod display_completion_tests {
    use super::*;
    use std::time::Duration;

    static WAKE: Mutex<Option<mpsc::Sender<()>>> = Mutex::new(None);
    fn wake() {
        if let Some(sender) = WAKE.lock().unwrap().as_ref() {
            let _ = sender.send(());
        }
    }

    #[test]
    fn drain_before_disable_finishes_the_armed_and_queued_flips() {
        let (finish, flips) = mpsc::channel();
        let mut queue = PresentationQueue::<()>::with_presenter(
            CompletionWake::default(),
            false,
            move |slot, _| {
                assert_eq!(flips.recv().unwrap(), slot);
                Ok(())
            },
        )
        .unwrap();
        queue.present(1, 0, None).unwrap();
        queue.present(2, 1, None).unwrap();
        finish.send(0).unwrap();
        finish.send(1).unwrap();
        queue.drain().unwrap();
        assert_eq!(queue.progress.completed, 2);
        assert!(queue.ready(Some(1)).unwrap());
    }

    #[test]
    fn completed_flip_wakes_after_reply_without_releasing_its_own_front() {
        let (wakes, awake) = mpsc::channel();
        *WAKE.lock().unwrap() = Some(wakes);
        let hook = CompletionWake::default();
        let (entered, worker_entered) = mpsc::channel();
        let (finish, worker_finish) = mpsc::channel::<bool>();
        let mut queue =
            PresentationQueue::<()>::with_presenter(hook.clone(), false, move |slot, _| {
                entered.send(slot).unwrap();
                if worker_finish.recv().unwrap() {
                    Ok(())
                } else {
                    Err("test display failure")
                }
            })
            .unwrap();
        // Prepare may run after worker construction. Its shared hook must reach
        // that already-created worker, without recreating the display queue.
        hook.set(wake);
        queue.present(1, 0, None).unwrap();
        worker_entered.recv_timeout(Duration::from_secs(2)).unwrap();
        queue.present(2, 1, None).unwrap();
        assert!(
            !queue.ready(None).unwrap(),
            "armed plus queued capacity stays two"
        );
        assert!(
            queue.present(3, 2, None).is_err(),
            "a full queue cannot admit a third flip"
        );
        assert!(matches!(awake.try_recv(), Err(TryRecvError::Empty)));
        finish.send(true).unwrap();
        awake.recv_timeout(Duration::from_secs(2)).unwrap();
        // No new input, frame grant, polling loop or timer was required. Seeing
        // this wake must already make the completion available to ready().
        assert!(queue.ready(None).unwrap());
        assert!(
            !queue.ready(Some(1)).unwrap(),
            "N's own flip does not retire image N"
        );
        worker_entered.recv_timeout(Duration::from_secs(2)).unwrap();
        finish.send(true).unwrap();
        awake.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            queue.ready(Some(1)).unwrap(),
            "only N+1 completion frees image N"
        );
        queue.present(3, 2, None).unwrap();
        worker_entered.recv_timeout(Duration::from_secs(2)).unwrap();
        finish.send(false).unwrap();
        awake.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(queue.ready(None), Err("test display failure"));
        drop(queue);
        *WAKE.lock().unwrap() = None;
    }
}

#[cfg(test)]
mod gpu_completion_tests {
    use super::*;
    use sgfx::backend::{Completion, CompletionStatus};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    static WAKE: Mutex<Option<mpsc::Sender<()>>> = Mutex::new(None);
    fn wake() {
        if let Some(sender) = WAKE.lock().unwrap().as_ref() {
            let _ = sender.send(());
        }
    }
    struct Receipt(
        Arc<(Mutex<Option<bool>>, Condvar)>,
        Arc<core::sync::atomic::AtomicUsize>,
    );
    impl Completion for Receipt {
        type Error = &'static str;
        fn poll(&self) -> Result<CompletionStatus, Self::Error> {
            match *self.0.0.lock().unwrap() {
                Some(true) => Ok(CompletionStatus::Complete),
                Some(false) => Err("GPU fault"),
                None => Ok(CompletionStatus::Pending),
            }
        }
        fn wait(&self, _: Option<Duration>) -> Result<CompletionStatus, Self::Error> {
            self.1.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
            let mut state = self.0.0.lock().unwrap();
            while state.is_none() {
                state = self.0.1.wait(state).unwrap();
            }
            if state.unwrap() {
                Ok(CompletionStatus::Complete)
            } else {
                Err("GPU fault")
            }
        }
    }

    #[test]
    fn replacement_worker_drains_old_receipts_and_missing_worker_never_drops_accepted_work() {
        struct GatedReceipt {
            entered: mpsc::Sender<()>,
            finish: Mutex<mpsc::Receiver<()>>,
        }
        impl Completion for GatedReceipt {
            type Error = &'static str;
            fn poll(&self) -> Result<CompletionStatus, Self::Error> {
                Ok(CompletionStatus::Pending)
            }
            fn wait(&self, _: Option<Duration>) -> Result<CompletionStatus, Self::Error> {
                self.entered.send(()).unwrap();
                self.finish.lock().unwrap().recv().unwrap();
                Ok(CompletionStatus::Complete)
            }
        }
        fn gated() -> (GatedReceipt, mpsc::Receiver<()>, mpsc::Sender<()>) {
            let (entered, waiting) = mpsc::channel();
            let (complete, finish) = mpsc::channel();
            (
                GatedReceipt {
                    entered,
                    finish: Mutex::new(finish),
                },
                waiting,
                complete,
            )
        }
        let (receipt, waiting, complete) = gated();
        let mut old = GpuCompletionQueue::new(|| {}).unwrap();
        GpuCompletionQueue::submit_to(Some(&mut old), vec![receipt]).unwrap();
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        // Production target construction creates a NEW queue before replacing
        // the old target. Dropping the old queue must still join accepted work.
        let mut replacement = GpuCompletionQueue::new(|| {}).unwrap();
        let (retired, retirement) = mpsc::channel();
        let dropping = thread::spawn(move || {
            drop(old);
            retired.send(()).unwrap();
        });
        assert!(matches!(retirement.try_recv(), Err(TryRecvError::Empty)));
        complete.send(()).unwrap();
        retirement.recv_timeout(Duration::from_secs(2)).unwrap();
        dropping.join().unwrap();
        let (receipt, waiting, complete) = gated();
        GpuCompletionQueue::submit_to(Some(&mut replacement), vec![receipt]).unwrap();
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(replacement.poll().unwrap().is_none());
        complete.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while replacement.poll().unwrap().is_none() {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        drop(replacement);
        // The exact journal failure used to drop these accepted receipts via
        // Option::ok_or before the worker handoff. Its rejection must retire
        // them too, rather than report a side-effect-free admission failure.
        let (receipt, waiting, complete) = gated();
        let (rejected, rejection) = mpsc::channel();
        let handoff = thread::spawn(move || {
            rejected
                .send(GpuCompletionQueue::submit_to(None, vec![receipt]))
                .unwrap();
        });
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(rejection.try_recv(), Err(TryRecvError::Empty)));
        complete.send(()).unwrap();
        assert_eq!(
            rejection.recv_timeout(Duration::from_secs(2)).unwrap(),
            Err("GPU completion worker unavailable")
        );
        handoff.join().unwrap();
    }

    #[test]
    fn owned_receipt_wait_is_nonblocking_and_wakes_without_new_input_even_on_fault() {
        let (sender, receiver) = mpsc::channel();
        *WAKE.lock().unwrap() = Some(sender);
        let pending = Arc::new((Mutex::new(None), Condvar::new()));
        let complete = Arc::new((Mutex::new(Some(true)), Condvar::new()));
        let mut queue = GpuCompletionQueue::<Receipt>::new(wake).unwrap();
        let waits = Arc::new(core::sync::atomic::AtomicUsize::new(0));
        queue
            .submit(vec![
                Receipt(pending.clone(), waits.clone()),
                Receipt(complete, waits.clone()),
            ])
            .unwrap();
        let rejected_waits = Arc::new(core::sync::atomic::AtomicUsize::new(0));
        assert!(
            queue
                .submit(vec![Receipt(
                    Arc::new((Mutex::new(Some(true)), Condvar::new())),
                    rejected_waits.clone()
                )])
                .is_err()
        );
        assert_eq!(rejected_waits.load(core::sync::atomic::Ordering::SeqCst), 1);
        assert!(queue.poll().unwrap().is_none());
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
        // The caller can dispatch input instead of waiting for the first fence.
        let dispatched_input = 1;
        *pending.0.lock().unwrap() = Some(true);
        pending.1.notify_all();
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(queue.poll().unwrap().is_some());
        assert_eq!(dispatched_input, 1);
        let fault = Arc::new((Mutex::new(Some(false)), Condvar::new()));
        queue.submit(vec![Receipt(fault, waits)]).unwrap();
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(queue.poll().is_err());
        drop(queue);
        *WAKE.lock().unwrap() = None;
    }
}
