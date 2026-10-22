//! One-shot client of the existing exclusive SWS capture protocol.
//! Never injects input or takes over another client's session.
use super::model::Pixels;
use super::transport::FrameReader;
use scarlet_os::handle::capability::memory_mapping::{MemoryMappingOps, flags as mmap_flags};
use scarlet_os::ipc::{SharedMemory, permissions};
use scarlet_os::socket::{Socket, SocketError};
use std::io::Write;
use std::thread;
use std::time::{Duration, Instant};
use sws_remote_protocol::{
    CaptureFormat, ClientMessage, ServerMessage, decode_server_message, encode_client_message,
};
const MAX_LOCAL_FRAME_BYTES: usize = super::model::MAX_PIXELS * 4;

pub fn capture() -> Result<Pixels, String> {
    capture_inner().map_err(String::from)
}
fn capture_inner() -> Result<Pixels, &'static str> {
    let mut socket = Socket::new().map_err(|_| "Cannot create capture connection")?;
    socket
        .connect("/tmp/sws-remote.sock")
        .map_err(|_| "SWS capture service is unavailable")?;
    socket
        .set_nonblocking(true)
        .map_err(|_| "Cannot configure capture connection")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    send(
        &mut socket,
        &ClientMessage::CreateCapture { output_id: 0 },
        deadline,
    )?;
    let mut reader = FrameReader::new();
    let mut buffer: Option<CaptureBuffer> = None;
    let mut requested = false;
    loop {
        if Instant::now() >= deadline {
            return Err(
                "Capture timed out. SWS may be busy with a remote desktop session; that session was left untouched.",
            );
        }
        if let Some((header, payload)) = reader.poll(&mut socket)? {
            match decode_server_message(header, &payload).map_err(|_| "Invalid capture response")? {
                ServerMessage::OutputChanged { width, height } => {
                    if buffer.is_some() {
                        return Err("Output size changed during capture. Please try again.");
                    }
                    let new_buffer = CaptureBuffer::new(1, width, height)?;
                    let registration = encode_client_message(&ClientMessage::RegisterBuffer {
                        buffer_id: 1,
                        width,
                        height,
                        stride: new_buffer.stride,
                        format: CaptureFormat::Bgra8888,
                    });
                    loop {
                        match socket.send_handle_and_data(
                            new_buffer.shared_memory.as_handle(),
                            &registration,
                        ) {
                            Ok(()) => break,
                            Err(SocketError::WouldBlock) if Instant::now() < deadline => {
                                thread::sleep(Duration::from_millis(5))
                            }
                            _ => return Err("Cannot register screenshot buffer"),
                        }
                    }
                    buffer = Some(new_buffer);
                }
                ServerMessage::FrameAvailable { .. } => {
                    if buffer.is_some() && !requested {
                        send(
                            &mut socket,
                            &ClientMessage::RequestFrame { buffer_id: 1 },
                            deadline,
                        )?;
                        requested = true;
                    }
                }
                ServerMessage::FrameReady { buffer_id, .. } => {
                    let active = buffer
                        .as_ref()
                        .filter(|b| b.buffer_id == buffer_id && requested)
                        .ok_or("Unexpected capture buffer")?;
                    // SAFETY: SWS has completed this buffer and this client requests no
                    // more frames. The owned mapping stays alive until the copy ends.
                    let pixels = unsafe {
                        std::slice::from_raw_parts(
                            active.mapped_address as *const u8,
                            active.mapped_length,
                        )
                    }
                    .to_vec();
                    return Pixels::new(active.width, active.height, pixels)
                        .map_err(|_| "Invalid captured image");
                }
            }
        } else {
            thread::sleep(Duration::from_millis(5));
        }
    }
}
fn send(
    socket: &mut Socket,
    message: &ClientMessage,
    deadline: Instant,
) -> Result<(), &'static str> {
    let bytes = encode_client_message(message);
    let mut offset = 0;
    while offset < bytes.len() {
        match socket.write(&bytes[offset..]) {
            Ok(0) => return Err("Capture connection closed"),
            Ok(n) => offset += n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(5))
            }
            Err(_) => return Err("Cannot send capture request"),
        }
    }
    Ok(())
}
struct CaptureBuffer {
    buffer_id: u32,
    width: u32,
    height: u32,
    stride: u32,
    shared_memory: SharedMemory,
    mapped_address: usize,
    mapped_length: usize,
}

impl CaptureBuffer {
    fn new(buffer_id: u32, width: u32, height: u32) -> Result<Self, &'static str> {
        if width == 0 || height == 0 {
            return Err("SWS reported an empty output");
        }
        let stride = width
            .checked_mul(4)
            .ok_or("Capture buffer stride overflow")?;
        let mapped_length = frame_length(stride, height)?;
        let shared_memory = SharedMemory::create(mapped_length, permissions::READ_WRITE)
            .map_err(|_| "Failed to create capture shared memory")?;
        // SAFETY: This requests a fresh non-fixed mapping; its owning buffer/stream retains the backing and controls all CPU views and unmapping.
        let mapped_address = unsafe {
            shared_memory
                .as_handle()
                .as_memory_mapping()
                .map_err(|_| "Capture shared memory cannot be mapped")?
                .mmap(
                    0,
                    mapped_length,
                    permissions::READ_WRITE,
                    mmap_flags::SHARED,
                    0,
                )
        }
        .map_err(|_| "Failed to map capture shared memory")?;
        Ok(Self {
            buffer_id,
            width,
            height,
            stride,
            shared_memory,
            mapped_address,
            mapped_length,
        })
    }
}

impl Drop for CaptureBuffer {
    fn drop(&mut self) {
        // SAFETY: This teardown/rollback path owns the exact mapping; its borrowed CPU views have ended before releasing the virtual range.
        let _ = unsafe { MemoryMappingOps::munmap(self.mapped_address, self.mapped_length) };
    }
}

fn frame_length(stride: u32, height: u32) -> Result<usize, &'static str> {
    let length = usize::try_from(
        u64::from(stride)
            .checked_mul(u64::from(height))
            .ok_or("Remote desktop framebuffer size overflow")?,
    )
    .map_err(|_| "Remote desktop framebuffer size is unsupported")?;
    if length == 0 || length > MAX_LOCAL_FRAME_BYTES {
        return Err("Remote desktop framebuffer size is unsupported");
    }
    Ok(length)
}
