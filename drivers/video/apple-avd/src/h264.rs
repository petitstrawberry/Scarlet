use alloc::vec::Vec;

use scarlet::device::video::{
    SCARLET_VIDEO_H264_DECODE_PARAM_FLAG_IDR, SCARLET_VIDEO_H264_DPB_FLAG_LONG_TERM,
    SCARLET_VIDEO_H264_DPB_FLAG_VALID, SCARLET_VIDEO_H264_PPS_FLAG_CONSTRAINED_INTRA_PRED,
    SCARLET_VIDEO_H264_PPS_FLAG_ENTROPY_CODING_MODE,
    SCARLET_VIDEO_H264_PPS_FLAG_TRANSFORM_8X8_MODE, SCARLET_VIDEO_H264_PPS_FLAG_WEIGHTED_PRED,
    SCARLET_VIDEO_H264_SLICE_FLAG_DIRECT_SPATIAL_MV_PRED,
    SCARLET_VIDEO_H264_SLICE_FLAG_REF_LISTS_PRESENT,
    SCARLET_VIDEO_H264_SPS_FLAG_DIRECT_8X8_INFERENCE, SCARLET_VIDEO_H264_SPS_FLAG_FRAME_CROPPING,
    SCARLET_VIDEO_H264_SPS_FLAG_FRAME_MBS_ONLY, SCARLET_VIDEO_PIXEL_FORMAT_NV12,
    ScarletVideoH264DecodeParams, ScarletVideoH264DpbEntry, ScarletVideoH264Pps,
    ScarletVideoH264PredWeights, ScarletVideoH264Reference, ScarletVideoH264SliceParams,
    ScarletVideoH264Sps, ScarletVideoH264StatelessParams,
};

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
    /// Slice payload range does not fit inside the submitted input buffer.
    InvalidSliceRange,
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

    /// Return the AVD RVRA scratch offsets for this frame layout.
    ///
    /// # Returns
    ///
    /// Four RVRA section offsets in bytes.
    pub fn rvra_offsets(&self) -> [u32; 4] {
        let height = align_up_u32(self.height, 32);
        let size0 = self.width * height + (self.width * height) / 4;
        let size1 =
            (self.width.next_power_of_two() * self.height.next_power_of_two() / 32).max(0x100);
        let size2 = size0 / 2;
        [size0, 0, size0 + size1 + size2, size0 + size1]
    }

    /// Return the AVD RVRA scratch size for this frame layout.
    ///
    /// # Returns
    ///
    /// Required RVRA scratch bytes.
    pub fn rvra_len(&self) -> usize {
        let offsets = self.rvra_offsets();
        let size = offsets[2] as usize;
        let aligned = align_up_usize(size, 0x4000);
        aligned
            + if self.width < 1000 {
                0
            } else if self.width < 1800 {
                2 * 0x4000
            } else if self.width < 3800 {
                3 * 0x4000
            } else {
                9 * 0x4000
            }
    }

    /// Return the AVD SPS scratch size for this frame layout.
    ///
    /// # Returns
    ///
    /// Required SPS scratch bytes.
    pub fn sps_scratch_len(&self) -> usize {
        ((((self.width - 1) as usize * (self.height - 1) as usize) / 0x10000) + 2) * 0x4000
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
    /// Build stream parameters from a Scarlet stateless H.264 SPS.
    ///
    /// # Arguments
    ///
    /// * `sps` - Userspace-provided stateless H.264 SPS.
    ///
    /// # Returns
    ///
    /// SPS-derived stream parameters accepted by the AVD frontend.
    pub fn from_stateless_sps(sps: &ScarletVideoH264Sps) -> Result<Self, H264FrontendError> {
        let chroma_format_idc = sps.chroma_format_idc as u32;
        let bit_depth_luma_minus8 = sps.bit_depth_luma_minus8 as u32;
        let bit_depth_chroma_minus8 = sps.bit_depth_chroma_minus8 as u32;
        if chroma_format_idc != 1 || bit_depth_luma_minus8 != 0 || bit_depth_chroma_minus8 != 0 {
            return Err(H264FrontendError::UnsupportedSps);
        }
        if sps.flags & SCARLET_VIDEO_H264_SPS_FLAG_FRAME_MBS_ONLY == 0 {
            return Err(H264FrontendError::UnsupportedSps);
        }

        let coded_width = (sps.pic_width_in_mbs_minus1 as u32 + 1) * 16;
        let coded_height = (sps.pic_height_in_map_units_minus1 as u32 + 1) * 16;
        let (crop_left, crop_right, crop_top, crop_bottom) =
            if sps.flags & SCARLET_VIDEO_H264_SPS_FLAG_FRAME_CROPPING != 0 {
                (
                    sps.frame_crop_left_offset,
                    sps.frame_crop_right_offset,
                    sps.frame_crop_top_offset,
                    sps.frame_crop_bottom_offset,
                )
            } else {
                (0, 0, 0, 0)
            };
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

        Ok(Self {
            sps_id: sps.seq_parameter_set_id as u32,
            profile_idc: sps.profile_idc,
            level_idc: sps.level_idc,
            chroma_format_idc,
            bit_depth_luma_minus8,
            bit_depth_chroma_minus8,
            direct_8x8_inference_flag: sps.flags & SCARLET_VIDEO_H264_SPS_FLAG_DIRECT_8X8_INFERENCE
                != 0,
            width,
            height,
            coded_width,
            coded_height,
            log2_max_frame_num_minus4: sps.log2_max_frame_num_minus4 as u32,
            pic_order_cnt_type: sps.pic_order_cnt_type as u32,
            max_num_ref_frames: sps.max_num_ref_frames as u32,
        })
    }

    /// Build the NV12 output layout used by the Scarlet video ABI.
    ///
    /// # Returns
    ///
    /// NV12 frame layout with AVD-friendly aligned strides.
    pub fn nv12_layout(&self) -> AvdFrameLayout {
        let y_stride = align_up_u32(self.coded_width, 64);
        AvdFrameLayout::nv12(self.coded_width, self.coded_height, y_stride, y_stride)
    }
}

