use alloc::vec::Vec;

use scarlet::device::video::SCARLET_VIDEO_PIXEL_FORMAT_NV12;

const H264_PROFILE_HIGH: u8 = 100;
const H264_PROFILE_HIGH_10: u8 = 110;
const H264_PROFILE_HIGH_422: u8 = 122;
const H264_PROFILE_HIGH_444: u8 = 244;
const H264_PROFILE_CAVLC_444: u8 = 44;
const H264_PROFILE_SCALABLE_BASELINE: u8 = 83;
const H264_PROFILE_SCALABLE_HIGH: u8 = 86;
const H264_PROFILE_MULTIVIEW_HIGH: u8 = 118;
const H264_PROFILE_STEREO_HIGH: u8 = 128;
const H264_PROFILE_MULTIVIEW_DEPTH_HIGH: u8 = 138;
const H264_PROFILE_ENHANCED_MULTIVIEW_DEPTH_HIGH: u8 = 139;
const H264_PROFILE_MFC_HIGH: u8 = 134;
const H264_PROFILE_MFC_DEPTH_HIGH: u8 = 135;

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
    /// SPS RBSP data could not be parsed.
    MalformedSps,
    /// Stream uses an H.264 feature this first AVD path does not accept.
    UnsupportedSps,
    /// Slice header metadata could not be parsed.
    MalformedSlice,
    /// Generated AVD instruction stream exceeded its destination buffer.
    InstructionStreamTooLarge,
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

    /// Parse stream parameters from the first SPS NAL in this access unit.
    ///
    /// # Returns
    ///
    /// Parsed SPS-derived stream parameters, or `None` when no SPS is present.
    pub fn stream_parameters(&self) -> Result<Option<H264StreamParameters>, H264FrontendError> {
        for unit in self.parse_nals()? {
            if matches!(unit.unit_type, H264NalUnitType::Sps) {
                return parse_sps(unit.payload).map(Some);
            }
        }
        Ok(None)
    }

    /// Parse metadata from the first coded slice NAL in this access unit.
    ///
    /// # Returns
    ///
    /// Parsed slice metadata, or `None` when the access unit has no slice.
    pub fn first_slice(&self) -> Result<Option<H264SliceParameters>, H264FrontendError> {
        for unit in self.parse_nals()? {
            if unit.unit_type.is_slice() {
                return parse_slice_header(&unit).map(Some);
            }
        }
        Ok(None)
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

/// SPS-derived H.264 stream parameters needed by the AVD frontend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct H264StreamParameters {
    /// Sequence parameter set identifier.
    pub sps_id: u32,
    /// Encoded profile IDC.
    pub profile_idc: u8,
    /// Encoded level IDC.
    pub level_idc: u8,
    /// Chroma format IDC. Initial AVD frontend accepts 4:2:0 only.
    pub chroma_format_idc: u32,
    /// Luma bit depth minus 8.
    pub bit_depth_luma_minus8: u32,
    /// Chroma bit depth minus 8.
    pub bit_depth_chroma_minus8: u32,
    /// Direct 8x8 inference flag.
    pub direct_8x8_inference_flag: bool,
    /// Decoded display width in pixels after cropping.
    pub width: u32,
    /// Decoded display height in pixels after cropping.
    pub height: u32,
    /// Coded width rounded to macroblock units.
    pub coded_width: u32,
    /// Coded height rounded to macroblock units.
    pub coded_height: u32,
    /// Log2 max frame number minus 4.
    pub log2_max_frame_num_minus4: u32,
    /// Pic order count type.
    pub pic_order_cnt_type: u32,
    /// Maximum decoded reference frames requested by the stream.
    pub max_num_ref_frames: u32,
}

impl H264StreamParameters {
    /// Build the NV12 output layout used by the Scarlet video ABI.
    ///
    /// # Returns
    ///
    /// NV12 frame layout with AVD-friendly aligned strides.
    pub fn nv12_layout(&self) -> AvdFrameLayout {
        let y_stride = align_up_u32(self.width, 64);
        AvdFrameLayout::nv12(self.width, self.height, y_stride, y_stride)
    }
}

