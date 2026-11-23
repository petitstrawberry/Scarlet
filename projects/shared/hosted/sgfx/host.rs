//! The SGFX execution thread owns the device context, its images and its sessions.
use super::{
    Config, Endpoint,
    scale::ScanoutScaler,
    session::{ResourceRead, Session},
    transport::{Command, Inbox, Rect, Request, ResultBytes},
};
use sgfx_backend_wgpu as sgfx;
use std::{
    collections::BTreeMap,
    sync::{Arc, mpsc},
};

pub(super) type Images = BTreeMap<u64, Arc<sgfx::Image>>;
type Retirement = (u64, ResultBytes, Option<wgpu::Maintain>);

pub struct Host {
    config: Config,
    gpu: sgfx::Context,
    scaler: std::cell::OnceCell<ScanoutScaler>,
    images: Images,
    sessions: BTreeMap<u64, Session>,
    requests: Option<mpsc::Receiver<Request>>,
    // FIFO retirement preserves submission and lease ordering without waiting
    // for Metal on the command dispatch thread.
    retirement: Option<mpsc::Sender<Retirement>>,
    retirement_thread: Option<std::thread::JoinHandle<()>>,
}
impl Host {
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        config: Config,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<(Self, Endpoint), String> {
        Self::open(device, queue, config, wake, true)
    }
    fn open(
        device: wgpu::Device,
        queue: wgpu::Queue,
        config: Config,
        wake: impl Fn() + Send + Sync + 'static,
        presentation: bool,
    ) -> Result<(Self, Endpoint), String> {
        let (endpoint, requests, inbox) = Endpoint::pair(config.irq, presentation, wake);
        let (retirement, pending) = mpsc::channel();
        let fence_device = device.clone();
        let retirement_thread = std::thread::Builder::new()
            .name("scarlet-metal-completion".into())
            .spawn(move || retire_submissions(fence_device, inbox, pending))
            .map_err(|error| error.to_string())?;
        Ok((
            Self {
                config,
                gpu: sgfx::Device::new(device, queue).create_context(),
                scaler: Default::default(),
                images: Images::new(),
                sessions: BTreeMap::new(),
                requests: Some(requests),
                retirement: Some(retirement),
                retirement_thread: Some(retirement_thread),
            },
            endpoint,
        ))
    }
    /// Process one event-loop turn. True means a present was submitted, not
    /// completed. Replies retire the source read after queue completion.
    pub fn dispatch(&mut self, scanout: &wgpu::Texture) -> bool {
        // Each request has one corresponding wake on the host's owning thread.
        self.requests
            .as_ref()
            .unwrap()
            .try_recv()
            .is_ok_and(|request| self.handle(request, Some(scanout)))
    }
    fn handle(&mut self, request: Request, scanout: Option<&wgpu::Texture>) -> bool {
        if let Some(trace) = self.config.trace {
            let fields = (|| {
                let (event, w, h) = match &request.command {
                    Command::Execute { words, .. } => {
                        return Some((
                            "gpu_execute_receive",
                            words.first().copied().unwrap_or(0),
                            std::mem::size_of_val(words.as_slice()) as u64,
                        ));
                    }
                    Command::CreateImage(_, w, h) => ("gpu_create_receive", *w, *h),
                    Command::Present(_, r) => ("gpu_present_receive", r.width, r.height),
                    Command::Upload(_, r, _) => ("gpu_upload_receive", r.width, r.height),
                    Command::Readback(_, r) => ("gpu_readback_receive", r.width, r.height),
                    _ => return None,
                };
                Some((event, u64::from(w), u64::from(h)))
            })();
            trace(request.id, fields);
        }
        let present = matches!(request.command, Command::Present(..));
        let mut wait = None;
        let result = self.execute(request.command, scanout, &mut wait);
        let presented = present && result.is_ok();
        self.retirement
            .as_ref()
            .unwrap()
            .send((request.id, result, wait))
            .expect("Metal completion thread stopped before shutdown");
        presented
    }
    fn execute(
        &mut self,
        command: Command,
        scanout: Option<&wgpu::Texture>,
        wait: &mut Option<wgpu::Maintain>,
    ) -> ResultBytes {
        match command {
            Command::CreateImage(id, w, h) => {
                let image = self
                    .gpu
                    .create_image(w, h, sgfx_core::ir::TextureFormat::Bgra8Unorm)
                    .map_err(|_| "Cannot create SGFX image")?;
                self.images.insert(id, image);
            }
            Command::CreateQueue(id) => {
                self.sessions.insert(id, Session::new(&self.gpu));
            }
            Command::DropQueue(id) => {
                self.sessions.remove(&id);
            }
            Command::DropImage(id) => {
                self.images.remove(&id);
            }
            Command::Execute {
                queue,
                words,
                attachments,
            } => {
                // Session::submit does not expose its submission index. Keep the
                // conservative fence for recordings (including partial failures)
                // and for an empty ordered checkpoint. These metadata and packet
                // assembly opcodes cannot submit work on the pinned backend.
                if !matches!(
                    words.first().copied(),
                    Some(
                        super::wire::START
                            | super::wire::APPEND
                            | super::wire::SNAPSHOT
                            | super::wire::MAP
                            | super::wire::UNMAP
                    )
                ) {
                    *wait = Some(wgpu::Maintain::Wait);
                }
                let session = self.sessions.get_mut(&queue).ok_or("Unknown SGFX queue")?;
                if let [
                    kind @ (super::wire::READ_BUFFER | super::wire::READ_TEXTURE),
                    slot,
                    offset,
                    len,
                    destination,
                ] = words.as_slice()
                {
                    let len = usize::try_from(*len).map_err(|_| "Invalid readback length")?;
                    if len == 0 || len > super::wire::MAX_READBACK {
                        return Err("Invalid readback length");
                    }
                    let target = self
                        .images
                        .get(destination)
                        .ok_or("Unknown readback image")?
                        .raw_texture();
                    let row = target.width() as usize * 4;
                    if len.div_ceil(row) != target.height() as usize {
                        return Err("Invalid readback extent");
                    }
                    match session.read_resource(
                        *kind,
                        u32::try_from(*slot).map_err(|_| "Invalid SGFX slot")?,
                        *offset,
                        len,
                        &attachments,
                        &self.images,
                        target,
                    )? {
                        ResourceRead::Texture(src) => {
                            let mut encoder = self
                                .gpu
                                .raw_device()
                                .create_command_encoder(&Default::default());
                            encoder.copy_texture_to_texture(
                                src.as_image_copy(),
                                target.as_image_copy(),
                                target.size(),
                            );
                            let index = self.gpu.raw_queue().submit([encoder.finish()]);
                            *wait = Some(wgpu::Maintain::WaitForSubmissionIndex(index));
                        }
                        ResourceRead::Bytes(mut bytes) => {
                            bytes.resize(row * target.height() as usize, 0);
                            self.gpu.raw_queue().write_texture(
                                target.as_image_copy(),
                                &bytes,
                                wgpu::TexelCopyBufferLayout {
                                    offset: 0,
                                    bytes_per_row: Some(row as u32),
                                    rows_per_image: None,
                                },
                                target.size(),
                            );
                            let index = self.gpu.raw_queue().submit([]);
                            *wait = Some(wgpu::Maintain::WaitForSubmissionIndex(index));
                        }
                    }
                } else if !words.is_empty() {
                    session.receive(&self.images, &attachments, words)?;
                }
            }
            Command::Readback(id, rect) => return self.readback(id, rect),
            Command::Upload(id, rect, bytes) => {
                let texture = self.texture(id)?;
                let src = rect.copy(texture)?;
                if bytes.len() != rect.width as usize * rect.height as usize * 4 {
                    return Err("Invalid SGFX upload length");
                }
                self.gpu.raw_queue().write_texture(
                    src,
                    &bytes,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(rect.width * 4),
                        rows_per_image: None,
                    },
                    rect.extent(),
                );
                // wgpu schedules write_texture on the next queue submission.
                let index = self.gpu.raw_queue().submit([]);
                *wait = Some(wgpu::Maintain::WaitForSubmissionIndex(index));
            }
            Command::Present(id, rect) => {
                let src = self.texture(id)?;
                let dst = scanout.ok_or("No Metal scanout")?;
                let device = self.gpu.raw_device();
                let mut encoder = device.create_command_encoder(&Default::default());
                if src.size() == dst.size() {
                    encoder.copy_texture_to_texture(
                        rect.copy(src)?,
                        rect.copy(dst)?,
                        rect.extent(),
                    );
                } else {
                    // Resize may race an accepted present. Validate source damage
                    // before converting it to destination coordinates.
                    rect.copy(src)?;
                    let damage = rect.scaled(src.size(), dst.size());
                    self.scaler
                        .get_or_init(|| ScanoutScaler::new(device))
                        .encode(device, &mut encoder, src, dst, damage);
                }
                let index = self.gpu.raw_queue().submit([encoder.finish()]);
                *wait = Some(wgpu::Maintain::WaitForSubmissionIndex(index));
            }
        }
        Ok(Vec::new())
    }
    fn texture(&self, id: u64) -> Result<&wgpu::Texture, &'static str> {
        Ok(self
            .images
            .get(&id)
            .ok_or("Unknown SGFX image")?
            .raw_texture())
    }
    fn readback(&self, id: u64, rect: Rect) -> ResultBytes {
        let source = rect.copy(self.texture(id)?)?;
        let row = rect.width * 4;
        let pitch = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let size = u64::from(rect.height) * u64::from(pitch);
        let device = self.gpu.raw_device();
        if size > device.limits().max_buffer_size {
            return Err("Readback exceeds device limit");
        }
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SGFX readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            source,
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(pitch),
                    rows_per_image: None,
                },
            },
            rect.extent(),
        );
        self.gpu.raw_queue().submit([encoder.finish()]);
        let slice = staging.slice(..);
        let (tx, rx) = mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .map_err(|_| "Readback interrupted")?
            .map_err(|_| "Metal readback failed")?;
        let mut bytes = Vec::with_capacity(row as usize * rect.height as usize);
        for src in slice.get_mapped_range().chunks_exact(pitch as usize) {
            bytes.extend_from_slice(&src[..row as usize]);
        }
        Ok(bytes)
    }
}
impl Rect {
    pub(super) fn scaled(self, from: wgpu::Extent3d, to: wgpu::Extent3d) -> Self {
        fn span(start: u32, len: u32, from: u32, to: u32) -> (u32, u32) {
            let from = u64::from(from);
            let to = u64::from(to);
            let end = ((u64::from(start) + u64::from(len)) * to).div_ceil(from);
            let start = u64::from(start) * to / from;
            (start as u32, (end - start) as u32)
        }
        let (x, width) = span(self.x, self.width, from.width, to.width);
        let (y, height) = span(self.y, self.height, from.height, to.height);
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn extent(&self) -> wgpu::Extent3d {
        wgpu::Extent3d {
            width: self.width,
            height: self.height,
            depth_or_array_layers: 1,
        }
    }
    fn copy<'a>(
        &self,
        texture: &'a wgpu::Texture,
    ) -> Result<wgpu::TexelCopyTextureInfo<'a>, &'static str> {
        if self.width == 0
            || self.height == 0
            || self
                .x
                .checked_add(self.width)
                .is_none_or(|n| n > texture.width())
            || self
                .y
                .checked_add(self.height)
                .is_none_or(|n| n > texture.height())
        {
            return Err("Invalid SGFX texture region");
        }
        let mut copy = texture.as_image_copy();
        copy.origin = wgpu::Origin3d {
            x: self.x,
            y: self.y,
            z: 0,
        };
        Ok(copy)
    }
}
fn retire_submissions(
    device: wgpu::Device,
    inbox: Arc<Inbox>,
    pending: mpsc::Receiver<Retirement>,
) {
    let mut batch = Vec::new();
    while let Ok(first) = pending.recv() {
        batch.push(first);
        batch.extend(pending.try_iter().take(31));
        // All GPU writes use the same ordered Metal queue. The last fence
        // covers every preceding request in this batch. Later scanout draws
        // have no bearing on when the guest may reuse its presentation source.
        if let Some(wait) = batch.iter_mut().rev().find_map(|(_, _, wait)| wait.take()) {
            device.poll(wait);
        }
        for (id, result, _) in batch.drain(..) {
            inbox.done(id, result);
        }
    }
    device.poll(wgpu::Maintain::Wait);
    inbox.close();
}