/// H.264 PPS-derived picture parameters needed by the AVD command stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct H264PictureParameters {
    /// CABAC entropy coding mode.
    pub entropy_coding_mode: bool,
    /// 8x8 transform mode.
    pub transform_8x8_mode: bool,
    /// Constrained intra prediction.
    pub constrained_intra_pred: bool,
    /// Weighted bipred IDC.
    pub weighted_bipred_idc: u8,
    /// Weighted prediction for P/SP slices.
    pub weighted_pred: bool,
    /// Initial picture QP minus 26.
    pub pic_init_qp_minus26: i8,
    /// First chroma QP offset.
    pub chroma_qp_index_offset: i8,
    /// Second chroma QP offset.
    pub second_chroma_qp_index_offset: i8,
}

impl H264PictureParameters {
    /// Build picture parameters from a Scarlet stateless H.264 PPS.
    ///
    /// # Arguments
    ///
    /// * `pps` - Userspace-provided stateless H.264 PPS.
    ///
    /// # Returns
    ///
    /// PPS-derived command stream parameters.
    pub fn from_stateless_pps(pps: &ScarletVideoH264Pps) -> Self {
        Self {
            entropy_coding_mode: pps.flags & SCARLET_VIDEO_H264_PPS_FLAG_ENTROPY_CODING_MODE != 0,
            transform_8x8_mode: pps.flags & SCARLET_VIDEO_H264_PPS_FLAG_TRANSFORM_8X8_MODE != 0,
            constrained_intra_pred: pps.flags & SCARLET_VIDEO_H264_PPS_FLAG_CONSTRAINED_INTRA_PRED
                != 0,
            weighted_bipred_idc: pps.weighted_bipred_idc,
            weighted_pred: pps.flags & SCARLET_VIDEO_H264_PPS_FLAG_WEIGHTED_PRED != 0,
            pic_init_qp_minus26: pps.pic_init_qp_minus26,
            chroma_qp_index_offset: pps.chroma_qp_index_offset,
            second_chroma_qp_index_offset: pps.second_chroma_qp_index_offset,
        }
    }

