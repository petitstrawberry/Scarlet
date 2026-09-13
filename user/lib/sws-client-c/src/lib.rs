//! C ABI for SWS window, input and shared-image clients.
//! Ordinary Linux-ABI libraries use scarlet-sys' explicit native-call transport.
//! The process must dynamically link one copy so every consumer shares the
//! connection and no consumer steals another window's GPU lifecycle events.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::{CStr, c_char},
    sync::{Mutex, OnceLock},
};
use sws_client::{
    Connection, Error, Event, EventReceiver, Handle, SgfxBufferIdentity, SgfxDamageRect,
    SurfaceBuilder,
};

#[repr(C)]
pub struct SwsDisplay {
    pub width: u32,
    pub height: u32,
    pub compositor_epoch: u32,
    pub compositor_backend: u32,
    pub capabilities: u64,
}
#[repr(C)]
#[derive(Default)]
pub struct SwsEvent {
    pub window_id: u32,
    pub kind: u32,
    pub time: u64,
    pub type_: u16,
    pub code: u16,
    pub value: i32,
    pub width: u32,
    pub height: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SwsBuffer {
    pub window_id: u32,
    pub buffer_id: u32,
    pub generation: u32,
    pub compositor_epoch: u32,
}
impl From<SwsBuffer> for SgfxBufferIdentity {
    fn from(b: SwsBuffer) -> Self {
        Self {
            window_id: b.window_id,
            buffer_id: b.buffer_id,
            generation: b.generation,
            compositor_epoch: b.compositor_epoch,
        }
    }
}
#[repr(C)]
pub struct SwsGpuEvent {
    pub buffer: SwsBuffer,
    pub commit_serial: u64,
    pub kind: u32,
    pub code: u32,
}

struct Client {
    connection: Connection,
    gpu_events: BTreeMap<u32, EventReceiver>,
    events: VecDeque<SwsEvent>,
    states: BTreeMap<u32, u32>,
    closed: BTreeSet<u32>,
}
impl Client {
    fn dispatch(&mut self) -> Result<(), Error> {
        self.connection.dispatch()?;
        while let Some(event) = self.connection.poll_event() {
            let event = match event {
                Event::Input(input) => SwsEvent {
                    window_id: input.surface_id,
                    kind: 1,
                    time: input.time,
                    type_: input.type_,
                    code: input.code,
                    value: input.value,
                    ..Default::default()
                },
                Event::SurfaceConfigure {
                    surface_id,
                    width,
                    height,
                } => {
                    if !self.gpu_events.contains_key(&surface_id) {
                        continue;
                    }
                    self.connection.resize_window(surface_id, width, height)?;
                    SwsEvent {
                        window_id: surface_id,
                        kind: 2,
                        width,
                        height,
                        ..Default::default()
                    }
                }
                Event::SurfaceDestroyed { surface_id } => {
                    if !self.gpu_events.contains_key(&surface_id) {
                        continue;
                    }
                    self.closed.insert(surface_id);
                    SwsEvent {
                        window_id: surface_id,
                        kind: 3,
                        ..Default::default()
                    }
                }
                Event::SurfaceStateChanged {
                    surface_id,
                    state_flags,
                } => {
                    if self.gpu_events.contains_key(&surface_id) {
                        self.states.insert(surface_id, state_flags);
                    }
                    continue;
                }
                Event::FocusChanged { window_id, .. } => {
                    for &id in self.gpu_events.keys() {
                        self.events.push_back(SwsEvent {
                            window_id: id,
                            kind: 4,
                            value: i32::from(id == window_id),
                            ..Default::default()
                        });
                    }
                    continue;
                }
                _ => continue,
            };
            if self.gpu_events.contains_key(&event.window_id) {
                self.events.push_back(event);
            }
        }
        Ok(())
    }
}

fn call(f: impl FnOnce(&mut Client) -> Result<i32, Error>) -> i32 {
    static CLIENT: OnceLock<Mutex<Option<Client>>> = OnceLock::new();
    let mut slot = match CLIENT.get_or_init(|| Mutex::new(None)).lock() {
        Ok(slot) => slot,
        Err(_) => return -3,
    };
    if slot.is_none() {
        let connection = match Connection::connect_default() {
            Ok(connection) => connection,
            Err(_) => return -2,
        };
        *slot = Some(Client {
            connection,
            gpu_events: BTreeMap::new(),
            events: VecDeque::new(),
            states: BTreeMap::new(),
            closed: BTreeSet::new(),
        });
    }
    match f(slot.as_mut().unwrap()) {
        Ok(result) => result,
        Err(Error::SurfaceNotFound) => -4,
        Err(Error::InvalidRequest) => -1,
        Err(Error::TimedOut) => -5,
        Err(Error::ServerError(_)) => -6,
        Err(_) => -3,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sws_get_display(out: *mut SwsDisplay) -> i32 {
    if out.is_null() {
        return -1;
    }
    call(|client| {
        let (width, height) = client.connection.get_screen_size()?;
        let caps = client.connection.get_capabilities()?;
        // SAFETY: the C caller supplies a writable output record.
        unsafe {
            out.write(SwsDisplay {
                width,
                height,
                compositor_epoch: caps.compositor_epoch,
                compositor_backend: caps.compositor_backend,
                capabilities: caps.capabilities,
            });
        }
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sws_window_create(
    app_id: *const c_char,
    title: *const c_char,
    width: u32,
    height: u32,
    out: *mut u32,
) -> i32 {
    if app_id.is_null() || title.is_null() || out.is_null() || width == 0 || height == 0 {
        return -1;
    }
    // SAFETY: the C caller supplies terminated strings valid for this call.
    let (Ok(app_id), Ok(title)) = (
        unsafe { CStr::from_ptr(app_id) }.to_str(),
        unsafe { CStr::from_ptr(title) }.to_str(),
    ) else {
        return -1;
    };
    call(|client| {
        let id = SurfaceBuilder::new()
            .app_id(app_id)
            .app_name(title)
            .size(width, height)
            .resizable(false)
            .build(&client.connection)?;
        client
            .gpu_events
            .insert(id, client.connection.subscribe_sgfx_events(id));
        unsafe {
            out.write(id);
        }
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn sws_window_destroy(id: u32) -> i32 {
    call(|client| {
        if !client.closed.contains(&id) {
            client.connection.destroy_surface(id)?;
        }
        client.gpu_events.remove(&id);
        client.states.remove(&id);
        client.closed.remove(&id);
        client.events.retain(|event| event.window_id != id);
        Ok(0)
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn sws_window_fullscreen(id: u32, enabled: u32) -> i32 {
    call(|client| {
        if enabled != 0 {
            client.connection.set_fullscreen(id)?;
        } else {
            client.connection.unset_fullscreen(id)?;
        }
        let started = std::time::Instant::now();
        loop {
            client.dispatch()?;
            if client.closed.contains(&id) {
                return Err(Error::SurfaceNotFound);
            }
            let fullscreen = client.states.get(&id).copied().unwrap_or(0)
                & sws_client::window_state::FULLSCREEN
                != 0;
            if fullscreen == (enabled != 0) {
                return Ok(0);
            }
            if started.elapsed() >= std::time::Duration::from_secs(5) {
                return Err(Error::TimedOut);
            }
            client
                .connection
                .wait_for_window_events(std::time::Duration::from_millis(10))?;
        }
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn sws_window_pointer_lock(id: u32, enabled: u32) -> i32 {
    call(|client| {
        client.connection.set_pointer_lock(id, enabled != 0)?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sws_poll_event(out: *mut SwsEvent) -> i32 {
    if out.is_null() {
        return -1;
    }
    call(|client| {
        client.dispatch()?;
        if let Some(event) = client.events.pop_front() {
            unsafe {
                out.write(event);
            }
            return Ok(1);
        }
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn sws_gpu_register(
    buffer: SwsBuffer,
    width: u32,
    height: u32,
    raw_handle: i32,
) -> i32 {
    if width == 0 || height == 0 || raw_handle < 0 {
        return -1;
    }
    call(|client| {
        client.dispatch()?;
        if client.closed.contains(&buffer.window_id)
            || !client.gpu_events.contains_key(&buffer.window_id)
        {
            return Err(Error::SurfaceNotFound);
        }
        // Duplicate the borrowed object first. from_raw consumes only this new
        // handle, including its cleanup if introspection or registration fails.
        let duplicated = unsafe {
            scarlet_sys::syscall1(scarlet_sys::Syscall::HandleDuplicate, raw_handle as usize)
        };
        if duplicated == usize::MAX {
            return Err(Error::InvalidRequest);
        }
        let handle =
            unsafe { Handle::from_raw(duplicated as i32) }.map_err(|_| Error::InvalidRequest)?;
        client
            .connection
            .register_sgfx_buffer(buffer.into(), width, height, &handle)?;
        Ok(0)
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn sws_gpu_commit(buffer: SwsBuffer, serial: u64, width: u32, height: u32) -> i32 {
    call(|client| {
        client.connection.commit_sgfx_frame(
            buffer.into(),
            serial,
            &[SgfxDamageRect {
                x: 0,
                y: 0,
                width,
                height,
            }],
        )?;
        Ok(0)
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn sws_gpu_destroy(buffer: SwsBuffer) -> i32 {
    call(|client| {
        client.connection.destroy_sgfx_buffer(buffer.into())?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sws_gpu_poll(id: u32, out: *mut SwsGpuEvent) -> i32 {
    if out.is_null() {
        return -1;
    }
    call(|client| {
        client.dispatch()?;
        if client.closed.contains(&id) {
            return Err(Error::SurfaceNotFound);
        }
        let receiver = client.gpu_events.get(&id).ok_or(Error::SurfaceNotFound)?;
        while let Some(event) = receiver.poll_event() {
            let (window_id, buffer_id, generation, compositor_epoch, commit_serial, kind, code) =
                match event {
                    Event::SgfxBufferReleased {
                        window_id,
                        buffer_id,
                        generation,
                        compositor_epoch,
                        commit_serial,
                    } => (
                        window_id,
                        buffer_id,
                        generation,
                        compositor_epoch,
                        commit_serial,
                        1,
                        0,
                    ),
                    Event::SgfxFrameRejected {
                        window_id,
                        buffer_id,
                        generation,
                        compositor_epoch,
                        commit_serial,
                        code,
                    } => (
                        window_id,
                        buffer_id,
                        generation,
                        compositor_epoch,
                        commit_serial,
                        2,
                        code,
                    ),
                    Event::SgfxBackendLost { compositor_epoch } => {
                        (id, 0, 0, compositor_epoch, 0, 3, 0)
                    }
                    _ => continue,
                };
            unsafe {
                out.write(SwsGpuEvent {
                    buffer: SwsBuffer {
                        window_id,
                        buffer_id,
                        generation,
                        compositor_epoch,
                    },
                    commit_serial,
                    kind,
                    code,
                });
            }
            return Ok(1);
        }
        Ok(0)
    })
}
