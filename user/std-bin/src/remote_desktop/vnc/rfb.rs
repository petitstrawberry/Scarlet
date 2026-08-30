//! RFB 3.8 wire parsing and Raw/ZRLE framebuffer-update encoding.

use miniz_oxide::deflate::core::{CompressorOxide, create_comp_flags_from_zip_params};
use miniz_oxide::deflate::stream::deflate;
use miniz_oxide::{MZ_DEFAULT_WINDOW_BITS, MZFlush};
use std::io::{self, ErrorKind, Read, Write};
use std::net::TcpStream;
use std::vec::Vec;
use sws_remote_protocol::Rect;

use super::super::sws::FrameState;

pub(super) const ENCODING_RAW: i32 = 0;
pub(super) const ENCODING_ZRLE: i32 = 16;
pub(super) const ENCODING_DESKTOP_SIZE: i32 = -223;
const PROTOCOL_VERSION: &[u8; 12] = b"RFB 003.008\n";
const SERVER_NAME: &[u8] = b"Scarlet Remote Desktop";
const MAX_CLIENT_MESSAGE_BYTES: usize = 2 * 1024 * 1024;
const MAX_ENCODINGS: usize = 4096;
const ZRLE_TILE_SIZE: u32 = 64;
const ZRLE_COMPRESSION_LEVEL: i32 = 1;
const ZRLE_COMPRESSION_CHUNK_BYTES: usize = 64 * 1024;

/// Pixel encoding selected from one client's ordered preference list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FramebufferEncoding {
    /// Uncompressed client-selected 32-bit pixels.
    Raw,
    /// Zlib-compressed 64x64 tiled pixels.
    Zrle,
}

/// Byte order used for client-visible 24-bit color components.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PixelOrder {
    /// Blue, green, then red bytes as captured by SWS.
    Bgr,
    /// Red, green, then blue bytes requested by common VNC clients.
    Rgb,
}

impl FramebufferEncoding {
    /// Select the first encoding supported by both peers.
    ///
    /// # Arguments
    ///
    /// * `encodings` - Client preference list from `SetEncodings`.
    ///
    /// # Returns
    ///
    /// ZRLE when it precedes Raw among supported entries, otherwise Raw.
    pub(super) fn select(encodings: &[i32]) -> Self {
        encodings
            .iter()
            .find_map(|encoding| match *encoding {
                ENCODING_ZRLE => Some(Self::Zrle),
                ENCODING_RAW => Some(Self::Raw),
                _ => None,
            })
            .unwrap_or(Self::Raw)
    }
}

/// Persistent zlib stream required by ZRLE for one RFB connection.
pub(super) struct ZrleEncoder {
    compressor: CompressorOxide,
    scratch: Vec<u8>,
}

/// Tightly packed pixels copied from one output-space damage rectangle.
///
/// ZRLE encoding deliberately owns this snapshot so its tile analysis and
/// compression do not hold the shared desktop framebuffer lock.
pub(super) struct ZrleRectangle {
    rect: Rect,
    stride: usize,
    pixels: Vec<u8>,
}

impl ZrleEncoder {
    /// Construct a best-speed ZRLE stream.
    ///
    /// # Returns
    ///
    /// An encoder whose zlib dictionary persists across framebuffer updates.
    pub(super) fn new() -> Self {
        let flags =
            create_comp_flags_from_zip_params(ZRLE_COMPRESSION_LEVEL, MZ_DEFAULT_WINDOW_BITS, 0);
        Self {
            compressor: CompressorOxide::new(flags),
            scratch: vec![0; ZRLE_COMPRESSION_CHUNK_BYTES],
        }
    }

    fn append_compressed(&mut self, input: &[u8], output: &mut Vec<u8>) -> io::Result<()> {
        let length_offset = output.len();
        output.extend_from_slice(&0u32.to_be_bytes());
        let compressed_start = output.len();
        let mut remaining = input;

        loop {
            let result = deflate(
                &mut self.compressor,
                remaining,
                &mut self.scratch,
                MZFlush::Sync,
            );
            result
                .status
                .map_err(|_| io::Error::new(ErrorKind::InvalidData, "ZRLE compression failed"))?;
            if result.bytes_consumed > remaining.len() || result.bytes_written > self.scratch.len()
            {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    "ZRLE compressor returned invalid progress",
                ));
            }
            output
                .try_reserve(result.bytes_written)
                .map_err(|_| io::Error::new(ErrorKind::OutOfMemory, "ZRLE allocation failed"))?;
            output.extend_from_slice(&self.scratch[..result.bytes_written]);
            remaining = &remaining[result.bytes_consumed..];