    /// Return conservative defaults used for the old access-unit path.
    ///
    /// # Returns
    ///
    /// Baseline PPS defaults.
    pub const fn baseline_defaults() -> Self {
        Self {
            entropy_coding_mode: false,
            transform_8x8_mode: false,
            constrained_intra_pred: false,
            weighted_bipred_idc: 0,
            weighted_pred: false,
            pic_init_qp_minus26: 0,
            chroma_qp_index_offset: 0,
            second_chroma_qp_index_offset: 0,
        }
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
    /// NAL reference IDC bits from the slice NAL header.
    pub nal_ref_idc: u8,
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
    /// Byte offset of `slice_data()` in the submitted input buffer.
    pub slice_data_offset: usize,
    /// Byte length from `slice_data()` to the end of the slice NAL.
    pub slice_data_len: usize,
    /// Approximate number of bits consumed from the slice RBSP by fields parsed here.
    pub parsed_header_bits: usize,
    /// First macroblock in this slice.
    pub first_mb_in_slice: u32,
    /// CABAC init IDC.
    pub cabac_init_idc: u8,
    /// Slice QP delta.
    pub slice_qp_delta: i8,
    /// Deblocking filter IDC.
    pub disable_deblocking_filter_idc: u8,
    /// Alpha C0 deblocking offset divided by two.
    pub slice_alpha_c0_offset_div2: i8,
    /// Beta deblocking offset divided by two.
    pub slice_beta_offset_div2: i8,
    /// Active L0 reference count minus one.
    pub num_ref_idx_l0_active_minus1: u8,
    /// Active L1 reference count minus one.
    pub num_ref_idx_l1_active_minus1: u8,
    /// Reference picture list 0.
    pub ref_pic_list0: [ScarletVideoH264Reference; 32],
    /// Reference picture list 1.
    pub ref_pic_list1: [ScarletVideoH264Reference; 32],
    /// `SCARLET_VIDEO_H264_SLICE_FLAG_*` bitset.
    pub flags: u32,
}

impl H264SliceParameters {
    /// Build slice parameters from Scarlet stateless H.264 controls.
    ///
    /// # Arguments
    ///
    /// * `slice` - Per-slice stateless parameters.
    /// * `decode` - Per-frame stateless decode parameters.
    ///
    /// # Returns
    ///
    /// Slice metadata accepted by the AVD instruction builder.
    pub fn from_stateless(
        slice: &ScarletVideoH264SliceParams,
        decode: &ScarletVideoH264DecodeParams,
        pps: &ScarletVideoH264Pps,
        input: &[u8],
    ) -> Result<Self, H264FrontendError> {
        if decode.nal_ref_idc > 3 || slice.nal_len == 0 {
            return Err(H264FrontendError::MalformedSlice);
        }
        let kind = match (slice.slice_type as u32) % 5 {
            0 | 3 => H264SliceKind::P,
            1 => H264SliceKind::B,
            2 | 4 => H264SliceKind::I,
            _ => return Err(H264FrontendError::MalformedSlice),
        };
        let nal_unit_type = if decode.flags & SCARLET_VIDEO_H264_DECODE_PARAM_FLAG_IDR != 0 {
            H264NalUnitType::IdrSlice
        } else {
            H264NalUnitType::Slice
        };
        let entropy_coding_mode = pps.flags & SCARLET_VIDEO_H264_PPS_FLAG_ENTROPY_CODING_MODE != 0;
        let (slice_data_offset, slice_data_len) =
            locate_slice_data(input, slice, entropy_coding_mode)?;
        Ok(Self {
            nal_ref_idc: decode.nal_ref_idc as u8,
            nal_unit_type,
            kind,
            slice_type: slice.slice_type as u32,
            pic_parameter_set_id: slice.pic_parameter_set_id as u32,
            nal_offset: slice.nal_offset as usize,
            nal_len: slice.nal_len as usize,
            slice_data_offset,
            slice_data_len,
            parsed_header_bits: slice.header_bit_size as usize,
            first_mb_in_slice: slice.first_mb_in_slice,
            cabac_init_idc: slice.cabac_init_idc,
            slice_qp_delta: slice.slice_qp_delta,
            disable_deblocking_filter_idc: slice.disable_deblocking_filter_idc,
            slice_alpha_c0_offset_div2: slice.slice_alpha_c0_offset_div2,
            slice_beta_offset_div2: slice.slice_beta_offset_div2,
            num_ref_idx_l0_active_minus1: slice.num_ref_idx_l0_active_minus1,
            num_ref_idx_l1_active_minus1: slice.num_ref_idx_l1_active_minus1,
            ref_pic_list0: slice.ref_pic_list0,
            ref_pic_list1: slice.ref_pic_list1,
            flags: slice.flags,
        })
    }

    /// Return whether this slice belongs to an IDR picture.
    ///
    /// # Returns
    ///
    /// `true` for IDR slice NALs.
    pub fn is_idr(&self) -> bool {
        matches!(self.nal_unit_type, H264NalUnitType::IdrSlice)
    }