/// H.264 slice kind collapsed to the modes the AVD command stream needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum H264SliceKind {
    /// I or SI slice.
    I,
    /// P or SP slice.
    P,
    /// B slice.
    B,
}

/// Minimal H.264 slice metadata used by first-pass AVD instruction generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct H264SliceParameters {
    /// Slice NAL unit type.
    pub nal_unit_type: H264NalUnitType,
    /// Slice kind derived from `slice_type`.
    pub kind: H264SliceKind,
    /// Raw `slice_type` value from the bitstream.
    pub slice_type: u32,
    /// Picture parameter set identifier from the slice header.
    pub pic_parameter_set_id: u32,
    /// Byte offset of the NAL header in the original access unit.
    pub nal_offset: usize,
    /// Byte length of the NAL payload including the NAL header.
    pub nal_len: usize,
    /// Approximate number of bits consumed from the slice RBSP by fields parsed here.
    pub parsed_header_bits: usize,
}

impl H264SliceParameters {
    /// Return whether this slice belongs to an IDR picture.
    ///
    /// # Returns
    ///
    /// `true` for IDR slice NALs.
    pub fn is_idr(&self) -> bool {
        matches!(self.nal_unit_type, H264NalUnitType::IdrSlice)
    }
}

/// AVD v3-style instruction stream produced from one H.264 access unit.
pub struct AvdH264InstructionStream {
    words: Vec<u32>,
}