            // A non-full output buffer means the synchronous flush marker fit
            // and this rectangle now ends at a zlib byte boundary.
            if remaining.is_empty() && result.bytes_written < self.scratch.len() {
                break;
            }
            if result.bytes_consumed == 0 && result.bytes_written == 0 {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    "ZRLE compressor made no progress",
                ));
            }
        }

        let compressed_length = u32::try_from(output.len().saturating_sub(compressed_start))
            .map_err(|_| io::Error::new(ErrorKind::InvalidData, "ZRLE rectangle is too large"))?;
        output[length_offset..length_offset + 4].copy_from_slice(&compressed_length.to_be_bytes());
        Ok(())
    }
}

/// One requested framebuffer region.
#[derive(Debug, Clone, Copy)]
pub(super) struct FramebufferUpdateRequest {
    /// Whether unchanged pixels may be omitted.
    pub(super) incremental: bool,
    /// Requested framebuffer rectangle.
    pub(super) rect: Rect,
}

/// Decoded RFB client-to-server message.
pub(super) enum ClientMessage {
    /// Client-selected pixel format.
    SetPixelFormat(PixelFormat),
    /// Ordered encoding preferences.
    SetEncodings(Vec<i32>),
    /// Demand for one framebuffer update.
    FramebufferUpdateRequest(FramebufferUpdateRequest),
    /// X11 KeySym transition.
    KeyEvent {
        /// Press or release state.
        pressed: bool,
        /// X11 KeySym.
        keysym: u32,
    },
    /// Absolute pointer coordinates and current RFB button mask.
    PointerEvent {
        /// RFB button bitmask.
        buttons: u8,
        /// Horizontal framebuffer coordinate.
        x: u16,
        /// Vertical framebuffer coordinate.
        y: u16,
    },
    /// Clipboard text ignored by the initial implementation.
    ClientCutText,
}

/// RFB pixel format selected by a client.
#[derive(Debug, Clone, Copy)]
pub(super) struct PixelFormat {
    bits_per_pixel: u8,
    depth: u8,
    big_endian: bool,
    true_color: bool,
    red_max: u16,
    green_max: u16,
    blue_max: u16,
    red_shift: u8,
    green_shift: u8,
    blue_shift: u8,
}

impl PixelFormat {
    /// Return the supported client-visible color byte order.
    ///
    /// # Returns
    ///
    /// `Some` for 32-bit little-endian RGB or BGR true color, otherwise
    /// `None`.
    pub(super) const fn pixel_order(self) -> Option<PixelOrder> {
        if !(self.bits_per_pixel == 32
            && self.depth == 24
            && !self.big_endian
            && self.true_color
            && self.red_max == 255
            && self.green_max == 255
            && self.blue_max == 255)
        {
            return None;
        }
        match (self.red_shift, self.green_shift, self.blue_shift) {
            (16, 8, 0) => Some(PixelOrder::Bgr),
            (0, 8, 16) => Some(PixelOrder::Rgb),
            _ => None,
        }
    }
}