    /// Return whether this slice should be retained as a reference picture.
    ///
    /// # Returns
    ///
    /// `true` when `nal_ref_idc` is non-zero.
    pub fn is_reference(&self) -> bool {
        self.nal_ref_idc != 0
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
        references: &[AvdH264ReferencePicture],
    ) -> Self {
        let mut words = Vec::new();
        let reference_plan = ReferencePlan::build(request, slice, references);
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
        if request.pps.transform_8x8_mode {
            sps_param |= 1 << 7;
        }
        push(&mut words, sps_param, "hdr_2c_sps_param");

        let mut flags = 0;
        if request.pps.entropy_coding_mode {
            flags |= 1 << 20;
        }
        if !is_idr {
            flags |= 1 << 21;
        }
        if request.pps.constrained_intra_pred {
            flags |= 1 << 19;
        }
        push(&mut words, flags, "hdr_44_flags");
        push(
            &mut words,
            (swrap_i8(request.pps.chroma_qp_index_offset, 32) << 5)
                | swrap_i8(request.pps.second_chroma_qp_index_offset, 32),
            "hdr_48_chroma_qp_index_offset",
        );
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
        push_rvra(
            &mut words,
            workspace.reference_dma_addr,
            workspace.reference_offsets,
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
        if !is_idr {
            stream_refs(&mut words, request, workspace, &reference_plan.table);
        }
        push(&mut words, 0, "cm3_mark_end_section_scl");

        let header_remainder = if request.pps.entropy_coding_mode {
            0
        } else {
            (slice.parsed_header_bits as u32 % 8) << 15
        };
        let slice_addr = request.input.dma_addr + slice.slice_data_offset as u64;
        push(
            &mut words,
            0x2d80_0000 | header_remainder | ((slice_addr >> 32) as u32),
            "slc_a7c_cmd_set_coded_slice",
        );
        push(&mut words, slice_addr as u32, "slc_a84_slice_addr_low");
        push(
            &mut words,
            slice.slice_data_len as u32,
            "slc_a88_slice_payload_size",
        );
        let mb_width = ((coded_width - 1) >> 4) + 1;
        push(
            &mut words,
            0x2c00_0000
                | ((slice.first_mb_in_slice / mb_width) << 12)
                | (slice.first_mb_in_slice % mb_width),
            "cm3_cmd_exec_mb_vp",
        );
        let qp = 26 + request.pps.pic_init_qp_minus26 as i32 + slice.slice_qp_delta as i32;
        push(
            &mut words,
            0x2d90_0000 | (((qp * 0x400) as u32) & 0x1fc00),
            "slc_a70_cmd_quant_param",
        );
        let mut deblock = 0x2da0_0000;
        if slice.disable_deblocking_filter_idc == 0 {
            deblock |= 1 << 17;
        }
        if slice.disable_deblocking_filter_idc != 1 {
            deblock |= 1 << 16;
            deblock |= swrap_i8(slice.slice_beta_offset_div2, 16) << 12;
            deblock |= swrap_i8(slice.slice_alpha_c0_offset_div2, 16) << 8;
        }
        push(&mut words, deblock, "slc_a74_cmd_deblocking_filter");
        if matches!(slice.kind, H264SliceKind::P | H264SliceKind::B) {
            for (index, reference_index) in reference_plan.list0.iter().copied().enumerate() {
                push(
                    &mut words,
                    0x2dc0_0000
                        | (((index as u32) & 0xf) << 4)
                        | (u32::from(reference_index) & 0xf),
                    "slc_6e8_cmd_ref_list_0",
                );
            }
            if matches!(slice.kind, H264SliceKind::B) {
                for (index, reference_index) in reference_plan.list1.iter().copied().enumerate() {
                    push(
                        &mut words,
                        0x2dc0_0000
                            | (1 << 8)
                            | (((index as u32) & 0xf) << 4)
                            | (u32::from(reference_index) & 0xf),
                        "slc_6e8_cmd_ref_list_1",
                    );
                }
            }
            stream_weights(&mut words, request, slice);
        }
        if slice.first_mb_in_slice == 0 {
            push(&mut words, 0x2a00_0000, "cm3_cmd_set_mb_dims");
            push(
                &mut words,
                (((coded_height - 1) >> 4) << 12) | ((coded_width - 1) >> 4),
                "cm3_set_mb_dims",
            );
        }
        let ref_type = match slice.kind {
            H264SliceKind::I => 0x20000,
            H264SliceKind::P => 0x10000,
            H264SliceKind::B => 0x40000,
        };
        let mut ref_type = ref_type;
        if matches!(slice.kind, H264SliceKind::P | H264SliceKind::B) {
            if request.pps.entropy_coding_mode {
                ref_type |= (slice.cabac_init_idc as u32) << 5;
            }
            if matches!(slice.kind, H264SliceKind::B) {
                ref_type |= (slice.num_ref_idx_l1_active_minus1 as u32) << 7;
                if slice.flags & SCARLET_VIDEO_H264_SLICE_FLAG_DIRECT_SPATIAL_MV_PRED == 0 {
                    ref_type |= 1 << 15;
                }
            }
            ref_type |= (slice.num_ref_idx_l0_active_minus1 as u32) << 11;
        }
        push(&mut words, 0x2d00_0000 | ref_type, "slc_6e4_cmd_ref_type");
        if matches!(slice.kind, H264SliceKind::B) {
            let colocated_sps_tile = reference_plan
                .list1
                .first()
                .and_then(|index| reference_plan.table.get(usize::from(*index)))
                .map(|reference| reference.sps_tile_dma_addr)
                .unwrap_or(workspace.sps_tile_dma_addr);
            push(
                &mut words,
                (colocated_sps_tile >> 8) as u32,
                "slc_a78_sps_tile_addr2_lsb8",
            );
        }
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
    /// RVRA section offsets relative to `reference_dma_addr`.
    pub reference_offsets: [u32; 4],
}

/// Previously decoded reference picture visible to the AVD command stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvdH264ReferencePicture {
    /// RVRA scratch base for the reference picture.
    pub reference_dma_addr: u64,
    /// SPS scratch tile base associated with this reference picture.
    pub sps_tile_dma_addr: u64,
    /// H.264 frame number.
    pub frame_num: u16,
    /// H.264 short-term picture number.
    pub pic_num: i32,
    /// Top field order count.
    pub top_field_order_cnt: i32,
    /// Whether this is a long-term reference.
    pub long_term: bool,
}

/// H.264 decode request lowered for the Apple AVD command path.
#[derive(Clone, Copy, Debug)]
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
    /// Current picture order count.
    pub current_poc: i32,
    /// Current H.264 frame number.
    pub frame_num: u16,
    /// Request flags derived from the access unit.
    pub flags: H264DecodeFlags,
    /// PPS-derived picture parameters.
    pub pps: H264PictureParameters,
    /// Userspace-provided decoded picture buffer.
    pub dpb: [ScarletVideoH264DpbEntry; 16],
    /// Explicit H.264 prediction weights.
    pub pred_weights: ScarletVideoH264PredWeights,
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
            current_poc: 0,
            frame_num: 0,
            flags,
            pps: H264PictureParameters::baseline_defaults(),
            dpb: [ScarletVideoH264DpbEntry::default(); 16],
            pred_weights: ScarletVideoH264PredWeights::default(),
            slice,
        })
    }

    /// Build a decode request from stateless H.264 controls and DMA buffers.
    ///
    /// # Arguments
    ///
    /// * `session_id` - Video session identifier.
    /// * `frame_number` - Driver-local frame number.
    /// * `params` - Userspace-provided stateless H.264 parameters.
    /// * `input` - Device-visible input range.
    /// * `output` - Device-visible output range.
    /// * `layout` - Expected decoded frame layout.
    ///
    /// # Returns
    ///
    /// H.264 decode request ready for firmware command lowering.
    pub fn from_stateless(
        session_id: u64,
        frame_number: u32,
        params: &ScarletVideoH264StatelessParams,
        input: AvdDmaRange,
        input_bytes: &[u8],
        output: AvdDmaRange,
        layout: AvdFrameLayout,
    ) -> Result<Self, H264FrontendError> {
        if layout.width == 0 || layout.height == 0 {
            return Err(H264FrontendError::InvalidDimensions);
        }

        let pps = H264PictureParameters::from_stateless_pps(&params.pps);
        let slice = H264SliceParameters::from_stateless(
            &params.slice_params,
            &params.decode_params,
            &params.pps,
            input_bytes,
        )?;
        let mut flags = H264DecodeFlags::empty();
        flags.insert(H264DecodeFlags::PARAMETER_SETS);
        if params.decode_params.flags & SCARLET_VIDEO_H264_DECODE_PARAM_FLAG_IDR != 0 {
            flags.insert(H264DecodeFlags::IDR);
        }

        Ok(Self {
            session_id,
            frame_number,
            input,
            output,
            layout,
            current_poc: params.decode_params.top_field_order_cnt,
            frame_num: params.decode_params.frame_num,
            flags,
            pps,
            dpb: params.decode_params.dpb,
            pred_weights: params.pred_weights,
            slice,
        })
    }
}

