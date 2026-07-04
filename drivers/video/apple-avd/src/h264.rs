use alloc::vec::Vec;

use scarlet::device::video::SCARLET_VIDEO_PIXEL_FORMAT_NV12;

/// H.264 frontend parsing error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum H264FrontendError {
    /// Input did not contain an Annex B start code.
    MissingStartCode,
    /// A start code was found without a following NAL header.
    EmptyNalUnit,
    /// SPS or PPS metadata is not available yet.
    MissingParameterSet,
    /// Request dimensions are not valid.
    InvalidDimensions,
}

/// H.264 NAL unit type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum H264NalUnitType {
    /// Coded slice of a non-IDR picture.
    Slice,
    /// Coded slice data partition A.
    DataPartitionA,
    /// Coded slice data partition B.
    DataPartitionB,
    /// Coded slice data partition C.
    DataPartitionC,
    /// Coded slice of an IDR picture.
    IdrSlice,
    /// Supplemental enhancement information.
    Sei,
    /// Sequence parameter set.
    Sps,
    /// Picture parameter set.
    Pps,
    /// Access unit delimiter.
    Aud,
    /// End of sequence.
    EndOfSequence,
    /// End of stream.
    EndOfStream,
    /// Filler data.
    Filler,
    /// Sequence parameter set extension.
    SpsExtension,
    /// Unknown or reserved type.
    Other(u8),
}

impl H264NalUnitType {
    /// Decode a raw H.264 NAL type value.
    ///
    /// # Arguments
    ///
    /// * `value` - Low five bits from a NAL unit header.
    ///
    /// # Returns
    ///
    /// Classified NAL unit type.
    pub fn from_raw(value: u8) -> Self {
        match value {
            1 => Self::Slice,
            2 => Self::DataPartitionA,
            3 => Self::DataPartitionB,
            4 => Self::DataPartitionC,
            5 => Self::IdrSlice,
            6 => Self::Sei,
            7 => Self::Sps,
            8 => Self::Pps,
            9 => Self::Aud,
            10 => Self::EndOfSequence,
            11 => Self::EndOfStream,
            12 => Self::Filler,
            13 => Self::SpsExtension,
            other => Self::Other(other),
        }
    }

    /// Return whether this NAL unit contains slice data.
    ///
    /// # Returns
    ///
    /// `true` for IDR and non-IDR slice types.
    pub fn is_slice(self) -> bool {
        matches!(self, Self::Slice | Self::IdrSlice)
    }
}

/// Borrowed H.264 Annex B NAL unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct H264NalUnit<'a> {
    /// Byte offset of the NAL header in the original access unit.
    pub offset: usize,
    /// NAL reference IDC bits.
    pub nal_ref_idc: u8,
    /// Classified NAL unit type.
    pub unit_type: H264NalUnitType,
    /// NAL payload including the one-byte NAL header.
    pub payload: &'a [u8],
}

/// Borrowed H.264 Annex B access unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnexBAccessUnit<'a> {
    bytes: &'a [u8],
}

impl<'a> AnnexBAccessUnit<'a> {
    /// Create an access unit wrapper from Annex B bytes.
    ///
    /// # Arguments
    ///
    /// * `bytes` - Annex B access unit containing one or more start codes.
    ///
    /// # Returns
    ///
    /// Borrowed access unit wrapper.
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// Return the raw access unit bytes.
    ///
    /// # Returns
    ///
    /// Borrowed input byte slice.
    pub const fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// Parse NAL units from this access unit.
    ///
    /// # Returns
    ///
    /// Vector of borrowed NAL units in stream order.
    pub fn parse_nals(&self) -> Result<Vec<H264NalUnit<'a>>, H264FrontendError> {
        let mut units = Vec::new();
        let mut cursor = 0usize;

        while let Some((start, prefix_len)) = find_start_code(self.bytes, cursor) {
            let nal_start = start + prefix_len;
            if nal_start >= self.bytes.len() {
                return Err(H264FrontendError::EmptyNalUnit);
            }

            let next = find_start_code(self.bytes, nal_start)
                .map(|(offset, _)| offset)
                .unwrap_or(self.bytes.len());
            if next == nal_start {
                return Err(H264FrontendError::EmptyNalUnit);
            }

            let header = self.bytes[nal_start];
            units.push(H264NalUnit {
                offset: nal_start,
                nal_ref_idc: (header >> 5) & 0x3,
                unit_type: H264NalUnitType::from_raw(header & 0x1f),
                payload: &self.bytes[nal_start..next],
            });
            cursor = next;
        }

        if units.is_empty() {
            Err(H264FrontendError::MissingStartCode)
        } else {
            Ok(units)
        }
    }

    /// Return true when the access unit contains an IDR slice.
    ///
    /// # Returns
    ///
    /// `true` if parsing succeeds and one NAL unit is an IDR slice.
    pub fn contains_idr(&self) -> bool {
        self.parse_nals()
            .map(|units| {
                units
                    .iter()
                    .any(|unit| matches!(unit.unit_type, H264NalUnitType::IdrSlice))
            })
            .unwrap_or(false)
    }

    /// Return true when the access unit contains both SPS and PPS NAL units.
    ///
    /// # Returns
    ///
    /// `true` when SPS and PPS are both present.
    pub fn contains_parameter_sets(&self) -> bool {
        self.parse_nals()
            .map(|units| {
                let has_sps = units
                    .iter()
                    .any(|unit| matches!(unit.unit_type, H264NalUnitType::Sps));
                let has_pps = units
                    .iter()
                    .any(|unit| matches!(unit.unit_type, H264NalUnitType::Pps));
                has_sps && has_pps
            })
            .unwrap_or(false)
    }
}