/// Complete the RFB 3.8 None-security handshake.
///
/// # Arguments
///
/// * `stream` - Newly accepted TCP connection.
/// * `width` - Initial framebuffer width.
/// * `height` - Initial framebuffer height.
///
/// # Returns
///
/// Success after `ServerInit` has been sent.
pub(super) fn handshake(stream: &mut TcpStream, width: u32, height: u32) -> io::Result<()> {
    let width = u16::try_from(width)
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "framebuffer width exceeds RFB"))?;
    let height = u16::try_from(height)
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "framebuffer height exceeds RFB"))?;

    stream.write_all(PROTOCOL_VERSION)?;
    let mut client_version = [0; 12];
    stream.read_exact(&mut client_version)?;
    if &client_version != PROTOCOL_VERSION {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "only RFB 3.8 is supported",
        ));
    }

    stream.write_all(&[1, 1])?; // one security type: None
    let mut selected_security = [0; 1];
    stream.read_exact(&mut selected_security)?;
    if selected_security[0] != 1 {
        return Err(io::Error::new(
            ErrorKind::PermissionDenied,
            "client rejected RFB None security",
        ));
    }
    stream.write_all(&0u32.to_be_bytes())?; // SecurityResult: OK

    let mut shared_flag = [0; 1];
    stream.read_exact(&mut shared_flag)?;

    let mut server_init = Vec::with_capacity(24 + SERVER_NAME.len());
    server_init.extend_from_slice(&width.to_be_bytes());
    server_init.extend_from_slice(&height.to_be_bytes());
    encode_native_pixel_format(&mut server_init);
    server_init.extend_from_slice(&(SERVER_NAME.len() as u32).to_be_bytes());
    server_init.extend_from_slice(SERVER_NAME);
    stream.write_all(&server_init)?;
    stream.flush()
}

/// Incremental parser for non-blocking RFB client messages.
pub(super) struct MessageReader {
    bytes: Vec<u8>,
    head: usize,
}

impl MessageReader {
    /// Construct an empty parser.
    ///
    /// # Returns
    ///
    /// A parser ready to receive client messages.
    pub(super) fn new() -> Self {
        Self {
            bytes: Vec::new(),
            head: 0,
        }
    }

    /// Read every currently available TCP byte.
    ///
    /// # Arguments
    ///
    /// * `stream` - Non-blocking RFB connection.
    ///
    /// # Returns
    ///
    /// `true` when at least one byte was appended, or an I/O error/disconnect.
    pub(super) fn read_available(&mut self, stream: &mut TcpStream) -> io::Result<bool> {
        let mut progressed = false;
        let mut chunk = [0u8; 16 * 1024];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => return Err(io::Error::new(ErrorKind::UnexpectedEof, "RFB disconnected")),
                Ok(count) => {
                    if self.bytes.len().saturating_add(count) > MAX_CLIENT_MESSAGE_BYTES {
                        return Err(io::Error::new(
                            ErrorKind::InvalidData,
                            "RFB input queue exceeded its limit",
                        ));
                    }
                    self.bytes.extend_from_slice(&chunk[..count]);
                    progressed = true;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(progressed),
                Err(error) => return Err(error),
            }
        }
    }

    /// Decode the next complete buffered message.
    ///
    /// # Returns
    ///
    /// A message, `None` for a partial message, or an error for invalid input.
    pub(super) fn next_message(&mut self) -> io::Result<Option<ClientMessage>> {
        let available = &self.bytes[self.head..];
        let Some(message_type) = available.first().copied() else {
            self.compact();
            return Ok(None);
        };
        let required = match message_type {
            0 => 20,
            2 => {
                if available.len() < 4 {
                    return Ok(None);
                }
                let count = u16::from_be_bytes([available[2], available[3]]) as usize;
                if count > MAX_ENCODINGS {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        "too many RFB encodings",
                    ));
                }
                4 + count * 4
            }
            3 => 10,
            4 => 8,
            5 => 6,
            6 => {
                if available.len() < 8 {
                    return Ok(None);
                }
                let length = read_u32(available, 4) as usize;
                if length > MAX_CLIENT_MESSAGE_BYTES.saturating_sub(8) {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        "RFB clipboard message is too large",
                    ));
                }
                8 + length
            }
            _ => {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    "unsupported RFB client message",
                ));
            }
        };
        if available.len() < required {
            return Ok(None);
        }

        let message = match message_type {
            0 => ClientMessage::SetPixelFormat(PixelFormat {
                bits_per_pixel: available[4],
                depth: available[5],
                big_endian: available[6] != 0,
                true_color: available[7] != 0,
                red_max: read_u16(available, 8),
                green_max: read_u16(available, 10),
                blue_max: read_u16(available, 12),
                red_shift: available[14],
                green_shift: available[15],
                blue_shift: available[16],
            }),
            2 => {
                let count = read_u16(available, 2) as usize;
                let mut encodings = Vec::with_capacity(count);
                for index in 0..count {
                    encodings.push(read_i32(available, 4 + index * 4));
                }
                ClientMessage::SetEncodings(encodings)
            }
            3 => ClientMessage::FramebufferUpdateRequest(FramebufferUpdateRequest {
                incremental: available[1] != 0,
                rect: Rect::new(
                    u32::from(read_u16(available, 2)),
                    u32::from(read_u16(available, 4)),
                    u32::from(read_u16(available, 6)),
                    u32::from(read_u16(available, 8)),
                ),
            }),
            4 => ClientMessage::KeyEvent {
                pressed: available[1] != 0,
                keysym: read_u32(available, 4),
            },
            5 => ClientMessage::PointerEvent {
                buttons: available[1],
                x: read_u16(available, 2),
                y: read_u16(available, 4),
            },
            6 => ClientMessage::ClientCutText,
            _ => unreachable!(),
        };
        self.head += required;
        self.compact();
        Ok(Some(message))
    }

    fn compact(&mut self) {
        if self.head == self.bytes.len() {
            self.bytes.clear();
            self.head = 0;
        } else if self.head > 64 * 1024 && self.head.saturating_mul(2) >= self.bytes.len() {
            self.bytes.drain(..self.head);
            self.head = 0;
        }
    }
}