impl AvdH264InstructionStream {
    /// Generate a first-pass AVD H.264 instruction stream.
    ///
    /// The word layout follows the public `eiln/avd` v3 HAL model for the
    /// header and slice command sections. Reference-list and scaling-list
    /// sections are intentionally omitted until the driver keeps a full DPB.
    ///
    /// # Arguments
    ///
    /// * `request` - H.264 decode request being submitted.
    /// * `stream` - Current SPS-derived stream parameters.
    /// * `slice` - First coded slice in the access unit.
    /// * `workspace` - Device-visible workspace addresses.
    ///
    /// # Returns
    ///
    /// Encoded instruction stream words.
    pub fn build(
        request: &H264DecodeRequest,
        stream: &H264StreamParameters,
        slice: &H264SliceParameters,
        workspace: &AvdH264Workspace,
    ) -> Self {
        let mut words = Vec::new();
        let coded_width = stream.coded_width.max(request.layout.width);
        let coded_height = stream.coded_height.max(request.layout.height);
        let y_addr = request.output.dma_addr;
        let uv_addr =
            request.output.dma_addr + request.layout.y_stride as u64 * request.layout.height as u64;
        let is_idr = slice.is_idr();

        push(&mut words, 0x2b00_0000 | 0x100, "cm3_cmd_inst_fifo_start");
        let mut start = 0x1000 | 0x02e0;
        if is_idr {
            start |= 0x2000;
        }
        push(&mut words, 0x2db0_0000 | start, "hdr_34_cmd_start_hdr");
        push(&mut words, 0x0100_0000, "hdr_38_mode");
        push(
            &mut words,
            (((coded_height - 1) & 0xffff) << 16) | ((coded_width - 1) & 0xffff),
            "hdr_3c_height_width",
        );
        push(&mut words, 0, "hdr_40_zero");
        push(
            &mut words,
            (((coded_height - 1) >> 3) << 16) | ((coded_width - 1) >> 3),
            "hdr_28_height_width_shift3",
        );

        let mut sps_param = (stream.chroma_format_idc & 3) << 24;
        sps_param |= (stream.bit_depth_luma_minus8 & 15) << 19;
        sps_param |= (stream.bit_depth_chroma_minus8 & 15) << 15;
        sps_param |= 0x2800;
        if stream.direct_8x8_inference_flag {
            sps_param |= 1;
        }
        push(&mut words, sps_param, "hdr_2c_sps_param");

        let mut flags = 0;
        if !is_idr {
            flags |= 1 << 21;
        }
        push(&mut words, flags, "hdr_44_flags");
        push(&mut words, 0, "hdr_48_chroma_qp_index_offset");
        push(&mut words, 0x0030_000a, "hdr_58_const_3a");
        push(&mut words, 0x0402_0002, "cm3_dma_config_1");
        push(&mut words, 0x0002_0002, "cm3_dma_config_2");
        push(&mut words, 0, "cm3_mark_end_section");
        push(
            &mut words,
            (workspace.pps_tile_dma_addr >> 8) as u32,
            "hdr_9c_pps_tile_addr_lsb8",
        );
        push(&mut words, 0x0402_0002, "cm3_dma_config_3");
        push(&mut words, 0x0402_0002, "cm3_dma_config_4");
        push(&mut words, 0, "cm3_mark_end_section");
        push(
            &mut words,
            ((workspace.pps_tile_dma_addr + 0x8000) >> 8) as u32,
            "hdr_9c_pps_tile_addr_lsb8",
        );
        push(
            &mut words,
            ((workspace.pps_tile_dma_addr + 0x10000) >> 8) as u32,
            "hdr_9c_pps_tile_addr_lsb8",
        );
        push(
            &mut words,
            ((workspace.pps_tile_dma_addr + 0x18000) >> 8) as u32,
            "hdr_9c_pps_tile_addr_lsb8",
        );
        push(&mut words, 0x0007_0007, "cm3_dma_config_5");
        push(
            &mut words,
            (workspace.reference_dma_addr >> 7) as u32,
            "hdr_c0_curr_ref_addr_lsb7",
        );
        push(
            &mut words,
            ((workspace.reference_dma_addr + 0x4000) >> 7) as u32,
            "hdr_c0_curr_ref_addr_lsb7",
        );
        push(
            &mut words,
            ((workspace.reference_dma_addr + 0x8000) >> 7) as u32,
            "hdr_c0_curr_ref_addr_lsb7",
        );
        push(
            &mut words,
            ((workspace.reference_dma_addr + 0xc000) >> 7) as u32,
            "hdr_c0_curr_ref_addr_lsb7",
        );
        push(&mut words, (y_addr >> 8) as u32, "hdr_210_y_addr_lsb8");
        push(
            &mut words,
            request.layout.y_stride >> 4,
            "hdr_218_width_align",
        );
        push(&mut words, (uv_addr >> 8) as u32, "hdr_214_uv_addr_lsb8");
        push(
            &mut words,
            request.layout.uv_stride >> 4,
            "hdr_21c_width_align",
        );
        push(&mut words, 0, "cm3_mark_end_section");
        push(
            &mut words,
            (((coded_height - 1) & 0xffff) << 16) | ((coded_width - 1) & 0xffff),
            "hdr_54_height_width",
        );
        push(&mut words, 0, "cm3_mark_end_section_scl");

        push(&mut words, 0x2d80_0000, "slc_a7c_cmd_set_coded_slice");
        push(
            &mut words,
            (request.input.dma_addr as usize + slice.nal_offset) as u32,
            "slc_a84_slice_addr_low",
        );
        push(
            &mut words,
            slice.nal_len as u32,
            "slc_a88_slice_payload_size",
        );
        push(&mut words, 0x2c00_0000, "cm3_cmd_exec_mb_vp");
        push(
            &mut words,
            0x2d90_0000 | (26 * 0x400),
            "slc_a70_cmd_quant_param",
        );
        push(&mut words, 0x2da0_0000, "slc_a74_cmd_deblocking_filter");
        push(&mut words, 0x2a00_0000, "cm3_cmd_set_mb_dims");
        push(
            &mut words,
            (((coded_height - 1) >> 4) << 12) | ((coded_width - 1) >> 4),
            "cm3_set_mb_dims",
        );
        let ref_type = match slice.kind {
            H264SliceKind::I => 0x20000,
            H264SliceKind::P => 0x10000,
            H264SliceKind::B => 0x40000,
        };
        push(&mut words, 0x2d00_0000 | ref_type, "slc_6e4_cmd_ref_type");
        push(&mut words, 0x2b00_0000 | 0x400, "cm3_cmd_inst_fifo_end");

        Self { words }
    }

    /// Return encoded instruction words.
    ///
    /// # Returns
    ///
    /// Instruction word slice.
    pub fn words(&self) -> &[u32] {
        &self.words
    }

