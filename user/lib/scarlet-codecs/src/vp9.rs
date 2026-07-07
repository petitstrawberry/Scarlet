//! VP9 stateless request construction types.

use alloc::string::String;

#[path = "vp9_probs.rs"]
mod vp9_probs;
use vp9_probs::*;

pub const SCARLET_VIDEO_VP9_LOOP_FILTER_FLAG_DELTA_ENABLED: u8 = 1 << 0;
pub const SCARLET_VIDEO_VP9_LOOP_FILTER_FLAG_DELTA_UPDATE: u8 = 1 << 1;
pub const SCARLET_VIDEO_VP9_SEGMENTATION_FLAG_ENABLED: u8 = 1 << 0;
pub const SCARLET_VIDEO_VP9_SEGMENTATION_FLAG_UPDATE_MAP: u8 = 1 << 1;
pub const SCARLET_VIDEO_VP9_SEGMENTATION_FLAG_TEMPORAL_UPDATE: u8 = 1 << 2;
pub const SCARLET_VIDEO_VP9_SEGMENTATION_FLAG_UPDATE_DATA: u8 = 1 << 3;
pub const SCARLET_VIDEO_VP9_SEGMENTATION_FLAG_ABS_OR_DELTA_UPDATE: u8 = 1 << 4;
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_KEY_FRAME: u32 = 1 << 0;
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_SHOW_FRAME: u32 = 1 << 1;
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_ERROR_RESILIENT: u32 = 1 << 2;
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_INTRA_ONLY: u32 = 1 << 3;
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_ALLOW_HIGH_PREC_MV: u32 = 1 << 4;
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_REFRESH_FRAME_CTX: u32 = 1 << 5;
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_PARALLEL_DEC_MODE: u32 = 1 << 6;
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_X_SUBSAMPLING: u32 = 1 << 7;
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_Y_SUBSAMPLING: u32 = 1 << 8;
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_COLOR_RANGE_FULL_SWING: u32 = 1 << 9;
pub const SCARLET_VIDEO_VP9_SIGN_BIAS_LAST: u8 = 1 << 0;
pub const SCARLET_VIDEO_VP9_SIGN_BIAS_GOLDEN: u8 = 1 << 1;
pub const SCARLET_VIDEO_VP9_SIGN_BIAS_ALT: u8 = 1 << 2;
pub const SCARLET_VIDEO_VP9_RESET_FRAME_CTX_NONE: u8 = 0;
pub const SCARLET_VIDEO_VP9_RESET_FRAME_CTX_SPEC: u8 = 1;
pub const SCARLET_VIDEO_VP9_RESET_FRAME_CTX_ALL: u8 = 2;
pub const SCARLET_VIDEO_VP9_INTERP_FILTER_EIGHTTAP: u8 = 0;
pub const SCARLET_VIDEO_VP9_INTERP_FILTER_EIGHTTAP_SMOOTH: u8 = 1;
pub const SCARLET_VIDEO_VP9_INTERP_FILTER_EIGHTTAP_SHARP: u8 = 2;
pub const SCARLET_VIDEO_VP9_INTERP_FILTER_BILINEAR: u8 = 3;
pub const SCARLET_VIDEO_VP9_INTERP_FILTER_SWITCHABLE: u8 = 4;
pub const SCARLET_VIDEO_VP9_REFERENCE_MODE_SINGLE_REFERENCE: u8 = 0;
pub const SCARLET_VIDEO_VP9_REFERENCE_MODE_COMPOUND_REFERENCE: u8 = 1;
pub const SCARLET_VIDEO_VP9_REFERENCE_MODE_SELECT: u8 = 2;
pub const SCARLET_VIDEO_VP9_TX_MODE_ONLY_4X4: u8 = 0;
pub const SCARLET_VIDEO_VP9_TX_MODE_ALLOW_8X8: u8 = 1;
pub const SCARLET_VIDEO_VP9_TX_MODE_ALLOW_16X16: u8 = 2;
pub const SCARLET_VIDEO_VP9_TX_MODE_ALLOW_32X32: u8 = 3;
pub const SCARLET_VIDEO_VP9_TX_MODE_SELECT: u8 = 4;
pub const SCARLET_VIDEO_VP9_PROBABILITY_BYTES: usize = 0x774;
pub const SCARLET_VIDEO_VP9_MAX_TILES: usize = 256;