/// Encode one Raw framebuffer update from the current complete frame.
///
/// # Arguments
///
/// * `frame` - Latest complete captured framebuffer.
/// * `rects` - Output-space rectangles to send.
/// * `pixel_order` - Client-selected RGB component byte order.
///
/// # Returns
///
/// A complete RFB `FramebufferUpdate` message.
pub(super) fn encode_raw_update(
    frame: &FrameState,
    rects: &[Rect],
    pixel_order: PixelOrder,
) -> io::Result<Vec<u8>> {
    let count = u16::try_from(rects.len())
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "too many RFB rectangles"))?;
    let pixel_bytes = rects.iter().try_fold(0usize, |total, rect| {
        let bytes = usize::try_from(u64::from(rect.width) * u64::from(rect.height) * 4)
            .map_err(|_| io::Error::new(ErrorKind::InvalidData, "RFB update is too large"))?;
        total
            .checked_add(bytes)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "RFB update is too large"))
    })?;
    let headers = rects
        .len()
        .checked_mul(12)
        .and_then(|value| value.checked_add(4))
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "RFB update is too large"))?;
    let mut message = Vec::new();
    message
        .try_reserve_exact(headers.saturating_add(pixel_bytes))
        .map_err(|_| io::Error::new(ErrorKind::OutOfMemory, "RFB update allocation failed"))?;
    message.extend_from_slice(&[0, 0]);
    message.extend_from_slice(&count.to_be_bytes());

    for rect in rects {
        append_rectangle_header(&mut message, *rect, ENCODING_RAW)?;

        let row_bytes = rect.width as usize * 4;
        for row in 0..rect.height {
            let offset = (rect.y as usize + row as usize)
                .saturating_mul(frame.stride as usize)
                .saturating_add(rect.x as usize * 4);
            if offset.saturating_add(row_bytes) > frame.pixels.len() {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    "RFB rectangle exceeds captured pixels",
                ));
            }
            let pixels = &frame.pixels[offset..offset + row_bytes];
            match pixel_order {
                PixelOrder::Bgr => message.extend_from_slice(pixels),
                PixelOrder::Rgb => {
                    for pixel in pixels.chunks_exact(4) {
                        message.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                    }
                }
            }
        }
    }
    Ok(message)
}

