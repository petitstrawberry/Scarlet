//! Native owner of Android's outer window and input. The Android HAL supplies
//! only CPU scanout frames on the inherited transport; it never owns this UI.
mod input;
mod bridge;
mod mouse_wire;
mod protocol;
use scarlet_os::{
    Handle,
    handle::capability::{StreamError, file::SeekFrom},
    socket::Socket,
};
use scarlet_ui_core::{
    event::{MouseButton, MouseEvent, WindowEvent},
    window_chrome::{WindowChrome, chrome_insets},
};
use std::time::Duration;
use sws_client_c::{
    self as sws, SwsEvent, SwsTouchChange, SwsTouchFrame,
    scene::{self, SwsInsets, SwsRect, SwsSceneFrame, SwsSceneGeometry},
};
type Result<T> = std::result::Result<T, &'static str>;
const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const EV_ABS: u16 = 3;
const SYN_REPORT: u16 = 0;
const SYN_DROPPED: u16 = 3;
enum PendingMouseEvent {
    Button { code: u16, value: i32 },
    Wheel { code: u16, value: i32 },
}

fn check(result: i32) -> Result<()> {
    if result < 0 {
        Err("SWS display operation")
    } else {
        Ok(())
    }
}
struct Window {
    id: u32,
    frame: WindowChrome,
    geometry: SwsSceneGeometry,
    chrome: Vec<u8>,
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    pending_x: Option<i32>,
    pending_y: Option<i32>,
    pending_mouse: Vec<PendingMouseEvent>,
    frame_seen: bool,
    expected_scanout: Option<(u32, u32)>,
    pending_resize: Option<(u32, u32)>,
    resize_settle_ticks: u8,
    maximized: bool,
}
impl Window {
    fn layout(width: u32, height: u32) -> Result<(SwsSceneGeometry, WindowChrome)> {
        let mut scale = 1000;
        check(unsafe { scene::sws_get_output_scale(&mut scale) })?;
        let inset = chrome_insets(scale, true);
        // Chrome and scanout content use the same physical output scale. The
        // scanout and input endpoints retain their original Android dimensions.
        let content_width = u32::try_from((u64::from(width) * u64::from(scale)).div_ceil(1000))
            .map_err(|_| "display content width")?;
        let content_height = u32::try_from((u64::from(height) * u64::from(scale)).div_ceil(1000))
            .map_err(|_| "display content height")?;
        let mut output = sws::SwsDisplay {
            width: 0, height: 0, compositor_epoch: 0, compositor_backend: 0, capabilities: 0,
        };
        check(unsafe { sws::sws_get_display(&mut output) })?;
        let available_width = output.width.checked_sub(inset.left + inset.right)
            .filter(|w| *w > 0).ok_or("display output width")?;
        let available_height = output.height.checked_sub(inset.top + inset.bottom)
            .filter(|h| *h > 0).ok_or("display output height")?;
        // Keep the entire initial window, including Android's navigation bar,
        // reachable when the scaled scanout exceeds the physical output.
        let fitted = input::fit(SwsRect {
            x: 0, y: 0,
            width: content_width.min(available_width),
            height: content_height.min(available_height),
        }, width, height);
        let outer_width = fitted.width.checked_add(inset.left).and_then(|w| w.checked_add(inset.right))
            .ok_or("display frame width")?;
        let outer_height = fitted.height.checked_add(inset.top).and_then(|h| h.checked_add(inset.bottom))
            .ok_or("display frame height")?;
        let geometry = SwsSceneGeometry {
            width: outer_width,
            height: outer_height,
            scale_milli: scale,
            state: 0,
            content: SwsRect::default(),
        };
        let frame = WindowChrome::new(
            "Android",
            geometry.width,
            geometry.height,
            scale,
            true,
            true,
        )?;
        Ok((geometry, frame))
    }
    fn new(width: u32, height: u32) -> Result<Self> {
        let (geometry, frame) = Self::layout(width, height)?;
        let mut id = 0;
        check(unsafe {
            scene::sws_scene_create(
                c"Android".as_ptr(),
                geometry.width,
                geometry.height,
                1,
                &mut id,
            )
        })?;
        if let Err(error) = check(sws::sws_window_set_touch_input(id, 1)) {
            let _ = sws::sws_window_destroy(id);
            return Err(error);
        }
        Ok(Self {
            id,
            frame,
            geometry,
            chrome: Vec::new(),
            pixels: vec![0; (width * height * 4) as usize],
            width,
            height,
            x: 0,
            y: 0,
            pending_x: None,
            pending_y: None,
            pending_mouse: Vec::new(),
            frame_seen: false,
            expected_scanout: None,
            pending_resize: None,
            resize_settle_ticks: 0,
            maximized: false,
        })
    }
    fn resize_scanout(&mut self, width: u32, height: u32, resize_outer: bool) -> Result<bool> {
        if width == self.width && height == self.height {
            return Ok(false);
        }
        if resize_outer {
            let (geometry, frame) = Self::layout(width, height)?;
            check(scene::sws_scene_resize(self.id, geometry.width, geometry.height))?;
            self.geometry = geometry;
            self.frame = frame;
        }
        self.width = width;
        self.height = height;
        self.pixels.resize((width * height * 4) as usize, 0);
        Ok(true)
    }
    fn requested_scanout_size(&self) -> (u32, u32) {
        let content = self.frame.content();
        let scale = u64::from(self.geometry.scale_milli.max(1));
        let width = (u64::from(content.width) * 1000 / scale).clamp(2, 4096) as u32;
        let height = (u64::from(content.height) * 1000 / scale).clamp(2, 4096) as u32;
        (width, height)
    }
    fn take_pending_resize(&mut self) -> Option<(u32, u32)> {
        if self.resize_settle_ticks != 0 {
            self.resize_settle_ticks -= 1;
            return None;
        }
        let requested = self.pending_resize.take()?;
        if requested == (self.width, self.height) {
            return None;
        }
        self.expected_scanout = Some(requested);
        Some(requested)
    }
    fn refresh(&mut self) -> Result<()> {
        let previous_size = (self.geometry.width, self.geometry.height);
        check(unsafe { scene::sws_scene_geometry(self.id, &mut self.geometry) })?;
        let geometry_changed = previous_size != (self.geometry.width, self.geometry.height);
        self.frame = WindowChrome::new(
            "Android",
            self.geometry.width,
            self.geometry.height,
            self.geometry.scale_milli,
            self.geometry.state & (4 | 16) == 0,
            true,
        )?;
        self.maximized = self.geometry.state & 2 != 0;
        if geometry_changed {
            let content = self.frame.content();
            if self.frame_seen {
                self.pending_resize = Some(self.requested_scanout_size());
                self.resize_settle_ticks = 15;
            }
            eprintln!(
                "android-runtime: SWS window {}x{} -> {}x{}, content {}x{}",
                previous_size.0,
                previous_size.1,
                self.geometry.width,
                self.geometry.height,
                content.width,
                content.height
            );
        }
        self.chrome.clear();
        Ok(())
    }
    fn present(&mut self) -> Result<()> {
        if let Some(buffer) = self.frame.render() {
            self.chrome = buffer.data().to_vec();
        }
        let clip: Vec<_> = self
            .frame
            .clip()
            .iter()
            .map(|r| SwsRect {
                x: r.x,
                y: r.y,
                width: r.width,
                height: r.height,
            })
            .collect();
        let inset = self.frame.insets();
        let frame = SwsSceneFrame {
            width: self.geometry.width,
            height: self.geometry.height,
            stride: self.geometry.width * 4,
            insets: SwsInsets {
                left: inset.left,
                top: inset.top,
                right: inset.right,
                bottom: inset.bottom,
            },
            pixels: self.chrome.as_ptr().cast(),
            bytes: self.chrome.len(),
            clip: clip.as_ptr(),
            clip_count: clip.len(),
        };
        check(unsafe {
            scene::sws_scene_cpu_present(
                self.id,
                &frame,
                self.pixels.as_ptr().cast(),
                self.pixels.len(),
                self.width,
                self.height,
                self.width as usize * 4,
            )
        })
    }
    fn pointer(&mut self, event: MouseEvent) -> Result<bool> {
        let mut close = false;
        for action in self.frame.mouse(event) {
            match action {
                WindowEvent::CloseRequested => close = true,
                WindowEvent::MoveRequested => check(sws::sws_window_action(self.id, 6))?,
                WindowEvent::MinimizeRequested => check(sws::sws_window_action(self.id, 2))?,
                WindowEvent::MaximizeRequested | WindowEvent::RestoreRequested => {
                    let action = if self.maximized { 5 } else { 4 };
                    check(sws::sws_window_action(self.id, action))?;
                    self.maximized = !self.maximized;
                }
            }
        }
        Ok(close)
    }
    fn content_fit(&self) -> SwsRect {
        let r = self.frame.content();
        input::fit(
            SwsRect {
                x: r.x,
                y: r.y,
                width: r.width,
                height: r.height,
            },
            self.width,
            self.height,
        )
    }
    fn inside_content(&self, fit: SwsRect) -> bool {
        input::contains(fit, self.x, self.y)
            && self.frame.clip().iter().any(|r| {
                input::contains(
                    SwsRect {
                        x: r.x,
                        y: r.y,
                        width: r.width,
                        height: r.height,
                    },
                    self.x,
                    self.y,
                )
            })
    }
    fn poll_touch(&self, input: &mut input::Input) -> Result<()> {
        let mut header = SwsTouchFrame {
            window_id: 0,
            seat_id: 0,
            serial: 0,
            time_ns: 0,
            change_count: 0,
            reserved: 0,
        };
        // SWS accepts up to 128 changes in one touch frame.
        let mut changes: [SwsTouchChange; 128] = std::array::from_fn(|_| SwsTouchChange {
            id: 0,
            phase: 0,
            x: 0,
            y: 0,
            pressure: 0,
            touch_major: 0,
            reserved: 0,
        });
        // Each kind-8 marker represents exactly one queued frame. Consuming one
        // here preserves its ordering relative to later mouse and focus events.
        let count = unsafe {
            sws::sws_poll_touch_frame(
                self.id,
                &mut header,
                changes.as_mut_ptr(),
                changes.len() as u32,
            )
        };
        if count < 0 {
            return Err("SWS touch poll");
        }
        if count == 0 {
            return Ok(());
        }
        if header.window_id != self.id || header.change_count as usize > changes.len() {
            return Err("SWS touch frame");
        }
        let fit = self.content_fit();
        for change in &changes[..header.change_count as usize] {
            let phase = match change.phase {
                0 => input::TouchPhase::Down,
                1 => input::TouchPhase::Move,
                2 => input::TouchPhase::Up,
                3 => input::TouchPhase::Cancel,
                _ => continue,
            };
            input.touch(
                fit,
                input::TouchChange {
                    id: change.id,
                    phase,
                    x: change.x,
                    y: change.y,
                },
            )?;
        }
        Ok(())
    }
    fn discard_mouse_frame(&mut self) {
        self.pending_x = None;
        self.pending_y = None;
        self.pending_mouse.clear();
    }
    fn commit_mouse_frame(&mut self, input: &mut input::Input) -> Result<bool> {
        let received_position = self.pending_x.is_some() || self.pending_y.is_some();
        if let Some(x) = self.pending_x.take() {
            self.x = x;
        }
        if let Some(y) = self.pending_y.take() {
            self.y = y;
        }
        if received_position {
            if self.pointer(MouseEvent::Moved {
                x: self.x,
                y: self.y,
            })? {
                return Ok(true);
            }
            let fit = self.content_fit();
            let inside = self.inside_content(fit);
            input.mouse_motion(fit, self.x, self.y, inside)?;
        }

        let pending = std::mem::take(&mut self.pending_mouse);
        for event in pending {
            match event {
                PendingMouseEvent::Button { code, value } => {
                    let button = pointer_button(code);
                    let mouse = if value != 0 {
                        MouseEvent::ButtonPressed {
                            button,
                            x: self.x,
                            y: self.y,
                            click_count: 1,
                        }
                    } else {
                        MouseEvent::ButtonReleased {
                            button,
                            x: self.x,
                            y: self.y,
                            click_count: 1,
                        }
                    };
                    if self.pointer(mouse)? {
                        return Ok(true);
                    }
                    let fit = self.content_fit();
                    let inside = self.inside_content(fit);
                    input.mouse_button(fit, self.x, self.y, code, value, inside)?;
                }
                PendingMouseEvent::Wheel { code, value } => {
                    let fit = self.content_fit();
                    let inside = self.inside_content(fit);
                    input.mouse_wheel(fit, self.x, self.y, code, value, inside)?;
                }
            }
        }
        Ok(false)
    }
    fn cancel_input(&mut self, input: &mut input::Input) -> Result<()> {
        self.discard_mouse_frame();
        input.release()?;
        for button in [MouseButton::Left, MouseButton::Middle, MouseButton::Right] {
            self.pointer(MouseEvent::ButtonCancelled {
                button,
                x: self.x,
                y: self.y,
            })?;
        }
        Ok(())
    }
    fn event(&mut self, event: SwsEvent, input: &mut input::Input) -> Result<bool> {
        if event.window_id != self.id {
            return Ok(false);
        }
        match event.kind {
            3 | 7 => {
                self.cancel_input(input)?;
                return Ok(true);
            }
            8 => self.poll_touch(input)?,
            2 | 5 | 6 => {
                self.refresh()?;
                self.present()?;
            }
            4 if event.value == 0 => {
                self.cancel_input(input)?;
            }
            1 => {
                if event.type_ == EV_ABS && event.code <= 1 {
                    if event.code == 0 {
                        self.pending_x = Some(event.value);
                    } else {
                        self.pending_y = Some(event.value);
                    }
                } else if event.type_ == EV_SYN && event.code == SYN_REPORT {
                    if self.commit_mouse_frame(input)? {
                        return Ok(true);
                    }
                } else if event.type_ == EV_SYN && event.code == SYN_DROPPED {
                    self.discard_mouse_frame();
                    input.mouse_cancel();
                } else if event.type_ == EV_REL && matches!(event.code, 6 | 8) {
                    self.pending_mouse.push(PendingMouseEvent::Wheel {
                        code: event.code,
                        value: event.value,
                    });
                } else if event.type_ == EV_KEY && matches!(event.code, 0x110..=0x112) {
                    self.pending_mouse.push(PendingMouseEvent::Button {
                        code: event.code,
                        value: event.value,
                    });
                } else if event.type_ == EV_KEY {
                    input.key(event.code, event.value)?;
                }
            }
            _ => {}
        }
        if self.frame.has_dirty() {
            self.present()?;
        }
        Ok(false)
    }
}
fn pointer_button(code: u16) -> MouseButton {
    match code {
        0x110 => MouseButton::Left,
        0x111 => MouseButton::Right,
        0x112 => MouseButton::Middle,
        _ => unreachable!(),
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        let _ = sws::sws_window_destroy(self.id);
    }
}
fn owned(raw: u32) -> Result<Handle> {
    // SAFETY: command arguments designate distinct owned descriptors explicitly
    // transferred by the launcher. Adoption consumes each descriptor once.
    unsafe { Handle::from_raw(raw as i32) }.map_err(|_| "inherited display handle")
}
fn run() -> Result<()> {
    let raw_args: Vec<_> = std::env::args().skip(1).collect();
    if raw_args.first().map(String::as_str) == Some("--input-bridge") {
        return bridge::run(&raw_args[1..]);
    }
    let args: Vec<_> = raw_args.iter()
        .map(|v| v.parse::<u32>())
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| "display arguments")?;
    if args.len() != 7
        || args[..5].iter().any(|v| *v > i32::MAX as u32)
        || args[0] == args[1]
        || args[0] == args[2]
        || args[0] == args[3]
        || args[1] == args[2]
        || args[1] == args[3]
        || args[2] == args[3]
        || args[..4].contains(&args[4])
        || !(2..=4096).contains(&args[5])
        || !(2..=4096).contains(&args[6])
    {
        return Err(
            "usage: android-runtime listener-handle keyboard-producer touch-producer mouse-pipe display-control width height",
        );
    }
    let server = Socket::from_handle(owned(args[0])?).map_err(|_| "display listener")?;
    let display_control = owned(args[4])?;
    let display_control_file = display_control
        .as_file()
        .map_err(|_| "display control file")?;
    let display_control = display_control
        .as_stream()
        .map_err(|_| "display control stream")?;
    server
        .set_nonblocking(true)
        .map_err(|_| "display listener mode")?;
    let mut input = input::Input::new(
        owned(args[1])?,
        owned(args[2])?,
        owned(args[3])?,
        args[5],
        args[6],
    );
    let mut peer: Option<Socket> = None;
    let mut window: Option<Window> = None;
    let mut bytes = Vec::new();
    let mut acknowledgement = Vec::new();
    let mut ack_sent = 0;
    let mut sequence = 1u64;
    loop {
        if peer.is_none() {
            match server.accept() {
                Ok(socket) => {
                    socket
                        .set_nonblocking(true)
                        .map_err(|_| "display stream mode")?;
                    peer = Some(socket);
                }
                Err(scarlet_os::socket::SocketError::WouldBlock) => {}
                Err(_) => return Err("display accept"),
            }
        }
        if let Some(socket) = peer.as_ref() {
            let stream = socket
                .as_handle()
                .as_stream()
                .map_err(|_| "display stream")?;
            if !acknowledgement.is_empty() {
                match stream.write(&acknowledgement[ack_sent..]) {
                    Ok(0) => return Err("display acknowledgement closed"),
                    Ok(count) => {
                        ack_sent += count;
                        if ack_sent == acknowledgement.len() {
                            acknowledgement.clear();
                            ack_sent = 0;
                        }
                    }
                    Err(StreamError::WouldBlock) => {}
                    Err(_) => return Err("display acknowledgement"),
                }
            } else {
                for _ in 0..128 {
                    let mut chunk = [0; 16384];
                    match stream.read(&mut chunk) {
                        Ok(0) => return Ok(()),
                        Ok(count) => {
                            bytes.extend_from_slice(&chunk[..count]);
                            if bytes.len() > protocol::MAX_BYTES + protocol::HEADER {
                                return Err("display frame bound");
                            }
                        }
                        Err(StreamError::WouldBlock | StreamError::Interrupted) => break,
                        Err(StreamError::EndOfStream) => return Ok(()),
                        Err(_) => return Err("display frame read"),
                    }
                }
                if bytes.len() >= protocol::HEADER {
                    let header = protocol::Header::decode(&bytes[..protocol::HEADER])?;
                    if header.sequence != sequence {
                        return Err("display sequence");
                    }
                    if bytes.len() >= protocol::HEADER + header.bytes {
                        if bytes.len() != protocol::HEADER + header.bytes {
                            return Err("display request overlap");
                        }
                        match header.kind {
                            protocol::HELLO => {
                                if window.is_some()
                                    || header.width != args[5]
                                    || header.height != args[6]
                                {
                                    return Err("display dimensions");
                                }
                                let mut value = Window::new(header.width, header.height)?;
                                value.present()?;
                                window = Some(value);
                            }
                            protocol::FRAME => {
                                let window = window.as_mut().ok_or("display handshake missing")?;
                                let requested = window.expected_scanout.take();
                                if window.resize_scanout(
                                    header.width,
                                    header.height,
                                    requested != Some((header.width, header.height)),
                                )? {
                                    input.resize(header.width, header.height)?;
                                }
                                window.frame_seen = true;
                                window.pixels.clear();
                                window.pixels.extend_from_slice(&bytes[protocol::HEADER..]);
                                window.present()?;
                            }
                            protocol::VISIBLE => {
                                let window = window.as_ref().ok_or("display handshake missing")?;
                                check(sws::sws_window_action(
                                    window.id,
                                    if header.width == 0 { 2 } else { 1 },
                                ))?;
                            }
                            protocol::CLOSE => return Ok(()),
                            _ => unreachable!(),
                        }
                        bytes.clear();
                        acknowledgement.extend_from_slice(&header.acknowledgement(0));
                        sequence = sequence
                            .checked_add(1)
                            .ok_or("display sequence exhausted")?;
                    }
                }
            }
        }
        if let Some(window) = window.as_mut() {
            for _ in 0..128 {
                let mut event = SwsEvent::default();
                let count = unsafe { sws::sws_poll_event(&mut event) };
                check(count)?;
                if count == 0 {
                    break;
                }
                if window.event(event, &mut input)? {
                    return Ok(());
                }
            }
        }
        input.flush_mouse();
        if let Some((width, height)) = window
            .as_mut()
            .and_then(Window::take_pending_resize)
        {
            let mut request = [0u8; 8];
            request[..4].copy_from_slice(&width.to_le_bytes());
            request[4..].copy_from_slice(&height.to_le_bytes());
            display_control_file
                .seek(SeekFrom::Start(0))
                .map_err(|_| "display control seek")?;
            display_control
                .write_all(&request)
                .map_err(|_| "display control write")?;
        }
        std::thread::sleep(Duration::from_millis(8));
    }
}
fn main() {
    if let Err(error) = run() {
        eprintln!("android-runtime: {error}");
        std::process::exit(1);
    }
}
