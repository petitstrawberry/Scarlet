//! RFB 3.8 server with negotiated ZRLE/Raw encoding and DesktopSize support.

mod keysym;
mod rfb;

use std::collections::{BTreeSet, VecDeque};
use std::io::{self, ErrorKind, Write};
use std::net::{TcpListener, TcpStream};
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::vec::Vec;
use sws_remote_protocol::{ClientMessage as SwsMessage, Rect};

use self::rfb::{ClientMessage, FramebufferEncoding, FramebufferUpdateRequest, PixelOrder};
use super::sws::DesktopState;

const CLIENT_LOOP_DELAY_MS: u64 = 2;
const MAX_CLIENT_OUTPUT_BYTES: usize = 128 * 1024 * 1024;
const WRITE_CHUNK_BYTES: usize = 64 * 1024;

struct ClientFramebufferState {
    supports_desktop_size: bool,
    encoding: FramebufferEncoding,
    pixel_order: PixelOrder,
    zrle_encoder: rfb::ZrleEncoder,
    last_sequence: Option<u64>,
    width: u32,
    height: u32,
}

impl ClientFramebufferState {
    fn new(width: u32, height: u32) -> Self {
        Self {
            supports_desktop_size: false,
            encoding: FramebufferEncoding::Raw,
            pixel_order: PixelOrder::Bgr,
            zrle_encoder: rfb::ZrleEncoder::new(),
            last_sequence: None,
            width,
            height,
        }
    }
}

/// Accept and serve RFB clients against the persistent SWS capture.
///
/// Clients are served serially to keep capture demand and compression work
/// bounded to one active desktop session. A disconnected client returns
/// control to the accept loop for the next connection.
///
/// # Arguments
///
/// * `listener` - Bound TCP listener.
/// * `state` - Shared captured desktop state.
///
/// # Returns
///
/// Failure only if the listener permanently stops accepting clients.
pub(crate) fn serve(listener: TcpListener, state: Arc<DesktopState>) -> ExitCode {
    loop {
        match listener.accept() {
            Ok((stream, peer)) => {
                println!("[remote-desktop] VNC client connected from {peer}");
                if let Err(error) = client_loop(stream, &state) {
                    eprintln!("[remote-desktop] VNC client ended: {error}");
                }
            }
            Err(error) => {
                eprintln!("[remote-desktop] VNC accept failed: {error}");
                if state.is_stopped() {
                    return ExitCode::from(1);
                }
                thread::sleep(core::time::Duration::from_millis(10));
            }
        }
    }
}

fn client_loop(mut stream: TcpStream, state: &DesktopState) -> io::Result<()> {
    let (initial_width, initial_height) = wait_for_output(state)?;
    rfb::handshake(&mut stream, initial_width, initial_height)?;
    stream.set_nonblocking(true)?;

    let mut reader = rfb::MessageReader::new();
    let mut writer = OutputWriter::new();
    let mut pending_request: Option<FramebufferUpdateRequest> = None;
    let mut framebuffer = ClientFramebufferState::new(initial_width, initial_height);
    let mut pressed_keys = BTreeSet::new();
    let mut pointer_buttons = 0u8;

    let result = 'connection: loop {
        let mut progressed = match reader.read_available(&mut stream) {
            Ok(progressed) => progressed,
            Err(error) => break Err(error),
        };
        loop {
            let message = match reader.next_message() {
                Ok(Some(message)) => message,
                Ok(None) => break,
                Err(error) => break 'connection Err(error),
            };
            progressed = true;
            match message {
                ClientMessage::SetPixelFormat(format) => {
                    framebuffer.pixel_order = match format.pixel_order() {
                        Some(pixel_order) => pixel_order,
                        None => {
                            break 'connection Err(io::Error::new(
                                ErrorKind::InvalidData,
                                "only 32bpp little-endian RGB/BGR RFB pixels are supported",
                            ));
                        }
                    };
                }
                ClientMessage::SetEncodings(encodings) => {
                    framebuffer.supports_desktop_size =
                        encodings.contains(&rfb::ENCODING_DESKTOP_SIZE);
                    framebuffer.encoding = FramebufferEncoding::select(&encodings);
                    println!(
                        "[remote-desktop] VNC framebuffer encoding: {:?}",
                        framebuffer.encoding
                    );
                }
                ClientMessage::FramebufferUpdateRequest(request) => {
                    if pending_request.is_none() {
                        state.add_update_request();
                    }
                    pending_request = Some(request);
                }
                ClientMessage::KeyEvent { pressed, keysym } => {
                    if let Some(code) = keysym::scarlet_keycode(keysym) {
                        if pressed {
                            pressed_keys.insert(code);
                        } else {
                            pressed_keys.remove(&code);
                        }
                        state.queue_input(SwsMessage::Key { code, pressed });
                    }
                }
                ClientMessage::PointerEvent { buttons, x, y } => {
                    state.queue_input(SwsMessage::PointerAbsolute {
                        x: i32::from(x),
                        y: i32::from(y),
                    });
                    queue_pointer_buttons(state, pointer_buttons, buttons);
                    queue_pointer_scroll(state, pointer_buttons, buttons);
                    pointer_buttons = buttons;
                }
                ClientMessage::ClientCutText => {}
            }
        }

        if !writer.has_pending()
            && let Some(request) = pending_request
            && let Some(update) = match build_update(state, request, &mut framebuffer) {
                Ok(update) => update,
                Err(error) => break 'connection Err(error),
            }
        {
            if let Err(error) = writer.enqueue(update) {
                break Err(error);
            }
            pending_request = None;
            state.remove_update_request();
            progressed = true;
        }
        match writer.flush(&mut stream) {
            Ok(writer_progressed) => progressed |= writer_progressed,
            Err(error) => break Err(error),
        }

        if state.is_stopped() {
            break Err(io::Error::new(
                ErrorKind::ConnectionAborted,
                "SWS capture connection stopped",
            ));
        }
        if !progressed {
            thread::sleep(core::time::Duration::from_millis(CLIENT_LOOP_DELAY_MS));
        }
    };

    if pending_request.is_some() {
        state.remove_update_request();
    }
    for code in pressed_keys {
        state.queue_input(SwsMessage::Key {
            code,
            pressed: false,
        });
    }
    queue_pointer_buttons(state, pointer_buttons, 0);
    result
}

