//! VideoToolbox backend for Scarlet's mapped video sessions.

mod access_unit;
mod toolbox;

use scarlet::{
    device::video::{
        SCARLET_VIDEO_FORMAT_AV1, SCARLET_VIDEO_FORMAT_H264, SCARLET_VIDEO_FORMAT_HEVC,
        SCARLET_VIDEO_PIXEL_FORMAT_NV12, ScarletVideoDequeuedFrame, VideoBackendCapabilities,
        VideoBackendDecodeRequest, VideoBackendDecodedFrame, VideoCompletionNotifier,
        VideoDecodeBackend, register_video_backend, register_video_decode_device,
    },
    environment::PAGE_SIZE,
    interrupt::{InterruptClaim, InterruptId, InterruptManager, InterruptResult, InterruptSource},
    sync::IrqSpinLock,
};
use std::sync::{
    Arc, Weak,
    atomic::{AtomicU64, Ordering},
    mpsc,
};
use toolbox::{Codec, Decoder, Pixels};

const MAX_SESSIONS: usize = 4;
const MAX_INPUT: usize = 8 * 1024 * 1024;
const MAX_OUTPUT: usize = 16 * 1024 * 1024;
const CPU: u32 = 0;
pub const COMPLETION_IRQ: InterruptId = 20;

scarlet::late_initcall!(register_decoder);

/// Retain the BSP's late video initcall in the executable.
pub fn link_boot_decoder() {
    std::hint::black_box(&__LATE_INITCALL__);
}

