//! VideoToolbox decoding and CoreVideo NV12 transfer on the host service.

use super::access_unit::{self, ParameterSets};
use scarlet::device::video::{
    SCARLET_VIDEO_FORMAT_AV1, SCARLET_VIDEO_FORMAT_H264, SCARLET_VIDEO_FORMAT_HEVC,
};
use std::{
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicI32, AtomicPtr, Ordering},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Codec {
    H264,
    Hevc,
    Av1,
}

impl Codec {
    pub(super) const fn scarlet_format(self) -> u32 {
        match self {
            Self::H264 => SCARLET_VIDEO_FORMAT_H264,
            Self::Hevc => SCARLET_VIDEO_FORMAT_HEVC,
            Self::Av1 => SCARLET_VIDEO_FORMAT_AV1,
        }
    }

    const fn apple_format(self) -> u32 {
        u32::from_be_bytes(match self {
            Self::H264 => *b"avc1",
            Self::Hevc => *b"hvc1",
            Self::Av1 => *b"av01",
        })
    }
}

pub(super) fn hardware_supported(codec: Codec) -> bool {
    unsafe { VTIsHardwareDecodeSupported(codec.apple_format()) != 0 }
}

pub(super) struct Pixels {
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

pub(super) struct Decoder {
    codec: Codec,
    parameters: ParameterSets,
    av1_config: Vec<u8>,
    av1_dimensions: (i32, i32),
    format: Option<Owned>,
    session: Option<Session>,
    format_dirty: bool,
}

impl Decoder {
    pub(super) fn new(codec: Codec) -> Self {
        Self {
            codec,
            parameters: ParameterSets::default(),
            av1_config: Vec::new(),
            av1_dimensions: (0, 0),
            format: None,
            session: None,
            format_dirty: true,
        }
    }

    pub(super) fn decode(
        &mut self,
        coded: &[u8],
        output_limit: usize,
    ) -> Result<Option<Pixels>, &'static str> {
        let sample;
        let bytes = match self.codec {
            Codec::H264 | Codec::Hevc => {
                let (annex_b, changed) = self.parameters.prepare(self.codec, coded)?;
                self.format_dirty |= changed;
                sample = annex_b;
                if sample.is_empty() {
                    return Ok(None);
                }
                &sample
            }
            Codec::Av1 => {
                let av1 = access_unit::av1(coded)?;
                if self.av1_config != av1.config || self.av1_dimensions != (av1.width, av1.height) {
                    self.av1_config = av1.config.to_vec();
                    self.av1_dimensions = (av1.width, av1.height);
                    self.format_dirty = true;
                }
                av1.sample
            }
        };

        if self.format_dirty {
            let format = self.make_format()?;
            verify_format_size(&format, output_limit)?;
            if self.session.as_ref().is_none_or(|session| unsafe {
                VTDecompressionSessionCanAcceptFormatDescription(session.0.ptr(), format.ptr()) == 0
            }) {
                self.session = Some(Session::new(&format)?);
            }
            self.format = Some(format);
            self.format_dirty = false;
        }

        let format = self
            .format
            .as_ref()
            .ok_or("Missing VideoToolbox input format")?;
        verify_format_size(format, output_limit)?;
        self.session
            .as_ref()
            .ok_or("Missing VideoToolbox decoder")?
            .decode(format, bytes, output_limit)
    }

    fn make_format(&self) -> Result<Owned, &'static str> {
        let mut format = ptr::null();
        let status = match self.codec {
            Codec::H264 | Codec::Hevc => {
                let sets = self.parameters.for_format(self.codec)?;
                let pointers: Vec<_> = sets.iter().map(|set| set.as_ptr()).collect();
                let lengths: Vec<_> = sets.iter().map(|set| set.len()).collect();
                unsafe {
                    if self.codec == Codec::H264 {
                        CMVideoFormatDescriptionCreateFromH264ParameterSets(
                            ptr::null(),
                            sets.len(),
                            pointers.as_ptr(),
                            lengths.as_ptr(),
                            4,
                            &mut format,
                        )
                    } else {
                        CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                            ptr::null(),
                            sets.len(),
                            pointers.as_ptr(),
                            lengths.as_ptr(),
                            4,
                            ptr::null(),
                            &mut format,
                        )
                    }
                }
            }
            Codec::Av1 => {
                let data = Owned::from_raw(
                    unsafe {
                        CFDataCreate(
                            ptr::null(),
                            self.av1_config.as_ptr(),
                            self.av1_config.len() as isize,
                        )
                    },
                    "Cannot allocate av1C data",
                )?;
                let key = Owned::from_raw(
                    unsafe { CFStringCreateWithCString(ptr::null(), c"av1C".as_ptr(), CF_UTF8) },
                    "Cannot allocate av1C key",
                )?;
                let atoms = dictionary(key.ptr(), data.ptr())?;
                let properties = dictionary(
                    unsafe { kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms },
                    atoms.ptr(),
                )?;
                unsafe {
                    CMVideoFormatDescriptionCreate(
                        ptr::null(),
                        self.codec.apple_format(),
                        self.av1_dimensions.0,
                        self.av1_dimensions.1,
                        properties.ptr(),
                        &mut format,
                    )
                }
            }
        };
        Owned::checked(status, format, "VideoToolbox rejected the stream format")
    }
}