    /// Copy this stream as little-endian u32 words.
    ///
    /// # Arguments
    ///
    /// * `dst` - Destination byte buffer.
    ///
    /// # Returns
    ///
    /// Number of bytes written.
    pub fn write_le_bytes(&self, dst: &mut [u8]) -> Result<usize, H264FrontendError> {
        let byte_len = self.words.len() * core::mem::size_of::<u32>();
        if byte_len > dst.len() {
            return Err(H264FrontendError::InstructionStreamTooLarge);
        }
        for (index, word) in self.words.iter().enumerate() {
            let offset = index * 4;
            dst[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
        }
        Ok(byte_len)
    }
}

/// Device-visible addresses of AVD session work areas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvdH264Workspace {
    /// Instruction FIFO memory.
    pub instruction_fifo_dma_addr: u64,
    /// PPS/intermediate tile memory.
    pub pps_tile_dma_addr: u64,
    /// SPS tile memory.
    pub sps_tile_dma_addr: u64,
    /// Reference scratch memory.
    pub reference_dma_addr: u64,
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
    /// First slice metadata used for AVD instruction generation.
    pub slice: H264SliceParameters,
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
        let slice = access_unit
            .first_slice()?
            .ok_or(H264FrontendError::MalformedSlice)?;
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
            slice,
        })
    }
}