fn register_decoder() {
    match VideoToolbox::attach() {
        Ok(backend) => {
            let backend = Arc::new(backend);
            register_video_backend(backend.clone());
            register_video_decode_device(backend);
        }
        Err(error) => scarlet::println!("[videotoolbox] {}", error),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SessionKey {
    slot: usize,
    epoch: u64,
}

struct Job {
    key: SessionKey,
    codec: Codec,
    coded: Vec<u8>,
    output_limit: usize,
}

enum Command {
    Decode(Job),
    Retire,
}

struct Reply {
    key: SessionKey,
    result: Result<Option<Pixels>, &'static str>,
}

struct Pending {
    request: VideoBackendDecodeRequest,
    result: Option<Result<Option<Pixels>, &'static str>>,
}

struct Session {
    key: SessionKey,
    codec: Codec,
    pending: Option<Pending>,
}

#[derive(Default)]
struct Slot {
    next_epoch: u64,
    session: Option<Session>,
}

struct Frontend {
    slots: [IrqSpinLock<Slot>; MAX_SESSIONS],
    live: Arc<[AtomicU64; MAX_SESSIONS]>,
    replies: IrqSpinLock<mpsc::Receiver<Reply>>,
    notifier: IrqSpinLock<Option<Weak<dyn VideoCompletionNotifier>>>,
}

impl Frontend {
    fn ingest(&self) {
        let replies = self.replies.lock();
        for reply in replies.try_iter() {
            let mut slot = self.slots[reply.key.slot].lock();
            if let Some(session) = slot.session.as_mut()
                && session.key == reply.key
                && let Some(pending) = session.pending.as_mut()
            {
                pending.result = Some(reply.result);
            }
        }
    }
}

struct Completion {
    frontend: Weak<Frontend>,
}

impl InterruptSource for Completion {
    fn interrupt_id(&self) -> Option<InterruptId> {
        Some(COMPLETION_IRQ)
    }

    fn claim_interrupt(&self) -> InterruptResult<InterruptClaim> {
        let Some(frontend) = self.frontend.upgrade() else {
            return Ok(InterruptClaim::NotMine);
        };
        frontend.ingest();
        let notifier = frontend.notifier.lock().as_ref().and_then(Weak::upgrade);
        if let Some(notifier) = notifier {
            notifier.notify_video_completion();
        }
        Ok(InterruptClaim::Handled)
    }
}

struct VideoToolbox {
    frontend: Arc<Frontend>,
    commands: mpsc::SyncSender<Command>,
    caps: VideoBackendCapabilities,
}

impl VideoToolbox {
    fn attach() -> Result<Self, String> {
        let (commands, jobs) = mpsc::sync_channel(2 * MAX_SESSIONS);
        // Only the bounded decode command queue can produce replies. Every
        // submit first drains earlier replies, including cancelled jobs.
        let (completion, replies) = mpsc::channel();
        let live = Arc::new(std::array::from_fn(|_| AtomicU64::new(0)));
        let frontend = Arc::new(Frontend {
            slots: std::array::from_fn(|_| IrqSpinLock::new(Slot::default())),
            live: live.clone(),
            replies: IrqSpinLock::new(replies),
            notifier: IrqSpinLock::new(None),
        });
        let source: Arc<dyn InterruptSource> = Arc::new(Completion {
            frontend: Arc::downgrade(&frontend),
        });
        let interrupts = InterruptManager::global();
        interrupts
            .register_interrupt_source(COMPLETION_IRQ, source.clone())
            .map_err(|error| error.to_string())?;
        if let Err(error) = interrupts.enable_external_interrupt(COMPLETION_IRQ, CPU) {
            interrupts.unregister_interrupt_source(COMPLETION_IRQ, &source);
            return Err(error.to_string());
        }
        if let Err(error) = std::thread::Builder::new()
            .name("scarlet-video-decode".into())
            .spawn(move || run_decoder(jobs, completion, live))
        {
            let _ = interrupts.disable_external_interrupt(COMPLETION_IRQ, CPU);
            interrupts.unregister_interrupt_source(COMPLETION_IRQ, &source);
            return Err(error.to_string());
        }

        let caps = VideoBackendCapabilities {
            max_sessions: MAX_SESSIONS as u32,
            max_inflight_decodes: MAX_SESSIONS as u32,
            mapped_input_len: MAX_INPUT as u32,
            mapped_output_len: MAX_OUTPUT as u32,
            output_pixel_format: SCARLET_VIDEO_PIXEL_FORMAT_NV12,
            supports_h264: true,
            supports_hevc: true,
            supports_av1: toolbox::hardware_supported(Codec::Av1),
            supports_stateless_h264: false,
        };
        Ok(Self {
            frontend,
            commands,
            caps,
        })
    }

    fn slot(&self, stream_id: u32) -> Result<&IrqSpinLock<Slot>, &'static str> {
        let index = stream_id
            .checked_sub(1)
            .ok_or("Invalid VideoToolbox session")?;
        self.frontend
            .slots
            .get(index as usize)
            .ok_or("Invalid VideoToolbox session")
    }
}

impl VideoDecodeBackend for VideoToolbox {
    fn name(&self) -> &'static str {
        "videotoolbox"
    }

    fn capabilities(&self) -> VideoBackendCapabilities {
        self.caps
    }

    fn supports_variable_mapped_buffers(&self) -> bool {
        true
    }

    fn set_completion_notifier(&self, notifier: Option<Weak<dyn VideoCompletionNotifier>>) {
        *self.frontend.notifier.lock() = notifier;
    }

    fn create_session(&self, coded_format: u32) -> Result<u32, &'static str> {
        let codec = match coded_format {
            SCARLET_VIDEO_FORMAT_H264 => Codec::H264,
            SCARLET_VIDEO_FORMAT_HEVC => Codec::Hevc,
            SCARLET_VIDEO_FORMAT_AV1 if self.caps.supports_av1 => Codec::Av1,
            _ => return Err("Unsupported VideoToolbox codec"),
        };
        self.frontend.ingest();
        for (index, slot) in self.frontend.slots.iter().enumerate() {
            let mut slot = slot.lock();
            if slot.session.is_some() {
                continue;
            }
            let epoch = slot
                .next_epoch
                .checked_add(1)
                .ok_or("Video session epoch exhausted")?;
            slot.next_epoch = epoch;
            slot.session = Some(Session {
                key: SessionKey { slot: index, epoch },
                codec,
                pending: None,
            });
            self.frontend.live[index].store(epoch, Ordering::Release);
            return Ok(index as u32 + 1);
        }
        Err("No free VideoToolbox session")
    }

    fn destroy_session(&self, stream_id: u32) -> Result<(), &'static str> {
        self.frontend.ingest();
        let mut slot = self.slot(stream_id)?.lock();
        if let Some(session) = slot.session.take() {
            self.frontend.live[session.key.slot].store(0, Ordering::Release);
            // Pending contains the only retained guest-buffer lease. The host
            // owns an independent copy and can finish retiring it in parallel.
            drop(slot);
            drop(session);
            let _ = self.commands.try_send(Command::Retire);
        }
        Ok(())
    }

    fn submit_decode(&self, request: &VideoBackendDecodeRequest) -> Result<(), &'static str> {
        self.frontend.ingest();
        validate_request(request)?;
        let mut slot = self.slot(request.stream_id)?.lock();
        let session = slot
            .session
            .as_mut()
            .ok_or("VideoToolbox session is closed")?;
        if session.codec.scarlet_format() != request.coded_format || session.pending.is_some() {
            return Err("VideoToolbox session is busy or has a different codec");
        }

        let mut coded = vec![0; request.input_len as usize];
        // SAFETY: The validated input range belongs to the live request lease.
        // Only the owned copy goes to the host service.
        unsafe {
            std::ptr::copy_nonoverlapping(
                request.input_vaddr as *const u8,
                coded.as_mut_ptr(),
                coded.len(),
            );
        }
        let job = Job {
            key: session.key,
            codec: session.codec,
            coded,
            output_limit: request.output_len as usize,
        };
        session.pending = Some(Pending {
            request: request.clone(),
            result: None,
        });
        if self.commands.try_send(Command::Decode(job)).is_err() {
            session.pending = None;
            return Err("VideoToolbox command queue is full or closed");
        }
        Ok(())
    }

    fn dequeue_frame(
        &self,
        stream_id: u32,
    ) -> Result<Option<VideoBackendDecodedFrame>, &'static str> {
        self.frontend.ingest();
        let mut slot = self.slot(stream_id)?.lock();
        let session = slot
            .session
            .as_mut()
            .ok_or("VideoToolbox session is closed")?;
        let Some(pending) = session.pending.as_mut() else {
            return Ok(None);
        };
        let Some(result) = pending.result.take() else {
            return Ok(None);
        };
        let request = session.pending.take().unwrap().request;
        let mut frame = ScarletVideoDequeuedFrame {
            timestamp: request.timestamp,
            pixel_format: SCARLET_VIDEO_PIXEL_FORMAT_NV12,
            payload_offset: request.output_offset,
            ..Default::default()
        };
        if let Some(pixels) = result? {
            // The slot serializes this write against destroy and reuse. The
            // validated request retains the mapped pages through completion.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    pixels.bytes.as_ptr(),
                    request.output_vaddr as *mut u8,
                    pixels.bytes.len(),
                );
            }
            frame.width = pixels.width;
            frame.height = pixels.height;
            frame.payload_len = pixels.bytes.len() as u32;
        }
        Ok(Some(VideoBackendDecodedFrame {
            image: None,
            stream_id,
            frame,
        }))
    }
}

