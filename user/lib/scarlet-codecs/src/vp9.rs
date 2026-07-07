//! VP9 stateless request construction types.

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

#[derive(Clone, Copy, Default)]
pub struct ScarletVideoVp9StatelessParams {
    pub frame: ScarletVideoVp9FrameParams,
    pub probabilities: ScarletVideoVp9Probabilities,
    pub tiles: ScarletVideoVp9Tiles,
}
