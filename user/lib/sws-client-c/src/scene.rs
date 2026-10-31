//! Generic composition of immutable host chrome and external GPU content.
//! No UI toolkit or application render loop is owned by this module.
use super::*;
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
#[derive(Default)]
pub(super) struct Scenes {
    registered: bool,
    next_resource: u32,
    next_serial: u64,
    layers_scratch: Vec<Layer>,
    payload_scratch: Vec<u8>,
    windows: BTreeMap<u32, Window>,
    buffers: BTreeMap<u32, Lease>,
}
impl Scenes {
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
            window
                .frame
                .as_ref()
                .map_or(SwsInsets::default(), |f| f.insets)
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
    ) -> Result<(), Error> {
        if self.buffers.values().any(|b| b.identity == identity) {
            return Err(Error::InvalidRequest);
        }
        let resource = self.resource()?;
        let token = connection.send_request_with_handle(
            msg::EXTENSION_DEFINE_GPU_BUFFER,
            &p::payload_extension_define_gpu_buffer(
                resource,
                identity.compositor_epoch,
                width,
                height,
            ),
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
    if title.is_null()
        || out.is_null()
        || !valid_size(width, height)
        || width as u64 * height as u64 > 16 * 1024 * 1024
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
        c.scenes.windows.insert(id, Window::default());
        unsafe {
            out.write(id);
        }
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
        if !c.scenes.contains(id) || c.closed.contains(&id) {
            return Err(Error::SurfaceNotFound);
        }
        let extent = c.connection.with_surface(id, |s| (s.width(), s.height()));
        if extent != Some((f.width, f.height)) {
            return Err(Error::InvalidRequest);
        }
        let size = f.width as usize * f.height as usize * 4;
        let shm = SharedMemory::create(size, permissions::READ_WRITE)
            .map_err(|_| Error::ShmHandleFailed)?;
        let mapping = shm
            .as_handle()
            .as_memory_mapping()
            .map_err(|_| Error::ShmHandleFailed)?;
        let address = unsafe { mapping.mmap(0, size, permissions::READ_WRITE, flags::SHARED, 0) }
            .map_err(|_| Error::ShmHandleFailed)?;
        for y in 0..f.height as usize {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    pixels.as_ptr().add(y * f.stride as usize),
                    (address as *mut u8).add(y * f.width as usize * 4),
                    f.width as usize * 4,
                );
            }
        }
        unsafe { munmap(address, size) }.map_err(|_| Error::ShmHandleFailed)?;
        let pool = c.scenes.resource()?;
        let buffer = c.scenes.resource()?;
        let token = c.connection.send_request_with_handle(
            msg::EXTENSION_REGISTER_SHM_POOL,
            &p::payload_extension_register_shm_pool(pool, size as u64),
            shm.as_handle(),
        )?;
        match c.connection.wait_response(token)?.message() {
            ServerMessage::ExtensionShmPoolRegistered { pool_id, .. } if pool_id == pool => {}
            ServerMessage::Error { code } => return Err(Error::ServerError(code)),
            _ => return Err(Error::InvalidResponse),
        }
        c.connection.send_message(
            msg::EXTENSION_DEFINE_BUFFER,
            &p::payload_extension_define_buffer(buffer, pool, 0, f.width, f.height, f.width * 4, 0),
        )?;
        let frame = Frame {
            width: f.width,
            height: f.height,
            insets: f.insets,
            clip: clip.to_vec(),
            pool,
            buffer,
        };
        let old = c.scenes.windows.get_mut(&id).unwrap().frame.replace(frame);
        if let Err(e) = c
            .scenes
            .publish(&c.connection, id, c.states.get(&id).copied().unwrap_or(0))
        {
            if let Some(failed) =
                std::mem::replace(&mut c.scenes.windows.get_mut(&id).unwrap().frame, old)
            {
                let _ = retire_frame(&c.connection, failed);
            }
            return Err(e);
        }
        if let Some(old) = old {
            retire_frame(&c.connection, old)?;
        }
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
