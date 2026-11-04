//! C ABI for SWS window, input and shared-image clients.
//! Native and Linux-ABI consumers share one connection per process.
//! The process must link one copy so consumers cannot steal each other's GPU events.

// Startup symbols belong to this DSO's Rust runtime and must not interpose.
#[cfg(target_os = "scarlet")]
core::arch::global_asm!(".hidden __scarlet_getauxval", ".hidden __scarlet_start");

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::{CStr, c_char},
    sync::{Mutex, OnceLock},
};
pub mod scene;

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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SwsTouchFrame {
    pub window_id: u32,
    pub seat_id: u32,
    pub serial: u64,
    pub time_ns: u64,
    pub change_count: u32,
    pub reserved: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SwsTouchChange {
    pub id: u64,
    pub phase: u32,
    pub x: i32,
    pub y: i32,
    pub pressure: i32,
    pub touch_major: i32,
    pub reserved: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

const MAX_PENDING_TOUCH_FRAMES: usize = 64;

struct QueuedTouchFrame {
    frame: SwsTouchFrame,
    changes: Vec<SwsTouchChange>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TouchQueueResult {
    Queued,
    Overflowed,
    AlreadyOverflowed,
}

#[derive(Default)]
struct TouchQueues {
    frames: BTreeMap<u32, VecDeque<QueuedTouchFrame>>,
    overflowed: BTreeSet<u32>,
}

impl TouchQueues {
    fn push(&mut self, window_id: u32, frame: sws_protocol::touch::Frame) -> TouchQueueResult {
        if self.overflowed.contains(&window_id) {
            return TouchQueueResult::AlreadyOverflowed;
        }
        let queue = self.frames.entry(window_id).or_default();
        if queue.len() >= MAX_PENDING_TOUCH_FRAMES {
            self.overflowed.insert(window_id);
            return TouchQueueResult::Overflowed;
        }

        let changes = frame
            .changes
            .into_iter()
            .map(|change| SwsTouchChange {
                id: change.id,
                phase: change.phase as u32,
                x: change.x,
                y: change.y,
                pressure: change.pressure.unwrap_or(-1),
                touch_major: change.touch_major.unwrap_or(-1),
                reserved: 0,
            })
            .collect::<Vec<_>>();
        queue.push_back(QueuedTouchFrame {
            frame: SwsTouchFrame {
                window_id,
                seat_id: frame.seat_id,
                serial: frame.serial,
                time_ns: frame.time_ns,
                change_count: changes.len() as u32,
                reserved: 0,
            },
            changes,
        });
        TouchQueueResult::Queued
    }

    fn pop(&mut self, window_id: u32) -> Option<QueuedTouchFrame> {
        let queue = self.frames.get_mut(&window_id)?;
        let frame = queue.pop_front()?;
        if queue.is_empty() {
            self.frames.remove(&window_id);
        }
        Some(frame)
    }

    fn remove(&mut self, window_id: u32) {
        self.frames.remove(&window_id);
        self.overflowed.remove(&window_id);
    }
}

/// Copy the front frame only when the destination can hold every change,
/// then consume exactly that one queue entry.
unsafe fn copy_touch_frame(
    touch: &mut TouchQueues,
    window_id: u32,
    frame: *mut SwsTouchFrame,
    changes: *mut SwsTouchChange,
    capacity: u32,
) -> Result<bool, Error> {
    let Some(queue) = touch.frames.get(&window_id) else {
        return Ok(false);
    };
    let Some(queued) = queue.front() else {
        return Ok(false);
    };
    if queued.changes.len() > capacity as usize {
        return Err(Error::InvalidRequest);
    }

    // SAFETY: the caller checks both outputs are non-null and promises storage
    // for `capacity` changes. The check above covers the complete frame.
    unsafe {
        frame.write(queued.frame);
        std::ptr::copy_nonoverlapping(queued.changes.as_ptr(), changes, queued.changes.len());
    }
    touch.pop(window_id);
    Ok(true)
}

fn queue_touch_frame(
    touch: &mut TouchQueues,
    events: &mut VecDeque<SwsEvent>,
    window_id: u32,
    frame: sws_protocol::touch::Frame,
) -> TouchQueueResult {
    let result = touch.push(window_id, frame);
    match result {
        TouchQueueResult::Queued => events.push_back(SwsEvent {
            window_id,
            kind: 8,
            ..Default::default()
        }),
        TouchQueueResult::Overflowed => events.push_back(SwsEvent {
            window_id,
            kind: 7,
            ..Default::default()
        }),
        TouchQueueResult::AlreadyOverflowed => {}
    }
    result
}

struct Client {
    connection: Connection,
    scenes: scene::Scenes,
    gpu_events: BTreeMap<u32, EventReceiver>,
    events: VecDeque<SwsEvent>,
    touch: TouchQueues,
    states: BTreeMap<u32, u32>,
    closed: BTreeSet<u32>,
    closing: BTreeSet<u32>,
}
impl Client {
    fn dispatch(&mut self) -> Result<(), Error> {
        self.connection.dispatch()?;
        while let Some(event) = self.connection.poll_event() {
            let event = match event {
                Event::Error { code } => return Err(Error::ServerError(code)),
                Event::SceneBufferReleased {
                    buffer_id,
                    commit_serial,
                } => {
                    self.scenes.release(buffer_id, commit_serial);
                    continue;
                }
                Event::OutputScaleChanged { scale_milli } => {
                    for &id in self.gpu_events.keys() {
                        self.events.push_back(SwsEvent {
                            window_id: id,
                            kind: 5,
                            value: scale_milli as i32,
                            ..Default::default()
                        });
                    }
                    continue;
                }
                Event::Input(input) => SwsEvent {
                    window_id: input.surface_id,
                    kind: 1,
                    time: input.time,
                    type_: input.type_,
                    code: input.code,
                    value: input.value,
                    ..Default::default()
                },
                Event::TouchFrame { surface_id, frame } => {
                    if !self.gpu_events.contains_key(&surface_id) {
                        continue;
                    }
                    queue_touch_frame(&mut self.touch, &mut self.events, surface_id, frame);
                    continue;
                }
                Event::SurfaceConfigure {
                    surface_id,
                    width,
                    height,
                } => {
                    if !self.gpu_events.contains_key(&surface_id)
                        || self.closing.contains(&surface_id)
                    {
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
                Event::WindowCloseRequested { window_id } => SwsEvent {
                    window_id,
                    kind: 7,
                    ..Default::default()
                },
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
                        self.events.push_back(SwsEvent {
                            window_id: surface_id,
                            kind: 6,
                            value: state_flags as i32,
                            ..Default::default()
                        });
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

type TimingCallback = extern "C" fn(u32, u64, u64);
static TIMING_CALLBACK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Optional per-call timing sink. The callback must remain loaded until cleared;
/// it runs after releasing the client mutex. None disables all clock reads.
#[unsafe(no_mangle)]
pub extern "C" fn sws_set_timing_callback(callback: Option<TimingCallback>) {
    TIMING_CALLBACK.store(
        callback.map_or(0, |callback| callback as usize),
        std::sync::atomic::Ordering::Release,
    );
}
struct CallTiming {
    callback: TimingCallback,
    operation: u32,
    started: std::time::Instant,
    acquired: Option<std::time::Instant>,
}
impl CallTiming {
    fn new(operation: u32) -> Option<Self> {
        let address = TIMING_CALLBACK.load(std::sync::atomic::Ordering::Acquire);
        if address == 0 {
            return None;
        }
        Some(Self {
            callback: unsafe { std::mem::transmute::<usize, TimingCallback>(address) },
            operation,
            started: std::time::Instant::now(),
            acquired: None,
        })
    }
}
impl Drop for CallTiming {
    fn drop(&mut self) {
        let ended = std::time::Instant::now();
        let acquired = self.acquired.unwrap_or(ended);
        (self.callback)(
            self.operation,
            acquired.duration_since(self.started).as_nanos() as u64,
            ended.duration_since(acquired).as_nanos() as u64,
        );
    }
}
fn call(f: impl FnOnce(&mut Client) -> Result<i32, Error>) -> i32 {
    call_profiled(0, f)
}
fn call_profiled(operation: u32, f: impl FnOnce(&mut Client) -> Result<i32, Error>) -> i32 {
    let mut timing = if operation == 0 {
        None
    } else {
        CallTiming::new(operation)
    };

    static CLIENT: OnceLock<Mutex<Option<Client>>> = OnceLock::new();
    let mut slot = match CLIENT.get_or_init(|| Mutex::new(None)).lock() {
        Ok(slot) => slot,
        Err(_) => return -3,
    };
    if let Some(timing) = timing.as_mut() {
        timing.acquired = Some(std::time::Instant::now());
    }
    if slot.is_none() {
        let connection = match Connection::connect_default() {
            Ok(connection) => connection,
            Err(_) => return -2,
        };
        *slot = Some(Client {
            connection,
            scenes: scene::Scenes::default(),
            gpu_events: BTreeMap::new(),
            events: VecDeque::new(),
            touch: TouchQueues::default(),
            states: BTreeMap::new(),
            closed: BTreeSet::new(),
            closing: BTreeSet::new(),
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
    unsafe { sws_window_create_ex(app_id, title, width, height, 0, out) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sws_window_create_ex(
    app_id: *const c_char,
    title: *const c_char,
    width: u32,
    height: u32,
    resizable: u32,
    out: *mut u32,
) -> i32 {
    if app_id.is_null() || title.is_null() || out.is_null() || !valid_size(width, height) {
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
            .resizable(resizable != 0)
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

fn valid_size(width: u32, height: u32) -> bool {
    width != 0 && height != 0 && width <= 16384 && height <= 16384
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sws_window_title(id: u32, title: *const c_char) -> i32 {
    if title.is_null() {
        return -1;
    }
    let Ok(title) = (unsafe { CStr::from_ptr(title) }).to_str() else {
        return -1;
    };
    call(|client| {
        client.connection.set_window_title(id, title)?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn sws_window_action(id: u32, action: u32) -> i32 {
    call(|client| {
        if client.closed.contains(&id) || !client.gpu_events.contains_key(&id) {
            return Err(Error::SurfaceNotFound);
        }
        match action {
            1 | 5 => client.connection.restore_window(id)?,
            2 => client.connection.minimize_window(id)?,
            3 => client.connection.focus_window(id)?,
            4 => client.connection.maximize_window(id)?,
            6 => client.connection.request_move_window(id)?,
            _ => return Err(Error::InvalidRequest),
        }
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sws_window_size(id: u32, width: *mut u32, height: *mut u32) -> i32 {
    if width.is_null() || height.is_null() {
        return -1;
    }
    call_profiled(3, |client| {
        client.dispatch()?;
        let (mut w, mut h) = client
            .connection
            .with_surface(id, |s| (s.width(), s.height()))
            .ok_or(Error::SurfaceNotFound)?;
        if client.scenes.contains(id) {
            let r = client
                .scenes
                .content(id, w, h, client.states.get(&id).copied().unwrap_or(0))
                .ok_or(Error::InvalidRequest)?;
            w = r.width;
            h = r.height;
        }
        unsafe {
            width.write(w);
            height.write(h);
        }
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn sws_window_resize(id: u32, width: u32, height: u32) -> i32 {
    if !valid_size(width, height) {
        return -1;
    }
    call(|client| {
        if client.closed.contains(&id) {
            return Err(Error::SurfaceNotFound);
        }
        let (mut w, mut h) = (width, height);
        if client.scenes.contains(id) {
            let (outer_w, outer_h) = client
                .connection
                .with_surface(id, |s| (s.width(), s.height()))
                .ok_or(Error::SurfaceNotFound)?;
            let r = client
                .scenes
                .content(
                    id,
                    outer_w,
                    outer_h,
                    client.states.get(&id).copied().unwrap_or(0),
                )
                .ok_or(Error::InvalidRequest)?;
            w = width
                .checked_add(outer_w - r.width)
                .ok_or(Error::InvalidRequest)?;
            h = height
                .checked_add(outer_h - r.height)
                .ok_or(Error::InvalidRequest)?;
        }
        if !valid_size(w, h) {
            return Err(Error::InvalidRequest);
        }
        client.connection.resize_window(id, w, h)?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sws_window_present(
    id: u32,
    pixels: *const std::ffi::c_void,
    bytes: usize,
    width: u32,
    height: u32,
    pitch: usize,
) -> i32 {
    if pixels.is_null() || !valid_size(width, height) {
        return -1;
    }
    let row = width as usize * 4;
    let Some(required) = (height as usize - 1)
        .checked_mul(pitch)
        .and_then(|n| n.checked_add(row))
    else {
        return -1;
    };
    if pitch < row || bytes < required || required > isize::MAX as usize {
        return -1;
    }
    call(|client| {
        if client.closed.contains(&id) {
            return Err(Error::SurfaceNotFound);
        }
        client
            .connection
            .with_surface_mut(id, |surface| {
                if surface.width() != width || surface.height() != height {
                    return Err(Error::InvalidRequest);
                }
                // SAFETY: the C caller borrows a readable, non-overlapping frame for
                // this call; bounds above cover every row. No pointer escapes.
                let source = unsafe { std::slice::from_raw_parts(pixels.cast::<u8>(), required) };
                for (y, target) in surface.buffer_mut().chunks_exact_mut(row).enumerate() {
                    target.copy_from_slice(&source[y * pitch..y * pitch + row]);
                }
                Ok(())
            })
            .ok_or(Error::SurfaceNotFound)??;
        client.connection.commit(id)?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn sws_window_destroy(id: u32) -> i32 {
    call(|client| {
        if !client.closed.contains(&id) && !client.closing.contains(&id) {
            client.connection.destroy_surface(id)?;
        }
        client.scenes.remove(&client.connection, id)?;
        client.gpu_events.remove(&id);
        client.states.remove(&id);
        client.closed.remove(&id);
        client.closing.remove(&id);
        client.touch.remove(id);
        client.events.retain(|event| event.window_id != id);
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn sws_window_destroy_sync(id: u32, timeout_ms: u32) -> i32 {
    call(|client| {
        client.dispatch()?;
        if !client.gpu_events.contains_key(&id) {
            return Err(Error::SurfaceNotFound);
        }
        if !client.closed.contains(&id) && !client.closing.contains(&id) {
            client.connection.destroy_surface(id)?;
            client.closing.insert(id);
        }
        let started = std::time::Instant::now();
        while !client.closed.contains(&id) {
            client.dispatch()?;
            if client.closed.contains(&id) {
                break;
            }
            if started.elapsed() >= std::time::Duration::from_millis(timeout_ms as u64) {
                return Err(Error::TimedOut);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        client.scenes.remove(&client.connection, id)?;
        client.gpu_events.remove(&id);
        client.states.remove(&id);
        client.closed.remove(&id);
        client.closing.remove(&id);
        client.touch.remove(id);
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

/// Enable or disable native direct-touch frames for an owned window.
/// Disabling asks SWS to cancel its active native contacts.
#[unsafe(no_mangle)]
pub extern "C" fn sws_window_set_touch_input(window_id: u32, enabled: i32) -> i32 {
    call(|client| {
        if client.closed.contains(&window_id) || !client.gpu_events.contains_key(&window_id) {
            return Err(Error::SurfaceNotFound);
        }
        client.connection.set_touch_input(window_id, enabled != 0)?;
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

/// Copy the oldest complete touch frame for an owned window when it fits.
/// An insufficient capacity leaves the frame queued and returns `-1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sws_poll_touch_frame(
    window_id: u32,
    frame: *mut SwsTouchFrame,
    changes: *mut SwsTouchChange,
    capacity: u32,
) -> i32 {
    if frame.is_null() || changes.is_null() {
        return -1;
    }
    call(|client| {
        client.dispatch()?;
        if client.closed.contains(&window_id) || !client.gpu_events.contains_key(&window_id) {
            return Err(Error::SurfaceNotFound);
        }
        // SAFETY: both outputs are non-null and the caller promises writable
        // storage for `capacity` change records.
        Ok(unsafe {
            if copy_touch_frame(&mut client.touch, window_id, frame, changes, capacity)? {
                1
            } else {
                0
            }
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn sws_gpu_register(
    buffer: SwsBuffer,
    width: u32,
    height: u32,
    raw_handle: i32,
) -> i32 {
    gpu_register(buffer, width, height, raw_handle, false)
}

/// Register native opaque content without interpreting its stored alpha.
#[unsafe(no_mangle)]
pub extern "C" fn sws_gpu_register_opaque(
    buffer: SwsBuffer,
    width: u32,
    height: u32,
    raw_handle: i32,
) -> i32 {
    gpu_register(buffer, width, height, raw_handle, true)
}

fn gpu_register(buffer: SwsBuffer, width: u32, height: u32, raw_handle: i32, opaque: bool) -> i32 {
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
        if client.scenes.contains(buffer.window_id) {
            client
                .scenes
                .register(&client.connection, buffer, width, height, &handle, opaque)?;
            return Ok(0);
        }
        client
            .connection
            .register_sgfx_buffer(buffer.into(), width, height, &handle)?;
        Ok(0)
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn sws_gpu_commit(buffer: SwsBuffer, serial: u64, width: u32, height: u32) -> i32 {
    call_profiled(2, |client| {
        if client.scenes.contains(buffer.window_id) {
            client.scenes.commit(
                &client.connection,
                buffer,
                serial,
                width,
                height,
                client.states.get(&buffer.window_id).copied().unwrap_or(0),
            )?;
            return Ok(0);
        }
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
        if client.scenes.contains(buffer.window_id) {
            client.scenes.destroy(&client.connection, buffer)?;
        } else {
            client.connection.destroy_sgfx_buffer(buffer.into())?;
        }
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sws_gpu_poll(id: u32, out: *mut SwsGpuEvent) -> i32 {
    if out.is_null() {
        return -1;
    }
    call_profiled(4, |client| {
        client.dispatch()?;
        if client.closed.contains(&id) {
            return Err(Error::SurfaceNotFound);
        }
        if let Some(event) = client.scenes.poll(id) {
            unsafe {
                out.write(event);
            }
            return Ok(1);
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

#[cfg(test)]
mod tests {
    use super::*;
    use sws_protocol::touch::{Change, Frame, Phase};

    fn change(id: u64, phase: Phase) -> Change {
        Change {
            id,
            phase,
            x: -(id as i32) + 3,
            y: id as i32 + 7,
            pressure: Some(id as i32 + 11),
            touch_major: Some(id as i32 + 13),
        }
    }

    fn frame(serial: u64) -> Frame {
        Frame {
            seat_id: 9,
            serial,
            time_ns: 1_000 + serial,
            changes: vec![
                change(10, Phase::Down),
                change(20, Phase::Move),
                change(30, Phase::Up),
                Change {
                    pressure: None,
                    touch_major: None,
                    ..change(40, Phase::Cancel)
                },
            ],
        }
    }

    #[test]
    fn touch_abi_layout_and_conversion_preserve_frame_fields() {
        assert_eq!(core::mem::size_of::<SwsTouchFrame>(), 32);
        assert_eq!(core::mem::size_of::<SwsTouchChange>(), 32);

        let mut queues = TouchQueues::default();
        assert_eq!(queues.push(77, frame(123)), TouchQueueResult::Queued);
        let queued = queues.pop(77).expect("queued frame");

        assert_eq!(
            queued.frame,
            SwsTouchFrame {
                window_id: 77,
                seat_id: 9,
                serial: 123,
                time_ns: 1_123,
                change_count: 4,
                reserved: 0,
            }
        );
        assert_eq!(
            queued.changes,
            vec![
                SwsTouchChange {
                    id: 10,
                    phase: 0,
                    x: -7,
                    y: 17,
                    pressure: 21,
                    touch_major: 23,
                    reserved: 0
                },
                SwsTouchChange {
                    id: 20,
                    phase: 1,
                    x: -17,
                    y: 27,
                    pressure: 31,
                    touch_major: 33,
                    reserved: 0
                },
                SwsTouchChange {
                    id: 30,
                    phase: 2,
                    x: -27,
                    y: 37,
                    pressure: 41,
                    touch_major: 43,
                    reserved: 0
                },
                SwsTouchChange {
                    id: 40,
                    phase: 3,
                    x: -37,
                    y: 47,
                    pressure: -1,
                    touch_major: -1,
                    reserved: 0
                },
            ]
        );
        assert!(queues.pop(77).is_none());
    }

    #[test]
    fn touch_queue_is_per_window_bounded_and_removed_with_the_window() {
        let mut queues = TouchQueues::default();
        let mut events = VecDeque::new();

        for serial in 0..MAX_PENDING_TOUCH_FRAMES as u64 {
            assert_eq!(
                queue_touch_frame(&mut queues, &mut events, 4, frame(serial)),
                TouchQueueResult::Queued
            );
        }
        assert_eq!(events.len(), MAX_PENDING_TOUCH_FRAMES);
        assert!(
            events
                .iter()
                .all(|event| event.window_id == 4 && event.kind == 8)
        );

        assert_eq!(
            queue_touch_frame(&mut queues, &mut events, 4, frame(64)),
            TouchQueueResult::Overflowed
        );
        assert_eq!(events.back().unwrap().kind, 7);
        assert_eq!(
            queue_touch_frame(&mut queues, &mut events, 4, frame(65)),
            TouchQueueResult::AlreadyOverflowed
        );
        assert_eq!(events.len(), MAX_PENDING_TOUCH_FRAMES + 1);

        assert_eq!(
            queue_touch_frame(&mut queues, &mut events, 5, frame(200)),
            TouchQueueResult::Queued
        );
        queues.remove(4);
        assert!(queues.pop(4).is_none());
        assert!(queues.pop(5).is_some());
        assert_eq!(
            queue_touch_frame(&mut queues, &mut events, 4, frame(300)),
            TouchQueueResult::Queued
        );
    }

    #[test]
    fn touch_copy_requires_complete_capacity_and_consumes_only_after_success() {
        let mut queues = TouchQueues::default();
        assert_eq!(queues.push(8, frame(400)), TouchQueueResult::Queued);
        let mut output = SwsTouchFrame::default();
        let mut changes = [SwsTouchChange::default(); 4];

        // SAFETY: both output buffers cover their stated capacities.
        assert_eq!(
            unsafe { copy_touch_frame(&mut queues, 8, &mut output, changes.as_mut_ptr(), 3) },
            Err(Error::InvalidRequest)
        );
        assert_eq!(
            queues.frames.get(&8).map(VecDeque::len),
            Some(1),
            "insufficient capacity must not consume the frame"
        );

        let mut output = SwsTouchFrame::default();
        // SAFETY: both output buffers cover their stated capacities.
        assert_eq!(
            unsafe { copy_touch_frame(&mut queues, 8, &mut output, changes.as_mut_ptr(), 4) },
            Ok(true)
        );
        assert_eq!(output.serial, 400);
        assert_eq!(changes[0].id, 10);
        assert_eq!(changes[3].phase, 3);
        assert_eq!(changes[3].pressure, -1);
        assert!(queues.frames.get(&8).is_none());
    }
}