fn push(words: &mut Vec<u32>, value: u32, _name: &'static str) {
    words.push(value);
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

fn parse_slice_header(unit: &H264NalUnit<'_>) -> Result<H264SliceParameters, H264FrontendError> {
    let rbsp = rbsp_from_nal_payload(unit.payload)?;
    let mut reader = BitReader::new(&rbsp);
    let _first_mb_in_slice = reader
        .read_ue()
        .map_err(|_| H264FrontendError::MalformedSlice)?;
    let slice_type = reader
        .read_ue()
        .map_err(|_| H264FrontendError::MalformedSlice)?;
    let pic_parameter_set_id = reader
        .read_ue()
        .map_err(|_| H264FrontendError::MalformedSlice)?;
    let kind = match slice_type % 5 {
        0 | 3 => H264SliceKind::P,
        1 => H264SliceKind::B,
        2 | 4 => H264SliceKind::I,
        _ => return Err(H264FrontendError::MalformedSlice),
    };
    Ok(H264SliceParameters {
        nal_unit_type: unit.unit_type,
        kind,
        slice_type,
        pic_parameter_set_id,
        nal_offset: unit.offset,
        nal_len: unit.payload.len(),
        parsed_header_bits: reader.position_bits(),
    })
}

fn parse_sps(nal_payload: &[u8]) -> Result<H264StreamParameters, H264FrontendError> {
    let rbsp = rbsp_from_nal_payload(nal_payload)?;
    let mut reader = BitReader::new(&rbsp);
    let profile_idc = reader
        .read_bits(8)
        .map_err(|_| H264FrontendError::MalformedSps)? as u8;
    let _constraint_flags = reader
        .read_bits(8)
        .map_err(|_| H264FrontendError::MalformedSps)?;
    let level_idc = reader
        .read_bits(8)
        .map_err(|_| H264FrontendError::MalformedSps)? as u8;
    let sps_id = reader
        .read_ue()
        .map_err(|_| H264FrontendError::MalformedSps)?;

    let mut chroma_format_idc = 1;
    let mut bit_depth_luma_minus8 = 0;
    let mut bit_depth_chroma_minus8 = 0;
    if is_high_profile(profile_idc) {
        chroma_format_idc = reader
            .read_ue()
            .map_err(|_| H264FrontendError::MalformedSps)?;
        if chroma_format_idc == 3 {
            let _separate_colour_plane_flag = reader
                .read_bit()
                .map_err(|_| H264FrontendError::MalformedSps)?;
        }
        bit_depth_luma_minus8 = reader
            .read_ue()
            .map_err(|_| H264FrontendError::MalformedSps)?;
        bit_depth_chroma_minus8 = reader
            .read_ue()
            .map_err(|_| H264FrontendError::MalformedSps)?;
        let _qpprime_y_zero_transform_bypass_flag = reader
            .read_bit()
            .map_err(|_| H264FrontendError::MalformedSps)?;
        let seq_scaling_matrix_present = reader
            .read_bit()
            .map_err(|_| H264FrontendError::MalformedSps)?
            != 0;
        if seq_scaling_matrix_present {
            let count = if chroma_format_idc != 3 { 8 } else { 12 };
            for index in 0..count {
                let present = reader
                    .read_bit()
                    .map_err(|_| H264FrontendError::MalformedSps)?
                    != 0;
                if present {
                    skip_scaling_list(&mut reader, if index < 6 { 16 } else { 64 })?;
                }
            }
        }
    }

    if chroma_format_idc != 1 || bit_depth_luma_minus8 != 0 || bit_depth_chroma_minus8 != 0 {
        return Err(H264FrontendError::UnsupportedSps);
    }

    let log2_max_frame_num_minus4 = reader
        .read_ue()
        .map_err(|_| H264FrontendError::MalformedSps)?;
    let pic_order_cnt_type = reader
        .read_ue()
        .map_err(|_| H264FrontendError::MalformedSps)?;
    match pic_order_cnt_type {
        0 => {
            let _log2_max_pic_order_cnt_lsb_minus4 = reader
                .read_ue()
                .map_err(|_| H264FrontendError::MalformedSps)?;
        }
        1 => {
            let _delta_pic_order_always_zero_flag = reader
                .read_bit()
                .map_err(|_| H264FrontendError::MalformedSps)?;
            let _offset_for_non_ref_pic = reader
                .read_se()
                .map_err(|_| H264FrontendError::MalformedSps)?;
            let _offset_for_top_to_bottom_field = reader
                .read_se()
                .map_err(|_| H264FrontendError::MalformedSps)?;
            let count = reader
                .read_ue()
                .map_err(|_| H264FrontendError::MalformedSps)?;
            for _ in 0..count {
                let _offset_for_ref_frame = reader
                    .read_se()
                    .map_err(|_| H264FrontendError::MalformedSps)?;
            }
        }
        _ => return Err(H264FrontendError::UnsupportedSps),
    }
    let max_num_ref_frames = reader
        .read_ue()
        .map_err(|_| H264FrontendError::MalformedSps)?;
    let _gaps_in_frame_num_value_allowed_flag = reader
        .read_bit()
        .map_err(|_| H264FrontendError::MalformedSps)?;
    let pic_width_in_mbs_minus1 = reader
        .read_ue()
        .map_err(|_| H264FrontendError::MalformedSps)?;
    let pic_height_in_map_units_minus1 = reader
        .read_ue()
        .map_err(|_| H264FrontendError::MalformedSps)?;
    let frame_mbs_only_flag = reader
        .read_bit()
        .map_err(|_| H264FrontendError::MalformedSps)?
        != 0;
    if !frame_mbs_only_flag {
        let _mb_adaptive_frame_field_flag = reader
            .read_bit()
            .map_err(|_| H264FrontendError::MalformedSps)?;
        return Err(H264FrontendError::UnsupportedSps);
    }
    let direct_8x8_inference_flag = reader
        .read_bit()
        .map_err(|_| H264FrontendError::MalformedSps)?
        != 0;
    let frame_cropping_flag = reader
        .read_bit()
        .map_err(|_| H264FrontendError::MalformedSps)?
        != 0;
    let (crop_left, crop_right, crop_top, crop_bottom) = if frame_cropping_flag {
        (
            reader
                .read_ue()
                .map_err(|_| H264FrontendError::MalformedSps)?,
            reader
                .read_ue()
                .map_err(|_| H264FrontendError::MalformedSps)?,
            reader
                .read_ue()
                .map_err(|_| H264FrontendError::MalformedSps)?,
            reader
                .read_ue()
                .map_err(|_| H264FrontendError::MalformedSps)?,
        )
    } else {
        (0, 0, 0, 0)
    };

    let coded_width = (pic_width_in_mbs_minus1 + 1) * 16;
    let coded_height = (pic_height_in_map_units_minus1 + 1) * 16;
    let crop_unit_x = 2;
    let crop_unit_y = 2;
    let crop_x = (crop_left + crop_right) * crop_unit_x;
    let crop_y = (crop_top + crop_bottom) * crop_unit_y;
    if crop_x >= coded_width || crop_y >= coded_height {
        return Err(H264FrontendError::InvalidDimensions);
    }
    let width = coded_width - crop_x;
    let height = coded_height - crop_y;
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err(H264FrontendError::InvalidDimensions);
    }

    Ok(H264StreamParameters {
        sps_id,
        profile_idc,
        level_idc,
        chroma_format_idc,
        bit_depth_luma_minus8,
        bit_depth_chroma_minus8,
        direct_8x8_inference_flag,
        width,
        height,
        coded_width,
        coded_height,
        log2_max_frame_num_minus4,
        pic_order_cnt_type,
        max_num_ref_frames,
    })
}