struct ReferencePlan {
    table: Vec<AvdH264ReferencePicture>,
    list0: Vec<u8>,
    list1: Vec<u8>,
}

impl ReferencePlan {
    fn build(
        request: &H264DecodeRequest,
        slice: &H264SliceParameters,
        references: &[AvdH264ReferencePicture],
    ) -> Self {
        if slice.flags & SCARLET_VIDEO_H264_SLICE_FLAG_REF_LISTS_PRESENT != 0 {
            let list0_original = resolve_explicit_reference_list(
                &slice.ref_pic_list0,
                usize::from(slice.num_ref_idx_l0_active_minus1) + 1,
                &request.dpb,
                references,
            );
            let list1_original = if matches!(slice.kind, H264SliceKind::B) {
                resolve_explicit_reference_list(
                    &slice.ref_pic_list1,
                    usize::from(slice.num_ref_idx_l1_active_minus1) + 1,
                    &request.dpb,
                    references,
                )
            } else {
                Vec::new()
            };
            return Self::from_original_lists(list0_original, list1_original, references);
        }

        let mut list0_original = Vec::new();
        let mut list1_original = Vec::new();

        match slice.kind {
            H264SliceKind::I => {}
            H264SliceKind::P => {
                let needed = (usize::from(slice.num_ref_idx_l0_active_minus1) + 1).min(16);
                list0_original.extend(0..references.len().min(needed));
            }
            H264SliceKind::B => {
                let mut before = Vec::new();
                let mut after = Vec::new();
                for (index, reference) in references.iter().enumerate() {
                    if reference.top_field_order_cnt < request.current_poc {
                        before.push(index);
                    } else {
                        after.push(index);
                    }
                }
                before.sort_by(|left, right| {
                    references[*right]
                        .top_field_order_cnt
                        .cmp(&references[*left].top_field_order_cnt)
                });
                after.sort_by(|left, right| {
                    references[*left]
                        .top_field_order_cnt
                        .cmp(&references[*right].top_field_order_cnt)
                });

                list0_original.extend(before.iter().copied());
                list0_original.extend(after.iter().copied());
                list1_original.extend(after.iter().copied());
                list1_original.extend(before.iter().copied());
                if list0_original == list1_original && list1_original.len() > 1 {
                    list1_original.swap(0, 1);
                }

                let l0_needed = (usize::from(slice.num_ref_idx_l0_active_minus1) + 1).min(16);
                let l1_needed = (usize::from(slice.num_ref_idx_l1_active_minus1) + 1).min(16);
                list0_original.truncate(l0_needed);
                list1_original.truncate(l1_needed);
            }
        }

        Self::from_original_lists(list0_original, list1_original, references)
    }