/// Encode one ZRLE framebuffer update using a connection-persistent stream.
///
/// Each 64x64 tile is represented as either a solid three-byte CPIXEL or raw
/// three-byte CPIXEL data before zlib compression. The unused alpha byte in
/// the negotiated 32-bit/24-depth native pixel format is omitted.
///
/// # Arguments
///
/// * `rectangles` - Owned framebuffer rectangles to encode.
/// * `encoder` - Zlib state owned by this RFB connection.
/// * `pixel_order` - Client-selected RGB component byte order.
///
/// # Returns
///
/// A complete RFB `FramebufferUpdate` message.
pub(super) fn encode_zrle_update(
    rectangles: &[ZrleRectangle],
    encoder: &mut ZrleEncoder,
    pixel_order: PixelOrder,
) -> io::Result<Vec<u8>> {
    let count = u16::try_from(rectangles.len())
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "too many RFB rectangles"))?;
    let mut message = Vec::new();
    message
        .try_reserve_exact(4usize.saturating_add(rectangles.len().saturating_mul(16)))
        .map_err(|_| io::Error::new(ErrorKind::OutOfMemory, "RFB update allocation failed"))?;
    message.extend_from_slice(&[0, 0]);
    message.extend_from_slice(&count.to_be_bytes());

    for rectangle in rectangles {
        append_rectangle_header(&mut message, rectangle.rect, ENCODING_ZRLE)?;
        let tiles = encode_zrle_tiles(rectangle, pixel_order)?;
        encoder.append_compressed(&tiles, &mut message)?;
    }
    Ok(message)
}

/// Copy the requested rectangles out of the shared complete framebuffer.
///
/// # Arguments
///
/// * `frame` - Latest complete captured framebuffer.
/// * `rects` - Output-space rectangles needed by one RFB client.
///
/// # Returns
///
/// Tightly packed, independently owned snapshots suitable for lock-free ZRLE
/// analysis and compression.
pub(super) fn snapshot_zrle_rectangles(
    frame: &FrameState,
    rects: &[Rect],
) -> io::Result<Vec<ZrleRectangle>> {
    let mut rectangles = Vec::new();
    rectangles
        .try_reserve_exact(rects.len())
        .map_err(|_| io::Error::new(ErrorKind::OutOfMemory, "ZRLE snapshot allocation failed"))?;

    for rect in rects {
        validate_frame_rect(frame, *rect)?;
        let stride = usize::try_from(u64::from(rect.width) * 4)
            .map_err(|_| io::Error::new(ErrorKind::InvalidData, "ZRLE row is too large"))?;
        let length = stride
            .checked_mul(rect.height as usize)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "ZRLE rectangle is too large"))?;
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(length).map_err(|_| {
            io::Error::new(ErrorKind::OutOfMemory, "ZRLE snapshot allocation failed")
        })?;

        for row in 0..rect.height {
            let offset = pixel_offset(frame.stride as usize, rect.x, rect.y + row);
            pixels.extend_from_slice(&frame.pixels[offset..offset + stride]);
        }
        rectangles.push(ZrleRectangle {
            rect: *rect,
            stride,
            pixels,
        });
    }

    Ok(rectangles)
}

fn encode_zrle_tiles(rectangle: &ZrleRectangle, pixel_order: PixelOrder) -> io::Result<Vec<u8>> {
    let rect = rectangle.rect;
    let pixel_count = usize::try_from(u64::from(rect.width) * u64::from(rect.height))
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "ZRLE rectangle is too large"))?;
    let tile_columns = rect.width.div_ceil(ZRLE_TILE_SIZE) as usize;
    let tile_rows = rect.height.div_ceil(ZRLE_TILE_SIZE) as usize;
    let maximum_length = pixel_count
        .checked_mul(3)
        .and_then(|length| length.checked_add(tile_columns.saturating_mul(tile_rows)))
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "ZRLE rectangle is too large"))?;
    let mut tiles = Vec::new();
    tiles
        .try_reserve_exact(maximum_length)
        .map_err(|_| io::Error::new(ErrorKind::OutOfMemory, "ZRLE tile allocation failed"))?;

    let mut tile_y = 0;
    while tile_y < rect.height {
        let tile_height = ZRLE_TILE_SIZE.min(rect.height - tile_y);
        let mut tile_x = 0;
        while tile_x < rect.width {
            let tile_width = ZRLE_TILE_SIZE.min(rect.width - tile_x);
            if let Some(color) =
                solid_tile_color(rectangle, tile_x, tile_y, tile_width, tile_height)
            {
                tiles.push(1);
                append_cpixel(&mut tiles, color, pixel_order);
            } else {
                tiles.push(0);
                append_raw_cpixels(
                    rectangle,
                    tile_x,
                    tile_y,
                    tile_width,
                    tile_height,
                    pixel_order,
                    &mut tiles,
                );
            }
            tile_x += tile_width;
        }
        tile_y += tile_height;
    }
    Ok(tiles)
}