fn wait_for_output(state: &DesktopState) -> io::Result<(u32, u32)> {
    loop {
        {
            let frame = state.frame();
            if frame.width != 0 && frame.height != 0 {
                return Ok((frame.width, frame.height));
            }
        }
        if state.is_stopped() {
            return Err(io::Error::new(
                ErrorKind::ConnectionAborted,
                "SWS capture connection stopped",
            ));
        }
        thread::sleep(core::time::Duration::from_millis(10));
    }
}

fn build_update(
    state: &DesktopState,
    request: FramebufferUpdateRequest,
    client: &mut ClientFramebufferState,
) -> io::Result<Option<Vec<u8>>> {
    let frame = state.frame();
    let Some(sequence) = frame.sequence else {
        return Ok(None);
    };

    if frame.width != client.width || frame.height != client.height {
        if !client.supports_desktop_size {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "RFB client did not negotiate DesktopSize",
            ));
        }
        let update = rfb::encode_desktop_size(frame.width, frame.height)?;
        client.width = frame.width;
        client.height = frame.height;
        client.last_sequence = None;
        return Ok(Some(update));
    }

    if request.incremental && client.last_sequence == Some(sequence) {
        return Ok(None);
    }
    let full_output = Rect::new(0, 0, frame.width, frame.height);
    let requested = intersect_rect(request.rect, full_output);
    let mut damage = if request.incremental {
        frame.damage_since(client.last_sequence)
    } else {
        requested.into_iter().collect()
    };
    if request.incremental {
        damage = damage
            .into_iter()
            .filter_map(|rect| requested.and_then(|requested| intersect_rect(rect, requested)))
            .collect();
    }
    let update = match client.encoding {
        FramebufferEncoding::Raw => rfb::encode_raw_update(&frame, &damage, client.pixel_order)?,
        FramebufferEncoding::Zrle => {
            let rectangles = rfb::snapshot_zrle_rectangles(&frame, &damage)?;
            drop(frame);
            rfb::encode_zrle_update(&rectangles, &mut client.zrle_encoder, client.pixel_order)?
        }
    };
    // A partial request does not prove that the client has synchronized pixels
    // outside the requested rectangle. Keep the older global sequence so a
    // later full-output request still receives all intervening damage.
    if requested == Some(full_output) {
        client.last_sequence = Some(sequence);
    }
    Ok(Some(update))
}

fn queue_pointer_buttons(state: &DesktopState, previous: u8, current: u8) {
    const BUTTONS: [(u8, u16); 3] = [(1 << 0, 0x110), (1 << 1, 0x112), (1 << 2, 0x111)];
    let changed = previous ^ current;
    for (mask, button) in BUTTONS {
        if changed & mask != 0 {
            state.queue_input(SwsMessage::PointerButton {
                button,
                pressed: current & mask != 0,
            });
        }
    }
}

fn queue_pointer_scroll(state: &DesktopState, previous: u8, current: u8) {
    let pressed = current & !previous;
    let dy = i32::from(pressed & (1 << 3) != 0) - i32::from(pressed & (1 << 4) != 0);
    let dx = i32::from(pressed & (1 << 6) != 0) - i32::from(pressed & (1 << 5) != 0);
    if dx != 0 || dy != 0 {
        state.queue_input(SwsMessage::PointerScroll { dx, dy });
    }
}

fn intersect_rect(a: Rect, b: Rect) -> Option<Rect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = a.x.saturating_add(a.width).min(b.x.saturating_add(b.width));
    let y1 =
        a.y.saturating_add(a.height)
            .min(b.y.saturating_add(b.height));
    (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
}

struct PendingOutput {
    bytes: Vec<u8>,
    offset: usize,
}

struct OutputWriter {
    pending: VecDeque<PendingOutput>,
    pending_bytes: usize,
}

impl OutputWriter {
    fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            pending_bytes: 0,
        }
    }

    fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    fn enqueue(&mut self, bytes: Vec<u8>) -> io::Result<()> {
        if self.pending_bytes.saturating_add(bytes.len()) > MAX_CLIENT_OUTPUT_BYTES {
            return Err(io::Error::new(
                ErrorKind::OutOfMemory,
                "RFB client output queue exceeded its limit",
            ));
        }
        self.pending_bytes += bytes.len();
        self.pending.push_back(PendingOutput { bytes, offset: 0 });
        Ok(())
    }

    fn flush(&mut self, stream: &mut TcpStream) -> io::Result<bool> {
        let mut progressed = false;
        while let Some(output) = self.pending.front_mut() {
            let end = output
                .offset
                .saturating_add(WRITE_CHUNK_BYTES)
                .min(output.bytes.len());
            match stream.write(&output.bytes[output.offset..end]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        ErrorKind::WriteZero,
                        "RFB connection closed while writing",
                    ));
                }
                Ok(count) => {
                    output.offset += count;
                    self.pending_bytes = self.pending_bytes.saturating_sub(count);
                    progressed = true;
                    if output.offset == output.bytes.len() {
                        self.pending.pop_front();
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        Ok(progressed)
    }
}