    fn from_original_lists(
        list0_original: Vec<usize>,
        list1_original: Vec<usize>,
        references: &[AvdH264ReferencePicture],
    ) -> Self {
        let mut table_indices = Vec::new();
        for index in list0_original.iter().chain(list1_original.iter()).copied() {
            if !table_indices.contains(&index) && table_indices.len() < 16 {
                table_indices.push(index);
            }
        }

        let mut table = Vec::new();
        for index in table_indices.iter().copied() {
            if let Some(reference) = references.get(index) {
                table.push(*reference);
            }
        }

        let list0 = remap_reference_list(&list0_original, &table_indices);
        let list1 = remap_reference_list(&list1_original, &table_indices);
        Self {
            table,
            list0,
            list1,
        }
    }
}

fn resolve_explicit_reference_list(
    list: &[ScarletVideoH264Reference; 32],
    count: usize,
    dpb: &[ScarletVideoH264DpbEntry; 16],
    references: &[AvdH264ReferencePicture],
) -> Vec<usize> {
    let mut resolved = Vec::new();
    for entry in list.iter().take(count.min(32)) {
        let dpb_index = usize::from(entry.index);
        let Some(dpb_entry) = dpb.get(dpb_index) else {
            continue;
        };
        if dpb_entry.flags & SCARLET_VIDEO_H264_DPB_FLAG_VALID == 0 {
            continue;
        }
        if let Some(reference_index) = references.iter().position(|reference| {
            reference.frame_num == dpb_entry.frame_num
                && reference.top_field_order_cnt == dpb_entry.top_field_order_cnt
                && reference.long_term
                    == (dpb_entry.flags & SCARLET_VIDEO_H264_DPB_FLAG_LONG_TERM != 0)
        }) {
            resolved.push(reference_index);
        } else if let Some(reference_index) = references.iter().position(|reference| {
            reference.top_field_order_cnt == dpb_entry.top_field_order_cnt
                && reference.pic_num == dpb_entry.pic_num
        }) {
            resolved.push(reference_index);
        }
    }
    resolved
}