fn rbsp_from_nal_payload(nal_payload: &[u8]) -> Result<Vec<u8>, H264FrontendError> {
    if nal_payload.is_empty() {
        return Err(H264FrontendError::EmptyNalUnit);
    }
    let mut rbsp = Vec::with_capacity(nal_payload.len());
    let mut zero_count = 0usize;
    for &byte in &nal_payload[1..] {
        if zero_count >= 2 && byte == 0x03 {
            zero_count = 0;
            continue;
        }
        rbsp.push(byte);
        if byte == 0 {
            zero_count += 1;
        } else {
            zero_count = 0;
        }
    }
    Ok(rbsp)
}

fn is_high_profile(profile_idc: u8) -> bool {
    matches!(
        profile_idc,
        H264_PROFILE_HIGH
            | H264_PROFILE_HIGH_10
            | H264_PROFILE_HIGH_422
            | H264_PROFILE_HIGH_444
            | H264_PROFILE_CAVLC_444
            | H264_PROFILE_SCALABLE_BASELINE
            | H264_PROFILE_SCALABLE_HIGH
            | H264_PROFILE_MULTIVIEW_HIGH
            | H264_PROFILE_STEREO_HIGH
            | H264_PROFILE_MULTIVIEW_DEPTH_HIGH
            | H264_PROFILE_ENHANCED_MULTIVIEW_DEPTH_HIGH
            | H264_PROFILE_MFC_HIGH
            | H264_PROFILE_MFC_DEPTH_HIGH
    )
}

fn skip_scaling_list(reader: &mut BitReader<'_>, count: usize) -> Result<(), H264FrontendError> {
    let mut last_scale = 8i32;
    let mut next_scale = 8i32;
    for _ in 0..count {
        if next_scale != 0 {
            let delta_scale = reader
                .read_se()
                .map_err(|_| H264FrontendError::MalformedSps)?;
            next_scale = (last_scale + delta_scale + 256) % 256;
        }
        last_scale = if next_scale == 0 {
            last_scale
        } else {
            next_scale
        };
    }
    Ok(())
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    fn read_bit(&mut self) -> Result<u8, ()> {
        if self.bit_pos >= self.data.len() * 8 {
            return Err(());
        }
        let byte = self.data[self.bit_pos / 8];
        let shift = 7 - (self.bit_pos % 8);
        self.bit_pos += 1;
        Ok((byte >> shift) & 1)
    }

    fn read_bits(&mut self, bits: u8) -> Result<u32, ()> {
        let mut value = 0u32;
        for _ in 0..bits {
            value = (value << 1) | self.read_bit()? as u32;
        }
        Ok(value)
    }

    fn read_ue(&mut self) -> Result<u32, ()> {
        let mut leading_zero_bits = 0u32;
        while self.read_bit()? == 0 {
            leading_zero_bits += 1;
            if leading_zero_bits > 31 {
                return Err(());
            }
        }
        if leading_zero_bits == 0 {
            return Ok(0);
        }
        let suffix = self.read_bits(leading_zero_bits as u8)?;
        Ok((1u32 << leading_zero_bits) - 1 + suffix)
    }

    fn read_se(&mut self) -> Result<i32, ()> {
        let code_num = self.read_ue()?;
        let magnitude = code_num.div_ceil(2) as i32;
        if code_num & 1 == 0 {
            Ok(-magnitude)
        } else {
            Ok(magnitude)
        }
    }

    fn position_bits(&self) -> usize {
        self.bit_pos
    }
}

const fn align_up_u32(value: u32, align: u32) -> u32 {
    (value + align - 1) & !(align - 1)
}