struct Session(Owned);

impl Session {
    fn new(format: &Owned) -> Result<Self, &'static str> {
        let pixel_format = i32::from_be_bytes(*b"420v");
        let value = Owned::from_raw(
            unsafe {
                CFNumberCreate(
                    ptr::null(),
                    CF_NUMBER_I32,
                    ptr::from_ref(&pixel_format).cast(),
                )
            },
            "Cannot allocate VideoToolbox pixel format",
        )?;
        let attributes = dictionary(unsafe { kCVPixelBufferPixelFormatTypeKey }, value.ptr())?;
        let callback = CallbackRecord {
            output: output_callback,
            context: ptr::null_mut(),
        };
        let mut session = ptr::null();
        let status = unsafe {
            VTDecompressionSessionCreate(
                ptr::null(),
                format.ptr(),
                ptr::null(),
                attributes.ptr(),
                &callback,
                &mut session,
            )
        };
        Owned::checked(status, session, "Cannot create VideoToolbox decoder").map(Self)
    }

    fn decode(
        &self,
        format: &Owned,
        bytes: &[u8],
        limit: usize,
    ) -> Result<Option<Pixels>, &'static str> {
        let mut block = ptr::null();
        let status = unsafe {
            CMBlockBufferCreateWithMemoryBlock(
                ptr::null(),
                ptr::null_mut(),
                bytes.len(),
                ptr::null(),
                ptr::null(),
                0,
                bytes.len(),
                0,
                &mut block,
            )
        };
        let block = Owned::checked(status, block, "Cannot allocate VideoToolbox sample data")?;
        if unsafe {
            CMBlockBufferReplaceDataBytes(bytes.as_ptr().cast(), block.ptr(), 0, bytes.len())
        } != 0
        {
            return Err("Cannot copy VideoToolbox sample data");
        }

        let mut sample = ptr::null();
        let status = unsafe {
            CMSampleBufferCreateReady(
                ptr::null(),
                block.ptr(),
                format.ptr(),
                1,
                0,
                ptr::null(),
                1,
                &bytes.len(),
                &mut sample,
            )
        };
        let sample = Owned::checked(status, sample, "Cannot create VideoToolbox sample")?;

        // With asynchronous and temporal processing disabled, the output
        // callback has finished before the call returns. It retains the
        // CoreVideo buffer for transfer on this thread.
        let output = Output::default();
        let status = unsafe {
            VTDecompressionSessionDecodeFrame(
                self.0.ptr(),
                sample.ptr(),
                0,
                ptr::from_ref(&output).cast_mut().cast(),
                ptr::null_mut(),
            )
        };
        let image = Owned::from_raw(
            output.image.load(Ordering::Acquire),
            "Missing VideoToolbox output",
        );
        if status != 0 || output.status.load(Ordering::Acquire) != 0 {
            return Err("VideoToolbox could not decode the access unit");
        }
        match image {
            Ok(image) => read_pixels(&image, limit).map(Some),
            Err(_) => Ok(None),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe { VTDecompressionSessionInvalidate(self.0.ptr()) };
    }
}

fn verify_format_size(format: &Owned, limit: usize) -> Result<(), &'static str> {
    let dimensions = unsafe { CMVideoFormatDescriptionGetDimensions(format.ptr()) };
    let width = usize::try_from(dimensions.width).unwrap_or(0);
    let height = usize::try_from(dimensions.height).unwrap_or(0);
    nv12_size(width, height, limit).map(|_| ())
}

