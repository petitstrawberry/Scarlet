//! Bounded display handoff, independent of GPU rendering completion.
//!
//! The worker calls the existing synchronous display ABI. Its reply therefore
//! proves that the old front has retired; queue admission alone never does.

use framebuffer::{DisplayPresentRegion, DisplaySurface};
use scarlet_os::handle::Handle;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::thread::{self, JoinHandle};

struct Present {
    sequence: u64,
    slot: usize,
    region: Option<DisplayPresentRegion>,
}

pub(super) struct PresentationQueue {
    sender: Option<SyncSender<Present>>,
    retired: Receiver<Result<u64, &'static str>>,
    worker: Option<JoinHandle<()>>,
    completed: u64,
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
        let worker = thread::Builder::new()
            .name("sws-present".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let result = images
                        .get(job.slot)
                        .ok_or("Invalid presentation slot")
                        .and_then(|image| {
                            presenter
                                .present_swapchain_image(image, job.region)
                                .map_err(|_| "Display page flip failed")
                        })
                        .map(|()| job.sequence);
                    let failed = result.is_err();
                    if responses.send(result).is_err() || failed {
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
        })
    }

    pub(super) fn present(
        &mut self,
        sequence: u64,
        slot: usize,
        region: Option<DisplayPresentRegion>,
    ) -> Result<(), &'static str> {
        self.poll()?;
        self.sender
            .as_ref()
            .ok_or("Display presentation stopped")?
            .send(Present {
                sequence,
                slot,
                region,
            })
            .map_err(|_| "Display presentation stopped")
    }

    fn poll(&mut self) -> Result<(), &'static str> {
        loop {
            match self.retired.try_recv() {
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
            self.completed = self
                .retired
                .recv()
                .map_err(|_| "Display presentation stopped")??;
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
