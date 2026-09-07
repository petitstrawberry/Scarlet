//! H.264/AVC stateless request construction.

use alloc::format;
use alloc::string::String;

pub const SCARLET_VIDEO_H264_SPS_FLAG_SEPARATE_COLOUR_PLANE: u32 = 1 << 0;
pub const SCARLET_VIDEO_H264_SPS_FLAG_QPPRIME_Y_ZERO_TRANSFORM_BYPASS: u32 = 1 << 1;
pub const SCARLET_VIDEO_H264_SPS_FLAG_DELTA_PIC_ORDER_ALWAYS_ZERO: u32 = 1 << 2;
pub const SCARLET_VIDEO_H264_SPS_FLAG_GAPS_IN_FRAME_NUM_VALUE_ALLOWED: u32 = 1 << 3;
pub const SCARLET_VIDEO_H264_SPS_FLAG_FRAME_MBS_ONLY: u32 = 1 << 4;
pub const SCARLET_VIDEO_H264_SPS_FLAG_MB_ADAPTIVE_FRAME_FIELD: u32 = 1 << 5;
pub const SCARLET_VIDEO_H264_SPS_FLAG_DIRECT_8X8_INFERENCE: u32 = 1 << 6;
pub const SCARLET_VIDEO_H264_SPS_FLAG_FRAME_CROPPING: u32 = 1 << 7;
pub const SCARLET_VIDEO_H264_PPS_FLAG_ENTROPY_CODING_MODE: u16 = 1 << 0;
pub const SCARLET_VIDEO_H264_PPS_FLAG_BOTTOM_FIELD_PIC_ORDER_IN_FRAME_PRESENT: u16 = 1 << 1;
pub const SCARLET_VIDEO_H264_PPS_FLAG_WEIGHTED_PRED: u16 = 1 << 2;
pub const SCARLET_VIDEO_H264_PPS_FLAG_DEBLOCKING_FILTER_CONTROL_PRESENT: u16 = 1 << 3;
pub const SCARLET_VIDEO_H264_PPS_FLAG_CONSTRAINED_INTRA_PRED: u16 = 1 << 4;
pub const SCARLET_VIDEO_H264_PPS_FLAG_REDUNDANT_PIC_CNT_PRESENT: u16 = 1 << 5;
pub const SCARLET_VIDEO_H264_PPS_FLAG_TRANSFORM_8X8_MODE: u16 = 1 << 6;
pub const SCARLET_VIDEO_H264_SLICE_FLAG_DIRECT_SPATIAL_MV_PRED: u32 = 1 << 0;
pub const SCARLET_VIDEO_H264_SLICE_FLAG_REF_LISTS_PRESENT: u32 = 1 << 1;
pub const SCARLET_VIDEO_H264_DECODE_PARAM_FLAG_IDR: u32 = 1 << 0;
pub const SCARLET_VIDEO_H264_DPB_FLAG_VALID: u32 = 1 << 0;
pub const SCARLET_VIDEO_H264_DPB_FLAG_LONG_TERM: u32 = 1 << 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScarletVideoH264Sps {
    pub profile_idc: u8,
    pub constraint_set_flags: u8,
    pub level_idc: u8,
    pub seq_parameter_set_id: u8,
    pub chroma_format_idc: u8,
    pub bit_depth_luma_minus8: u8,
    pub bit_depth_chroma_minus8: u8,
    pub log2_max_frame_num_minus4: u8,
    pub pic_order_cnt_type: u8,
    pub log2_max_pic_order_cnt_lsb_minus4: u8,
    pub max_num_ref_frames: u8,
    pub num_ref_frames_in_pic_order_cnt_cycle: u8,
    pub offset_for_ref_frame: [i32; 255],
    pub offset_for_non_ref_pic: i32,
    pub offset_for_top_to_bottom_field: i32,
    pub pic_width_in_mbs_minus1: u16,
    pub pic_height_in_map_units_minus1: u16,
    pub frame_crop_left_offset: u32,
    pub frame_crop_right_offset: u32,
    pub frame_crop_top_offset: u32,
    pub frame_crop_bottom_offset: u32,
    pub flags: u32,
}