fn nv12_size(width: usize, height: usize, limit: usize) -> Result<usize, &'static str> {
    if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err("Invalid VideoToolbox NV12 dimensions");
    }
    width
        .checked_mul(height)
        .and_then(|area| area.checked_add(area / 2))
        .filter(|&size| size <= limit)
        .ok_or("VideoToolbox NV12 output exceeds its mapped buffer")
}

fn read_pixels(image: &Owned, limit: usize) -> Result<Pixels, &'static str> {
    let width = unsafe { CVPixelBufferGetWidth(image.ptr()) };
    let height = unsafe { CVPixelBufferGetHeight(image.ptr()) };
    let size = nv12_size(width, height, limit)?;
    if unsafe { CVPixelBufferGetPlaneCount(image.ptr()) } != 2
        || unsafe { CVPixelBufferGetPixelFormatType(image.ptr()) } != u32::from_be_bytes(*b"420v")
    {
        return Err("VideoToolbox output is not NV12 video range");
    }

    if unsafe { CVPixelBufferLockBaseAddress(image.ptr(), CV_READ_ONLY) } != 0 {
        return Err("Cannot map VideoToolbox output");
    }
    let _lock = PixelLock(image.ptr());
    let mut bytes = vec![0; size];
    for (plane, rows, offset) in [(0, height, 0), (1, height / 2, width * height)] {
        let plane_base =
            unsafe { CVPixelBufferGetBaseAddressOfPlane(image.ptr(), plane) }.cast::<u8>();
        let stride = unsafe { CVPixelBufferGetBytesPerRowOfPlane(image.ptr(), plane) };
        let available = unsafe { CVPixelBufferGetHeightOfPlane(image.ptr(), plane) };
        if plane_base.is_null()
            || stride < width
            || available < rows
            || stride.checked_mul(rows).is_none()
        {
            return Err("Invalid VideoToolbox pixel plane");
        }
        for row in 0..rows {
            unsafe {
                ptr::copy_nonoverlapping(
                    plane_base.add(row * stride),
                    bytes.as_mut_ptr().add(offset + row * width),
                    width,
                );
            }
        }
    }
    Ok(Pixels {
        width: u32::try_from(width).map_err(|_| "VideoToolbox picture width overflow")?,
        height: u32::try_from(height).map_err(|_| "VideoToolbox picture height overflow")?,
        bytes,
    })
}

struct PixelLock(CfRef);

impl Drop for PixelLock {
    fn drop(&mut self) {
        unsafe { CVPixelBufferUnlockBaseAddress(self.0, CV_READ_ONLY) };
    }
}

#[derive(Default)]
struct Output {
    status: AtomicI32,
    image: AtomicPtr<c_void>,
}

unsafe extern "C" fn output_callback(
    _: *mut c_void,
    source: *mut c_void,
    status: i32,
    _: u32,
    image: CfRef,
    _: Time,
    _: Time,
) {
    let Some(output) = (unsafe { source.cast::<Output>().as_ref() }) else {
        return;
    };
    output.status.store(status, Ordering::Relaxed);
    if !image.is_null() {
        unsafe { CFRetain(image) };
        let old = output.image.swap(image.cast_mut(), Ordering::Release);
        if !old.is_null() {
            unsafe { CFRelease(old) };
        }
    }
}

type CfRef = *const c_void;
const CF_UTF8: u32 = 0x0800_0100;
const CF_NUMBER_I32: isize = 3;
const CV_READ_ONLY: u64 = 1;

#[repr(C)]
struct Time {
    value: i64,
    scale: i32,
    flags: u32,
    epoch: i64,
}

#[repr(C)]
struct Dimensions {
    width: i32,
    height: i32,
}

#[repr(C)]
struct CallbackRecord {
    output: unsafe extern "C" fn(*mut c_void, *mut c_void, i32, u32, CfRef, Time, Time),
    context: *mut c_void,
}

struct Owned(CfRef);

impl Owned {
    fn ptr(&self) -> CfRef {
        self.0
    }

