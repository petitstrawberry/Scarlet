//! Shared-memory ring buffer stream for SAS.

use core::sync::atomic::{Ordering, compiler_fence};

use sas_protocol::{self as protocol, RING_HEADER_SIZE};

/// Stream configuration parameters.
///
/// This is the client-side mirror of `sas_protocol::Config` with a more
/// ergonomic interface.
#[derive(Clone, Copy, Debug)]
pub struct StreamConfig {
    pub format: u32,
    pub rate: u32,
    pub channels: u16,
    pub period_frames: u32,
    pub buffer_frames: u32,
}

/// A shared-memory ring buffer for streaming PCM samples to SAS.
///
/// The server creates the ring during `SasClient::configure()` and the client
/// writes interleaved PCM frames into it.  All multi-byte fields in the ring
/// header are accessed with `read_volatile` / `write_volatile` to avoid
/// undefined behaviour on the shared mapping.
pub struct SasStream {
    ring_addr: usize,
    ring_size: usize,
    frame_bytes: usize,
    buffer_frames: usize,
    rate: u32,
    channels: u16,
}

impl SasStream {
    /// Create a new stream wrapper.
    ///
    /// # Safety
    ///
    /// `ring_addr` must point to a valid shared-memory mapping of at least
    /// `ring_size` bytes that was set up by the SAS server.
    pub(crate) fn new(ring_addr: usize, ring_size: usize, config: &StreamConfig) -> Self {
        let frame_bytes = config.channels as usize * 2; // S16LE
        Self {
            ring_addr,
            ring_size,
            frame_bytes,
            buffer_frames: config.buffer_frames as usize,
            rate: config.rate,
            channels: config.channels,
        }
    }

    /// Number of frames that SAS has consumed (useful for timing / clock).
    pub fn read_frames(&self) -> u64 {
        let header = self.ring_addr as *const protocol::RingHeader;
        // SAFETY: `ring_addr` is a mapped SAS ring header.
        unsafe { core::ptr::addr_of!((*header).read_frames).read_volatile() }
    }

    /// Total number of frames written since the last reset.
    pub fn write_frames(&self) -> u64 {
        let header = self.ring_addr as *const protocol::RingHeader;
        // SAFETY: `ring_addr` is a mapped SAS ring header.
        unsafe { core::ptr::addr_of!((*header).write_frames).read_volatile() }
    }

    /// Number of frames that can be written without overflowing the ring.
    pub fn writable_frames(&self) -> usize {
        let header = self.ring_addr as *const protocol::RingHeader;
        // SAFETY: `ring_addr` is a mapped SAS ring header.
        let buffer_frames =
            unsafe { core::ptr::addr_of!((*header).buffer_frames).read_volatile() as usize };
        let read_frames = unsafe { core::ptr::addr_of!((*header).read_frames).read_volatile() };
        let write_frames = unsafe { core::ptr::addr_of!((*header).write_frames).read_volatile() };
        let queued = write_frames.saturating_sub(read_frames) as usize;
        buffer_frames.saturating_sub(queued)
    }

    /// Write interleaved PCM sample data to the ring buffer.
    ///
    /// `data` must contain a whole number of frames (`data.len() % frame_bytes == 0`).
    /// Returns the number of frames written (may be less than requested if the
    /// ring is nearly full).
    pub fn write(&mut self, data: &[u8]) -> usize {
        if data.is_empty() || self.frame_bytes == 0 {
            return 0;
        }

        let writable = self.writable_frames();
        let total_frames = data.len() / self.frame_bytes;
        let frames = total_frames.min(writable);
        if frames == 0 {
            return 0;
        }

        let header = self.ring_addr as *mut protocol::RingHeader;
        let data_ptr = (self.ring_addr + RING_HEADER_SIZE) as *mut u8;

        // SAFETY: `ring_addr` is a mapped SAS ring header.
        let write_frames = unsafe { core::ptr::addr_of!((*header).write_frames).read_volatile() };
        let ring_frame = write_frames as usize % self.buffer_frames;
        let first_chunk = frames.min(self.buffer_frames - ring_frame);
        let first_bytes = first_chunk * self.frame_bytes;

        // SAFETY: caller bounded `frames` by the ring writable space.
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                data_ptr.add(ring_frame * self.frame_bytes),
                first_bytes,
            );
        }

        let remaining_frames = frames - first_chunk;
        if remaining_frames > 0 {
            let second_bytes = remaining_frames * self.frame_bytes;
            // SAFETY: wrap copy writes from the start of the same mapped ring.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data[first_bytes..].as_ptr(),
                    data_ptr,
                    second_bytes,
                );
            }
        }

        compiler_fence(Ordering::Release);
        // SAFETY: `ring_addr` is a mapped SAS ring header.
        unsafe {
            core::ptr::addr_of_mut!((*header).write_frames)
                .write_volatile(write_frames + frames as u64);
        }

        frames
    }

    /// Check if the ring buffer is empty (all written data has been consumed).
    pub fn is_empty(&self) -> bool {
        let header = self.ring_addr as *const protocol::RingHeader;
        // SAFETY: `ring_addr` is a mapped SAS ring header.
        let read_frames = unsafe { core::ptr::addr_of!((*header).read_frames).read_volatile() };
        let write_frames = unsafe { core::ptr::addr_of!((*header).write_frames).read_volatile() };
        read_frames >= write_frames
    }

    /// Reset the ring buffer to empty state.
    ///
    /// Clears all pending data and resets read/write cursors to zero.
    pub fn reset(&mut self) {
        let header = self.ring_addr as *mut protocol::RingHeader;
        // SAFETY: `ring_addr` is a mapped SAS ring header.
        unsafe {
            let buffer_frames =
                core::ptr::addr_of!((*header).buffer_frames).read_volatile() as usize;
            let frame_bytes = core::ptr::addr_of!((*header).frame_bytes).read_volatile() as usize;
            // Reset the producer index first so SAS sees the ring as empty
            // before the consumer index is rewound.
            core::ptr::addr_of_mut!((*header).write_frames).write_volatile(0);
            core::ptr::addr_of_mut!((*header).read_frames).write_volatile(0);
            core::ptr::addr_of_mut!((*header).flags).write_volatile(0);
            core::ptr::write_bytes(
                (self.ring_addr + RING_HEADER_SIZE) as *mut u8,
                0,
                buffer_frames * frame_bytes,
            );
        }
        compiler_fence(Ordering::Release);
    }

    /// Sample rate of the configured stream.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Channel count of the configured stream.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Ring buffer capacity in frames.
    pub fn buffer_frames(&self) -> usize {
        self.buffer_frames
    }

    /// Bytes per frame (channels * bytes_per_sample).
    pub fn frame_bytes(&self) -> usize {
        self.frame_bytes
    }

    /// Total ring mapping size in bytes (header + data).
    pub fn ring_size(&self) -> usize {
        self.ring_size
    }
}