const VP9_SYNC_CODE: u32 = 0x49_83_42;
const VP9_REFS_PER_FRAME: usize = 3;
const VP9_NUM_REF_FRAMES: usize = 8;
const VP9_MIN_TILE_WIDTH_B64: u32 = 4;
const VP9_MAX_TILE_WIDTH_B64: u32 = 64;
const VP9_COEF_PROBS_PER_TX: usize = 396;
const VP9_COEF_PROBS_PER_PLANE: usize = 198;
const VP9_COEF_PROBS_PER_REF: usize = 99;
const LOTS_OF_BITS: i32 = 0x4000_0000;
const SEG_FEATURE_Q: u8 = 1 << 0;
const SEG_FEATURE_LF: u8 = 1 << 1;
const SEG_FEATURE_REF: u8 = 1 << 2;
const SEG_FEATURE_SKIP: u8 = 1 << 3;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ScarletVideoVp9LoopFilter {
    pub ref_deltas: [i8; 4],
    pub mode_deltas: [i8; 2],
    pub level: u8,
    pub sharpness: u8,
    pub flags: u8,
    pub reserved: [u8; 7],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ScarletVideoVp9Quantization {
    pub base_q_idx: u8,
    pub delta_q_y_dc: i8,
    pub delta_q_uv_dc: i8,
    pub delta_q_uv_ac: i8,
    pub reserved: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ScarletVideoVp9Segmentation {
    pub feature_data: [[i16; 4]; 8],
    pub feature_enabled: [u8; 8],
    pub tree_probs: [u8; 7],
    pub pred_probs: [u8; 3],
    pub flags: u8,
    pub reserved: [u8; 5],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ScarletVideoVp9FrameParams {
    pub loop_filter: ScarletVideoVp9LoopFilter,
    pub quantization: ScarletVideoVp9Quantization,
    pub segmentation: ScarletVideoVp9Segmentation,
    pub flags: u32,
    pub compressed_header_size: u16,
    pub uncompressed_header_size: u16,
    pub frame_width_minus_1: u16,
    pub frame_height_minus_1: u16,
    pub render_width_minus_1: u16,
    pub render_height_minus_1: u16,
    pub last_frame_ts: u64,
    pub golden_frame_ts: u64,
    pub alt_frame_ts: u64,
    pub ref_frame_sign_bias: u8,
    pub reset_frame_context: u8,
    pub frame_context_idx: u8,
    pub profile: u8,
    pub bit_depth: u8,
    pub interpolation_filter: u8,
    pub tile_cols_log2: u8,
    pub tile_rows_log2: u8,
    pub reference_mode: u8,
    pub refresh_frame_flags: u8,
    pub show_existing_frame_index: u8,
    pub tx_mode: u8,
    pub reserved: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ScarletVideoVp9Tile {
    pub row: u16,
    pub col: u16,
    pub offset: u32,
    pub size: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScarletVideoVp9Tiles {
    pub tile_count: u32,
    pub reserved: u32,
    pub tiles: [ScarletVideoVp9Tile; SCARLET_VIDEO_VP9_MAX_TILES],
}

impl Default for ScarletVideoVp9Tiles {
    fn default() -> Self {
        Self {
            tile_count: 0,
            reserved: 0,
            tiles: [ScarletVideoVp9Tile::default(); SCARLET_VIDEO_VP9_MAX_TILES],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScarletVideoVp9Probabilities {
    pub data: [u8; SCARLET_VIDEO_VP9_PROBABILITY_BYTES],
}

impl Default for ScarletVideoVp9Probabilities {
    fn default() -> Self {
        Self {
            data: [0; SCARLET_VIDEO_VP9_PROBABILITY_BYTES],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ScarletVideoVp9StatelessParams {
    pub frame: ScarletVideoVp9FrameParams,
    pub probabilities: ScarletVideoVp9Probabilities,
    pub tiles: ScarletVideoVp9Tiles,
}

pub struct Vp9PreparedFrame {
    pub params: ScarletVideoVp9StatelessParams,
    pub timestamp: u64,
    pub is_keyframe: bool,
}

#[derive(Clone, Copy, Default)]
struct Vp9ReferenceFrame {
    timestamp: u64,
    width: u16,
    height: u16,
    bit_depth: u8,
    subsampling_x: bool,
    subsampling_y: bool,
}

#[derive(Clone, Copy)]
struct Vp9MvComponent {
    sign: u8,
    classes: [u8; 10],
    class0: u8,
    bits: [u8; 10],
    class0_fp: [[u8; 3]; 2],
    fp: [u8; 3],
    class0_hp: u8,
    hp: u8,
}

#[derive(Clone, Copy)]
struct Vp9ProbContext {
    y_mode: [u8; 36],
    uv_mode: [u8; 90],
    partition: [u8; 48],
    switchable_interp: [u8; 8],
    inter_mode: [u8; 21],
    intra_inter: [u8; 4],
    comp_inter: [u8; 5],
    single_ref: [u8; 10],
    comp_ref: [u8; 5],
    tx_32: [u8; 6],
    tx_16: [u8; 4],
    tx_8: [u8; 2],
    skip: [u8; 3],
    mv_joint: [u8; 3],
    mv_comp: [Vp9MvComponent; 2],
    coef: [u8; 1584],
}

impl Vp9ProbContext {
    fn pack_avd(self, intra_only: bool) -> ScarletVideoVp9Probabilities {
        let mut probabilities = ScarletVideoVp9Probabilities::default();
        let mut cursor = 10usize;
        push_bytes(&mut probabilities.data, &mut cursor, &self.tx_8);
        push_bytes(&mut probabilities.data, &mut cursor, &self.tx_16);
        push_bytes(&mut probabilities.data, &mut cursor, &self.tx_32);
        push_bytes(&mut probabilities.data, &mut cursor, &self.coef);
        push_bytes(&mut probabilities.data, &mut cursor, &self.skip);
        push_bytes(&mut probabilities.data, &mut cursor, &self.inter_mode);
        push_bytes(
            &mut probabilities.data,
            &mut cursor,
            &self.switchable_interp,
        );
        push_bytes(&mut probabilities.data, &mut cursor, &self.intra_inter);
        push_bytes(&mut probabilities.data, &mut cursor, &self.comp_inter);
        push_bytes(&mut probabilities.data, &mut cursor, &self.single_ref);
        push_bytes(&mut probabilities.data, &mut cursor, &self.comp_ref);
        push_bytes(&mut probabilities.data, &mut cursor, &self.y_mode);
        if intra_only {
            push_bytes(
                &mut probabilities.data,
                &mut cursor,
                &KEYFRAME_UV_MODE_PROBS,
            );
            push_bytes(
                &mut probabilities.data,
                &mut cursor,
                &KEYFRAME_PARTITION_PROBS,
            );
        } else {
            push_bytes(&mut probabilities.data, &mut cursor, &self.uv_mode);
            push_bytes(&mut probabilities.data, &mut cursor, &self.partition);
        }
        push_bytes(&mut probabilities.data, &mut cursor, &self.mv_joint);
        for component in self.mv_comp {
            push_byte(&mut probabilities.data, &mut cursor, component.sign);
            push_bytes(&mut probabilities.data, &mut cursor, &component.classes);
            push_byte(&mut probabilities.data, &mut cursor, component.class0);
            push_bytes(&mut probabilities.data, &mut cursor, &component.bits);
        }
        for component in self.mv_comp {
            for class0_fp in component.class0_fp {
                push_bytes(&mut probabilities.data, &mut cursor, &class0_fp);
            }
            push_bytes(&mut probabilities.data, &mut cursor, &component.fp);
        }
        for component in self.mv_comp {
            push_byte(&mut probabilities.data, &mut cursor, component.class0_hp);
            push_byte(&mut probabilities.data, &mut cursor, component.hp);
        }
        probabilities
    }
}

impl Default for Vp9ProbContext {
    fn default() -> Self {
        Self {
            y_mode: DEFAULT_Y_MODE_PROBS,
            uv_mode: DEFAULT_UV_MODE_PROBS,
            partition: DEFAULT_PARTITION_PROBS,
            switchable_interp: DEFAULT_SWITCHABLE_INTERP_PROBS,
            inter_mode: DEFAULT_INTER_MODE_PROBS,
            intra_inter: DEFAULT_INTRA_INTER_PROBS,
            comp_inter: DEFAULT_COMP_INTER_PROBS,
            single_ref: DEFAULT_SINGLE_REF_PROBS,
            comp_ref: DEFAULT_COMP_REF_PROBS,
            tx_32: DEFAULT_TX_32_PROBS,
            tx_16: DEFAULT_TX_16_PROBS,
            tx_8: DEFAULT_TX_8_PROBS,
            skip: DEFAULT_SKIP_PROBS,
            mv_joint: DEFAULT_MV_JOINT_PROBS,
            mv_comp: [
                Vp9MvComponent {
                    sign: DEFAULT_MV_SIGN_PROBS[0],
                    classes: DEFAULT_MV_CLASS_PROBS[0],
                    class0: DEFAULT_MV_CLASS0_PROBS[0],
                    bits: DEFAULT_MV_BITS_PROBS,
                    class0_fp: DEFAULT_MV_CLASS0_FP_PROBS,
                    fp: DEFAULT_MV_FP_PROBS,
                    class0_hp: DEFAULT_MV_CLASS0_HP_PROB,
                    hp: DEFAULT_MV_HP_PROB,
                },
                Vp9MvComponent {
                    sign: DEFAULT_MV_SIGN_PROBS[1],
                    classes: DEFAULT_MV_CLASS_PROBS[1],
                    class0: DEFAULT_MV_CLASS0_PROBS[1],
                    bits: DEFAULT_MV_BITS_PROBS,
                    class0_fp: DEFAULT_MV_CLASS0_FP_PROBS,
                    fp: DEFAULT_MV_FP_PROBS,
                    class0_hp: DEFAULT_MV_CLASS0_HP_PROB,
                    hp: DEFAULT_MV_HP_PROB,
                },
            ],
            coef: DEFAULT_COEF_PROBS,
        }
    }
}

#[derive(Clone, Copy)]
struct ParsedVp9Frame {
    frame: ScarletVideoVp9FrameParams,
    current_reference: Vp9ReferenceFrame,
    intra_probability_tables: bool,
    lossless: bool,
    allow_comp_inter: bool,
}

pub struct Vp9RequestContext {
    refs: [Option<Vp9ReferenceFrame>; VP9_NUM_REF_FRAMES],
    frame_contexts: [Vp9ProbContext; 4],
    loop_filter_ref_deltas: [i8; 4],
    loop_filter_mode_deltas: [i8; 2],
    segmentation: ScarletVideoVp9Segmentation,
    next_timestamp: u64,
}

impl Default for Vp9RequestContext {
    fn default() -> Self {
        Self {
            refs: [None; VP9_NUM_REF_FRAMES],
            frame_contexts: [Vp9ProbContext::default(); 4],
            loop_filter_ref_deltas: [1, 0, -1, -1],
            loop_filter_mode_deltas: [0, 0],
            segmentation: ScarletVideoVp9Segmentation::default(),
            next_timestamp: 0,
        }
    }
}

impl Vp9RequestContext {
    pub fn params_for_frame(&mut self, data: &[u8]) -> Result<Vp9PreparedFrame, String> {
        if data.is_empty() {
            return Err(String::from("VP9 frame is empty"));
        }
        let mut parsed = self.parse_uncompressed_header(data)?;
        let context_index = usize::from(parsed.frame.frame_context_idx.min(3));
        let mut probabilities = self.frame_contexts[context_index];
        self.parse_compressed_header(data, &mut parsed, &mut probabilities)?;
        let tiles = parse_tiles(data, &parsed.frame)?;
        let timestamp = self.next_submit_timestamp();
        let params = ScarletVideoVp9StatelessParams {
            frame: parsed.frame,
            probabilities: probabilities.pack_avd(parsed.intra_probability_tables),
            tiles,
        };

        if params.frame.flags & SCARLET_VIDEO_VP9_FRAME_FLAG_REFRESH_FRAME_CTX != 0
            && params.frame.flags & SCARLET_VIDEO_VP9_FRAME_FLAG_PARALLEL_DEC_MODE != 0
        {
            self.frame_contexts[context_index] = probabilities;
        }
        self.update_references(&params.frame, parsed.current_reference, timestamp);
        Ok(Vp9PreparedFrame {
            params,
            timestamp,
            is_keyframe: parsed.intra_probability_tables,
        })
    }

    fn parse_uncompressed_header(&mut self, data: &[u8]) -> Result<ParsedVp9Frame, String> {
        let mut reader = Vp9BitReader::new(data);
        if reader.read_bits(2)? != 0x2 {
            return Err(String::from("VP9 frame marker is invalid"));
        }

        let mut profile = reader.read_bit()? as u8;
        profile |= (reader.read_bit()? as u8) << 1;
        if profile == 3 {
            profile += reader.read_bit()? as u8;
        }
        if profile > 3 {
            return Err(String::from("VP9 profile is invalid"));
        }

        if reader.read_bool()? {
            let _show_existing_frame_index = reader.read_bits_u8(3)?;
            return Err(String::from(
                "VP9 show-existing-frame is not supported by the stateless path yet",
            ));
        }

        let key_frame = !reader.read_bool()?;
        let show_frame = reader.read_bool()?;
        let error_resilient = reader.read_bool()?;
        let mut flags = 0u32;
        if key_frame {
            flags |= SCARLET_VIDEO_VP9_FRAME_FLAG_KEY_FRAME;
        }
        if show_frame {
            flags |= SCARLET_VIDEO_VP9_FRAME_FLAG_SHOW_FRAME;
        }
        if error_resilient {
            flags |= SCARLET_VIDEO_VP9_FRAME_FLAG_ERROR_RESILIENT;
        }

        let mut width = 0u16;
        let mut height = 0u16;
        let render_width;
        let render_height;
        let mut bit_depth = 8u8;
        let mut subsampling_x = true;
        let mut subsampling_y = true;
        let mut color_range_full = false;
        let refresh_frame_flags;
        let mut reset_frame_context_raw = 0u8;
        let mut ref_indices = [0u8; VP9_REFS_PER_FRAME];
        let mut ref_frame_sign_bias = 0u8;
        let mut allow_comp_inter = false;
        let mut interpolation_filter = SCARLET_VIDEO_VP9_INTERP_FILTER_EIGHTTAP;
        let mut intra_only = false;

        if key_frame {
            read_sync_code(&mut reader)?;
            let color = read_color_config(&mut reader, profile)?;
            bit_depth = color.bit_depth;
            subsampling_x = color.subsampling_x;
            subsampling_y = color.subsampling_y;
            color_range_full = color.full_range;
            refresh_frame_flags = 0xff;
            let size = read_frame_size(&mut reader)?;
            width = size.0;
            height = size.1;
            let render_size = read_render_size(&mut reader, width, height)?;
            render_width = render_size.0;
            render_height = render_size.1;
        } else {
            intra_only = if !show_frame {
                reader.read_bool()?
            } else {
                false
            };
            if intra_only {
                flags |= SCARLET_VIDEO_VP9_FRAME_FLAG_INTRA_ONLY;
            }
            reset_frame_context_raw = if error_resilient {
                0
            } else {
                reader.read_bits_u8(2)?
            };
            if intra_only {
                read_sync_code(&mut reader)?;
                if profile >= 1 {
                    let color = read_color_config(&mut reader, profile)?;
                    bit_depth = color.bit_depth;
                    subsampling_x = color.subsampling_x;
                    subsampling_y = color.subsampling_y;
                    color_range_full = color.full_range;
                }
                refresh_frame_flags = reader.read_bits_u8(8)?;
                let size = read_frame_size(&mut reader)?;
                width = size.0;
                height = size.1;
                let render_size = read_render_size(&mut reader, width, height)?;
                render_width = render_size.0;
                render_height = render_size.1;
            } else {
                refresh_frame_flags = reader.read_bits_u8(8)?;
                for index in 0..VP9_REFS_PER_FRAME {
                    let ref_index = reader.read_bits_u8(3)?;
                    ref_indices[index] = ref_index;
                    if reader.read_bool()? && !error_resilient {
                        ref_frame_sign_bias |= 1 << index;
                    }
                }

                let mut copied_size_from_ref = false;
                for ref_index in ref_indices {
                    if reader.read_bool()? {
                        let reference = self.refs[usize::from(ref_index)]
                            .ok_or_else(|| String::from("VP9 frame references an empty slot"))?;
                        width = reference.width;
                        height = reference.height;
                        bit_depth = reference.bit_depth;
                        subsampling_x = reference.subsampling_x;
                        subsampling_y = reference.subsampling_y;
                        copied_size_from_ref = true;
                        break;
                    }
                }
                if !copied_size_from_ref {
                    let size = read_frame_size(&mut reader)?;
                    width = size.0;
                    height = size.1;
                }
                let render_size = read_render_size(&mut reader, width, height)?;
                render_width = render_size.0;
                render_height = render_size.1;

                if reader.read_bool()? {
                    flags |= SCARLET_VIDEO_VP9_FRAME_FLAG_ALLOW_HIGH_PREC_MV;
                }
                interpolation_filter = if reader.read_bool()? {
                    SCARLET_VIDEO_VP9_INTERP_FILTER_SWITCHABLE
                } else {
                    match reader.read_bits_u8(2)? {
                        0 => SCARLET_VIDEO_VP9_INTERP_FILTER_EIGHTTAP,
                        1 => SCARLET_VIDEO_VP9_INTERP_FILTER_EIGHTTAP_SMOOTH,
                        2 => SCARLET_VIDEO_VP9_INTERP_FILTER_EIGHTTAP_SHARP,
                        _ => SCARLET_VIDEO_VP9_INTERP_FILTER_BILINEAR,
                    }
                };
                allow_comp_inter = sign_bias(ref_frame_sign_bias, 0)
                    != sign_bias(ref_frame_sign_bias, 1)
                    || sign_bias(ref_frame_sign_bias, 0) != sign_bias(ref_frame_sign_bias, 2);
            }
        }

        let refresh_frame_context = if error_resilient {
            false
        } else {
            reader.read_bool()?
        };
        let parallel_dec_mode = if error_resilient {
            true
        } else {
            reader.read_bool()?
        };
        let frame_context_idx = reader.read_bits_u8(2)?;
        let frame_context_idx = if key_frame || intra_only {
            0
        } else {
            frame_context_idx
        };
        if refresh_frame_context {
            flags |= SCARLET_VIDEO_VP9_FRAME_FLAG_REFRESH_FRAME_CTX;
        }
        if parallel_dec_mode {
            flags |= SCARLET_VIDEO_VP9_FRAME_FLAG_PARALLEL_DEC_MODE;
        }
        if subsampling_x {
            flags |= SCARLET_VIDEO_VP9_FRAME_FLAG_X_SUBSAMPLING;
        }
        if subsampling_y {
            flags |= SCARLET_VIDEO_VP9_FRAME_FLAG_Y_SUBSAMPLING;
        }
        if color_range_full {
            flags |= SCARLET_VIDEO_VP9_FRAME_FLAG_COLOR_RANGE_FULL_SWING;
        }

        self.setup_past_independence(flags, reset_frame_context_raw, frame_context_idx);
        let loop_filter = self.read_loop_filter(&mut reader)?;
        let quantization = self.read_quantization(&mut reader)?;
        let lossless = quantization.base_q_idx == 0
            && quantization.delta_q_y_dc == 0
            && quantization.delta_q_uv_dc == 0
            && quantization.delta_q_uv_ac == 0;
        let segmentation = self.read_segmentation(&mut reader)?;
        let cols = (u32::from(width) + 7) >> 3;
        let sb_cols = (cols + 7) >> 3;
        let tile_cols_log2 = read_tile_cols_log2(&mut reader, sb_cols)?;
        let tile_rows_log2 = read_increment(&mut reader, 0, 2)?;
        let compressed_header_size = reader.read_bits_u16(16)?;
        reader.skip_to_byte()?;
        let uncompressed_header_size = u16::try_from(reader.byte_offset())
            .map_err(|_| String::from("VP9 uncompressed header is too large"))?;

        let mut frame = ScarletVideoVp9FrameParams {
            loop_filter,
            quantization,
            segmentation,
            flags,
            compressed_header_size,
            uncompressed_header_size,
            frame_width_minus_1: width.saturating_sub(1),
            frame_height_minus_1: height.saturating_sub(1),
            render_width_minus_1: render_width.saturating_sub(1),
            render_height_minus_1: render_height.saturating_sub(1),
            ref_frame_sign_bias,
            reset_frame_context: reset_frame_context_abi(reset_frame_context_raw),
            frame_context_idx,
            profile,
            bit_depth,
            interpolation_filter,
            tile_cols_log2,
            tile_rows_log2,
            reference_mode: SCARLET_VIDEO_VP9_REFERENCE_MODE_SINGLE_REFERENCE,
            refresh_frame_flags,
            ..Default::default()
        };

        if !key_frame && !intra_only {
            frame.last_frame_ts = self.reference_timestamp(ref_indices[0])?;
            frame.golden_frame_ts = self.reference_timestamp(ref_indices[1])?;
            frame.alt_frame_ts = self.reference_timestamp(ref_indices[2])?;
        }

        Ok(ParsedVp9Frame {
            frame,
            current_reference: Vp9ReferenceFrame {
                timestamp: 0,
                width,
                height,
                bit_depth,
                subsampling_x,
                subsampling_y,
            },
            intra_probability_tables: key_frame || intra_only,
            lossless,
            allow_comp_inter,
        })
    }

    fn parse_compressed_header(
        &self,
        data: &[u8],
        parsed: &mut ParsedVp9Frame,
        probabilities: &mut Vp9ProbContext,
    ) -> Result<(), String> {
        let start = usize::from(parsed.frame.uncompressed_header_size);
        let end = start
            .checked_add(usize::from(parsed.frame.compressed_header_size))
            .ok_or_else(|| String::from("VP9 compressed header size overflow"))?;
        let compressed = data
            .get(start..end)
            .ok_or_else(|| String::from("VP9 compressed header is truncated"))?;
        let mut reader = Vp9BoolReader::new(compressed)?;

        if parsed.lossless {
            parsed.frame.tx_mode = SCARLET_VIDEO_VP9_TX_MODE_ONLY_4X4;
        } else {
            let mut tx_mode = reader.read_literal(2)? as u8;
            if tx_mode == SCARLET_VIDEO_VP9_TX_MODE_ALLOW_32X32 {
                tx_mode += reader.read_bit()? as u8;
            }
            parsed.frame.tx_mode = tx_mode;
            if tx_mode == SCARLET_VIDEO_VP9_TX_MODE_SELECT {
                for value in &mut probabilities.tx_8 {
                    diff_update_prob(&mut reader, value)?;
                }
                for value in &mut probabilities.tx_16 {
                    diff_update_prob(&mut reader, value)?;
                }
                for value in &mut probabilities.tx_32 {
                    diff_update_prob(&mut reader, value)?;
                }
            }
        }

        read_coef_probs(probabilities, parsed.frame.tx_mode, &mut reader)?;
        for value in &mut probabilities.skip {
            diff_update_prob(&mut reader, value)?;
        }

        let inter_frame = parsed.frame.flags & SCARLET_VIDEO_VP9_FRAME_FLAG_KEY_FRAME == 0
            && parsed.frame.flags & SCARLET_VIDEO_VP9_FRAME_FLAG_INTRA_ONLY == 0;
        if inter_frame {
            for value in &mut probabilities.inter_mode {
                diff_update_prob(&mut reader, value)?;
            }
            if parsed.frame.interpolation_filter == SCARLET_VIDEO_VP9_INTERP_FILTER_SWITCHABLE {
                for value in &mut probabilities.switchable_interp {
                    diff_update_prob(&mut reader, value)?;
                }
            }
            for value in &mut probabilities.intra_inter {
                diff_update_prob(&mut reader, value)?;
            }

            if parsed.allow_comp_inter {
                let mut reference_mode = reader.read_bit()? as u8;
                if reference_mode != 0 {
                    reference_mode += reader.read_bit()? as u8;
                }
                parsed.frame.reference_mode = reference_mode;
                if reference_mode == SCARLET_VIDEO_VP9_REFERENCE_MODE_SELECT {
                    for value in &mut probabilities.comp_inter {
                        diff_update_prob(&mut reader, value)?;
                    }
                }
            }

            if parsed.frame.reference_mode != SCARLET_VIDEO_VP9_REFERENCE_MODE_COMPOUND_REFERENCE {
                for value in &mut probabilities.single_ref {
                    diff_update_prob(&mut reader, value)?;
                }
            }
            if parsed.frame.reference_mode != SCARLET_VIDEO_VP9_REFERENCE_MODE_SINGLE_REFERENCE {
                for value in &mut probabilities.comp_ref {
                    diff_update_prob(&mut reader, value)?;
                }
            }
            for value in &mut probabilities.y_mode {
                diff_update_prob(&mut reader, value)?;
            }
            for value in &mut probabilities.partition {
                diff_update_prob(&mut reader, value)?;
            }
            for value in &mut probabilities.mv_joint {
                update_mv_prob(&mut reader, value)?;
            }
            for component in &mut probabilities.mv_comp {
                update_mv_prob(&mut reader, &mut component.sign)?;
                for value in &mut component.classes {
                    update_mv_prob(&mut reader, value)?;
                }
                update_mv_prob(&mut reader, &mut component.class0)?;
                for value in &mut component.bits {
                    update_mv_prob(&mut reader, value)?;
                }
            }
            for component in &mut probabilities.mv_comp {
                for class0_fp in &mut component.class0_fp {
                    for value in class0_fp {
                        update_mv_prob(&mut reader, value)?;
                    }
                }
                for value in &mut component.fp {
                    update_mv_prob(&mut reader, value)?;
                }
            }
            if parsed.frame.flags & SCARLET_VIDEO_VP9_FRAME_FLAG_ALLOW_HIGH_PREC_MV != 0 {
                for component in &mut probabilities.mv_comp {
                    update_mv_prob(&mut reader, &mut component.class0_hp)?;
                    update_mv_prob(&mut reader, &mut component.hp)?;
                }
            }
        }

        if reader.has_error() {
            return Err(String::from("VP9 compressed header reads past end"));
        }
        Ok(())
    }

    fn setup_past_independence(
        &mut self,
        frame_flags: u32,
        reset_frame_context_raw: u8,
        frame_context_idx: u8,
    ) {
        let key_frame = frame_flags & SCARLET_VIDEO_VP9_FRAME_FLAG_KEY_FRAME != 0;
        let intra_only = frame_flags & SCARLET_VIDEO_VP9_FRAME_FLAG_INTRA_ONLY != 0;
        let error_resilient = frame_flags & SCARLET_VIDEO_VP9_FRAME_FLAG_ERROR_RESILIENT != 0;
        if key_frame || error_resilient || intra_only {
            self.loop_filter_ref_deltas = [1, 0, -1, -1];
            self.loop_filter_mode_deltas = [0, 0];
            self.segmentation = ScarletVideoVp9Segmentation::default();
        }
        let default_context = Vp9ProbContext::default();
        if key_frame || error_resilient || (intra_only && reset_frame_context_raw == 3) {
            self.frame_contexts = [default_context; 4];
        } else if reset_frame_context_raw == 2 {
            self.frame_contexts[usize::from(frame_context_idx.min(3))] = default_context;
        }
    }

    fn read_loop_filter(
        &mut self,
        reader: &mut Vp9BitReader<'_>,
    ) -> Result<ScarletVideoVp9LoopFilter, String> {
        let level = reader.read_bits_u8(6)?;
        let sharpness = reader.read_bits_u8(3)?;
        let mut flags = 0u8;
        if reader.read_bool()? {
            flags |= SCARLET_VIDEO_VP9_LOOP_FILTER_FLAG_DELTA_ENABLED;
            if reader.read_bool()? {
                flags |= SCARLET_VIDEO_VP9_LOOP_FILTER_FLAG_DELTA_UPDATE;
                for index in 0..4 {
                    if reader.read_bool()? {
                        self.loop_filter_ref_deltas[index] = reader.read_signed_inverted(6)? as i8;
                    }
                }
                for index in 0..2 {
                    if reader.read_bool()? {
                        self.loop_filter_mode_deltas[index] = reader.read_signed_inverted(6)? as i8;
                    }
                }
            }
        }
        Ok(ScarletVideoVp9LoopFilter {
            ref_deltas: self.loop_filter_ref_deltas,
            mode_deltas: self.loop_filter_mode_deltas,
            level,
            sharpness,
            flags,
            reserved: [0; 7],
        })
    }

    fn read_quantization(
        &mut self,
        reader: &mut Vp9BitReader<'_>,
    ) -> Result<ScarletVideoVp9Quantization, String> {
        Ok(ScarletVideoVp9Quantization {
            base_q_idx: reader.read_bits_u8(8)?,
            delta_q_y_dc: read_optional_delta(reader, 4)?,
            delta_q_uv_dc: read_optional_delta(reader, 4)?,
            delta_q_uv_ac: read_optional_delta(reader, 4)?,
            reserved: [0; 4],
        })
    }

    fn read_segmentation(
        &mut self,
        reader: &mut Vp9BitReader<'_>,
    ) -> Result<ScarletVideoVp9Segmentation, String> {
        let mut segmentation = self.segmentation;
        segmentation.flags = 0;
        if !reader.read_bool()? {
            self.segmentation = segmentation;
            return Ok(segmentation);
        }
        segmentation.flags |= SCARLET_VIDEO_VP9_SEGMENTATION_FLAG_ENABLED;
        if reader.read_bool()? {
            segmentation.flags |= SCARLET_VIDEO_VP9_SEGMENTATION_FLAG_UPDATE_MAP;
            for value in &mut segmentation.tree_probs {
                *value = if reader.read_bool()? {
                    reader.read_bits_u8(8)?
                } else {
                    255
                };
            }
            if reader.read_bool()? {
                segmentation.flags |= SCARLET_VIDEO_VP9_SEGMENTATION_FLAG_TEMPORAL_UPDATE;
                for value in &mut segmentation.pred_probs {
                    *value = if reader.read_bool()? {
                        reader.read_bits_u8(8)?
                    } else {
                        255
                    };
                }
            }
        }
        if reader.read_bool()? {
            segmentation.flags |= SCARLET_VIDEO_VP9_SEGMENTATION_FLAG_UPDATE_DATA;
            if reader.read_bool()? {
                segmentation.flags |= SCARLET_VIDEO_VP9_SEGMENTATION_FLAG_ABS_OR_DELTA_UPDATE;
            }
            segmentation.feature_data = [[0; 4]; 8];
            segmentation.feature_enabled = [0; 8];
            for index in 0..8 {
                if reader.read_bool()? {
                    segmentation.feature_enabled[index] |= SEG_FEATURE_Q;
                    segmentation.feature_data[index][0] = reader.read_signed_inverted(8)?;
                }
                if reader.read_bool()? {
                    segmentation.feature_enabled[index] |= SEG_FEATURE_LF;
                    segmentation.feature_data[index][1] = reader.read_signed_inverted(6)?;
                }
                if reader.read_bool()? {
                    segmentation.feature_enabled[index] |= SEG_FEATURE_REF;
                    segmentation.feature_data[index][2] = reader.read_bits(2)? as i16;
                }
                if reader.read_bool()? {
                    segmentation.feature_enabled[index] |= SEG_FEATURE_SKIP;
                    segmentation.feature_data[index][3] = 1;
                }
            }
        }
        self.segmentation = segmentation;
        Ok(segmentation)
    }

    fn reference_timestamp(&self, ref_index: u8) -> Result<u64, String> {
        self.refs[usize::from(ref_index)]
            .map(|reference| reference.timestamp)
            .ok_or_else(|| String::from("VP9 frame references an empty slot"))
    }

    fn update_references(
        &mut self,
        frame: &ScarletVideoVp9FrameParams,
        mut reference: Vp9ReferenceFrame,
        timestamp: u64,
    ) {
        reference.timestamp = timestamp;
        for index in 0..VP9_NUM_REF_FRAMES {
            if frame.refresh_frame_flags & (1 << index) != 0 {
                self.refs[index] = Some(reference);
            }
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
}

struct Vp9ColorConfig {
    bit_depth: u8,
    subsampling_x: bool,
    subsampling_y: bool,
    full_range: bool,
}

fn read_color_config(reader: &mut Vp9BitReader<'_>, profile: u8) -> Result<Vp9ColorConfig, String> {
    let bit_depth = if profile <= 1 {
        8
    } else if reader.read_bool()? {
        12
    } else {
        10
    };
    let color_space = reader.read_bits_u8(3)?;
    if color_space == 7 {
        return Err(String::from("VP9 RGB color space is not supported"));
    }
    let full_range = reader.read_bool()?;
    let (subsampling_x, subsampling_y) = if profile & 1 != 0 {
        let subsampling_x = reader.read_bool()?;
        let subsampling_y = reader.read_bool()?;
        if reader.read_bool()? {
            return Err(String::from("VP9 color config reserved bit is set"));
        }
        (subsampling_x, subsampling_y)
    } else {
        (true, true)
    };
    Ok(Vp9ColorConfig {
        bit_depth,
        subsampling_x,
        subsampling_y,
        full_range,
    })
}

fn read_sync_code(reader: &mut Vp9BitReader<'_>) -> Result<(), String> {
    if reader.read_bits(24)? != VP9_SYNC_CODE {
        return Err(String::from("VP9 sync code is invalid"));
    }
    Ok(())
}

fn read_frame_size(reader: &mut Vp9BitReader<'_>) -> Result<(u16, u16), String> {
    Ok((
        reader.read_bits_u16(16)?.saturating_add(1),
        reader.read_bits_u16(16)?.saturating_add(1),
    ))
}

fn read_render_size(
    reader: &mut Vp9BitReader<'_>,
    width: u16,
    height: u16,
) -> Result<(u16, u16), String> {
    if reader.read_bool()? {
        read_frame_size(reader)
    } else {
        Ok((width, height))
    }
}

fn read_optional_delta(reader: &mut Vp9BitReader<'_>, bits: usize) -> Result<i8, String> {
    if reader.read_bool()? {
        Ok(reader.read_signed_inverted(bits)? as i8)
    } else {
        Ok(0)
    }
}

fn read_tile_cols_log2(reader: &mut Vp9BitReader<'_>, sb_cols: u32) -> Result<u8, String> {
    let mut min_log2 = 0;
    while (VP9_MAX_TILE_WIDTH_B64 << min_log2) < sb_cols {
        min_log2 += 1;
    }
    let mut max_log2 = 0;
    while (sb_cols >> (max_log2 + 1)) >= VP9_MIN_TILE_WIDTH_B64 {
        max_log2 += 1;
    }
    read_increment(reader, min_log2, max_log2)
}

fn read_increment(reader: &mut Vp9BitReader<'_>, minimum: u32, maximum: u32) -> Result<u8, String> {
    let mut value = minimum;
    while value < maximum {
        if reader.read_bool()? {
            value += 1;
        } else {
            break;
        }
    }
    u8::try_from(value).map_err(|_| String::from("VP9 increment value is too large"))
}

fn parse_tiles(
    data: &[u8],
    frame: &ScarletVideoVp9FrameParams,
) -> Result<ScarletVideoVp9Tiles, String> {
    let tile_cols = 1usize << frame.tile_cols_log2;
    let tile_rows = 1usize << frame.tile_rows_log2;
    let tile_count = tile_cols
        .checked_mul(tile_rows)
        .ok_or_else(|| String::from("VP9 tile count overflow"))?;
    if tile_count == 0 || tile_count > SCARLET_VIDEO_VP9_MAX_TILES {
        return Err(String::from("VP9 tile count is unsupported"));
    }

    let mut cursor = usize::from(frame.uncompressed_header_size)
        .checked_add(usize::from(frame.compressed_header_size))
        .ok_or_else(|| String::from("VP9 tile data offset overflow"))?;
    if cursor > data.len() {
        return Err(String::from("VP9 tile data is truncated"));
    }

    let mut tiles = ScarletVideoVp9Tiles {
        tile_count: tile_count as u32,
        ..Default::default()
    };
    for row in 0..tile_rows {
        for col in 0..tile_cols {
            let index = row * tile_cols + col;
            let size = if index + 1 == tile_count {
                data.len()
                    .checked_sub(cursor)
                    .ok_or_else(|| String::from("VP9 tile payload offset overflow"))?
            } else {
                let size_end = cursor
                    .checked_add(4)
                    .ok_or_else(|| String::from("VP9 tile size offset overflow"))?;
                let bytes = data
                    .get(cursor..size_end)
                    .ok_or_else(|| String::from("VP9 tile size table is truncated"))?;
                cursor = size_end;
                u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize
            };
            let end = cursor
                .checked_add(size)
                .ok_or_else(|| String::from("VP9 tile payload size overflow"))?;
            if size == 0 || end > data.len() {
                return Err(String::from("VP9 tile payload is truncated"));
            }
            tiles.tiles[index] = ScarletVideoVp9Tile {
                row: row as u16,
                col: col as u16,
                offset: cursor as u32,
                size: size as u32,
            };
            cursor = end;
        }
    }
    Ok(tiles)
}

fn read_coef_probs(
    probabilities: &mut Vp9ProbContext,
    tx_mode: u8,
    reader: &mut Vp9BoolReader<'_>,
) -> Result<(), String> {
    let max_tx_size = match tx_mode {
        SCARLET_VIDEO_VP9_TX_MODE_ONLY_4X4 => 0,
        SCARLET_VIDEO_VP9_TX_MODE_ALLOW_8X8 => 1,
        SCARLET_VIDEO_VP9_TX_MODE_ALLOW_16X16 => 2,
        _ => 3,
    };
    for tx_size in 0..=max_tx_size {
        if !reader.read_bit()? {
            continue;
        }
        for plane in 0..2 {
            for ref_type in 0..2 {
                for band in 0..6 {
                    let contexts = if band == 0 { 3 } else { 6 };
                    for context in 0..contexts {
                        for node in 0..3 {
                            let index = coef_index(tx_size, plane, ref_type, band, context, node);
                            diff_update_prob(reader, &mut probabilities.coef[index])?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn diff_update_prob(reader: &mut Vp9BoolReader<'_>, probability: &mut u8) -> Result<(), String> {
    if reader.read_bool(252)? {
        let delta = decode_term_subexp(reader)?;
        *probability = inv_remap_prob(delta, *probability);
    }
    Ok(())
}

fn update_mv_prob(reader: &mut Vp9BoolReader<'_>, probability: &mut u8) -> Result<(), String> {
    if reader.read_bool(252)? {
        *probability = ((reader.read_literal(7)? << 1) | 1) as u8;
    }
    Ok(())
}

fn decode_term_subexp(reader: &mut Vp9BoolReader<'_>) -> Result<i32, String> {
    if !reader.read_bit()? {
        return Ok(reader.read_literal(4)? as i32);
    }
    if !reader.read_bit()? {
        return Ok(reader.read_literal(4)? as i32 + 16);
    }
    if !reader.read_bit()? {
        return Ok(reader.read_literal(5)? as i32 + 32);
    }
    Ok(decode_uniform(reader)? + 64)
}

fn decode_uniform(reader: &mut Vp9BoolReader<'_>) -> Result<i32, String> {
    let value = reader.read_literal(7)? as i32;
    let split = 256 - 191;
    if value < split {
        Ok(value)
    } else {
        Ok((value << 1) - split + reader.read_bit()? as i32)
    }
}

fn inv_remap_prob(value: i32, old: u8) -> u8 {
    let value = INV_MAP_TABLE[value as usize] as i32;
    let center = i32::from(old) - 1;
    if (center << 1) <= 255 {
        (1 + inv_recenter_nonneg(value, center)) as u8
    } else {
        (255 - inv_recenter_nonneg(value, 255 - 1 - center)) as u8
    }
}

fn inv_recenter_nonneg(value: i32, center: i32) -> i32 {
    if value > 2 * center {
        value
    } else if value & 1 != 0 {
        center - ((value + 1) >> 1)
    } else {
        center + (value >> 1)
    }
}

fn coef_index(
    tx_size: usize,
    plane: usize,
    ref_type: usize,
    band: usize,
    context: usize,
    node: usize,
) -> usize {
    let band_offset = if band == 0 {
        context * 3
    } else {
        9 + (band - 1) * 18 + context * 3
    };
    tx_size * VP9_COEF_PROBS_PER_TX
        + plane * VP9_COEF_PROBS_PER_PLANE
        + ref_type * VP9_COEF_PROBS_PER_REF
        + band_offset
        + node
}

fn reset_frame_context_abi(value: u8) -> u8 {
    match value {
        2 => SCARLET_VIDEO_VP9_RESET_FRAME_CTX_SPEC,
        3 => SCARLET_VIDEO_VP9_RESET_FRAME_CTX_ALL,
        _ => SCARLET_VIDEO_VP9_RESET_FRAME_CTX_NONE,
    }
}

fn sign_bias(value: u8, index: usize) -> bool {
    value & (1 << index) != 0
}

fn push_byte(dst: &mut [u8], cursor: &mut usize, value: u8) {
    if let Some(slot) = dst.get_mut(*cursor) {
        *slot = value;
        *cursor += 1;
    }
}

fn push_bytes(dst: &mut [u8], cursor: &mut usize, src: &[u8]) {
    let end = *cursor + src.len();
    if let Some(range) = dst.get_mut(*cursor..end) {
        range.copy_from_slice(src);
        *cursor = end;
    }
}

struct Vp9BitReader<'a> {
    data: &'a [u8],
    byte_index: usize,
    bit_index: u8,
}

impl<'a> Vp9BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_index: 0,
            bit_index: 0,
        }
    }

    fn read_bool(&mut self) -> Result<bool, String> {
        Ok(self.read_bit()? != 0)
    }

    fn read_bit(&mut self) -> Result<u32, String> {
        let byte = *self
            .data
            .get(self.byte_index)
            .ok_or_else(|| String::from("VP9 bitstream ended inside header"))?;
        let bit = (byte >> (7 - self.bit_index)) & 1;
        self.bit_index += 1;
        if self.bit_index == 8 {
            self.bit_index = 0;
            self.byte_index += 1;
        }
        Ok(u32::from(bit))
    }

    fn read_bits(&mut self, bits: usize) -> Result<u32, String> {
        let mut value = 0u32;
        for _ in 0..bits {
            value = (value << 1) | self.read_bit()?;
        }
        Ok(value)
    }

    fn read_bits_u8(&mut self, bits: usize) -> Result<u8, String> {
        u8::try_from(self.read_bits(bits)?).map_err(|_| String::from("VP9 u8 field overflow"))
    }

    fn read_bits_u16(&mut self, bits: usize) -> Result<u16, String> {
        u16::try_from(self.read_bits(bits)?).map_err(|_| String::from("VP9 u16 field overflow"))
    }

    fn read_signed_inverted(&mut self, bits: usize) -> Result<i16, String> {
        let value = self.read_bits(bits)? as i16;
        if self.read_bool()? {
            Ok(-value)
        } else {
            Ok(value)
        }
    }

    fn skip_to_byte(&mut self) -> Result<(), String> {
        while self.bit_index != 0 {
            let _ = self.read_bit()?;
        }
        Ok(())
    }

    fn byte_offset(&self) -> usize {
        self.byte_index
    }
}

struct Vp9BoolReader<'a> {
    data: &'a [u8],
    pos: usize,
    value: u64,
    range: u32,
    count: i32,
}

impl<'a> Vp9BoolReader<'a> {
    fn new(data: &'a [u8]) -> Result<Self, String> {
        let mut reader = Self {
            data,
            pos: 0,
            value: 0,
            range: 255,
            count: -8,
        };
        reader.fill();
        if reader.read_bit()? {
            return Err(String::from("VP9 compressed header marker bit is set"));
        }
        Ok(reader)
    }

    fn read_bit(&mut self) -> Result<bool, String> {
        self.read_bool(128)
    }

    fn read_bool(&mut self, probability: u32) -> Result<bool, String> {
        if self.count < 0 {
            self.fill();
        }
        let split = (self.range * probability + (256 - probability)) >> 8;
        let bigsplit = u64::from(split) << 56;
        let mut range = split;
        let mut value = self.value;
        let bit = if value >= bigsplit {
            range = self.range - split;
            value -= bigsplit;
            true
        } else {
            false
        };
        let shift = vpx_norm(range);
        self.range = range << shift;
        self.value = value << shift;
        self.count -= shift as i32;
        Ok(bit)
    }

    fn read_literal(&mut self, bits: usize) -> Result<u32, String> {
        let mut literal = 0u32;
        for bit in (0..bits).rev() {
            if self.read_bit()? {
                literal |= 1 << bit;
            }
        }
        Ok(literal)
    }

    fn fill(&mut self) {
        let bytes_left = self.data.len().saturating_sub(self.pos);
        let bits_left = bytes_left * 8;
        let mut shift = 64 - 8 - (self.count + 8);
        if bits_left > 64 {
            let bits = (shift & !7) + 8;
            let mut word = 0u64;
            for byte in &self.data[self.pos..self.pos + 8] {
                word = (word << 8) | u64::from(*byte);
            }
            let new_value = word >> (64 - bits);
            self.count += bits;
            self.pos += (bits >> 3) as usize;
            self.value |= new_value << (shift & 7);
        } else {
            let bits_over = shift + 8 - bits_left as i32;
            let mut loop_end = 0;
            if bits_over >= 0 {
                self.count += LOTS_OF_BITS;
                loop_end = bits_over;
            }
            if bits_over < 0 || bits_left != 0 {
                while shift >= loop_end && self.pos < self.data.len() {
                    self.count += 8;
                    self.value |= u64::from(self.data[self.pos]) << shift;
                    self.pos += 1;
                    shift -= 8;
                }
            }
        }
    }

    fn has_error(&self) -> bool {
        self.count > 64 && self.count < LOTS_OF_BITS
    }
}

fn vpx_norm(range: u32) -> u32 {
    if range == 0 {
        0
    } else {
        range.leading_zeros().saturating_sub(24)
    }
}