impl Drop for Host {
    fn drop(&mut self) {
        drop(self.requests.take());
        drop(self.retirement.take());
        if let Some(thread) = self.retirement_thread.take() {
            let _ = thread.join();
        }
    }
}
/// In headless mode the main thread remains the Metal owner while the BSP boots.
pub fn run_offscreen(
    config: Config,
    boot: impl FnOnce(Option<Endpoint>) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::METAL,
        ..Default::default()
    });
    let pair = pollster::block_on(instance.request_adapter(&Default::default()))
        .and_then(|a| pollster::block_on(a.request_device(&Default::default(), None)).ok());
    let Some((device, queue)) = pair else {
        return boot(None);
    };
    let (notify, events) = mpsc::channel();
    let wake = notify.clone();
    let (mut host, endpoint) = Host::open(
        device,
        queue,
        config,
        move || {
            let _ = wake.send(());
        },
        false,
    )?;
    let (done, finished) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("scarlet-bsp".into())
        .spawn(move || {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| boot(Some(endpoint))))
                    .unwrap_or_else(|_| Err("Headless BSP panicked".into()));
            let _ = done.send(result);
            let _ = notify.send(());
        })
        .map_err(|e| e.to_string())?;
    loop {
        events.recv().map_err(|e| e.to_string())?;
        if let Ok(result) = finished.try_recv() {
            thread.join().map_err(|_| "BSP panicked")?;
            return result;
        }
        while let Ok(request) = host.requests.as_ref().unwrap().try_recv() {
            host.handle(request, None);
        }
    }
}