fn validate_frame_rect(frame: &FrameState, rect: Rect) -> io::Result<()> {
    let minimum_stride = frame
        .width
        .checked_mul(4)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "RFB frame stride overflow"))?;
    let required_length = usize::try_from(u64::from(frame.stride) * u64::from(frame.height))
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "RFB frame is too large"))?;
    let right = rect
        .x
        .checked_add(rect.width)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "RFB rectangle overflow"))?;
    let bottom = rect
        .y
        .checked_add(rect.height)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "RFB rectangle overflow"))?;
    if rect.width == 0
        || rect.height == 0
        || right > frame.width
        || bottom > frame.height
        || frame.stride < minimum_stride
        || frame.pixels.len() < required_length
    {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "ZRLE rectangle exceeds captured pixels",
        ));
    }
    Ok(())
}

fn solid_tile_color(
    rectangle: &ZrleRectangle,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Option<[u8; 3]> {
    let first_offset = pixel_offset(rectangle.stride, x, y);
    let color = [
        rectangle.pixels[first_offset],
        rectangle.pixels[first_offset + 1],
        rectangle.pixels[first_offset + 2],
    ];
    for row in 0..height {
        for column in 0..width {
            let offset = pixel_offset(rectangle.stride, x + column, y + row);
            if rectangle.pixels[offset..offset + 3] != color {
                return None;
            }
        }
    }
    Some(color)
}

fn append_raw_cpixels(
    rectangle: &ZrleRectangle,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    pixel_order: PixelOrder,
    output: &mut Vec<u8>,
) {
    for row in 0..height {
        for column in 0..width {
            let offset = pixel_offset(rectangle.stride, x + column, y + row);
            let color = [
                rectangle.pixels[offset],
                rectangle.pixels[offset + 1],
                rectangle.pixels[offset + 2],
            ];
            append_cpixel(output, color, pixel_order);
        }
    }
}

fn append_cpixel(output: &mut Vec<u8>, color: [u8; 3], pixel_order: PixelOrder) {
    match pixel_order {
        PixelOrder::Bgr => output.extend_from_slice(&color),
        PixelOrder::Rgb => output.extend_from_slice(&[color[2], color[1], color[0]]),
    }
}

fn pixel_offset(stride: usize, x: u32, y: u32) -> usize {
    y as usize * stride + x as usize * 4
}

fn append_rectangle_header(message: &mut Vec<u8>, rect: Rect, encoding: i32) -> io::Result<()> {
    let x = u16::try_from(rect.x)
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "RFB x coordinate overflow"))?;
    let y = u16::try_from(rect.y)
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "RFB y coordinate overflow"))?;
    let width = u16::try_from(rect.width)
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "RFB width overflow"))?;
    let height = u16::try_from(rect.height)
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "RFB height overflow"))?;
    message.extend_from_slice(&x.to_be_bytes());
    message.extend_from_slice(&y.to_be_bytes());
    message.extend_from_slice(&width.to_be_bytes());
    message.extend_from_slice(&height.to_be_bytes());
    message.extend_from_slice(&encoding.to_be_bytes());
    Ok(())
}

/// Encode a DesktopSize pseudo-rectangle update.
///
/// # Arguments
///
/// * `width` - New framebuffer width.
/// * `height` - New framebuffer height.
///
/// # Returns
///
/// A complete one-rectangle framebuffer update.
pub(super) fn encode_desktop_size(width: u32, height: u32) -> io::Result<Vec<u8>> {
    let width = u16::try_from(width)
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "RFB width overflow"))?;
    let height = u16::try_from(height)
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "RFB height overflow"))?;
    let mut message = Vec::with_capacity(16);
    message.extend_from_slice(&[0, 0, 0, 1]);
    message.extend_from_slice(&0u16.to_be_bytes());
    message.extend_from_slice(&0u16.to_be_bytes());
    message.extend_from_slice(&width.to_be_bytes());
    message.extend_from_slice(&height.to_be_bytes());
    message.extend_from_slice(&ENCODING_DESKTOP_SIZE.to_be_bytes());
    Ok(message)
}