/// H.264 decode request flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct H264DecodeFlags(u32);

impl H264DecodeFlags {
    /// Request contains an IDR picture.
    pub const IDR: Self = Self(1 << 0);
    /// Request carries SPS/PPS NAL units.
    pub const PARAMETER_SETS: Self = Self(1 << 1);
    /// Request should be treated as end of stream.
    pub const END_OF_STREAM: Self = Self(1 << 2);

    /// Empty flag set.
    ///
    /// # Returns
    ///
    /// No decode flags set.
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Return the raw flag bits.
    ///
    /// # Returns
    ///
    /// Raw flag bitset.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Return whether all bits in `other` are present.
    ///
    /// # Arguments
    ///
    /// * `other` - Flags that must be present.
    ///
    /// # Returns
    ///
    /// `true` when all requested flags are set.
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Add flags to this set.
    ///
    /// # Arguments
    ///
    /// * `other` - Flags to set.
    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

/// Device-visible buffer range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvdDmaRange {
    /// Device-visible DMA address.
    pub dma_addr: u64,
    /// Byte length of the range.
    pub len: usize,
}

/// Decoded NV12 frame layout expected from Apple AVD.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvdFrameLayout {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Luma plane stride in bytes.
    pub y_stride: u32,
    /// Interleaved UV plane stride in bytes.
    pub uv_stride: u32,
    /// Pixel format; currently NV12.
    pub pixel_format: u32,
}

impl AvdFrameLayout {
    /// Construct an NV12 frame layout.
    ///
    /// # Arguments
    ///
    /// * `width` - Frame width in pixels.
    /// * `height` - Frame height in pixels.
    /// * `y_stride` - Luma plane stride in bytes.
    /// * `uv_stride` - Interleaved UV plane stride in bytes.
    ///
    /// # Returns
    ///
    /// NV12 frame layout.
    pub const fn nv12(width: u32, height: u32, y_stride: u32, uv_stride: u32) -> Self {
        Self {
            width,
            height,
            y_stride,
            uv_stride,
            pixel_format: SCARLET_VIDEO_PIXEL_FORMAT_NV12,
        }
    }

    /// Return the minimum output buffer size for this frame.
    ///
    /// # Returns
    ///
    /// Number of bytes required for tightly stacked Y and UV planes.
    pub fn output_len(&self) -> usize {
        self.y_stride as usize * self.height as usize
            + self.uv_stride as usize * (self.height as usize / 2)
    }
}

/// H.264 decode request lowered for the Apple AVD command path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct H264DecodeRequest {
    /// Video session identifier.
    pub session_id: u64,
    /// Driver-local frame number.
    pub frame_number: u32,
    /// Input Annex B byte stream.
    pub input: AvdDmaRange,
    /// Output NV12 frame buffer.
    pub output: AvdDmaRange,
    /// Decoded frame layout.
    pub layout: AvdFrameLayout,
    /// Request flags derived from the access unit.
    pub flags: H264DecodeFlags,
}

impl H264DecodeRequest {
    /// Build a decode request from an Annex B access unit and DMA buffers.
    ///
    /// # Arguments
    ///
    /// * `session_id` - Video session identifier.
    /// * `frame_number` - Driver-local frame number.
    /// * `access_unit` - Annex B H.264 access unit.
    /// * `input` - Device-visible input range.
    /// * `output` - Device-visible output range.
    /// * `layout` - Expected decoded frame layout.
    ///
    /// # Returns
    ///
    /// H.264 decode request ready for firmware command lowering.
    pub fn from_access_unit(
        session_id: u64,
        frame_number: u32,
        access_unit: &AnnexBAccessUnit<'_>,
        input: AvdDmaRange,
        output: AvdDmaRange,
        layout: AvdFrameLayout,
    ) -> Result<Self, H264FrontendError> {
        if layout.width == 0 || layout.height == 0 {
            return Err(H264FrontendError::InvalidDimensions);
        }

        let nals = access_unit.parse_nals()?;
        let mut flags = H264DecodeFlags::empty();
        if nals
            .iter()
            .any(|unit| matches!(unit.unit_type, H264NalUnitType::IdrSlice))
        {
            flags.insert(H264DecodeFlags::IDR);
        }

        let has_sps = nals
            .iter()
            .any(|unit| matches!(unit.unit_type, H264NalUnitType::Sps));
        let has_pps = nals
            .iter()
            .any(|unit| matches!(unit.unit_type, H264NalUnitType::Pps));
        if has_sps && has_pps {
            flags.insert(H264DecodeFlags::PARAMETER_SETS);
        }

        if nals.iter().any(|unit| {
            matches!(
                unit.unit_type,
                H264NalUnitType::EndOfSequence | H264NalUnitType::EndOfStream
            )
        }) {
            flags.insert(H264DecodeFlags::END_OF_STREAM);
        }

        Ok(Self {
            session_id,
            frame_number,
            input,
            output,
            layout,
            flags,
        })
    }
}

fn find_start_code(bytes: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut i = from;
    while i + 3 <= bytes.len() {
        if bytes[i] == 0 && bytes[i + 1] == 0 {
            if bytes[i + 2] == 1 {
                return Some((i, 3));
            }
            if i + 4 <= bytes.len() && bytes[i + 2] == 0 && bytes[i + 3] == 1 {
                return Some((i, 4));
            }
        }
        i += 1;
    }
    None
}
