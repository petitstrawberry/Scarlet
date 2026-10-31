//! Bounded display handoff, independent of GPU rendering completion.
//!
//! The worker calls the existing synchronous display ABI. Its reply therefore
//! proves that the old front has retired; queue admission alone never does.

use framebuffer::{DisplayPresentRegion, DisplaySurface};
use scarlet_os::handle::Handle;
use std::io::{BufWriter, Write};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Instant;

struct Present {
    sequence: u64,
    slot: usize,
    region: Option<DisplayPresentRegion>,
    queued_at: Option<Instant>,
}

pub(super) struct PresentationQueue {
    sender: Option<SyncSender<Present>>,
    retired: Receiver<Result<u64, &'static str>>,
    worker: Option<JoinHandle<()>>,
    completed: u64,
    profiling: bool,
}

impl PresentationQueue {
    pub(super) fn new(display: &DisplaySurface, images: Vec<Handle>) -> Result<Self, &'static str> {
        let presenter = display
            .presenter()
            .map_err(|_| "Failed to retain display presenter")?;
        // One armed flip plus one queued image. The third swapchain slot may
        // be rendered only after a later completed flip released its old use.
        let (sender, jobs) = mpsc::sync_channel::<Present>(1);
        let (responses, retired) = mpsc::sync_channel(3);
        let profiling = std::env::var("SWS_FRAME_PROFILE").is_ok_and(|value| value == "1");
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
                    let result = images
                        .get(job.slot)
                        .ok_or("Invalid presentation slot")
                        .and_then(|image| {
                            let result =
                                presenter.present_completed_swapchain_image(image, job.region);
                            result.map_err(|_| "Display page flip failed")
                        })
                        .map(|()| job.sequence);
                    let failed = result.is_err();
                    let finished = started.map(|_| Instant::now());
                    let reply = responses.send(result);
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
            completed: 0,
            profiling,
        })
    }

    pub(super) fn present(
        &mut self,
        sequence: u64,
        slot: usize,
        region: Option<DisplayPresentRegion>,
    ) -> Result<(), &'static str> {
        self.poll()?;
        let sender = self.sender.as_ref().ok_or("Display presentation stopped")?;
        let result = sender
            .send(Present {
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
                Ok(result) => self.completed = result?,
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => return Err("Display presentation stopped"),
            }
        }
    }

    /// An image last presented at N is writable only after another image at
    /// N+1 (or later) has actually reached scanout. Its own fence is not enough.
    pub(super) fn wait_after(&mut self, sequence: u64) -> Result<(), &'static str> {
        self.poll()?;
        while self.completed <= sequence {
            let result = self.retired.recv();
            self.completed = result.map_err(|_| "Display presentation stopped")??;
        }
        Ok(())
    }
}

impl Drop for PresentationQueue {
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