fn remap_reference_list(original: &[usize], table_indices: &[usize]) -> Vec<u8> {
    let mut remapped = Vec::new();
    for index in original {
        if let Some(table_index) = table_indices
            .iter()
            .position(|candidate| candidate == index)
        {
            remapped.push(table_index as u8);
        }
    }
    remapped
}

fn stream_refs(
    words: &mut Vec<u32>,
    request: &H264DecodeRequest,
    workspace: &AvdH264Workspace,
    references: &[AvdH264ReferencePicture],
) {
    push(words, 0x0402_0002, "cm3_dma_config_6");
    push(
        words,
        ((workspace.pps_tile_dma_addr + 0x20000) >> 8) as u32,
        "hdr_9c_pps_tile_addr_lsb8",
    );
    push(
        words,
        (workspace.sps_tile_dma_addr >> 8) as u32,
        "hdr_bc_sps_tile_addr_lsb8",
    );
    push(words, 0x0007_0007, "cm3_dma_config_7");
    push(words, 0x0007_0007, "cm3_dma_config_8");
    push(words, 0x0007_0007, "cm3_dma_config_9");
    push(words, 0x0007_0007, "cm3_dma_config_a");

    let count = references.len().min(16);
    if count == 0 {
        return;
    }
    for reference in references.iter().take(count) {
        let poc_delta = request
            .current_poc
            .wrapping_sub(reference.top_field_order_cnt);
        push(
            words,
            (((count as u32 - 1) & 0xf) << 28)
                | 0x0100_0000
                | ((reference.long_term as u32) << 17)
                | swrap_i32(poc_delta, 1 << 17),
            "hdr_d0_ref_hdr",
        );
        push_rvra(
            words,
            reference.reference_dma_addr,
            workspace.reference_offsets,
            "hdr_c0_ref_addr_lsb7",
        );
    }
}

fn stream_weights(words: &mut Vec<u32>, request: &H264DecodeRequest, slice: &H264SliceParameters) {
    let pred_weight = (request.pps.weighted_pred && matches!(slice.kind, H264SliceKind::P))
        || (request.pps.weighted_bipred_idc == 1 && matches!(slice.kind, H264SliceKind::B));
    let mut denom = 0;
    if request.pps.weighted_bipred_idc == 2 {
        denom |= 0x5 | (0x5 << 3);
    } else {
        denom |= (u32::from(request.pred_weights.luma_log2_weight_denom) & 0x7) << 3;
        denom |= u32::from(request.pred_weights.chroma_log2_weight_denom) & 0x7;
    }
    push(
        words,
        0x2dd0_0000
            | (((request.pps.weighted_bipred_idc == 2) as u32) << 7)
            | ((pred_weight as u32) << 6)
            | denom,
        "slc_76c_cmd_weights_denom",
    );
    if !pred_weight {
        return;
    }

    let default_luma_weight = 1i16
        .checked_shl(u32::from(request.pred_weights.luma_log2_weight_denom))
        .unwrap_or(0);
    let default_chroma_weight = 1i16
        .checked_shl(u32::from(request.pred_weights.chroma_log2_weight_denom))
        .unwrap_or(0);
    let list_count = if matches!(slice.kind, H264SliceKind::B) {
        2
    } else {
        1
    };
    for list_index in 0..list_count {
        let active = if list_index == 0 {
            slice.num_ref_idx_l0_active_minus1
        } else {
            slice.num_ref_idx_l1_active_minus1
        };
        let factors = &request.pred_weights.weight_factors[list_index];
        for index in 0..=usize::from(active) {
            let list_bit = (list_index as u32) << 13;
            let ref_bits = (index as u32) << 9;
            if factors.luma_weight[index] != default_luma_weight || factors.luma_offset[index] != 0
            {
                push(
                    words,
                    0x2de0_0000
                        | (1 << 14)
                        | list_bit
                        | ref_bits
                        | (u32::from(factors.luma_weight[index] as u16) & 0x1ff),
                    "slc_luma_weights",
                );
                push(
                    words,
                    0x2df0_0000 | swrap_i32(i32::from(factors.luma_offset[index]), 0x10000),
                    "slc_luma_offsets",
                );
            }

            if factors.chroma_weight[index][0] != default_chroma_weight
                || factors.chroma_offset[index][0] != 0
                || factors.chroma_weight[index][1] != default_chroma_weight
                || factors.chroma_offset[index][1] != 0
            {
                push(
                    words,
                    0x2de0_0000
                        | (2 << 14)
                        | list_bit
                        | ref_bits
                        | (u32::from(factors.chroma_weight[index][0] as u16) & 0x1ff),
                    "slc_chroma_weights_0",
                );
                push(
                    words,
                    0x2df0_0000 | swrap_i32(i32::from(factors.chroma_offset[index][0]), 0x10000),
                    "slc_chroma_offsets_0",
                );
                push(
                    words,
                    0x2de0_0000
                        | (3 << 14)
                        | list_bit
                        | ref_bits
                        | (u32::from(factors.chroma_weight[index][1] as u16) & 0x1ff),
                    "slc_chroma_weights_1",
                );
                push(
                    words,
                    0x2df0_0000 | swrap_i32(i32::from(factors.chroma_offset[index][1]), 0x10000),
                    "slc_chroma_offsets_1",
                );
            }
        }
    }
}

