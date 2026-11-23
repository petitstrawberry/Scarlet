//! AudioToolbox binding for an interleaved PCM render callback.

use super::{CHANNELS, FRAME_BYTES, RATE, RenderQueue};
use std::{
    ffi::c_void,
    mem::{MaybeUninit, size_of},
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

type AudioUnit = *mut c_void;
type RenderCallback = unsafe extern "C" fn(
    *mut c_void,
    *mut u32,
    *const c_void,
    u32,
    u32,
    *mut AudioBufferList,
) -> i32;

const SCOPE_INPUT: u32 = 1;
const PROPERTY_STREAM_FORMAT: u32 = 8;
const PROPERTY_RENDER_CALLBACK: u32 = 23;
const FORMAT_SIGNED_PACKED: u32 = (1 << 2) | (1 << 3);
const OUTPUT_IS_SILENCE: u32 = 1 << 4;
const PARAM_ERROR: i32 = -50;

#[repr(C)]
struct ComponentDescription {
    component_type: u32,
    subtype: u32,
    manufacturer: u32,
    flags: u32,
    flags_mask: u32,
}

#[repr(C)]
struct StreamFormat {
    sample_rate: f64,
    format_id: u32,
    flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct Callback {
    render: RenderCallback,
    context: *mut c_void,
}

#[repr(C)]
struct AudioBuffer {
    channels: u32,
    size: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    count: u32,
    first: AudioBuffer,
}

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioComponentFindNext(
        current: *mut c_void,
        desc: *const ComponentDescription,
    ) -> *mut c_void;
    fn AudioComponentInstanceNew(component: *mut c_void, unit: *mut AudioUnit) -> i32;
    fn AudioComponentInstanceDispose(unit: AudioUnit) -> i32;
    fn AudioUnitSetProperty(
        unit: AudioUnit,
        property: u32,
        scope: u32,
        element: u32,
        value: *const c_void,
        size: u32,
    ) -> i32;
    fn AudioUnitInitialize(unit: AudioUnit) -> i32;
    fn AudioOutputUnitStart(unit: AudioUnit) -> i32;
    fn AudioOutputUnitStop(unit: AudioUnit) -> i32;
}

struct RenderContext {
    queue: Arc<RenderQueue>,
    enabled: AtomicBool,
}

pub(super) struct OutputUnit {
    handle: AudioUnit,
    context: Arc<RenderContext>,
    needs_stop: bool,
}

// Control calls on the instance are serialized by the playback device. Its
// callback only accesses the independently owned, synchronized RenderQueue.
unsafe impl Send for OutputUnit {}

impl OutputUnit {
    pub(super) fn open(queue: Arc<RenderQueue>) -> Result<Self, &'static str> {
        let description = ComponentDescription {
            component_type: u32::from_be_bytes(*b"auou"),
            #[cfg(target_os = "macos")]
            subtype: u32::from_be_bytes(*b"def "),
            #[cfg(target_os = "ios")]
            subtype: u32::from_be_bytes(*b"rioc"),
            manufacturer: u32::from_be_bytes(*b"appl"),
            flags: 0,
            flags_mask: 0,
        };
        let component = unsafe { AudioComponentFindNext(ptr::null_mut(), &description) };
        if component.is_null() {
            return Err("Core Audio default output unit is unavailable");
        }
        let mut handle = ptr::null_mut();
        check(
            unsafe { AudioComponentInstanceNew(component, &mut handle) },
            "Cannot open Core Audio output unit",
        )?;
        let unit = Self {
            handle,
            context: Arc::new(RenderContext {
                queue,
                enabled: AtomicBool::new(false),
            }),
            needs_stop: false,
        };
        let format = StreamFormat {
            sample_rate: f64::from(RATE),
            format_id: u32::from_be_bytes(*b"lpcm"),
            flags: FORMAT_SIGNED_PACKED,
            bytes_per_packet: FRAME_BYTES as u32,
            frames_per_packet: 1,
            bytes_per_frame: FRAME_BYTES as u32,
            channels_per_frame: u32::from(CHANNELS),
            bits_per_channel: 16,
            reserved: 0,
        };
        unit.set_property(
            PROPERTY_STREAM_FORMAT,
            &format,
            "Cannot set Core Audio PCM format",
        )?;
        let callback = Callback {
            render,
            context: Arc::as_ptr(&unit.context).cast_mut().cast(),
        };
        unit.set_property(
            PROPERTY_RENDER_CALLBACK,
            &callback,
            "Cannot install Core Audio render callback",
        )?;
        check(
            unsafe { AudioUnitInitialize(handle) },
            "Cannot initialize Core Audio output",
        )?;
        Ok(unit)
    }

    fn set_property<T>(
        &self,
        property: u32,
        value: &T,
        error: &'static str,
    ) -> Result<(), &'static str> {
        let status = unsafe {
            AudioUnitSetProperty(
                self.handle,
                property,
                SCOPE_INPUT,
                0,
                ptr::from_ref(value).cast(),
                size_of::<T>() as u32,
            )
        };
        check(status, error)
    }

    pub(super) fn start(&mut self) -> Result<(), &'static str> {
        if self.context.queue.enabled.load(Ordering::Acquire) {
            return Ok(());
        }
        if self.needs_stop {
            return Err("Core Audio output must be stopped before restart");
        }
        self.needs_stop = true;
        self.context.enabled.store(true, Ordering::Release);
        self.context.queue.enabled.store(true, Ordering::Release);
        let result = check(
            unsafe { AudioOutputUnitStart(self.handle) },
            "Cannot start Core Audio output",
        );
        if result.is_err() {
            self.context.queue.enabled.store(false, Ordering::Release);
        }
        result
    }

    pub(super) fn stop(&mut self) -> Result<(), &'static str> {
        self.context.enabled.store(false, Ordering::Release);
        self.context.queue.enabled.store(false, Ordering::Release);
        let result = if self.needs_stop {
            check(
                unsafe { AudioOutputUnitStop(self.handle) },
                "Cannot stop Core Audio output",
            )
        } else {
            Ok(())
        };
        self.needs_stop = false;
        // Taking the render flag after disabling playback synchronizes with
        // any callback that was already copying. Contenders render silence.
        while self.context.queue.rendering.swap(true, Ordering::AcqRel) {
            std::thread::yield_now();
        }
        self.context.queue.rendering.store(false, Ordering::Release);
        result
    }

    fn dispose(&mut self) -> Result<(), &'static str> {
        self.context.queue.enabled.store(false, Ordering::Release);
        let handle = std::mem::replace(&mut self.handle, ptr::null_mut());
        if handle.is_null() {
            return Ok(());
        }

        let result = check(
            unsafe { AudioComponentInstanceDispose(handle) },
            "Cannot dispose Core Audio output",
        );
        if result.is_err() {
            // Disposal failure does not prove that Core Audio has dropped the
            // callback context. Retain the disabled, host-owned queue.
            std::mem::forget(self.context.clone());
        }
        result
    }

    pub(super) fn close(mut self) -> Result<(), &'static str> {
        let stopped = self.stop();
        let disposed = self.dispose();
        stopped.and(disposed)
    }
}