fn encode_native_pixel_format(bytes: &mut Vec<u8>) {
    bytes.extend_from_slice(&[32, 24, 0, 1]);
    bytes.extend_from_slice(&255u16.to_be_bytes());
    bytes.extend_from_slice(&255u16.to_be_bytes());
    bytes.extend_from_slice(&255u16.to_be_bytes());
    bytes.extend_from_slice(&[16, 8, 0, 0, 0, 0]);
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn read_i32(bytes: &[u8], offset: usize) -> i32 {
    i32::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

#[cfg(test)]
mod tests {
    use super::{
        FrameState, FramebufferEncoding, PixelFormat, PixelOrder, encode_raw_update,
        encode_zrle_tiles, snapshot_zrle_rectangles,
    };
    use sws_remote_protocol::Rect;

    #[test]
    fn selects_first_supported_framebuffer_encoding() {
        assert_eq!(
            FramebufferEncoding::select(&[7, 16, 5, 0]),
            FramebufferEncoding::Zrle
        );
        assert_eq!(
            FramebufferEncoding::select(&[7, 0, 16]),
            FramebufferEncoding::Raw
        );
        assert_eq!(
            FramebufferEncoding::select(&[7, 5]),
            FramebufferEncoding::Raw
        );
    }

    #[test]
    fn encodes_solid_and_raw_zrle_tiles() {
        let width = 128u32;
        let height = 64u32;
        let stride = width * 4;
        let mut frame = FrameState::new();
        frame.width = width;
        frame.height = height;
        frame.stride = stride;
        frame.pixels = vec![0; (stride * height) as usize];
        for y in 0..height {
            for x in 0..width {
                let offset = (y * stride + x * 4) as usize;
                let pixel = if x < 64 {
                    [1, 2, 3, 255]
                } else {
                    [x as u8, y as u8, (x ^ y) as u8, 255]
                };
                frame.pixels[offset..offset + 4].copy_from_slice(&pixel);
            }
        }

        let rectangles =
            snapshot_zrle_rectangles(&frame, &[Rect::new(0, 0, width, height)]).unwrap();
        let encoded = encode_zrle_tiles(&rectangles[0], PixelOrder::Bgr).unwrap();
        assert_eq!(&encoded[..4], &[1, 1, 2, 3]);
        assert_eq!(encoded[4], 0);
        assert_eq!(encoded.len(), 4 + 1 + 64 * 64 * 3);
    }

    #[test]
    fn accepts_common_little_endian_pixel_orders() {
        let native = PixelFormat {
            bits_per_pixel: 32,
            depth: 24,
            big_endian: false,
            true_color: true,
            red_max: 255,
            green_max: 255,
            blue_max: 255,
            red_shift: 16,
            green_shift: 8,
            blue_shift: 0,
        };
        let rgb = PixelFormat {
            red_shift: 0,
            blue_shift: 16,
            ..native
        };
        assert_eq!(native.pixel_order(), Some(PixelOrder::Bgr));
        assert_eq!(rgb.pixel_order(), Some(PixelOrder::Rgb));
        assert_eq!(PixelFormat { depth: 16, ..rgb }.pixel_order(), None);
    }

    #[test]
    fn converts_raw_and_zrle_pixels_to_client_order() {
        let mut frame = FrameState::new();
        frame.width = 1;
        frame.height = 1;
        frame.stride = 4;
        frame.pixels = vec![1, 2, 3, 255];
        let rect = Rect::new(0, 0, 1, 1);

        let raw_bgr = encode_raw_update(&frame, &[rect], PixelOrder::Bgr).unwrap();
        let raw_rgb = encode_raw_update(&frame, &[rect], PixelOrder::Rgb).unwrap();
        assert_eq!(&raw_bgr[16..20], &[1, 2, 3, 255]);
        assert_eq!(&raw_rgb[16..20], &[3, 2, 1, 255]);

        let rectangles = snapshot_zrle_rectangles(&frame, &[rect]).unwrap();
        let zrle_bgr = encode_zrle_tiles(&rectangles[0], PixelOrder::Bgr).unwrap();
        let zrle_rgb = encode_zrle_tiles(&rectangles[0], PixelOrder::Rgb).unwrap();
        assert_eq!(&zrle_bgr, &[1, 1, 2, 3]);
        assert_eq!(&zrle_rgb, &[1, 3, 2, 1]);
    }
}
