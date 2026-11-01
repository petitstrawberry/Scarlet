//! Generic composition of immutable host chrome and external GPU content.
//! CPU hosts can request ScarletUI's standard chrome without owning a UI loop.
use super::*;
use scarlet_ui_core::window_chrome::{WindowChrome, chrome_insets};
use scarlet_os::{
    SharedMemory,
    handle::capability::memory_mapping::{flags, munmap},
    ipc::permissions,
};
use sws_protocol::{
    self as p, ServerMessage, client_msg as msg,
    surface_scene::{Commit, Layer},
};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SwsRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SwsInsets {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}
impl SwsInsets {
    fn content(self, w: u32, h: u32) -> Option<SwsRect> {
        let width = w.checked_sub(self.left.checked_add(self.right)?)?;
        let height = h.checked_sub(self.top.checked_add(self.bottom)?)?;
        (width > 0 && height > 0).then_some(SwsRect {
            x: self.left,
            y: self.top,
            width,
            height,
        })
    }
}
#[repr(C)]
pub struct SwsSceneGeometry {
    pub width: u32,
    pub height: u32,
    pub scale_milli: u32,
    pub state: u32,
    pub content: SwsRect,
}
#[repr(C)]
pub struct SwsSceneFrame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub insets: SwsInsets,
    pub pixels: *const std::ffi::c_void,
    pub bytes: usize,
    pub clip: *const SwsRect,
    pub clip_count: usize,
}
struct Frame {
    width: u32,
    height: u32,
    insets: SwsInsets,
    clip: Vec<SwsRect>,
    // Immutable SHM is retained by the server until its selected scene retires.
    pool: u32,
    buffer: u32,
}
#[derive(Default)]
struct Window {
    frame: Option<Frame>,
    selected: Option<u32>,
    transparent: bool,
    events: VecDeque<SwsGpuEvent>,
}
struct Lease {
    identity: SwsBuffer,
    width: u32,
    height: u32,
    serial: u64,
    scene_serial: Option<u64>,
    destroyed: bool,
}
struct ChromeCache {
    title: String,
    width: u32,
    height: u32,
    scale_milli: u32,
    resizable: bool,
    decorated: bool,
    insets: SwsInsets,
    clip: Vec<SwsRect>,
    pixels: Vec<u8>,
}
impl ChromeCache {
    fn new(
        title: &str,
        width: u32,
        height: u32,
        scale_milli: u32,
        resizable: bool,
        decorated: bool,
    ) -> Result<Self, Error> {
        if !decorated {
            return Ok(Self {
                title: title.into(),
                width,
                height,
                scale_milli,
                resizable,
                decorated,
                insets: SwsInsets::default(),
                clip: vec![SwsRect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                }],
                pixels: Vec::new(),
            });
        }
        let mut chrome = WindowChrome::new(title, width, height, scale_milli, true, resizable)
            .map_err(|_| Error::InvalidRequest)?;
        let insets = chrome.insets();
        let clip = chrome
            .clip()
            .iter()
            .map(|rect| SwsRect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            })
            .collect();
        let pixels = chrome
            .render()
            .ok_or(Error::InvalidRequest)?
            .data()
            .to_vec();
        Ok(Self {
            title: title.into(),
            width,
            height,
            scale_milli,
            resizable,
            decorated,
            insets: SwsInsets {
                left: insets.left,
                top: insets.top,
                right: insets.right,
                bottom: insets.bottom,
            },
            clip,
            pixels,
        })
    }

    fn matches(
        &self,
        title: &str,
        width: u32,
        height: u32,
        scale_milli: u32,
        resizable: bool,
        decorated: bool,
    ) -> bool {
        self.title == title
            && self.width == width
            && self.height == height
            && self.scale_milli == scale_milli
            && self.resizable == resizable
            && self.decorated == decorated
    }
}
#[derive(Default)]
pub(super) struct Scenes {
    registered: bool,
    next_resource: u32,
    next_serial: u64,
    layers_scratch: Vec<Layer>,
    payload_scratch: Vec<u8>,
    windows: BTreeMap<u32, Window>,
    buffers: BTreeMap<u32, Lease>,
    chrome: BTreeMap<u32, ChromeCache>,
    // One immutable opaque pixel, shared by windows awaiting their first image.
    // The server retains its pool/buffer until this client disconnects.
    background: Option<u32>,
}
impl Scenes {
    fn ensure_background(&mut self, connection: &Connection) -> Result<(), Error> {
        if self.background.is_some() {
            return Ok(());
        }
        let shm =
            SharedMemory::create(4, permissions::READ_WRITE).map_err(|_| Error::ShmHandleFailed)?;
        let mapping = shm
            .as_handle()
            .as_memory_mapping()
            .map_err(|_| Error::ShmHandleFailed)?;
        // SAFETY: this fresh four-byte mapping exclusively owns one BGRA pixel.
        let address = unsafe { mapping.mmap(0, 4, permissions::READ_WRITE, flags::SHARED, 0) }
            .map_err(|_| Error::ShmHandleFailed)?;
        unsafe { (address as *mut u32).write(u32::from_ne_bytes([0, 0, 0, 255])) };
        unsafe { munmap(address, 4) }.map_err(|_| Error::ShmHandleFailed)?;
        let pool = self.resource()?;
        let buffer = self.resource()?;
        let token = connection.send_request_with_handle(
            msg::EXTENSION_REGISTER_SHM_POOL,
            &p::payload_extension_register_shm_pool(pool, 4),
            shm.as_handle(),
        )?;
        match connection.wait_response(token)?.message() {
            ServerMessage::ExtensionShmPoolRegistered { pool_id, .. } if pool_id == pool => {}
            ServerMessage::Error { code } => return Err(Error::ServerError(code)),
            _ => return Err(Error::InvalidResponse),
        }
        connection.send_message(
            msg::EXTENSION_DEFINE_BUFFER,
            &p::payload_extension_define_buffer(buffer, pool, 0, 1, 1, 4, 1),
        )?;
        self.background = Some(buffer);
        Ok(())
    }
    fn resource(&mut self) -> Result<u32, Error> {
        self.next_resource = self
            .next_resource
            .checked_add(1)
            .ok_or(Error::InvalidRequest)?;
        Ok(self.next_resource)
    }
    pub fn contains(&self, id: u32) -> bool {
        self.windows.contains_key(&id)
    }
    pub fn content(&self, id: u32, w: u32, h: u32, state: u32) -> Option<SwsRect> {
        let window = self.windows.get(&id)?;
        let insets = if state
            & (sws_client::window_state::FULLSCREEN | sws_client::window_state::TABLET_MANAGED)
            != 0
        {
            SwsInsets::default()
        } else {
            window.frame.as_ref().map_or_else(
                || {
                    self.chrome
                        .get(&id)
                        .map_or(SwsInsets::default(), |chrome| chrome.insets)
                },
                |f| f.insets,
            )
        };
        insets.content(w, h)
    }
    pub fn release(&mut self, resource: u32, serial: u64) {
        let Some(lease) = self.buffers.get_mut(&resource) else {
            return;
        };
        if lease.scene_serial != Some(serial) {
            return;
        }
        lease.scene_serial = None;
        if let Some(window) = self.windows.get_mut(&lease.identity.window_id) {
            window.events.push_back(SwsGpuEvent {
                buffer: lease.identity,
                commit_serial: lease.serial,
                kind: 1,
                code: 0,
            });
        }
        if lease.destroyed {
            self.buffers.remove(&resource);
        }
    }
    pub fn poll(&mut self, id: u32) -> Option<SwsGpuEvent> {
        self.windows.get_mut(&id)?.events.pop_front()
    }
    pub fn remove(&mut self, connection: &Connection, id: u32) -> Result<(), Error> {
        self.chrome.remove(&id);
        if let Some(window) = self.windows.remove(&id) {
            if let Some(frame) = window.frame {
                retire_frame(connection, frame)?;
            }
            let resources: Vec<_> = self
                .buffers
                .iter()
                .filter_map(|(&r, l)| (l.identity.window_id == id).then_some(r))
                .collect();
            for r in resources {
                connection.send_message(
                    msg::EXTENSION_DESTROY_BUFFER,
                    &p::payload_extension_destroy_buffer(r),
                )?;
                self.buffers.remove(&r);
            }
        }
        Ok(())
    }
    pub fn register(
        &mut self,
        connection: &Connection,
        identity: SwsBuffer,
        width: u32,
        height: u32,
        handle: &Handle,
        opaque: bool,
    ) -> Result<(), Error> {
        if self.buffers.values().any(|b| b.identity == identity) {
            return Err(Error::InvalidRequest);
        }
        let resource = self.resource()?;
        let alpha_payload = p::payload_extension_define_gpu_buffer(
            resource,
            identity.compositor_epoch,
            width,
            height,
        );
        let opaque_payload;
        let payload = if opaque {
            opaque_payload = p::payload_extension_define_opaque_gpu_buffer(
                resource,
                identity.compositor_epoch,
                width,
                height,
            );
            opaque_payload.as_slice()
        } else {
            alpha_payload.as_slice()
        };
        let token = connection.send_request_with_handle(
            msg::EXTENSION_DEFINE_GPU_BUFFER,
            payload,
            handle,
        )?;
        match connection.wait_response(token)?.message() {
            ServerMessage::ExtensionGpuBufferDefined { buffer_id } if buffer_id == resource => {}
            ServerMessage::Error { code } => return Err(Error::ServerError(code)),
            _ => return Err(Error::InvalidResponse),
        }
        self.buffers.insert(
            resource,
            Lease {
                identity,
                width,
                height,
                serial: 0,
                scene_serial: None,
                destroyed: false,
            },
        );
        Ok(())
    }
    pub fn destroy(&mut self, connection: &Connection, identity: SwsBuffer) -> Result<(), Error> {
        let (&resource, lease) = self
            .buffers
            .iter_mut()
            .find(|(_, l)| l.identity == identity)
            .ok_or(Error::InvalidRequest)?;
        connection.send_message(
            msg::EXTENSION_DESTROY_BUFFER,
            &p::payload_extension_destroy_buffer(resource),
        )?;
        lease.destroyed = true;
        if lease.scene_serial.is_none() {
            self.buffers.remove(&resource);
        }
        Ok(())
    }
    pub fn commit(
        &mut self,
        connection: &Connection,
        identity: SwsBuffer,
        serial: u64,
        width: u32,
        height: u32,
        state: u32,
    ) -> Result<(), Error> {
        let (w, h) = connection
            .with_surface(identity.window_id, |s| (s.width(), s.height()))
            .ok_or(Error::SurfaceNotFound)?;
        let content = self
            .content(identity.window_id, w, h, state)
            .ok_or(Error::InvalidRequest)?;
        if (width, height) != (content.width, content.height) || serial == 0 {
            return Err(Error::InvalidRequest);
        }
        let (&resource, lease) = self
            .buffers
            .iter_mut()
            .find(|(_, l)| l.identity == identity)
            .ok_or(Error::InvalidRequest)?;
        if lease.destroyed
            || lease.scene_serial.is_some()
            || (width, height) != (lease.width, lease.height)
            || serial <= lease.serial
        {
            return Err(Error::InvalidRequest);
        }
        let old = self
            .windows
            .get_mut(&identity.window_id)
            .unwrap()
            .selected
            .replace(resource);
        lease.serial = serial;
        if let Err(error) = self.publish(connection, identity.window_id, state) {
            self.windows.get_mut(&identity.window_id).unwrap().selected = old;
            return Err(error);
        }
        Ok(())
    }
    fn publish(&mut self, connection: &Connection, id: u32, state: u32) -> Result<(), Error> {
        let (width, height) = connection
            .with_surface(id, |s| (s.width(), s.height()))
            .ok_or(Error::SurfaceNotFound)?;
        let content = self
            .content(id, width, height, state)
            .ok_or(Error::InvalidRequest)?;
        let window = self.windows.get(&id).ok_or(Error::SurfaceNotFound)?;
        self.next_serial = self
            .next_serial
            .checked_add(1)
            .ok_or(Error::InvalidRequest)?;
        let serial = self.next_serial;
        let mut layers = std::mem::take(&mut self.layers_scratch);
        layers.clear();
        let frame = window.frame.as_ref().filter(|f| {
            (f.width, f.height) == (width, height)
                && f.insets.content(width, height) == Some(content)
        });
        let mut selected = None;
        if let Some(resource) = window.selected {
            if let Some(lease) = self.buffers.get(&resource) {
                if (lease.width, lease.height) == (content.width, content.height) {
                    let fallback = [SwsRect {
                        x: 0,
                        y: 0,
                        width,
                        height,
                    }];
                    let clip = frame.map_or(fallback.as_slice(), |f| f.clip.as_slice());
                    append_content_layers(&mut layers, resource, content, clip);
                    if !layers.is_empty() {
                        selected = Some(resource);
                    }
                }
            }
        }
        if selected.is_none() && !window.transparent {
            let buffer = self.background.ok_or(Error::InvalidRequest)?;
            let fallback = [SwsRect {
                x: 0,
                y: 0,
                width,
                height,
            }];
            let clip = frame.map_or(fallback.as_slice(), |f| f.clip.as_slice());
            append_background_layers(&mut layers, buffer, content, clip);
        }
        if let Some(f) = frame {
            layers.push(Layer {
                surface_id: layers.len() as u32 + 1,
                buffer_id: f.buffer,
                x: 0,
                y: 0,
                width,
                height,
                source_x: 0,
                source_y: 0,
                source_width: (width * 256) as i32,
                source_height: (height * 256) as i32,
                transform: 0,
            });
        }
        let scene = Commit {
            external_client_id: 1,
            window_id: id,
            serial,
            origin_x: 0,
            origin_y: 0,
            width,
            height,
            layers,
        };
        let sent = scene
            .encode_into(&mut self.payload_scratch)
            .map_err(|_| Error::InvalidRequest)
            .and_then(|()| {
                connection.send_message(msg::EXTENSION_COMMIT_SCENE, &self.payload_scratch)
            });
        self.layers_scratch = scene.layers;
        sent?;
        if let Some(resource) = selected {
            self.buffers.get_mut(&resource).unwrap().scene_serial = Some(serial);
        } else {
            self.windows.get_mut(&id).unwrap().selected = None;
        }
        Ok(())
    }
}
fn append_background_layers(
    layers: &mut Vec<Layer>,
    buffer: u32,
    content: SwsRect,
    clip: &[SwsRect],
) {
    let first = layers.len();
    append_content_layers(layers, buffer, content, clip);
    for layer in &mut layers[first..] {
        // Scale the same immutable pixel into each clipped content rectangle.
        layer.source_x = 0;
        layer.source_y = 0;
        layer.source_width = 256;
        layer.source_height = 256;
    }
}
#[cfg(test)]
fn content_layers(resource: u32, content: SwsRect, clip: &[SwsRect]) -> Vec<Layer> {
    let mut layers = Vec::new();
    append_content_layers(&mut layers, resource, content, clip);
    layers
}
fn append_content_layers(
    layers: &mut Vec<Layer>,
    resource: u32,
    content: SwsRect,
    clip: &[SwsRect],
) {
    for r in clip {
        let x = r.x.max(content.x);
        let y = r.y.max(content.y);
        let right = (r.x + r.width).min(content.x + content.width);
        let bottom = (r.y + r.height).min(content.y + content.height);
        if x >= right || y >= bottom {
            continue;
        }
        layers.push(Layer {
            surface_id: layers.len() as u32 + 1,
            buffer_id: resource,
            x: x as i32,
            y: y as i32,
            width: right - x,
            height: bottom - y,
            source_x: ((x - content.x) * 256) as i32,
            source_y: ((y - content.y) * 256) as i32,
            source_width: ((right - x) * 256) as i32,
            source_height: ((bottom - y) * 256) as i32,
            transform: 0,
        });
    }
}
fn retire_frame(connection: &Connection, frame: Frame) -> Result<(), Error> {
    connection.send_message(
        msg::EXTENSION_DESTROY_BUFFER,
        &p::payload_extension_destroy_buffer(frame.buffer),
    )?;
    connection.send_message(
        msg::EXTENSION_DESTROY_SHM_POOL,
        &p::payload_extension_destroy_shm_pool(frame.pool),
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sws_get_output_scale(out: *mut u32) -> i32 {
    if out.is_null() {
        return -1;
    }
    call(|c| {
        unsafe {
            out.write(c.connection.get_output_scale()?);
        }
        Ok(0)
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sws_scene_create(
    title: *const c_char,
    width: u32,
    height: u32,
    resizable: u32,
    out: *mut u32,
) -> i32 {
    unsafe { sws_scene_create_with_flags(title, width, height, resizable, 0, out) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sws_scene_create_with_flags(
    title: *const c_char,
    width: u32,
    height: u32,
    resizable: u32,
    scene_flags: u32,
    out: *mut u32,
) -> i32 {
    if title.is_null()
        || out.is_null()
        || !valid_size(width, height)
        || width as u64 * height as u64 > 16 * 1024 * 1024
        || scene_flags & !1 != 0
    {
        return -1;
    }
    let Ok(title) = (unsafe { CStr::from_ptr(title) }).to_str() else {
        return -1;
    };
    call(|c| {
        if !c.scenes.registered {
            let token = c.connection.send_request(
                msg::REGISTER_EXTENSION,
                &p::payload_register_extension(b"native-scene"),
            )?;
            match c.connection.wait_response(token)?.message() {
                ServerMessage::ExtensionRegistered { .. } => c.scenes.registered = true,
                ServerMessage::Error { code } => return Err(Error::ServerError(code)),
                _ => return Err(Error::InvalidResponse),
            }
        }
        let transparent = scene_flags & 1 != 0;
        if !transparent {
            c.scenes.ensure_background(&c.connection)?;
        }
        let id = c.connection.create_scene_surface(1, width, height)?;
        if let Err(e) = c
            .connection
            .set_window_title(id, title)
            .and_then(|_| c.connection.set_window_resizable(id, resizable != 0))
        {
            let _ = c.connection.destroy_surface(id);
            return Err(e);
        }
        c.gpu_events
            .insert(id, c.connection.subscribe_sgfx_events(id));
        c.scenes.windows.insert(
            id,
            Window {
                transparent,
                ..Window::default()
            },
        );
        if let Err(error) = c.scenes.publish(&c.connection, id, 0) {
            c.scenes.windows.remove(&id);
            c.gpu_events.remove(&id);
            let _ = c.connection.destroy_surface(id);
            return Err(error);
        }
        unsafe {
            out.write(id);
        }
        Ok(0)
    })
}

fn clip_span(clip: &[SwsRect], y: u32) -> Option<(u32, u32)> {
    clip.iter()
        .find(|row| y >= row.y && y < row.y.saturating_add(row.height))
        .map(|row| (row.x, row.x.saturating_add(row.width)))
}

fn compose_chrome_frame(
    chrome: &ChromeCache,
    pixels: &[u8],
    width: u32,
    height: u32,
    pitch: usize,
) -> Result<Vec<u8>, Error> {
    let stride = chrome.width as usize * 4;
    let bytes = stride
        .checked_mul(chrome.height as usize)
        .ok_or(Error::InvalidRequest)?;
    let mut output = vec![0; bytes];
    if chrome.decorated {
        if chrome.pixels.len() != bytes {
            return Err(Error::InvalidRequest);
        }
        output.copy_from_slice(&chrome.pixels);
    }
    let content = chrome
        .insets
        .content(chrome.width, chrome.height)
        .ok_or(Error::InvalidRequest)?;
    let fit_width = if u64::from(content.width) * u64::from(height)
        <= u64::from(content.height) * u64::from(width)
    {
        content.width
    } else {
        (u64::from(content.height) * u64::from(width) / u64::from(height)) as u32
    };
    let fit_height = if u64::from(content.width) * u64::from(height)
        <= u64::from(content.height) * u64::from(width)
    {
        (u64::from(content.width) * u64::from(height) / u64::from(width)) as u32
    } else {
        content.height
    };
    let fit_x = content.x + (content.width - fit_width) / 2;
    let fit_y = content.y + (content.height - fit_height) / 2;

    let fill_row = |output: &mut [u8], y: u32, left: u32, right: u32| {
        for x in left..right {
            let offset = y as usize * stride + x as usize * 4;
            output[offset..offset + 4].copy_from_slice(&[0, 0, 0, 255]);
        }
    };
    // Keep letterboxing opaque inside ScarletUI's rounded outline while the
    // transparent margin remains available to the desktop behind the window.
    for y in content.y..content.y + content.height {
        let Some((clip_left, clip_right)) = clip_span(&chrome.clip, y) else {
            continue;
        };
        let left = clip_left.max(content.x);
        let right = clip_right.min(content.x + content.width);
        if y < fit_y || y >= fit_y + fit_height {
            fill_row(&mut output, y, left, right);
        } else {
            fill_row(&mut output, y, left, right.min(fit_x));
            fill_row(&mut output, y, right.max(fit_x + fit_width), right);
        }
    }

    for y in fit_y..fit_y + fit_height {
        let Some((clip_left, clip_right)) = clip_span(&chrome.clip, y) else {
            continue;
        };
        let left = clip_left.max(fit_x);
        let right = clip_right.min(fit_x + fit_width);
        if left >= right {
            continue;
        }
        let source_y = ((y - fit_y) as u64 * u64::from(height) / u64::from(fit_height)) as usize;
        for x in left..right {
            let source_x = ((x - fit_x) as u64 * u64::from(width) / u64::from(fit_width)) as usize;
            let source = source_y * pitch + source_x * 4;
            let target = y as usize * stride + x as usize * 4;
            output[target..target + 4].copy_from_slice(&pixels[source..source + 4]);
        }
    }
    Ok(output)
}

#[allow(clippy::too_many_arguments)]
fn submit_scene_frame(
    client: &mut Client,
    id: u32,
    width: u32,
    height: u32,
    stride: u32,
    insets: SwsInsets,
    pixels: &[u8],
    clip: &[SwsRect],
) -> Result<(), Error> {
    if !client.scenes.contains(id) || client.closed.contains(&id) {
        return Err(Error::SurfaceNotFound);
    }
    let extent = client.connection.with_surface(id, |s| (s.width(), s.height()));
    if extent != Some((width, height)) {
        return Err(Error::InvalidRequest);
    }
    let size = width as usize * height as usize * 4;
    let shm = SharedMemory::create(size, permissions::READ_WRITE)
        .map_err(|_| Error::ShmHandleFailed)?;
    let mapping = shm
        .as_handle()
        .as_memory_mapping()
        .map_err(|_| Error::ShmHandleFailed)?;
    let address = unsafe { mapping.mmap(0, size, permissions::READ_WRITE, flags::SHARED, 0) }
        .map_err(|_| Error::ShmHandleFailed)?;
    for y in 0..height as usize {
        unsafe {
            std::ptr::copy_nonoverlapping(
                pixels.as_ptr().add(y * stride as usize),
                (address as *mut u8).add(y * width as usize * 4),
                width as usize * 4,
            );
        }
    }
    unsafe { munmap(address, size) }.map_err(|_| Error::ShmHandleFailed)?;
    let pool = client.scenes.resource()?;
    let buffer = client.scenes.resource()?;
    let token = client.connection.send_request_with_handle(
        msg::EXTENSION_REGISTER_SHM_POOL,
        &p::payload_extension_register_shm_pool(pool, size as u64),
        shm.as_handle(),
    )?;
    match client.connection.wait_response(token)?.message() {
        ServerMessage::ExtensionShmPoolRegistered { pool_id, .. } if pool_id == pool => {}
        ServerMessage::Error { code } => return Err(Error::ServerError(code)),
        _ => return Err(Error::InvalidResponse),
    }
    client.connection.send_message(
        msg::EXTENSION_DEFINE_BUFFER,
        &p::payload_extension_define_buffer(buffer, pool, 0, width, height, width * 4, 0),
    )?;
    let frame = Frame {
        width,
        height,
        insets,
        clip: clip.to_vec(),
        pool,
        buffer,
    };
    let old = client.scenes.windows.get_mut(&id).unwrap().frame.replace(frame);
    if let Err(error) = client
        .scenes
        .publish(&client.connection, id, client.states.get(&id).copied().unwrap_or(0))
    {
        if let Some(failed) =
            std::mem::replace(&mut client.scenes.windows.get_mut(&id).unwrap().frame, old)
        {
            let _ = retire_frame(&client.connection, failed);
        }
        return Err(error);
    }
    if let Some(old) = old {
        retire_frame(&client.connection, old)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sws_scene_chrome_create(
    title: *const c_char,
    content_width: u32,
    content_height: u32,
    resizable: u32,
    out: *mut u32,
) -> i32 {
    if title.is_null() || out.is_null() || !valid_size(content_width, content_height) {
        return -1;
    }
    let Ok(title) = (unsafe { CStr::from_ptr(title) }).to_str() else {
        return -1;
    };
    call(|c| {
        if !c.scenes.registered {
            let token = c.connection.send_request(
                msg::REGISTER_EXTENSION,
                &p::payload_register_extension(b"native-scene"),
            )?;
            match c.connection.wait_response(token)?.message() {
                ServerMessage::ExtensionRegistered { .. } => c.scenes.registered = true,
                ServerMessage::Error { code } => return Err(Error::ServerError(code)),
                _ => return Err(Error::InvalidResponse),
            }
        }
        let scale_milli = c.connection.get_output_scale()?;
        let inset = chrome_insets(scale_milli, true);
        let width = content_width
            .checked_add(inset.left.checked_add(inset.right).ok_or(Error::InvalidRequest)?)
            .ok_or(Error::InvalidRequest)?;
        let height = content_height
            .checked_add(inset.top.checked_add(inset.bottom).ok_or(Error::InvalidRequest)?)
            .ok_or(Error::InvalidRequest)?;
        if !valid_size(width, height) || width as u64 * height as u64 > 16 * 1024 * 1024 {
            return Err(Error::InvalidRequest);
        }
        let chrome = ChromeCache::new(title, width, height, scale_milli, resizable != 0, true)?;
        c.scenes.ensure_background(&c.connection)?;
        let id = c.connection.create_scene_surface(1, width, height)?;
        if let Err(error) = c
            .connection
            .set_window_title(id, title)
            .and_then(|_| c.connection.set_window_resizable(id, resizable != 0))
        {
            let _ = c.connection.destroy_surface(id);
            return Err(error);
        }
        c.gpu_events
            .insert(id, c.connection.subscribe_sgfx_events(id));
        c.scenes.windows.insert(id, Window::default());
        c.scenes.chrome.insert(id, chrome);
        if let Err(error) = c.scenes.publish(&c.connection, id, 0) {
            c.scenes.chrome.remove(&id);
            c.scenes.windows.remove(&id);
            c.gpu_events.remove(&id);
            let _ = c.connection.destroy_surface(id);
            return Err(error);
        }
        unsafe { out.write(id) };
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sws_scene_chrome_present(
    id: u32,
    pixels: *const std::ffi::c_void,
    bytes: usize,
    width: u32,
    height: u32,
    pitch: usize,
) -> i32 {
    if pixels.is_null() || !valid_size(width, height) || pitch < width as usize * 4 {
        return -1;
    }
    let required = (height as usize - 1) * pitch + width as usize * 4;
    if required > bytes {
        return -1;
    }
    let pixels = unsafe { std::slice::from_raw_parts(pixels.cast::<u8>(), required) };
    call(|c| {
        c.dispatch()?;
        if !c.scenes.contains(id) || c.closed.contains(&id) {
            return Err(Error::SurfaceNotFound);
        }
        let (outer_width, outer_height) = c
            .connection
            .with_surface(id, |surface| (surface.width(), surface.height()))
            .ok_or(Error::SurfaceNotFound)?;
        let state = c.states.get(&id).copied().unwrap_or(0);
        let decorated = state & sws_client::window_state::FULLSCREEN == 0;
        let scale_milli = c.connection.get_output_scale()?;
        let resizable = c.scenes.chrome.get(&id).is_none_or(|chrome| chrome.resizable);
        let title = c
            .scenes
            .chrome
            .get(&id)
            .map(|chrome| chrome.title.clone())
            .unwrap_or_else(|| String::from("Android"));
        let rebuild = c.scenes.chrome.get(&id).is_none_or(|chrome| {
            !chrome.matches(&title, outer_width, outer_height, scale_milli, resizable, decorated)
        });
        if rebuild {
            c.scenes.chrome.insert(
                id,
                ChromeCache::new(
                    &title,
                    outer_width,
                    outer_height,
                    scale_milli,
                    resizable,
                    decorated,
                )?,
            );
        }
        let chrome = c.scenes.chrome.get(&id).ok_or(Error::SurfaceNotFound)?;
        let composite = compose_chrome_frame(chrome, pixels, width, height, pitch)?;
        let insets = if decorated {
            chrome.insets
        } else {
            SwsInsets::default()
        };
        let clip = [SwsRect {
            x: 0,
            y: 0,
            width: outer_width,
            height: outer_height,
        }];
        submit_scene_frame(
            c,
            id,
            outer_width,
            outer_height,
            outer_width * 4,
            insets,
            &composite,
            &clip,
        )?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sws_scene_geometry(id: u32, out: *mut SwsSceneGeometry) -> i32 {
    if out.is_null() {
        return -1;
    }
    call(|c| {
        c.dispatch()?;
        let (width, height) = c
            .connection
            .with_surface(id, |s| (s.width(), s.height()))
            .ok_or(Error::SurfaceNotFound)?;
        let state = c.states.get(&id).copied().unwrap_or(0);
        let content = c
            .scenes
            .content(id, width, height, state)
            .ok_or(Error::InvalidRequest)?;
        unsafe {
            out.write(SwsSceneGeometry {
                width,
                height,
                scale_milli: c.connection.get_output_scale()?,
                state,
                content,
            });
        }
        Ok(0)
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sws_scene_frame(id: u32, input: *const SwsSceneFrame) -> i32 {
    let Some(f) = (unsafe { input.as_ref() }) else {
        return -1;
    };
    if !valid_size(f.width, f.height)
        || f.width as u64 * f.height as u64 > 16 * 1024 * 1024
        || f.insets.content(f.width, f.height).is_none()
        || f.stride < f.width * 4
        || f.pixels.is_null()
        || f.clip.is_null()
        || f.clip_count == 0
        || f.clip_count >= 256
    {
        return -1;
    }
    let required = (f.height as usize - 1) * f.stride as usize + f.width as usize * 4;
    if required > f.bytes || required > isize::MAX as usize {
        return -1;
    }
    // SAFETY: caller borrows these validated arrays for the duration of this call.
    let pixels = unsafe { std::slice::from_raw_parts(f.pixels.cast::<u8>(), required) };
    let clip = unsafe { std::slice::from_raw_parts(f.clip, f.clip_count) };
    if clip.iter().any(|r| {
        r.width == 0
            || r.height == 0
            || r.x.checked_add(r.width).is_none_or(|x| x > f.width)
            || r.y.checked_add(r.height).is_none_or(|y| y > f.height)
    }) {
        return -1;
    }
    call(|c| {
        submit_scene_frame(c, id, f.width, f.height, f.stride, f.insets, pixels, clip)?;
        Ok(0)
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn sws_window_resizable(id: u32, enabled: u32) -> i32 {
    call(|c| {
        c.connection.set_window_resizable(id, enabled != 0)?;
        Ok(0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chrome_content_uses_the_shared_inset_and_clip() {
        let chrome = ChromeCache {
            title: String::from("Android"),
            width: 6,
            height: 4,
            scale_milli: 1000,
            resizable: true,
            decorated: true,
            insets: SwsInsets {
                left: 1,
                top: 1,
                right: 1,
                bottom: 1,
            },
            clip: vec![
                SwsRect {
                    x: 1,
                    y: 0,
                    width: 4,
                    height: 1,
                },
                SwsRect {
                    x: 0,
                    y: 1,
                    width: 6,
                    height: 2,
                },
                SwsRect {
                    x: 1,
                    y: 3,
                    width: 4,
                    height: 1,
                },
            ],
            pixels: vec![0; 6 * 4 * 4],
        };
        let source = [
            1, 2, 3, 255, 4, 5, 6, 255,
            7, 8, 9, 255, 10, 11, 12, 255,
        ];
        let output = compose_chrome_frame(&chrome, &source, 2, 2, 8).unwrap();
        assert_eq!(&output[((1 * 6 + 1) * 4)..((1 * 6 + 1) * 4 + 4)], &[1, 2, 3, 255]);
        assert_eq!(&output[((2 * 6 + 4) * 4)..((2 * 6 + 4) * 4 + 4)], &[10, 11, 12, 255]);
        assert_eq!(&output[((1 * 6 + 0) * 4)..((1 * 6 + 0) * 4 + 4)], &[0, 0, 0, 0]);
        assert_eq!(&output[((0 * 6 + 0) * 4)..((0 * 6 + 0) * 4 + 4)], &[0, 0, 0, 0]);
    }
    #[test]
    fn cropped_gpu_layers_keep_content_coordinates() {
        let content = SwsRect {
            x: 2,
            y: 32,
            width: 320,
            height: 240,
        };
        let layers = content_layers(
            7,
            content,
            &[
                SwsRect {
                    x: 0,
                    y: 0,
                    width: 324,
                    height: 260,
                },
                SwsRect {
                    x: 4,
                    y: 260,
                    width: 316,
                    height: 12,
                },
            ],
        );
        assert_eq!(layers.len(), 2);
        assert_eq!(
            (
                layers[0].x,
                layers[0].y,
                layers[0].source_x,
                layers[0].source_y
            ),
            (2, 32, 0, 0)
        );
        assert_eq!(
            (layers[1].x, layers[1].source_x, layers[1].source_y),
            (4, 512, 228 * 256)
        );
        assert!(layers.iter().all(|l| l.fits_buffer(320, 240)));
        let mut background = Vec::new();
        append_background_layers(
            &mut background,
            9,
            content,
            &[SwsRect {
                x: 4,
                y: 260,
                width: 316,
                height: 12,
            }],
        );
        assert_eq!(background.len(), 1);
        assert_eq!(
            (
                background[0].x,
                background[0].y,
                background[0].width,
                background[0].height
            ),
            (4, 260, 316, 12)
        );
        assert!(background[0].fits_buffer(1, 1));
    }
    #[test]
    fn chrome_repaint_does_not_release_a_selected_gpu_use() {
        let mut scenes = Scenes::default();
        scenes.windows.insert(1, Window::default());
        let identity = SwsBuffer {
            window_id: 1,
            buffer_id: 2,
            generation: 3,
            compositor_epoch: 4,
        };
        scenes.buffers.insert(
            5,
            Lease {
                identity,
                width: 320,
                height: 240,
                serial: 17,
                scene_serial: Some(21),
                destroyed: false,
            },
        );
        scenes.release(5, 20);
        assert!(scenes.poll(1).is_none());
        scenes.release(5, 21);
        let released = scenes.poll(1).unwrap();
        assert_eq!(released.buffer, identity);
        assert_eq!(released.commit_serial, 17);
        scenes.release(5, 21);
        assert!(scenes.poll(1).is_none());
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn sws_scene_resize(id: u32, width: u32, height: u32) -> i32 {
    if !valid_size(width, height) || width as u64 * height as u64 > 16 * 1024 * 1024 {
        return -1;
    }
    call(|c| {
        if !c.scenes.contains(id) {
            return Err(Error::SurfaceNotFound);
        }
        c.connection.resize_window(id, width, height)?;
        Ok(0)
    })
}