impl Drop for OutputUnit {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            let _ = self.stop();
            let _ = self.dispose();
        }
    }
}

fn check(status: i32, error: &'static str) -> Result<(), &'static str> {
    if status == 0 {
        Ok(())
    } else {
        eprintln!("scarlet: {error} (OSStatus {status})");
        Err(error)
    }
}

unsafe extern "C" fn render(
    context: *mut c_void,
    flags: *mut u32,
    _: *const c_void,
    _: u32,
    frames: u32,
    buffers: *mut AudioBufferList,
) -> i32 {
    if frames == 0 {
        return 0;
    }
    // Safety: AudioToolbox owns the buffer list for the callback and the
    // configured unit retains the RenderQueue through disposal.
    unsafe {
        if buffers.is_null() || (*buffers).count != 1 {
            return PARAM_ERROR;
        }
        let buffer = &mut (*buffers).first;
        let bytes = frames as usize * FRAME_BYTES;
        if buffer.data.is_null()
            || buffer.channels != u32::from(CHANNELS)
            || bytes > buffer.size as usize
        {
            return PARAM_ERROR;
        }
        let output = std::slice::from_raw_parts_mut(buffer.data.cast::<MaybeUninit<u8>>(), bytes);
        let context = &*context.cast::<RenderContext>();
        if !context.queue.render_gated(output, &context.enabled) && !flags.is_null() {
            *flags |= OUTPUT_IS_SILENCE;
        }
        buffer.size = bytes as u32;
    }
    0
}