    fn from_raw(raw: CfRef, error: &'static str) -> Result<Self, &'static str> {
        if raw.is_null() {
            Err(error)
        } else {
            Ok(Self(raw))
        }
    }

    fn checked(status: i32, raw: CfRef, error: &'static str) -> Result<Self, &'static str> {
        let value = Self::from_raw(raw, error)?;
        if status == 0 { Ok(value) } else { Err(error) }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

fn dictionary(key: CfRef, value: CfRef) -> Result<Owned, &'static str> {
    Owned::from_raw(
        unsafe {
            CFDictionaryCreate(
                ptr::null(),
                &key,
                &value,
                1,
                (&raw const kCFTypeDictionaryKeyCallBacks).cast(),
                (&raw const kCFTypeDictionaryValueCallBacks).cast(),
            )
        },
        "Cannot allocate VideoToolbox attributes",
    )
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeDictionaryKeyCallBacks: u8;
    static kCFTypeDictionaryValueCallBacks: u8;
    fn CFRetain(value: CfRef) -> CfRef;
    fn CFRelease(value: CfRef);
    fn CFDataCreate(allocator: CfRef, bytes: *const u8, length: isize) -> CfRef;
    fn CFNumberCreate(allocator: CfRef, kind: isize, value: CfRef) -> CfRef;
    fn CFStringCreateWithCString(
        allocator: CfRef,
        string: *const std::ffi::c_char,
        encoding: u32,
    ) -> CfRef;
    fn CFDictionaryCreate(
        allocator: CfRef,
        keys: *const CfRef,
        values: *const CfRef,
        count: isize,
        key_callbacks: CfRef,
        value_callbacks: CfRef,
    ) -> CfRef;
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    static kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms: CfRef;
    fn CMVideoFormatDescriptionCreate(
        allocator: CfRef,
        codec: u32,
        width: i32,
        height: i32,
        extensions: CfRef,
        result: *mut CfRef,
    ) -> i32;
    fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
        allocator: CfRef,
        count: usize,
        parameter_sets: *const *const u8,
        sizes: *const usize,
        length_size: i32,
        result: *mut CfRef,
    ) -> i32;
    fn CMVideoFormatDescriptionCreateFromHEVCParameterSets(
        allocator: CfRef,
        count: usize,
        parameter_sets: *const *const u8,
        sizes: *const usize,
        length_size: i32,
        extensions: CfRef,
        result: *mut CfRef,
    ) -> i32;
    fn CMVideoFormatDescriptionGetDimensions(format: CfRef) -> Dimensions;
    fn CMBlockBufferCreateWithMemoryBlock(
        allocator: CfRef,
        memory: *mut c_void,
        length: usize,
        memory_allocator: CfRef,
        custom: CfRef,
        offset: usize,
        data_length: usize,
        flags: u32,
        result: *mut CfRef,
    ) -> i32;
    fn CMBlockBufferReplaceDataBytes(
        source: CfRef,
        block: CfRef,
        offset: usize,
        length: usize,
    ) -> i32;
    fn CMSampleBufferCreateReady(
        allocator: CfRef,
        data: CfRef,
        format: CfRef,
        samples: isize,
        num_timing: isize,
        timing: CfRef,
        num_sizes: isize,
        sizes: *const usize,
        result: *mut CfRef,
    ) -> i32;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: CfRef;
    fn CVPixelBufferGetPlaneCount(image: CfRef) -> usize;
    fn CVPixelBufferGetPixelFormatType(image: CfRef) -> u32;
    fn CVPixelBufferGetHeight(image: CfRef) -> usize;
    fn CVPixelBufferGetWidth(image: CfRef) -> usize;
    fn CVPixelBufferGetHeightOfPlane(image: CfRef, plane: usize) -> usize;
    fn CVPixelBufferGetBytesPerRowOfPlane(image: CfRef, plane: usize) -> usize;
    fn CVPixelBufferGetBaseAddressOfPlane(image: CfRef, plane: usize) -> *mut c_void;
    fn CVPixelBufferLockBaseAddress(image: CfRef, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(image: CfRef, flags: u64) -> i32;
}

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    fn VTIsHardwareDecodeSupported(codec: u32) -> u8;
    fn VTDecompressionSessionCreate(
        allocator: CfRef,
        format: CfRef,
        specification: CfRef,
        attributes: CfRef,
        callback: *const CallbackRecord,
        result: *mut CfRef,
    ) -> i32;
    fn VTDecompressionSessionCanAcceptFormatDescription(session: CfRef, format: CfRef) -> u8;
    fn VTDecompressionSessionInvalidate(session: CfRef);
    fn VTDecompressionSessionDecodeFrame(
        session: CfRef,
        sample: CfRef,
        flags: u32,
        source: *mut c_void,
        info: *mut u32,
    ) -> i32;
}
