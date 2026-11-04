//! Native owner of Android's outer window and input. The Android HAL supplies
//! only CPU scanout frames on the inherited transport; it never owns this UI.
mod input;
mod protocol;
use scarlet_os::{Handle, handle::capability::StreamError, socket::Socket};
use scarlet_ui_core::{
    event::{MouseButton, MouseEvent, WindowEvent},
    window_chrome::{WindowChrome, chrome_insets},
};
use std::time::Duration;
use sws_client_c::{
    self as sws, SwsEvent,
    scene::{self, SwsInsets, SwsRect, SwsSceneFrame, SwsSceneGeometry},
};
type Result<T> = std::result::Result<T, &'static str>;
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
}
impl Window {
    fn new(width: u32, height: u32) -> Result<Self> {
        let mut scale = 1000;
        check(unsafe { scene::sws_get_output_scale(&mut scale) })?;
        let inset = chrome_insets(scale, true);
        let mut id = 0;
        check(unsafe {
            scene::sws_scene_create(
                c"Android".as_ptr(),
                width + inset.left + inset.right,
                height + inset.top + inset.bottom,
                1,
                &mut id,
            )
        })?;
        let geometry = SwsSceneGeometry {
            width: width + inset.left + inset.right,
            height: height + inset.top + inset.bottom,
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
        })
    }
    fn refresh(&mut self) -> Result<()> {
        check(unsafe { scene::sws_scene_geometry(self.id, &mut self.geometry) })?;
        self.frame = WindowChrome::new(
            "Android",
            self.geometry.width,
            self.geometry.height,
            self.geometry.scale_milli,
            self.geometry.state & (4 | 16) == 0,
            true,
        )?;
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
                WindowEvent::MaximizeRequested => check(sws::sws_window_action(self.id, 4))?,
                WindowEvent::RestoreRequested => check(sws::sws_window_action(self.id, 5))?,
            }
        }
        Ok(close)
    }
    fn event(&mut self, event: SwsEvent, input: &mut input::Input) -> Result<bool> {
        if event.window_id != self.id {
            return Ok(false);
        }
        match event.kind {
            3 | 7 => return Ok(true),
            2 | 5 | 6 => {
                self.refresh()?;
                self.present()?;
            }
            4 if event.value == 0 => {
                input.release()?;
                self.pointer(MouseEvent::ButtonCancelled {
                    button: MouseButton::Left,
                    x: self.x,
                    y: self.y,
                })?;
            }
            1 => {
                if matches!(event.type_, 2 | 3) && event.code <= 1 {
                    let value = if event.code == 0 {
                        &mut self.x
                    } else {
                        &mut self.y
                    };
                    *value = if event.type_ == 3 {
                        event.value
                    } else {
                        value.saturating_add(event.value)
                    };
                    self.pointer(MouseEvent::Moved {
                        x: self.x,
                        y: self.y,
                    })?;
                    let r = self.frame.content();
                    let fit = input::fit(
                        SwsRect {
                            x: r.x,
                            y: r.y,
                            width: r.width,
                            height: r.height,
                        },
                        self.width,
                        self.height,
                    );
                    input.pointer(fit, self.x, self.y, None, false)?;
                } else if event.type_ == 1 && matches!(event.code, 0x110 | 0x14a) {
                    let event = if event.value != 0 {
                        MouseEvent::ButtonPressed {
                            button: MouseButton::Left,
                            x: self.x,
                            y: self.y,
                            click_count: 1,
                        }
                    } else {
                        MouseEvent::ButtonReleased {
                            button: MouseButton::Left,
                            x: self.x,
                            y: self.y,
                            click_count: 1,
                        }
                    };
                    if self.pointer(event)? {
                        return Ok(true);
                    }
                    let r = self.frame.content();
                    let fit = input::fit(
                        SwsRect {
                            x: r.x,
                            y: r.y,
                            width: r.width,
                            height: r.height,
                        },
                        self.width,
                        self.height,
                    );
                    let inside = input::contains(fit, self.x, self.y)
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
                        });
                    input.pointer(fit, self.x, self.y, Some(event_value(event)), inside)?;
                } else if event.type_ == 1 {
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
fn event_value(event: MouseEvent) -> bool {
    matches!(event, MouseEvent::ButtonPressed { .. })
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
    let args: Vec<_> = std::env::args()
        .skip(1)
        .map(|v| v.parse::<u32>())
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| "display arguments")?;
    if args.len() != 5
        || args[..3].iter().any(|v| *v > i32::MAX as u32)
        || args[0] == args[1]
        || args[0] == args[2]
        || args[1] == args[2]
        || !(2..=4096).contains(&args[3])
        || !(2..=4096).contains(&args[4])
    {
        return Err(
            "usage: android-runtime listener-handle keyboard-producer touch-producer width height",
        );
    }
    let server = Socket::from_handle(owned(args[0])?).map_err(|_| "display listener")?;
    server
        .set_nonblocking(true)
        .map_err(|_| "display listener mode")?;
    let mut input = input::Input::new(owned(args[1])?, owned(args[2])?, args[3], args[4]);
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
                                    || header.width != args[3]
                                    || header.height != args[4]
                                {
                                    return Err("display dimensions");
                                }
                                let mut value = Window::new(header.width, header.height)?;
                                value.present()?;
                                window = Some(value);
                            }
                            protocol::FRAME => {
                                let window = window.as_mut().ok_or("display handshake missing")?;
                                if header.width != window.width || header.height != window.height {
                                    return Err("display scanout changed");
                                }
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
        std::thread::sleep(Duration::from_millis(8));
    }
}
fn main() {
    if let Err(error) = run() {
        eprintln!("android-runtime: {error}");
        std::process::exit(1);
    }
}