fn push_rvra(words: &mut Vec<u32>, base: u64, offsets: [u32; 4], _name: &'static str) {
    for offset in offsets {
        push(words, ((base + offset as u64) >> 7) as u32, _name);
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
        nal_ref_idc: unit.nal_ref_idc,
        nal_unit_type: unit.unit_type,
        kind,
        slice_type,
        pic_parameter_set_id,
        nal_offset: unit.offset,
        nal_len: unit.payload.len(),
        slice_data_offset: unit.offset + 1,
        slice_data_len: unit.payload.len().saturating_sub(1),
        parsed_header_bits: reader.position_bits(),
        first_mb_in_slice: 0,
        cabac_init_idc: 0,
        slice_qp_delta: 0,
        disable_deblocking_filter_idc: 0,
        slice_alpha_c0_offset_div2: 0,
        slice_beta_offset_div2: 0,
        num_ref_idx_l0_active_minus1: 0,
        num_ref_idx_l1_active_minus1: 0,
        ref_pic_list0: [ScarletVideoH264Reference::default(); 32],
        ref_pic_list1: [ScarletVideoH264Reference::default(); 32],
        flags: 0,
    })
}

fn locate_slice_data(
    input: &[u8],
    slice: &ScarletVideoH264SliceParams,
    entropy_coding_mode: bool,
) -> Result<(usize, usize), H264FrontendError> {
    let nal_start = slice.nal_offset as usize;
    let nal_len = slice.nal_len as usize;
    let nal_end = nal_start
        .checked_add(nal_len)
        .ok_or(H264FrontendError::InvalidSliceRange)?;
    if nal_len == 0 || nal_end > input.len() {
        return Err(H264FrontendError::InvalidSliceRange);
    }

    let mut offset = nal_start + 1;
    let rbsp_header_bytes = if entropy_coding_mode {
        (slice.header_bit_size as usize + 7) / 8
    } else {
        slice.header_bit_size as usize / 8
    };
    let mut rbsp_read = 0usize;
    let mut zero_count = 0u8;

    while rbsp_read < rbsp_header_bytes {
        let byte = *input
            .get(offset)
            .ok_or(H264FrontendError::InvalidSliceRange)?;
        offset += 1;
        if zero_count >= 2 && byte == 0x03 {
            zero_count = 0;
            continue;
        }
        rbsp_read += 1;
        if byte == 0 {
            zero_count = zero_count.saturating_add(1);
        } else {
            zero_count = 0;
        }
    }

    if offset > nal_end {
        return Err(H264FrontendError::InvalidSliceRange);
    }
    Ok((offset, nal_end - offset))
}

fn swrap_i8(value: i8, width: u32) -> u32 {
    (value as i32 as u32) & (width - 1)
}

fn swrap_i32(value: i32, width: u32) -> u32 {
    (value as u32) & (width - 1)
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

const fn align_up_usize(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}
