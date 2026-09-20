//! Producer-independent, ready/read-only shared image descriptors.
//!
//! All addresses in this ABI are offsets within capability-owned buffers,
//! never physical or virtual addresses. A returned image is ready to read and
//! remains immutable until the last consumer releases its capability.

pub const SHARED_IMAGE_ABI_VERSION: u32 = 1;
pub const SHARED_IMAGE_QUERY: u32 = 0x4940;
pub const SHARED_IMAGE_MAX_PLANES: usize = 4;
pub const SHARED_IMAGE_MAX_BUFFERS: usize = 4;
pub const IMAGE_FORMAT_NV12: u32 = u32::from_le_bytes(*b"NV12");
pub const IMAGE_FORMAT_BGRA8888: u32 = u32::from_le_bytes(*b"AR24");
pub const IMAGE_MODIFIER_LINEAR: u64 = 0;

pub const COLOR_UNSPECIFIED: u32 = 0;
pub const COLOR_MATRIX_BT601: u32 = 1;
pub const COLOR_MATRIX_BT709: u32 = 2;
pub const COLOR_MATRIX_BT2020: u32 = 3;
pub const COLOR_RANGE_LIMITED: u32 = 1;
pub const COLOR_RANGE_FULL: u32 = 2;
pub const CHROMA_COSITED: u32 = 1;
pub const CHROMA_MIDPOINT: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImageRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Color interpretation is independent of the storage format.
/// Primaries and transfer use ISO/IEC 23091-2 code points; zero is unspecified.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImageColor {
    pub matrix: u32,
    pub range: u32,
    pub primaries: u32,
    pub transfer: u32,
    pub chroma_x: u32,
    pub chroma_y: u32,
    pub reserved: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SharedImagePlane {
    pub buffer_index: u32,
    pub row_pitch: u32,
    pub offset: u64,
    pub size: u64,
    pub reserved: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SharedImageDescriptor {
    pub version: u32,
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub visible: ImageRect,
    pub modifier: u64,
    pub color: ImageColor,
    pub buffer_count: u32,
    pub plane_count: u32,
    pub buffer_sizes: [u64; SHARED_IMAGE_MAX_BUFFERS],
    pub planes: [SharedImagePlane; SHARED_IMAGE_MAX_PLANES],
    pub reserved: [u64; 2],
}

impl SharedImageDescriptor {
    /// Check generic geometry, storage ranges and reserved bytes. Modifier
    /// interpretation and consumer support are checked separately on import.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != SHARED_IMAGE_ABI_VERSION
            || self.width == 0
            || self.height == 0
            || self.visible.width == 0
            || self.visible.height == 0
            || self
                .visible
                .x
                .checked_add(self.visible.width)
                .is_none_or(|v| v > self.width)
            || self
                .visible
                .y
                .checked_add(self.visible.height)
                .is_none_or(|v| v > self.height)
            || self.buffer_count == 0
            || self.buffer_count as usize > SHARED_IMAGE_MAX_BUFFERS
            || self.reserved != [0; 2]
            || self.color.reserved != [0; 2]
            || self.color.matrix > COLOR_MATRIX_BT2020
            || self.color.range > COLOR_RANGE_FULL
            || self.color.chroma_x > CHROMA_MIDPOINT
            || self.color.chroma_y > CHROMA_MIDPOINT
        {
            return Err("invalid shared image descriptor");
        }
        let plane_count = match self.format {
            IMAGE_FORMAT_NV12 => 2,
            IMAGE_FORMAT_BGRA8888 => 1,
            _ => return Err("unsupported shared image format"),
        };
        if self.plane_count != plane_count {
            return Err("invalid shared image plane count");
        }
        for (index, &size) in self.buffer_sizes.iter().enumerate() {
            if (index < self.buffer_count as usize) != (size != 0) {
                return Err("invalid shared image buffer size");
            }
        }
        for (index, plane) in self.planes.iter().enumerate() {
            if index >= plane_count as usize {
                if *plane != SharedImagePlane::default() {
                    return Err("unused shared image plane");
                }
                continue;
            }
            if plane.buffer_index >= self.buffer_count
                || plane.size == 0
                || plane.row_pitch == 0
                || plane.reserved != 0
                || plane
                    .offset
                    .checked_add(plane.size)
                    .is_none_or(|end| end > self.buffer_sizes[plane.buffer_index as usize])
            {
                return Err("shared image plane exceeds backing");
            }
            if self.modifier == IMAGE_MODIFIER_LINEAR {
                let (row_bytes, rows) = if self.format == IMAGE_FORMAT_BGRA8888 {
                    (u64::from(self.width) * 4, self.height)
                } else if index == 0 {
                    (u64::from(self.width), self.height)
                } else {
                    (
                        u64::from(self.width).div_ceil(2) * 2,
                        self.height.div_ceil(2),
                    )
                };
                let required = u64::from(rows - 1)
                    .checked_mul(u64::from(plane.row_pitch))
                    .and_then(|v| v.checked_add(row_bytes))
                    .ok_or("shared image plane size overflow")?;
                if u64::from(plane.row_pitch) < row_bytes || plane.size < required {
                    return Err("shared image linear plane is truncated");
                }
            }
        }
        Ok(())
    }
}

const _: [(); 256] = [(); core::mem::size_of::<SharedImageDescriptor>()];

/// Import a read-only image into a GPU device. The conversion describes the
/// sampled RGB interpretation; it does not change the producer's metadata.
pub const GPU_IMPORT_SHARED_IMAGE: u32 = 0x4770;
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GpuImportSharedImage {
    pub abi_version: u32,
    pub source_handle: u32,
    pub image_handle: u32,
    pub result: u32,
    pub color: ImageColor,
}
const _: [(); 48] = [(); core::mem::size_of::<GpuImportSharedImage>()];

#[cfg(test)]
mod tests {
    use super::*;
    fn odd_nv12() -> SharedImageDescriptor {
        let mut d = SharedImageDescriptor {
            version: 1,
            format: IMAGE_FORMAT_NV12,
            width: 3,
            height: 3,
            visible: ImageRect {
                x: 0,
                y: 0,
                width: 3,
                height: 3,
            },
            buffer_count: 2,
            plane_count: 2,
            buffer_sizes: [9, 8, 0, 0],
            ..Default::default()
        };
        d.planes[0] = SharedImagePlane {
            row_pitch: 3,
            size: 9,
            ..Default::default()
        };
        d.planes[1] = SharedImagePlane {
            buffer_index: 1,
            row_pitch: 4,
            size: 8,
            ..Default::default()
        };
        d
    }
    #[test]
    fn separate_buffers_and_odd_chroma_extent() {
        assert!(odd_nv12().validate().is_ok());
    }
    #[test]
    fn rejects_truncated_chroma() {
        let mut d = odd_nv12();
        d.planes[1].size = 7;
        assert!(d.validate().is_err());
    }
    #[test]
    fn rejects_plane_range_overflow() {
        let mut d = odd_nv12();
        d.planes[1].offset = u64::MAX;
        assert!(d.validate().is_err());
    }
    #[test]
    fn rejects_crop_overflow_and_unused_metadata() {
        let mut d = odd_nv12();
        d.visible.x = u32::MAX;
        assert!(d.validate().is_err());
        let mut d = odd_nv12();
        d.planes[3].size = 1;
        assert!(d.validate().is_err());
    }
    #[test]
    fn unknown_modifier_requires_consumer_validation() {
        let mut d = odd_nv12();
        d.modifier = 0x0300_0000_000f_e011;
        assert!(d.validate().is_ok());
    }
}