impl Default for ScarletVideoH264Sps {
    fn default() -> Self {
        Self {
            profile_idc: 0,
            constraint_set_flags: 0,
            level_idc: 0,
            seq_parameter_set_id: 0,
            chroma_format_idc: 0,
            bit_depth_luma_minus8: 0,
            bit_depth_chroma_minus8: 0,
            log2_max_frame_num_minus4: 0,
            pic_order_cnt_type: 0,
            log2_max_pic_order_cnt_lsb_minus4: 0,
            max_num_ref_frames: 0,
            num_ref_frames_in_pic_order_cnt_cycle: 0,
            offset_for_ref_frame: [0; 255],
            offset_for_non_ref_pic: 0,
            offset_for_top_to_bottom_field: 0,
            pic_width_in_mbs_minus1: 0,
            pic_height_in_map_units_minus1: 0,
            frame_crop_left_offset: 0,
            frame_crop_right_offset: 0,
            frame_crop_top_offset: 0,
            frame_crop_bottom_offset: 0,
            flags: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ScarletVideoH264Pps {
    pub pic_parameter_set_id: u8,
    pub seq_parameter_set_id: u8,
    pub num_slice_groups_minus1: u8,
    pub num_ref_idx_l0_default_active_minus1: u8,
    pub num_ref_idx_l1_default_active_minus1: u8,
    pub weighted_bipred_idc: u8,
    pub pic_init_qp_minus26: i8,
    pub pic_init_qs_minus26: i8,
    pub chroma_qp_index_offset: i8,
    pub second_chroma_qp_index_offset: i8,
    pub flags: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScarletVideoH264ScalingMatrix {
    pub scaling_list_4x4: [[u8; 16]; 6],
    pub scaling_list_8x8: [[u8; 64]; 6],
}

impl Default for ScarletVideoH264ScalingMatrix {
    fn default() -> Self {
        Self {
            scaling_list_4x4: [[0; 16]; 6],
            scaling_list_8x8: [[0; 64]; 6],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ScarletVideoH264Reference {
    pub fields: u8,
    pub index: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ScarletVideoH264WeightFactors {
    pub luma_weight: [i16; 32],
    pub luma_offset: [i16; 32],
    pub chroma_weight: [[i16; 2]; 32],
    pub chroma_offset: [[i16; 2]; 32],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ScarletVideoH264PredWeights {
    pub luma_log2_weight_denom: u16,
    pub chroma_log2_weight_denom: u16,
    pub weight_factors: [ScarletVideoH264WeightFactors; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScarletVideoH264SliceParams {
    pub header_bit_size: u32,
    pub nal_offset: u32,
    pub nal_len: u32,
    pub first_mb_in_slice: u32,
    pub slice_type: u8,
    pub pic_parameter_set_id: u8,
    pub colour_plane_id: u8,
    pub redundant_pic_cnt: u8,
    pub cabac_init_idc: u8,
    pub slice_qp_delta: i8,
    pub slice_qs_delta: i8,
    pub disable_deblocking_filter_idc: u8,
    pub slice_alpha_c0_offset_div2: i8,
    pub slice_beta_offset_div2: i8,
    pub num_ref_idx_l0_active_minus1: u8,
    pub num_ref_idx_l1_active_minus1: u8,
    pub reserved: u8,
    pub ref_pic_list0: [ScarletVideoH264Reference; 32],
    pub ref_pic_list1: [ScarletVideoH264Reference; 32],
    pub flags: u32,
}

impl Default for ScarletVideoH264SliceParams {
    fn default() -> Self {
        Self {
            header_bit_size: 0,
            nal_offset: 0,
            nal_len: 0,
            first_mb_in_slice: 0,
            slice_type: 0,
            pic_parameter_set_id: 0,
            colour_plane_id: 0,
            redundant_pic_cnt: 0,
            cabac_init_idc: 0,
            slice_qp_delta: 0,
            slice_qs_delta: 0,
            disable_deblocking_filter_idc: 0,
            slice_alpha_c0_offset_div2: 0,
            slice_beta_offset_div2: 0,
            num_ref_idx_l0_active_minus1: 0,
            num_ref_idx_l1_active_minus1: 0,
            reserved: 0,
            ref_pic_list0: [ScarletVideoH264Reference::default(); 32],
            ref_pic_list1: [ScarletVideoH264Reference::default(); 32],
            flags: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ScarletVideoH264DpbEntry {
    pub reference_ts: u64,
    pub pic_num: i32,
    pub frame_num: u16,
    pub fields: u8,
    pub reserved: [u8; 5],
    pub top_field_order_cnt: i32,
    pub bottom_field_order_cnt: i32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ScarletVideoH264DecodeParams {
    pub dpb: [ScarletVideoH264DpbEntry; 16],
    pub nal_ref_idc: u16,
    pub frame_num: u16,
    pub top_field_order_cnt: i32,
    pub bottom_field_order_cnt: i32,
    pub idr_pic_id: u16,
    pub pic_order_cnt_lsb: u16,
    pub delta_pic_order_cnt_bottom: i32,
    pub delta_pic_order_cnt0: i32,
    pub delta_pic_order_cnt1: i32,
    pub dec_ref_pic_marking_bit_size: u32,
    pub pic_order_cnt_bit_size: u32,
    pub slice_group_change_cycle: u32,
    pub reserved: u32,
    pub flags: u32,
}

#[derive(Clone, Copy, Default)]
pub struct ScarletVideoH264StatelessParams {
    pub sps: ScarletVideoH264Sps,
    pub pps: ScarletVideoH264Pps,
    pub scaling_matrix: ScarletVideoH264ScalingMatrix,
    pub pred_weights: ScarletVideoH264PredWeights,
    pub slice_params: ScarletVideoH264SliceParams,
    pub decode_params: ScarletVideoH264DecodeParams,
}

struct RawNalUnit<'a> {
    nal_type: u8,
    nal_ref_idc: u8,
    offset: usize,
    bytes: &'a [u8],
}

fn for_each_raw_annex_b<'a, E, F>(data: &'a [u8], mut visit: F) -> Result<(), E>
where
    F: FnMut(RawNalUnit<'a>) -> Result<(), E>,
{
    let Some((mut nal_start, _)) = find_start_code(data, 0) else {
        return Ok(());
    };

    loop {
        if nal_start >= data.len() {
            break;
        }

        let (mut nal_end, next_start) = match find_start_code(data, nal_start) {
            Some((next_nal_start, next_code_start)) => (next_code_start, Some(next_nal_start)),
            None => (data.len(), None),
        };
        while nal_end > nal_start && data[nal_end - 1] == 0 {
            nal_end -= 1;
        }

        if nal_start < nal_end {
            let header = data[nal_start];
            if header & 0x80 == 0 {
                visit(RawNalUnit {
                    nal_type: header & 0x1f,
                    nal_ref_idc: (header >> 5) & 0x3,
                    offset: nal_start,
                    bytes: &data[nal_start..nal_end],
                })?;
            }
        }

        let Some(next_start) = next_start else {
            break;
        };
        nal_start = next_start;
    }

    Ok(())
}

fn find_start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut index = from;
    while index + 3 <= data.len() {
        if index + 3 <= data.len()
            && data[index] == 0
            && data[index + 1] == 0
            && data[index + 2] == 1
        {
            return Some((index + 3, index));
        }
        if index + 4 <= data.len()
            && data[index] == 0
            && data[index + 1] == 0
            && data[index + 2] == 0
            && data[index + 3] == 1
        {
            return Some((index + 4, index));
        }
        index += 1;
    }
    None
}

#[derive(Clone)]
struct EbspBitReader<'a> {
    bytes: &'a [u8],
    byte_index: usize,
    bit_index: u8,
    zero_count: u8,
}

impl<'a> EbspBitReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            byte_index: 0,
            bit_index: 0,
            zero_count: 0,
        }
    }

    fn read_bit(&mut self) -> Option<u8> {
        loop {
            let byte = *self.bytes.get(self.byte_index)?;
            if self.zero_count >= 2 && byte == 0x03 {
                self.byte_index += 1;
                self.bit_index = 0;
                self.zero_count = 0;
                continue;
            }

            let bit = (byte >> (7 - self.bit_index)) & 1;
            self.bit_index += 1;
            if self.bit_index == 8 {
                self.byte_index += 1;
                self.bit_index = 0;
                if byte == 0 {
                    self.zero_count = self.zero_count.saturating_add(1);
                } else {
                    self.zero_count = 0;
                }
            }
            return Some(bit);
        }
    }

    fn read_ue(&mut self) -> Option<u32> {
        let mut leading_zero_bits = 0u32;
        while self.read_bit()? == 0 {
            leading_zero_bits += 1;
            if leading_zero_bits >= 32 {
                return None;
            }
        }

        let mut value = 1u32;
        for _ in 0..leading_zero_bits {
            value = (value << 1) | u32::from(self.read_bit()?);
        }
        Some(value - 1)
    }

    fn read_bits(&mut self, bits: u8) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..bits {
            value = (value << 1) | u32::from(self.read_bit()?);
        }
        Some(value)
    }

    fn read_se(&mut self) -> Option<i32> {
        let code_num = self.read_ue()?;
        let magnitude = code_num.div_ceil(2) as i32;
        if code_num & 1 == 0 {
            Some(-magnitude)
        } else {
            Some(magnitude)
        }
    }

    fn position_bits(&self) -> usize {
        self.byte_index * 8 + usize::from(self.bit_index)
    }
}

#[derive(Default)]
pub struct H264RequestContext {
    // The kernel ABI is stateless per request; userspace still keeps the
    // stream context needed to build those requests.
    sps: Option<ScarletVideoH264Sps>,
    pps: Option<ScarletVideoH264Pps>,
    scaling_matrix: ScarletVideoH264ScalingMatrix,
    pred_weights: ScarletVideoH264PredWeights,
    dpb: FixedList<H264DpbFrame, H264_MAX_DPB_FRAMES>,
    poc: H264PocState,
    next_timestamp: u64,
}

pub struct H264PreparedAccessUnit {
    pub params: ScarletVideoH264StatelessParams,
    pub timestamp: u64,
}

const H264_MAX_DPB_FRAMES: usize = 16;
const H264_MAX_REF_LIST_ENTRIES: usize = 32;
const H264_MAX_REF_LIST_MODIFICATIONS: usize = 32;
const H264_MAX_MMCO_OPERATIONS: usize = 32;

#[derive(Clone, Copy)]
struct FixedList<T: Copy + Default, const N: usize> {
    items: [T; N],
    len: usize,
}

impl<T: Copy + Default, const N: usize> FixedList<T, N> {
    fn new() -> Self {
        Self {
            items: [T::default(); N],
            len: 0,
        }
    }

    fn as_slice(&self) -> &[T] {
        &self.items[..self.len]
    }

    fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.items[..self.len]
    }

    fn iter(&self) -> core::slice::Iter<'_, T> {
        self.as_slice().iter()
    }

    fn len(&self) -> usize {
        self.len
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn clear(&mut self) {
        self.items[..self.len].fill(T::default());
        self.len = 0;
    }

    fn push(&mut self, value: T) -> Result<(), String> {
        if self.len >= N {
            return Err(String::from("fixed H.264 list capacity exceeded"));
        }
        self.items[self.len] = value;
        self.len += 1;
        Ok(())
    }

    fn insert(&mut self, index: usize, value: T) -> Result<(), String> {
        if self.len >= N {
            return Err(String::from("fixed H.264 list capacity exceeded"));
        }
        let index = index.min(self.len);
        for cursor in (index..self.len).rev() {
            self.items[cursor + 1] = self.items[cursor];
        }
        self.items[index] = value;
        self.len += 1;
        Ok(())
    }

    fn remove(&mut self, index: usize) -> Option<T> {
        if index >= self.len {
            return None;
        }
        let removed = self.items[index];
        for cursor in (index + 1)..self.len {
            self.items[cursor - 1] = self.items[cursor];
        }
        self.len -= 1;
        self.items[self.len] = T::default();
        Some(removed)
    }

    fn retain<F>(&mut self, mut keep: F)
    where
        F: FnMut(&T) -> bool,
    {
        let mut write = 0;
        for read in 0..self.len {
            let item = self.items[read];
            if keep(&item) {
                self.items[write] = item;
                write += 1;
            }
        }
        self.items[write..self.len].fill(T::default());
        self.len = write;
    }

    fn truncate(&mut self, len: usize) {
        let len = len.min(self.len);
        self.items[len..self.len].fill(T::default());
        self.len = len;
    }

    fn last(&self) -> Option<T> {
        (self.len != 0).then_some(self.items[self.len - 1])
    }

    fn swap(&mut self, left: usize, right: usize) {
        if left < self.len && right < self.len {
            self.items.swap(left, right);
        }
    }
}

impl<T: Copy + Default, const N: usize> Default for FixedList<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy + Default + PartialEq, const N: usize> PartialEq for FixedList<T, N> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

#[derive(Clone, Copy, Default)]
struct H264DpbFrame {
    reference_ts: u64,
    pic_num: i32,
    frame_num: u16,
    top_field_order_cnt: i32,
    bottom_field_order_cnt: i32,
    long_term: bool,
}

#[derive(Clone, Copy, Default)]
struct H264PocState {
    prev_pic_order_cnt_msb: i32,
    prev_pic_order_cnt_lsb: u16,
}

#[derive(Clone, Copy, Default)]
enum H264RefListModification {
    #[default]
    Unused,
    ShortTermSubtract(u32),
    ShortTermAdd(u32),
    LongTerm(u32),
}

#[derive(Default)]
struct H264RefPicMarking {
    idr_long_term: bool,
    adaptive: bool,
    operations: FixedList<H264MemoryManagementControl, H264_MAX_MMCO_OPERATIONS>,
}

impl H264RefPicMarking {
    fn resets_poc(&self) -> bool {
        self.operations
            .iter()
            .any(|operation| matches!(operation, H264MemoryManagementControl::Reset))
    }
}

#[derive(Clone, Copy, Default)]
enum H264MemoryManagementControl {
    #[default]
    Unused,
    ShortTermUnused {
        difference_of_pic_nums_minus1: u32,
    },
    LongTermUnused {
        long_term_pic_num: u32,
    },
    ShortTermToLongTerm {
        difference_of_pic_nums_minus1: u32,
        long_term_frame_idx: u32,
    },
    MaxLongTermFrameIdx {
        max_long_term_frame_idx_plus1: u32,
    },
    Reset,
    CurrentToLongTerm {
        long_term_frame_idx: u32,
    },
}

impl H264RequestContext {
    /// Reset decode-order state after a stream discontinuity while retaining
    /// parameter sets and the monotonically increasing request timestamp.
    ///
    /// A seek resumes at a random-access picture, so references, picture order
    /// state, and per-slice prediction weights from the old position must not
    /// leak into the new decode pass. SPS/PPS data is deliberately preserved:
    /// raw Annex-B streams are not required to repeat parameter sets at every
    /// IDR picture.
    pub fn reset_decode_state(&mut self) {
        self.pred_weights = ScarletVideoH264PredWeights::default();
        self.dpb.clear();
        self.poc = H264PocState::default();
    }

    /// Build a stateless request using an automatically assigned reference
    /// timestamp.
    ///
    /// # Arguments
    ///
    /// * `access_unit` - Complete H.264 Annex B access unit.
    ///
    /// # Returns
    ///
    /// Parsed request parameters and the nonzero timestamp used to identify
    /// the decoded reference frame.
    pub fn params_for_access_unit(
        &mut self,
        access_unit: &[u8],
    ) -> Result<H264PreparedAccessUnit, String> {
        self.params_for_access_unit_inner(access_unit, None)
    }

    /// Build a stateless request with a caller-provided reference timestamp.
    ///
    /// A nonzero timestamp is preserved in both the returned request and the
    /// decoder-picture-buffer references constructed for later access units.
    /// It must remain unique while the decoded picture can be referenced.
    /// Zero retains the automatic timestamp behavior because the Scarlet video
    /// ABI reserves zero for driver-side timestamp assignment.
    ///
    /// # Arguments
    ///
    /// * `access_unit` - Complete H.264 Annex B access unit.
    /// * `timestamp` - Caller timestamp, or zero to allocate one internally.
    ///
    /// # Returns
    ///
    /// Parsed request parameters and the actual nonzero reference timestamp.
    pub fn params_for_access_unit_with_timestamp(
        &mut self,
        access_unit: &[u8],
        timestamp: u64,
    ) -> Result<H264PreparedAccessUnit, String> {
        self.params_for_access_unit_inner(access_unit, Some(timestamp))
    }

    fn params_for_access_unit_inner(
        &mut self,
        access_unit: &[u8],
        requested_timestamp: Option<u64>,
    ) -> Result<H264PreparedAccessUnit, String> {
        let mut slice = None;
        let mut decode = None;
        let mut ref_pic_marking = None;

        for_each_raw_annex_b(access_unit, |nal| -> Result<(), String> {
            match nal.nal_type {
                7 => {
                    self.sps = Some(parse_h264_sps(nal.bytes)?);
                }
                8 => {
                    self.pps = Some(parse_h264_pps(nal.bytes)?);
                }
                1 | 5 if slice.is_none() => {
                    let sps = self
                        .sps
                        .ok_or_else(|| String::from("H.264 stateless submit missing SPS"))?;
                    let pps = self
                        .pps
                        .ok_or_else(|| String::from("H.264 stateless submit missing PPS"))?;
                    let (slice_params, decode_params, marking) = parse_h264_slice(
                        &nal,
                        &sps,
                        &pps,
                        self.dpb.as_slice(),
                        &mut self.pred_weights,
                        &mut self.poc,
                    )?;
                    slice = Some(slice_params);
                    decode = Some(decode_params);
                    ref_pic_marking = Some(marking);
                }
                _ => {}
            }
            Ok(())
        })?;

        let params = ScarletVideoH264StatelessParams {
            sps: self
                .sps
                .ok_or_else(|| String::from("H.264 stateless submit missing SPS"))?,
            pps: self
                .pps
                .ok_or_else(|| String::from("H.264 stateless submit missing PPS"))?,
            scaling_matrix: self.scaling_matrix,
            pred_weights: self.pred_weights,
            slice_params: slice
                .ok_or_else(|| String::from("H.264 stateless submit has no slice"))?,
            decode_params: decode
                .ok_or_else(|| String::from("H.264 stateless submit has no decode params"))?,
        };
        let timestamp = self.resolve_submit_timestamp(requested_timestamp);
        self.update_dpb_after_submit(
            &params,
            &ref_pic_marking.unwrap_or_else(H264RefPicMarking::default),
            timestamp,
        );
        Ok(H264PreparedAccessUnit { params, timestamp })
    }

    fn update_dpb_after_submit(
        &mut self,
        params: &ScarletVideoH264StatelessParams,
        marking: &H264RefPicMarking,
        timestamp: u64,
    ) {
        let is_idr = params.decode_params.flags & SCARLET_VIDEO_H264_DECODE_PARAM_FLAG_IDR != 0;
        let max_frame_num = h264_max_frame_num(&params.sps);
        let current_frame_num = params.decode_params.frame_num;
        let current_long_term_idx = if is_idr {
            self.dpb.clear();
            marking.idr_long_term.then_some(0)
        } else {
            self.apply_ref_pic_marking(marking, current_frame_num, max_frame_num)
        };

        if params.decode_params.nal_ref_idc == 0 {
            return;
        }
        let max_refs = usize::from(params.sps.max_num_ref_frames)
            .max(1)
            .min(H264_MAX_DPB_FRAMES);
        self.cap_dpb(max_refs.saturating_sub(1));
        let _ = self.dpb.push(H264DpbFrame {
            reference_ts: timestamp,
            pic_num: current_long_term_idx
                .and_then(|index| i32::try_from(index).ok())
                .unwrap_or_else(|| i32::from(params.decode_params.frame_num)),
            frame_num: params.decode_params.frame_num,
            top_field_order_cnt: params.decode_params.top_field_order_cnt,
            bottom_field_order_cnt: params.decode_params.bottom_field_order_cnt,
            long_term: current_long_term_idx.is_some(),
        });
        if !is_idr && marking.adaptive {
            self.cap_dpb(max_refs);
            return;
        }
        self.cap_dpb(max_refs);
    }

    fn apply_ref_pic_marking(
        &mut self,
        marking: &H264RefPicMarking,
        current_frame_num: u16,
        max_frame_num: u32,
    ) -> Option<u32> {
        if !marking.adaptive {
            return None;
        }

        let mut current_long_term_idx = None;
        for operation in marking.operations.as_slice() {
            match *operation {
                H264MemoryManagementControl::ShortTermUnused {
                    difference_of_pic_nums_minus1,
                } => {
                    let target = h264_mmco_short_pic_num(
                        current_frame_num,
                        max_frame_num,
                        difference_of_pic_nums_minus1,
                    );
                    self.dpb.retain(|frame| {
                        frame.long_term
                            || h264_short_pic_num(frame, current_frame_num, max_frame_num) != target
                    });
                }
                H264MemoryManagementControl::LongTermUnused { long_term_pic_num } => {
                    self.dpb.retain(|frame| {
                        !frame.long_term
                            || frame.pic_num != i32::try_from(long_term_pic_num).unwrap_or(-1)
                    });
                }
                H264MemoryManagementControl::ShortTermToLongTerm {
                    difference_of_pic_nums_minus1,
                    long_term_frame_idx,
                } => {
                    let target = h264_mmco_short_pic_num(
                        current_frame_num,
                        max_frame_num,
                        difference_of_pic_nums_minus1,
                    );
                    self.dpb.retain(|frame| {
                        !frame.long_term
                            || frame.pic_num != i32::try_from(long_term_frame_idx).unwrap_or(-1)
                    });
                    for frame in self.dpb.as_mut_slice() {
                        if !frame.long_term
                            && h264_short_pic_num(frame, current_frame_num, max_frame_num) == target
                        {
                            frame.long_term = true;
                            frame.pic_num = i32::try_from(long_term_frame_idx).unwrap_or(-1);
                            break;
                        }
                    }
                }
                H264MemoryManagementControl::MaxLongTermFrameIdx {
                    max_long_term_frame_idx_plus1,
                } => {
                    if max_long_term_frame_idx_plus1 == 0 {
                        self.dpb.retain(|frame| !frame.long_term);
                    } else {
                        let max_idx =
                            i32::try_from(max_long_term_frame_idx_plus1 - 1).unwrap_or(i32::MAX);
                        self.dpb
                            .retain(|frame| !frame.long_term || frame.pic_num <= max_idx);
                    }
                }
                H264MemoryManagementControl::Reset => {
                    self.dpb.clear();
                    current_long_term_idx = None;
                }
                H264MemoryManagementControl::CurrentToLongTerm {
                    long_term_frame_idx,
                } => {
                    self.dpb.retain(|frame| {
                        !frame.long_term
                            || frame.pic_num != i32::try_from(long_term_frame_idx).unwrap_or(-1)
                    });
                    current_long_term_idx = Some(long_term_frame_idx);
                }
                H264MemoryManagementControl::Unused => {}
            }
        }
        current_long_term_idx
    }

    fn cap_dpb(&mut self, max_refs: usize) {
        while self.dpb.len() > max_refs {
            self.dpb.remove(0);
        }
    }

    fn next_submit_timestamp(&mut self) -> u64 {
        if self.next_timestamp == 0 {
            self.next_timestamp = 1;
        }
        let timestamp = self.next_timestamp;
        self.next_timestamp = self.next_timestamp.wrapping_add(1);
        if self.next_timestamp == 0 {
            self.next_timestamp = 1;
        }
        timestamp
    }

    fn resolve_submit_timestamp(&mut self, requested_timestamp: Option<u64>) -> u64 {
        let Some(timestamp) = requested_timestamp.filter(|timestamp| *timestamp != 0) else {
            return self.next_submit_timestamp();
        };
        if self.next_timestamp == 0 || timestamp >= self.next_timestamp {
            self.next_timestamp = timestamp.wrapping_add(1);
            if self.next_timestamp == 0 {
                self.next_timestamp = 1;
            }
        }
        timestamp
    }
}

#[cfg(test)]
mod tests {
    use super::H264RequestContext;

    #[test]
    fn preserves_explicit_nonzero_timestamp() {
        let mut context = H264RequestContext::default();
        assert_eq!(context.resolve_submit_timestamp(Some(42)), 42);
        assert_eq!(context.resolve_submit_timestamp(Some(0)), 43);
        assert_eq!(context.resolve_submit_timestamp(None), 44);
    }
}

fn parse_h264_sps(nal: &[u8]) -> Result<ScarletVideoH264Sps, String> {
    if nal.len() < 2 {
        return Err(String::from("H.264 SPS is truncated"));
    }
    let mut reader = EbspBitReader::new(&nal[1..]);
    let profile_idc = read_u8_bits(&mut reader, 8, "H.264 SPS profile_idc")?;
    let constraint_set_flags = read_u8_bits(&mut reader, 8, "H.264 SPS constraint flags")?;
    let level_idc = read_u8_bits(&mut reader, 8, "H.264 SPS level_idc")?;
    let seq_parameter_set_id = read_u8_ue(&mut reader, "H.264 SPS id")?;

    let mut chroma_format_idc = 1u8;
    let mut bit_depth_luma_minus8 = 0u8;
    let mut bit_depth_chroma_minus8 = 0u8;
    let mut flags = 0u32;
    if is_h264_high_profile(profile_idc) {
        chroma_format_idc = read_u8_ue(&mut reader, "H.264 chroma_format_idc")?;
        if chroma_format_idc == 3 {
            if reader
                .read_bit()
                .ok_or_else(|| String::from("H.264 SPS separate_colour_plane_flag missing"))?
                != 0
            {
                flags |= SCARLET_VIDEO_H264_SPS_FLAG_SEPARATE_COLOUR_PLANE;
            }
        }
        bit_depth_luma_minus8 = read_u8_ue(&mut reader, "H.264 bit_depth_luma_minus8")?;
        bit_depth_chroma_minus8 = read_u8_ue(&mut reader, "H.264 bit_depth_chroma_minus8")?;
        if reader
            .read_bit()
            .ok_or_else(|| String::from("H.264 SPS transform bypass flag missing"))?
            != 0
        {
            flags |= SCARLET_VIDEO_H264_SPS_FLAG_QPPRIME_Y_ZERO_TRANSFORM_BYPASS;
        }
        let scaling_matrix_present = reader
            .read_bit()
            .ok_or_else(|| String::from("H.264 SPS scaling matrix flag missing"))?
            != 0;
        if scaling_matrix_present {
            let count = if chroma_format_idc != 3 { 8 } else { 12 };
            for index in 0..count {
                let present = reader
                    .read_bit()
                    .ok_or_else(|| String::from("H.264 SPS scaling list flag missing"))?
                    != 0;
                if present {
                    skip_h264_scaling_list(&mut reader, if index < 6 { 16 } else { 64 })?;
                }
            }
        }
    }

    let log2_max_frame_num_minus4 = read_u8_ue(&mut reader, "H.264 log2_max_frame_num_minus4")?;
    let pic_order_cnt_type = read_u8_ue(&mut reader, "H.264 pic_order_cnt_type")?;
    let mut log2_max_pic_order_cnt_lsb_minus4 = 0u8;
    let mut offset_for_ref_frame = [0i32; 255];
    let mut offset_for_non_ref_pic = 0i32;
    let mut offset_for_top_to_bottom_field = 0i32;
    let mut num_ref_frames_in_pic_order_cnt_cycle = 0u8;
    match pic_order_cnt_type {
        0 => {
            log2_max_pic_order_cnt_lsb_minus4 =
                read_u8_ue(&mut reader, "H.264 log2_max_pic_order_cnt_lsb_minus4")?;
        }
        1 => {
            if reader
                .read_bit()
                .ok_or_else(|| String::from("H.264 delta_pic_order_always_zero_flag missing"))?
                != 0
            {
                flags |= SCARLET_VIDEO_H264_SPS_FLAG_DELTA_PIC_ORDER_ALWAYS_ZERO;
            }
            offset_for_non_ref_pic = reader
                .read_se()
                .ok_or_else(|| String::from("H.264 offset_for_non_ref_pic missing"))?;
            offset_for_top_to_bottom_field = reader
                .read_se()
                .ok_or_else(|| String::from("H.264 offset_for_top_to_bottom_field missing"))?;
            let count = read_u8_ue(&mut reader, "H.264 num_ref_frames_in_pic_order_cnt_cycle")?;
            num_ref_frames_in_pic_order_cnt_cycle = count;
            for index in 0..usize::from(count) {
                offset_for_ref_frame[index] = reader
                    .read_se()
                    .ok_or_else(|| String::from("H.264 offset_for_ref_frame missing"))?;
            }
        }
        _ => return Err(String::from("H.264 pic_order_cnt_type is unsupported")),
    }
    let max_num_ref_frames = read_u8_ue(&mut reader, "H.264 max_num_ref_frames")?;
    if reader
        .read_bit()
        .ok_or_else(|| String::from("H.264 gaps_in_frame_num_value_allowed_flag missing"))?
        != 0
    {
        flags |= SCARLET_VIDEO_H264_SPS_FLAG_GAPS_IN_FRAME_NUM_VALUE_ALLOWED;
    }
    let pic_width_in_mbs_minus1 = read_u16_ue(&mut reader, "H.264 pic_width_in_mbs_minus1")?;
    let pic_height_in_map_units_minus1 =
        read_u16_ue(&mut reader, "H.264 pic_height_in_map_units_minus1")?;
    if reader
        .read_bit()
        .ok_or_else(|| String::from("H.264 frame_mbs_only_flag missing"))?
        != 0
    {
        flags |= SCARLET_VIDEO_H264_SPS_FLAG_FRAME_MBS_ONLY;
    } else if reader
        .read_bit()
        .ok_or_else(|| String::from("H.264 mb_adaptive_frame_field_flag missing"))?
        != 0
    {
        flags |= SCARLET_VIDEO_H264_SPS_FLAG_MB_ADAPTIVE_FRAME_FIELD;
    }
    if reader
        .read_bit()
        .ok_or_else(|| String::from("H.264 direct_8x8_inference_flag missing"))?
        != 0
    {
        flags |= SCARLET_VIDEO_H264_SPS_FLAG_DIRECT_8X8_INFERENCE;
    }
    let mut frame_crop_left_offset = 0;
    let mut frame_crop_right_offset = 0;
    let mut frame_crop_top_offset = 0;
    let mut frame_crop_bottom_offset = 0;
    if reader
        .read_bit()
        .ok_or_else(|| String::from("H.264 frame_cropping_flag missing"))?
        != 0
    {
        flags |= SCARLET_VIDEO_H264_SPS_FLAG_FRAME_CROPPING;
        frame_crop_left_offset = read_u32_ue(&mut reader, "H.264 frame_crop_left_offset")?;
        frame_crop_right_offset = read_u32_ue(&mut reader, "H.264 frame_crop_right_offset")?;
        frame_crop_top_offset = read_u32_ue(&mut reader, "H.264 frame_crop_top_offset")?;
        frame_crop_bottom_offset = read_u32_ue(&mut reader, "H.264 frame_crop_bottom_offset")?;
    }

    Ok(ScarletVideoH264Sps {
        profile_idc,
        constraint_set_flags,
        level_idc,
        seq_parameter_set_id,
        chroma_format_idc,
        bit_depth_luma_minus8,
        bit_depth_chroma_minus8,
        log2_max_frame_num_minus4,
        pic_order_cnt_type,
        log2_max_pic_order_cnt_lsb_minus4,
        max_num_ref_frames,
        num_ref_frames_in_pic_order_cnt_cycle,
        offset_for_ref_frame,
        offset_for_non_ref_pic,
        offset_for_top_to_bottom_field,
        pic_width_in_mbs_minus1,
        pic_height_in_map_units_minus1,
        frame_crop_left_offset,
        frame_crop_right_offset,
        frame_crop_top_offset,
        frame_crop_bottom_offset,
        flags,
    })
}

fn parse_h264_pps(nal: &[u8]) -> Result<ScarletVideoH264Pps, String> {
    if nal.len() < 2 {
        return Err(String::from("H.264 PPS is truncated"));
    }
    let mut reader = EbspBitReader::new(&nal[1..]);
    let pic_parameter_set_id = read_u8_ue(&mut reader, "H.264 PPS id")?;
    let seq_parameter_set_id = read_u8_ue(&mut reader, "H.264 PPS SPS id")?;
    let mut flags = 0u16;
    if read_bool(&mut reader, "H.264 entropy_coding_mode_flag")? {
        flags |= SCARLET_VIDEO_H264_PPS_FLAG_ENTROPY_CODING_MODE;
    }
    if read_bool(
        &mut reader,
        "H.264 bottom_field_pic_order_in_frame_present_flag",
    )? {
        flags |= SCARLET_VIDEO_H264_PPS_FLAG_BOTTOM_FIELD_PIC_ORDER_IN_FRAME_PRESENT;
    }
    let num_slice_groups_minus1 = read_u8_ue(&mut reader, "H.264 num_slice_groups_minus1")?;
    if num_slice_groups_minus1 != 0 {
        return Err(String::from("H.264 slice groups are unsupported"));
    }
    let num_ref_idx_l0_default_active_minus1 =
        read_u8_ue(&mut reader, "H.264 num_ref_idx_l0_default_active_minus1")?;
    let num_ref_idx_l1_default_active_minus1 =
        read_u8_ue(&mut reader, "H.264 num_ref_idx_l1_default_active_minus1")?;
    if read_bool(&mut reader, "H.264 weighted_pred_flag")? {
        flags |= SCARLET_VIDEO_H264_PPS_FLAG_WEIGHTED_PRED;
    }
    let weighted_bipred_idc = read_u8_bits(&mut reader, 2, "H.264 weighted_bipred_idc")?;
    let pic_init_qp_minus26 = read_i8_se(&mut reader, "H.264 pic_init_qp_minus26")?;
    let pic_init_qs_minus26 = read_i8_se(&mut reader, "H.264 pic_init_qs_minus26")?;
    let chroma_qp_index_offset = read_i8_se(&mut reader, "H.264 chroma_qp_index_offset")?;
    if read_bool(&mut reader, "H.264 deblocking_filter_control_present_flag")? {
        flags |= SCARLET_VIDEO_H264_PPS_FLAG_DEBLOCKING_FILTER_CONTROL_PRESENT;
    }
    if read_bool(&mut reader, "H.264 constrained_intra_pred_flag")? {
        flags |= SCARLET_VIDEO_H264_PPS_FLAG_CONSTRAINED_INTRA_PRED;
    }
    if read_bool(&mut reader, "H.264 redundant_pic_cnt_present_flag")? {
        flags |= SCARLET_VIDEO_H264_PPS_FLAG_REDUNDANT_PIC_CNT_PRESENT;
    }
    if h264_more_rbsp_data(&reader) {
        if read_bool(&mut reader, "H.264 transform_8x8_mode_flag")? {
            flags |= SCARLET_VIDEO_H264_PPS_FLAG_TRANSFORM_8X8_MODE;
        }
        if read_bool(&mut reader, "H.264 pic_scaling_matrix_present_flag")? {
            return Err(String::from("H.264 PPS scaling matrices are unsupported"));
        }
        if h264_more_rbsp_data(&reader) {
            let second_chroma_qp_index_offset =
                read_i8_se(&mut reader, "H.264 second_chroma_qp_index_offset")?;
            return Ok(ScarletVideoH264Pps {
                pic_parameter_set_id,
                seq_parameter_set_id,
                num_slice_groups_minus1,
                num_ref_idx_l0_default_active_minus1,
                num_ref_idx_l1_default_active_minus1,
                weighted_bipred_idc,
                pic_init_qp_minus26,
                pic_init_qs_minus26,
                chroma_qp_index_offset,
                second_chroma_qp_index_offset,
                flags,
            });
        }
    }

    Ok(ScarletVideoH264Pps {
        pic_parameter_set_id,
        seq_parameter_set_id,
        num_slice_groups_minus1,
        num_ref_idx_l0_default_active_minus1,
        num_ref_idx_l1_default_active_minus1,
        weighted_bipred_idc,
        pic_init_qp_minus26,
        pic_init_qs_minus26,
        chroma_qp_index_offset,
        second_chroma_qp_index_offset: chroma_qp_index_offset,
        flags,
    })
}

fn parse_h264_slice(
    nal: &RawNalUnit<'_>,
    sps: &ScarletVideoH264Sps,
    pps: &ScarletVideoH264Pps,
    dpb: &[H264DpbFrame],
    pred_weights: &mut ScarletVideoH264PredWeights,
    poc_state: &mut H264PocState,
) -> Result<
    (
        ScarletVideoH264SliceParams,
        ScarletVideoH264DecodeParams,
        H264RefPicMarking,
    ),
    String,
> {
    if nal.bytes.len() < 2 {
        return Err(String::from("H.264 slice is truncated"));
    }
    let mut reader = EbspBitReader::new(&nal.bytes[1..]);
    let first_mb_in_slice = read_u32_ue(&mut reader, "H.264 first_mb_in_slice")?;
    let slice_type = read_u8_ue(&mut reader, "H.264 slice_type")?;
    let pic_parameter_set_id = read_u8_ue(&mut reader, "H.264 slice PPS id")?;
    if pic_parameter_set_id != pps.pic_parameter_set_id {
        return Err(String::from("H.264 slice references unknown PPS"));
    }

    let mut colour_plane_id = 0;
    if sps.flags & SCARLET_VIDEO_H264_SPS_FLAG_SEPARATE_COLOUR_PLANE != 0 {
        colour_plane_id = read_u8_bits(&mut reader, 2, "H.264 colour_plane_id")?;
    }
    let frame_num_bits = sps.log2_max_frame_num_minus4.saturating_add(4);
    let frame_num = read_u16_bits(&mut reader, frame_num_bits, "H.264 frame_num")?;
    if sps.flags & SCARLET_VIDEO_H264_SPS_FLAG_FRAME_MBS_ONLY == 0 {
        return Err(String::from("H.264 field pictures are unsupported"));
    }

    let mut decode_params = ScarletVideoH264DecodeParams {
        nal_ref_idc: u16::from(nal.nal_ref_idc),
        frame_num,
        ..Default::default()
    };
    let max_frame_num = h264_max_frame_num(sps);
    fill_h264_decode_dpb(&mut decode_params, dpb, frame_num, max_frame_num);
    if nal.nal_type == 5 {
        decode_params.flags |= SCARLET_VIDEO_H264_DECODE_PARAM_FLAG_IDR;
        decode_params.idr_pic_id = read_u16_ue(&mut reader, "H.264 idr_pic_id")?;
    }
    if sps.pic_order_cnt_type == 0 {
        let poc_bits = sps.log2_max_pic_order_cnt_lsb_minus4.saturating_add(4);
        decode_params.pic_order_cnt_lsb =
            read_u16_bits(&mut reader, poc_bits, "H.264 pic_order_cnt_lsb")?;
        let pic_order_cnt_msb =
            h264_pic_order_cnt_msb(sps, nal, decode_params.pic_order_cnt_lsb, poc_state);
        decode_params.top_field_order_cnt =
            pic_order_cnt_msb.saturating_add(i32::from(decode_params.pic_order_cnt_lsb));
        decode_params.bottom_field_order_cnt = decode_params.top_field_order_cnt;
        if pps.flags & SCARLET_VIDEO_H264_PPS_FLAG_BOTTOM_FIELD_PIC_ORDER_IN_FRAME_PRESENT != 0 {
            decode_params.delta_pic_order_cnt_bottom = reader
                .read_se()
                .ok_or_else(|| String::from("H.264 delta_pic_order_cnt_bottom missing"))?;
            decode_params.bottom_field_order_cnt = decode_params
                .top_field_order_cnt
                .saturating_add(decode_params.delta_pic_order_cnt_bottom);
        }
    } else if sps.pic_order_cnt_type == 1
        && sps.flags & SCARLET_VIDEO_H264_SPS_FLAG_DELTA_PIC_ORDER_ALWAYS_ZERO == 0
    {
        decode_params.delta_pic_order_cnt0 = reader
            .read_se()
            .ok_or_else(|| String::from("H.264 delta_pic_order_cnt0 missing"))?;
        if pps.flags & SCARLET_VIDEO_H264_PPS_FLAG_BOTTOM_FIELD_PIC_ORDER_IN_FRAME_PRESENT != 0 {
            decode_params.delta_pic_order_cnt1 = reader
                .read_se()
                .ok_or_else(|| String::from("H.264 delta_pic_order_cnt1 missing"))?;
        }
        decode_params.top_field_order_cnt = decode_params.delta_pic_order_cnt0;
        decode_params.bottom_field_order_cnt = decode_params
            .top_field_order_cnt
            .saturating_add(decode_params.delta_pic_order_cnt1);
    }
    let mut redundant_pic_cnt = 0;
    if pps.flags & SCARLET_VIDEO_H264_PPS_FLAG_REDUNDANT_PIC_CNT_PRESENT != 0 {
        redundant_pic_cnt = read_u8_ue(&mut reader, "H.264 redundant_pic_cnt")?;
    }

    let slice_class = slice_type % 5;
    let mut num_ref_idx_l0_active_minus1 = pps.num_ref_idx_l0_default_active_minus1;
    let mut num_ref_idx_l1_active_minus1 = pps.num_ref_idx_l1_default_active_minus1;
    let mut slice_flags = 0u32;
    if slice_class == 1 {
        if read_bool(&mut reader, "H.264 direct_spatial_mv_pred_flag")? {
            slice_flags |= SCARLET_VIDEO_H264_SLICE_FLAG_DIRECT_SPATIAL_MV_PRED;
        }
    }
    if slice_class == 0 || slice_class == 1 || slice_class == 3 {
        let override_refs = read_bool(&mut reader, "H.264 num_ref_idx_active_override_flag")?;
        if override_refs {
            num_ref_idx_l0_active_minus1 =
                read_u8_ue(&mut reader, "H.264 num_ref_idx_l0_active_minus1")?;
            if slice_class == 1 {
                num_ref_idx_l1_active_minus1 =
                    read_u8_ue(&mut reader, "H.264 num_ref_idx_l1_active_minus1")?;
            }
        }
    }
    let (list0_modifications, list1_modifications) =
        parse_h264_ref_pic_list_modification(&mut reader, slice_class)?;
    let (ref_pic_list0, ref_pic_list1) = build_h264_ref_pic_lists(
        dpb,
        &decode_params,
        sps,
        slice_class,
        num_ref_idx_l0_active_minus1,
        num_ref_idx_l1_active_minus1,
        &list0_modifications,
        &list1_modifications,
    )?;
    if slice_class == 0 || slice_class == 1 || slice_class == 3 {
        slice_flags |= SCARLET_VIDEO_H264_SLICE_FLAG_REF_LISTS_PRESENT;
    }
    parse_h264_pred_weight_table(
        &mut reader,
        sps,
        pps,
        slice_class,
        num_ref_idx_l0_active_minus1,
        num_ref_idx_l1_active_minus1,
        pred_weights,
    )?;

    let dec_ref_start_bits = reader.position_bits();
    let ref_pic_marking = if nal.nal_ref_idc != 0 {
        parse_h264_dec_ref_pic_marking(&mut reader, nal.nal_type)?
    } else {
        H264RefPicMarking::default()
    };
    decode_params.dec_ref_pic_marking_bit_size =
        reader.position_bits().saturating_sub(dec_ref_start_bits) as u32;

    let mut cabac_init_idc = 0;
    if pps.flags & SCARLET_VIDEO_H264_PPS_FLAG_ENTROPY_CODING_MODE != 0
        && slice_class != 2
        && slice_class != 4
    {
        cabac_init_idc = read_u8_ue(&mut reader, "H.264 cabac_init_idc")?;
    }
    let slice_qp_delta = read_i8_se(&mut reader, "H.264 slice_qp_delta")?;
    let mut slice_qs_delta = 0;
    if slice_class == 3 || slice_class == 4 {
        if slice_class == 3 {
            let _sp_for_switch_flag = read_bool(&mut reader, "H.264 sp_for_switch_flag")?;
        }
        slice_qs_delta = read_i8_se(&mut reader, "H.264 slice_qs_delta")?;
    }

    let mut disable_deblocking_filter_idc = 0;
    let mut slice_alpha_c0_offset_div2 = 0;
    let mut slice_beta_offset_div2 = 0;
    if pps.flags & SCARLET_VIDEO_H264_PPS_FLAG_DEBLOCKING_FILTER_CONTROL_PRESENT != 0 {
        disable_deblocking_filter_idc =
            read_u8_ue(&mut reader, "H.264 disable_deblocking_filter_idc")?;
        if disable_deblocking_filter_idc != 1 {
            slice_alpha_c0_offset_div2 =
                read_i8_se(&mut reader, "H.264 slice_alpha_c0_offset_div2")?;
            slice_beta_offset_div2 = read_i8_se(&mut reader, "H.264 slice_beta_offset_div2")?;
        }
    }

    if sps.pic_order_cnt_type == 0 && nal.nal_ref_idc != 0 {
        if ref_pic_marking.resets_poc() {
            poc_state.prev_pic_order_cnt_msb = 0;
            poc_state.prev_pic_order_cnt_lsb = 0;
        } else {
            poc_state.prev_pic_order_cnt_msb = decode_params
                .top_field_order_cnt
                .saturating_sub(i32::from(decode_params.pic_order_cnt_lsb));
            poc_state.prev_pic_order_cnt_lsb = decode_params.pic_order_cnt_lsb;
        }
    }

    let mut slice_params = ScarletVideoH264SliceParams {
        header_bit_size: reader.position_bits() as u32,
        nal_offset: nal.offset as u32,
        nal_len: nal.bytes.len() as u32,
        first_mb_in_slice,
        slice_type,
        pic_parameter_set_id,
        colour_plane_id,
        redundant_pic_cnt,
        cabac_init_idc,
        slice_qp_delta,
        slice_qs_delta,
        disable_deblocking_filter_idc,
        slice_alpha_c0_offset_div2,
        slice_beta_offset_div2,
        num_ref_idx_l0_active_minus1,
        num_ref_idx_l1_active_minus1,
        flags: slice_flags,
        ..Default::default()
    };
    slice_params.ref_pic_list0 = ref_pic_list0;
    slice_params.ref_pic_list1 = ref_pic_list1;
    Ok((slice_params, decode_params, ref_pic_marking))
}

fn h264_pic_order_cnt_msb(
    sps: &ScarletVideoH264Sps,
    nal: &RawNalUnit<'_>,
    pic_order_cnt_lsb: u16,
    poc_state: &H264PocState,
) -> i32 {
    let poc_bits = u32::from(sps.log2_max_pic_order_cnt_lsb_minus4.saturating_add(4));
    let max_pic_order_cnt_lsb = 1i32.checked_shl(poc_bits).unwrap_or(i32::MAX);
    if nal.nal_type == 5 || max_pic_order_cnt_lsb <= 0 {
        return 0;
    }

    let pic_order_cnt_lsb = i32::from(pic_order_cnt_lsb);
    let prev_pic_order_cnt_lsb = i32::from(poc_state.prev_pic_order_cnt_lsb);
    let prev_pic_order_cnt_msb = poc_state.prev_pic_order_cnt_msb;
    let half_range = max_pic_order_cnt_lsb / 2;

    if pic_order_cnt_lsb < prev_pic_order_cnt_lsb
        && prev_pic_order_cnt_lsb - pic_order_cnt_lsb >= half_range
    {
        prev_pic_order_cnt_msb.saturating_add(max_pic_order_cnt_lsb)
    } else if pic_order_cnt_lsb > prev_pic_order_cnt_lsb
        && pic_order_cnt_lsb - prev_pic_order_cnt_lsb > half_range
    {
        prev_pic_order_cnt_msb.saturating_sub(max_pic_order_cnt_lsb)
    } else {
        prev_pic_order_cnt_msb
    }
}

fn fill_h264_decode_dpb(
    decode: &mut ScarletVideoH264DecodeParams,
    dpb: &[H264DpbFrame],
    current_frame_num: u16,
    max_frame_num: u32,
) {
    for (index, frame) in dpb.iter().take(16).enumerate() {
        decode.dpb[index] = ScarletVideoH264DpbEntry {
            reference_ts: frame.reference_ts,
            pic_num: if frame.long_term {
                frame.pic_num
            } else {
                h264_short_pic_num(frame, current_frame_num, max_frame_num)
            },
            frame_num: frame.frame_num,
            fields: 3,
            reserved: [0; 5],
            top_field_order_cnt: frame.top_field_order_cnt,
            bottom_field_order_cnt: frame.bottom_field_order_cnt,
            flags: SCARLET_VIDEO_H264_DPB_FLAG_VALID
                | if frame.long_term {
                    SCARLET_VIDEO_H264_DPB_FLAG_LONG_TERM
                } else {
                    0
                },
        };
    }
}

fn parse_h264_ref_pic_list_modification(
    reader: &mut EbspBitReader<'_>,
    slice_class: u8,
) -> Result<
    (
        FixedList<H264RefListModification, H264_MAX_REF_LIST_MODIFICATIONS>,
        FixedList<H264RefListModification, H264_MAX_REF_LIST_MODIFICATIONS>,
    ),
    String,
> {
    if slice_class == 2 || slice_class == 4 {
        return Ok((FixedList::new(), FixedList::new()));
    }

    let mut list0 = FixedList::new();
    if read_bool(reader, "H.264 ref_pic_list_modification_flag_l0")? {
        loop {
            let idc = read_u32_ue(reader, "H.264 modification_of_pic_nums_idc_l0")?;
            match idc {
                0 | 1 => {
                    let abs_diff_pic_num_minus1 =
                        read_u32_ue(reader, "H.264 abs_diff_pic_num_minus1_l0")?;
                    list0.push(if idc == 0 {
                        H264RefListModification::ShortTermSubtract(abs_diff_pic_num_minus1)
                    } else {
                        H264RefListModification::ShortTermAdd(abs_diff_pic_num_minus1)
                    })?;
                }
                2 => {
                    let long_term_pic_num = read_u32_ue(reader, "H.264 long_term_pic_num_l0")?;
                    list0.push(H264RefListModification::LongTerm(long_term_pic_num))?;
                }
                3 => break,
                _ => return Err(String::from("H.264 invalid ref list modification idc")),
            }
        }
    }

    let mut list1 = FixedList::new();
    if slice_class == 1 && read_bool(reader, "H.264 ref_pic_list_modification_flag_l1")? {
        loop {
            let idc = read_u32_ue(reader, "H.264 modification_of_pic_nums_idc_l1")?;
            match idc {
                0 | 1 => {
                    let abs_diff_pic_num_minus1 =
                        read_u32_ue(reader, "H.264 abs_diff_pic_num_minus1_l1")?;
                    list1.push(if idc == 0 {
                        H264RefListModification::ShortTermSubtract(abs_diff_pic_num_minus1)
                    } else {
                        H264RefListModification::ShortTermAdd(abs_diff_pic_num_minus1)
                    })?;
                }
                2 => {
                    let long_term_pic_num = read_u32_ue(reader, "H.264 long_term_pic_num_l1")?;
                    list1.push(H264RefListModification::LongTerm(long_term_pic_num))?;
                }
                3 => break,
                _ => return Err(String::from("H.264 invalid ref list modification idc")),
            }
        }
    }

    Ok((list0, list1))
}

fn build_h264_ref_pic_lists(
    dpb: &[H264DpbFrame],
    decode: &ScarletVideoH264DecodeParams,
    sps: &ScarletVideoH264Sps,
    slice_class: u8,
    num_ref_idx_l0_active_minus1: u8,
    num_ref_idx_l1_active_minus1: u8,
    list0_modifications: &FixedList<H264RefListModification, H264_MAX_REF_LIST_MODIFICATIONS>,
    list1_modifications: &FixedList<H264RefListModification, H264_MAX_REF_LIST_MODIFICATIONS>,
) -> Result<
    (
        [ScarletVideoH264Reference; 32],
        [ScarletVideoH264Reference; 32],
    ),
    String,
> {
    let mut list0 = FixedList::new();
    let mut list1 = FixedList::new();
    let l0_active = h264_active_count(num_ref_idx_l0_active_minus1);
    let l1_active = h264_active_count(num_ref_idx_l1_active_minus1);
    let max_frame_num = h264_max_frame_num(sps);

    match slice_class {
        0 | 3 => {
            list0 = h264_default_p_ref_list(dpb, decode.frame_num, max_frame_num);
        }
        1 => {
            let (default_l0, default_l1) =
                h264_default_b_ref_lists(dpb, decode.top_field_order_cnt);
            list0 = default_l0;
            list1 = default_l1;
        }
        _ => {}
    }

    apply_h264_ref_list_modifications(
        &mut list0,
        list0_modifications.as_slice(),
        dpb,
        decode.frame_num,
        max_frame_num,
        l0_active,
    )?;
    if slice_class == 1 {
        apply_h264_ref_list_modifications(
            &mut list1,
            list1_modifications.as_slice(),
            dpb,
            decode.frame_num,
            max_frame_num,
            l1_active,
        )?;
    }

    Ok((
        write_h264_ref_list(list0.as_slice(), l0_active),
        write_h264_ref_list(list1.as_slice(), l1_active),
    ))
}

fn h264_default_p_ref_list(
    dpb: &[H264DpbFrame],
    current_frame_num: u16,
    max_frame_num: u32,
) -> FixedList<usize, H264_MAX_REF_LIST_ENTRIES> {
    let mut refs = FixedList::new();
    for (index, frame) in dpb.iter().enumerate().rev() {
        if !frame.long_term {
            let _ = refs.push(index);
        }
    }
    refs.as_mut_slice().sort_by(|left, right| {
        h264_short_pic_num(&dpb[*right], current_frame_num, max_frame_num).cmp(&h264_short_pic_num(
            &dpb[*left],
            current_frame_num,
            max_frame_num,
        ))
    });
    for (index, frame) in dpb.iter().enumerate().rev() {
        if frame.long_term {
            let _ = refs.push(index);
        }
    }
    refs
}

fn h264_default_b_ref_lists(
    dpb: &[H264DpbFrame],
    current_poc: i32,
) -> (
    FixedList<usize, H264_MAX_REF_LIST_ENTRIES>,
    FixedList<usize, H264_MAX_REF_LIST_ENTRIES>,
) {
    let mut before: FixedList<usize, H264_MAX_REF_LIST_ENTRIES> = FixedList::new();
    let mut after: FixedList<usize, H264_MAX_REF_LIST_ENTRIES> = FixedList::new();
    let mut long_term: FixedList<usize, H264_MAX_REF_LIST_ENTRIES> = FixedList::new();
    for (index, frame) in dpb.iter().enumerate() {
        if frame.long_term {
            let _ = long_term.push(index);
        } else if frame.top_field_order_cnt < current_poc {
            let _ = before.push(index);
        } else {
            let _ = after.push(index);
        }
    }

    before.as_mut_slice().sort_by(|left, right| {
        dpb[*right]
            .top_field_order_cnt
            .cmp(&dpb[*left].top_field_order_cnt)
    });
    after.as_mut_slice().sort_by(|left, right| {
        dpb[*left]
            .top_field_order_cnt
            .cmp(&dpb[*right].top_field_order_cnt)
    });
    long_term
        .as_mut_slice()
        .sort_by(|left, right| dpb[*left].pic_num.cmp(&dpb[*right].pic_num));

    let mut list0 = FixedList::new();
    for index in before.as_slice() {
        let _ = list0.push(*index);
    }
    for index in after.as_slice() {
        let _ = list0.push(*index);
    }
    for index in long_term.as_slice() {
        let _ = list0.push(*index);
    }

    let mut list1 = FixedList::new();
    for index in after.as_slice() {
        let _ = list1.push(*index);
    }
    for index in before.as_slice() {
        let _ = list1.push(*index);
    }
    for index in long_term.as_slice() {
        let _ = list1.push(*index);
    }
    if list0 == list1 && list1.len() > 1 {
        list1.swap(0, 1);
    }

    (list0, list1)
}

fn apply_h264_ref_list_modifications(
    list: &mut FixedList<usize, H264_MAX_REF_LIST_ENTRIES>,
    modifications: &[H264RefListModification],
    dpb: &[H264DpbFrame],
    current_frame_num: u16,
    max_frame_num: u32,
    active_count: usize,
) -> Result<(), String> {
    if active_count == 0 {
        list.clear();
        return Ok(());
    }

    let max_frame_num_u32 = max_frame_num.max(1);
    let mut pic_num_pred = i64::from(current_frame_num);
    let max_frame_num = i64::from(max_frame_num_u32);
    let mut ref_idx = 0usize;
    for modification in modifications {
        let dpb_index = match *modification {
            H264RefListModification::ShortTermSubtract(abs_diff_pic_num_minus1) => {
                let diff = i64::from(abs_diff_pic_num_minus1) + 1;
                let mut pic_num_no_wrap = pic_num_pred - diff;
                if pic_num_no_wrap < 0 {
                    pic_num_no_wrap += max_frame_num;
                }
                pic_num_pred = pic_num_no_wrap;
                let pic_num = if pic_num_no_wrap > i64::from(current_frame_num) {
                    pic_num_no_wrap - max_frame_num
                } else {
                    pic_num_no_wrap
                };
                find_h264_short_ref_by_pic_num(dpb, current_frame_num, max_frame_num_u32, pic_num)
            }
            H264RefListModification::ShortTermAdd(abs_diff_pic_num_minus1) => {
                let diff = i64::from(abs_diff_pic_num_minus1) + 1;
                let mut pic_num_no_wrap = pic_num_pred + diff;
                if pic_num_no_wrap >= max_frame_num {
                    pic_num_no_wrap -= max_frame_num;
                }
                pic_num_pred = pic_num_no_wrap;
                let pic_num = if pic_num_no_wrap > i64::from(current_frame_num) {
                    pic_num_no_wrap - max_frame_num
                } else {
                    pic_num_no_wrap
                };
                find_h264_short_ref_by_pic_num(dpb, current_frame_num, max_frame_num_u32, pic_num)
            }
            H264RefListModification::LongTerm(long_term_pic_num) => dpb.iter().position(|frame| {
                frame.long_term && frame.pic_num == i32::try_from(long_term_pic_num).unwrap_or(-1)
            }),
            H264RefListModification::Unused => None,
        };

        let Some(dpb_index) = dpb_index else {
            continue;
        };
        if ref_idx > list.len() {
            list.push(dpb_index)?;
        } else {
            list.insert(ref_idx, dpb_index)?;
        }
        let mut scan = ref_idx + 1;
        while scan < list.len() {
            if list.as_slice()[scan] == dpb_index {
                list.remove(scan);
            } else {
                scan += 1;
            }
        }
        ref_idx = ref_idx.saturating_add(1).min(active_count);
    }

    if list.is_empty() {
        return Ok(());
    }
    while list.len() < active_count {
        let last = list.last().unwrap_or(0);
        list.push(last)?;
    }
    list.truncate(active_count);
    Ok(())
}

fn find_h264_short_ref_by_pic_num(
    dpb: &[H264DpbFrame],
    current_frame_num: u16,
    max_frame_num: u32,
    pic_num: i64,
) -> Option<usize> {
    dpb.iter().position(|frame| {
        !frame.long_term
            && i64::from(h264_short_pic_num(frame, current_frame_num, max_frame_num)) == pic_num
    })
}

fn write_h264_ref_list(list: &[usize], active_count: usize) -> [ScarletVideoH264Reference; 32] {
    let mut refs = [ScarletVideoH264Reference::default(); 32];
    for (output_index, dpb_index) in list.iter().copied().take(active_count.min(32)).enumerate() {
        refs[output_index] = ScarletVideoH264Reference {
            fields: 3,
            index: dpb_index as u8,
        };
    }
    refs
}

fn h264_active_count(active_minus1: u8) -> usize {
    usize::from(active_minus1).saturating_add(1).min(32)
}

fn h264_max_frame_num(sps: &ScarletVideoH264Sps) -> u32 {
    1u32.checked_shl(u32::from(sps.log2_max_frame_num_minus4.saturating_add(4)))
        .unwrap_or(u32::MAX)
}

fn h264_short_pic_num(frame: &H264DpbFrame, current_frame_num: u16, max_frame_num: u32) -> i32 {
    let max_frame_num = i32::try_from(max_frame_num).unwrap_or(i32::MAX);
    let frame_num = i32::from(frame.frame_num);
    if frame.frame_num > current_frame_num {
        frame_num.saturating_sub(max_frame_num)
    } else {
        frame_num
    }
}

fn h264_mmco_short_pic_num(
    current_frame_num: u16,
    _max_frame_num: u32,
    difference_of_pic_nums_minus1: u32,
) -> i32 {
    i32::from(current_frame_num)
        .saturating_sub(i32::try_from(difference_of_pic_nums_minus1).unwrap_or(i32::MAX))
        .saturating_sub(1)
}

fn parse_h264_pred_weight_table(
    reader: &mut EbspBitReader<'_>,
    sps: &ScarletVideoH264Sps,
    pps: &ScarletVideoH264Pps,
    slice_class: u8,
    num_ref_idx_l0_active_minus1: u8,
    num_ref_idx_l1_active_minus1: u8,
    pred_weights: &mut ScarletVideoH264PredWeights,
) -> Result<(), String> {
    let weighted_p = pps.flags & SCARLET_VIDEO_H264_PPS_FLAG_WEIGHTED_PRED != 0
        && (slice_class == 0 || slice_class == 3);
    let weighted_b = pps.weighted_bipred_idc == 1 && slice_class == 1;
    *pred_weights = ScarletVideoH264PredWeights::default();
    if !weighted_p && !weighted_b {
        return Ok(());
    }

    let luma_denom = read_u16_ue(reader, "H.264 luma_log2_weight_denom")?;
    let chroma_denom = if sps.chroma_format_idc != 0 {
        read_u16_ue(reader, "H.264 chroma_log2_weight_denom")?
    } else {
        0
    };
    pred_weights.luma_log2_weight_denom = luma_denom;
    pred_weights.chroma_log2_weight_denom = chroma_denom;

    let list_count = if slice_class == 1 { 2 } else { 1 };
    for list in 0..list_count {
        let active = if list == 0 {
            num_ref_idx_l0_active_minus1
        } else {
            num_ref_idx_l1_active_minus1
        };
        for index in 0..=usize::from(active) {
            pred_weights.weight_factors[list].luma_weight[index] =
                1i16.checked_shl(u32::from(luma_denom)).unwrap_or(0);
            if read_bool(reader, "H.264 luma_weight_lX_flag")? {
                pred_weights.weight_factors[list].luma_weight[index] =
                    read_i16_se(reader, "H.264 luma_weight_lX")?;
                pred_weights.weight_factors[list].luma_offset[index] =
                    read_i16_se(reader, "H.264 luma_offset_lX")?;
            }

            if sps.chroma_format_idc != 0 {
                for component in 0..2 {
                    pred_weights.weight_factors[list].chroma_weight[index][component] =
                        1i16.checked_shl(u32::from(chroma_denom)).unwrap_or(0);
                }
                if read_bool(reader, "H.264 chroma_weight_lX_flag")? {
                    for component in 0..2 {
                        pred_weights.weight_factors[list].chroma_weight[index][component] =
                            read_i16_se(reader, "H.264 chroma_weight_lX")?;
                        pred_weights.weight_factors[list].chroma_offset[index][component] =
                            read_i16_se(reader, "H.264 chroma_offset_lX")?;
                    }
                }
            }
        }
    }

    Ok(())
}

fn parse_h264_dec_ref_pic_marking(
    reader: &mut EbspBitReader<'_>,
    nal_type: u8,
) -> Result<H264RefPicMarking, String> {
    if nal_type == 5 {
        let _no_output_of_prior_pics_flag =
            read_bool(reader, "H.264 no_output_of_prior_pics_flag")?;
        let long_term_reference_flag = read_bool(reader, "H.264 long_term_reference_flag")?;
        return Ok(H264RefPicMarking {
            idr_long_term: long_term_reference_flag,
            adaptive: false,
            operations: FixedList::new(),
        });
    }

    if !read_bool(reader, "H.264 adaptive_ref_pic_marking_mode_flag")? {
        return Ok(H264RefPicMarking::default());
    }
    let mut marking = H264RefPicMarking {
        idr_long_term: false,
        adaptive: true,
        operations: FixedList::new(),
    };
    loop {
        let op = read_u32_ue(reader, "H.264 memory_management_control_operation")?;
        match op {
            0 => break,
            1 => {
                let difference_of_pic_nums_minus1 =
                    read_u32_ue(reader, "H.264 difference_of_pic_nums_minus1")?;
                marking
                    .operations
                    .push(H264MemoryManagementControl::ShortTermUnused {
                        difference_of_pic_nums_minus1,
                    })?;
            }
            2 => {
                let long_term_pic_num = read_u32_ue(reader, "H.264 long_term_pic_num")?;
                marking
                    .operations
                    .push(H264MemoryManagementControl::LongTermUnused { long_term_pic_num })?;
            }
            3 => {
                let difference_of_pic_nums_minus1 =
                    read_u32_ue(reader, "H.264 difference_of_pic_nums_minus1")?;
                let long_term_frame_idx = read_u32_ue(reader, "H.264 long_term_frame_idx")?;
                marking
                    .operations
                    .push(H264MemoryManagementControl::ShortTermToLongTerm {
                        difference_of_pic_nums_minus1,
                        long_term_frame_idx,
                    })?;
            }
            4 => {
                let max_long_term_frame_idx_plus1 =
                    read_u32_ue(reader, "H.264 max_long_term_frame_idx_plus1")?;
                marking
                    .operations
                    .push(H264MemoryManagementControl::MaxLongTermFrameIdx {
                        max_long_term_frame_idx_plus1,
                    })?;
            }
            5 => {
                marking
                    .operations
                    .push(H264MemoryManagementControl::Reset)?;
            }
            6 => {
                let long_term_frame_idx = read_u32_ue(reader, "H.264 long_term_frame_idx")?;
                marking
                    .operations
                    .push(H264MemoryManagementControl::CurrentToLongTerm {
                        long_term_frame_idx,
                    })?;
            }
            _ => return Err(String::from("H.264 invalid MMCO")),
        }
    }
    Ok(marking)
}

fn h264_more_rbsp_data(reader: &EbspBitReader<'_>) -> bool {
    let mut clone = reader.clone();
    let Some(first) = clone.read_bit() else {
        return false;
    };
    if first == 0 {
        return true;
    }
    while let Some(bit) = clone.read_bit() {
        if bit != 0 {
            return true;
        }
    }
    false
}

fn read_bool(reader: &mut EbspBitReader<'_>, name: &'static str) -> Result<bool, String> {
    reader
        .read_bit()
        .map(|bit| bit != 0)
        .ok_or_else(|| format!("{name} missing"))
}

fn read_u8_bits(
    reader: &mut EbspBitReader<'_>,
    bits: u8,
    name: &'static str,
) -> Result<u8, String> {
    let value = reader
        .read_bits(bits)
        .ok_or_else(|| format!("{name} missing"))?;
    u8::try_from(value).map_err(|_| format!("{name} overflows u8"))
}

fn read_u16_bits(
    reader: &mut EbspBitReader<'_>,
    bits: u8,
    name: &'static str,
) -> Result<u16, String> {
    let value = reader
        .read_bits(bits)
        .ok_or_else(|| format!("{name} missing"))?;
    u16::try_from(value).map_err(|_| format!("{name} overflows u16"))
}

fn read_u8_ue(reader: &mut EbspBitReader<'_>, name: &'static str) -> Result<u8, String> {
    let value = read_u32_ue(reader, name)?;
    u8::try_from(value).map_err(|_| format!("{name} overflows u8"))
}

fn read_u16_ue(reader: &mut EbspBitReader<'_>, name: &'static str) -> Result<u16, String> {
    let value = read_u32_ue(reader, name)?;
    u16::try_from(value).map_err(|_| format!("{name} overflows u16"))
}

fn read_u32_ue(reader: &mut EbspBitReader<'_>, name: &'static str) -> Result<u32, String> {
    reader.read_ue().ok_or_else(|| format!("{name} missing"))
}

fn read_i8_se(reader: &mut EbspBitReader<'_>, name: &'static str) -> Result<i8, String> {
    let value = reader.read_se().ok_or_else(|| format!("{name} missing"))?;
    i8::try_from(value).map_err(|_| format!("{name} overflows i8"))
}

fn read_i16_se(reader: &mut EbspBitReader<'_>, name: &'static str) -> Result<i16, String> {
    let value = reader.read_se().ok_or_else(|| format!("{name} missing"))?;
    i16::try_from(value).map_err(|_| format!("{name} overflows i16"))
}

fn skip_h264_scaling_list(reader: &mut EbspBitReader<'_>, count: usize) -> Result<(), String> {
    let mut last_scale = 8i32;
    let mut next_scale = 8i32;
    for _ in 0..count {
        if next_scale != 0 {
            let delta_scale = reader
                .read_se()
                .ok_or_else(|| String::from("H.264 scaling list is truncated"))?;
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

fn is_h264_high_profile(profile_idc: u8) -> bool {
    matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    )
}