fn validate_request(request: &VideoBackendDecodeRequest) -> Result<(), &'static str> {
    let lease = request
        .buffer_lease
        .as_ref()
        .ok_or("Video request has no buffer lease")?;
    let base = lease.as_vaddr();
    let size = lease
        .len()
        .checked_mul(PAGE_SIZE)
        .ok_or("Invalid video buffer lease")?;
    let contained = |address: usize, len: u32, limit: usize| {
        let len = len as usize;
        len != 0
            && len <= limit
            && address
                .checked_sub(base)
                .and_then(|offset| offset.checked_add(len))
                .is_some_and(|end| end <= size)
    };
    if request.shared_output
        || !contained(request.input_vaddr, request.input_len, MAX_INPUT)
        || !contained(request.output_vaddr, request.output_len, MAX_OUTPUT)
        || request.output_vaddr.checked_sub(base) != usize::try_from(request.output_offset).ok()
    {
        return Err("Video request does not fit its mapped buffers");
    }
    Ok(())
}

fn run_decoder(
    jobs: mpsc::Receiver<Command>,
    replies: mpsc::Sender<Reply>,
    live: Arc<[AtomicU64; MAX_SESSIONS]>,
) {
    let mut decoders: [Option<(SessionKey, Decoder)>; MAX_SESSIONS] = std::array::from_fn(|_| None);
    let current = |key: SessionKey| live[key.slot].load(Ordering::Acquire) == key.epoch;
    while let Ok(command) = jobs.recv() {
        for decoder in &mut decoders {
            if decoder.as_ref().is_some_and(|(key, _)| !current(*key)) {
                *decoder = None;
            }
        }
        let Command::Decode(job) = command else {
            continue;
        };
        if !current(job.key) {
            continue;
        }
        let slot = &mut decoders[job.key.slot];
        if slot.as_ref().is_none_or(|(key, _)| *key != job.key) {
            *slot = Some((job.key, Decoder::new(job.codec)));
        }
        let result = slot
            .as_mut()
            .unwrap()
            .1
            .decode(&job.coded, job.output_limit);
        if current(job.key) {
            if replies
                .send(Reply {
                    key: job.key,
                    result,
                })
                .is_err()
            {
                return;
            }
            let _ = scarlet::hosted::raise_external_interrupt(CPU, COMPLETION_IRQ);
        } else {
            *slot = None;
        }
    }
}
