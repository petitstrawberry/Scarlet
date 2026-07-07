use scarlet::device::video::{
    SCARLET_VIDEO_PIXEL_FORMAT_NV12, SCARLET_VIDEO_VP9_FRAME_FLAG_ERROR_RESILIENT,
    SCARLET_VIDEO_VP9_FRAME_FLAG_KEY_FRAME, SCARLET_VIDEO_VP9_FRAME_FLAG_SHOW_FRAME,
    SCARLET_VIDEO_VP9_INTERP_FILTER_EIGHTTAP, SCARLET_VIDEO_VP9_INTERP_FILTER_EIGHTTAP_SHARP,
    SCARLET_VIDEO_VP9_INTERP_FILTER_SWITCHABLE, SCARLET_VIDEO_VP9_MAX_TILES,
    SCARLET_VIDEO_VP9_TX_MODE_SELECT, ScarletVideoVp9FrameParams, ScarletVideoVp9StatelessParams,
    ScarletVideoVp9Tile,
};

use crate::h264::AvdDmaRange;

const AVD_VP9_MAX_INSTRUCTION_WORDS: usize = 4096;

/// VP9 stateless request lowering error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vp9FrontendError {
    /// Request dimensions are not valid.
    InvalidDimensions,
    /// Stream uses a VP9 feature this AVD path does not accept yet.
    UnsupportedFrame,
    /// Tile table is malformed.
    InvalidTiles,
    /// Generated AVD instruction stream exceeded its destination buffer.
    InstructionStreamTooLarge,
}

/// Decoded NV12 frame layout expected from Apple AVD VP9.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvdVp9FrameLayout {
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

impl AvdVp9FrameLayout {
    /// Construct an NV12 frame layout.
    ///
    /// # Arguments
    ///
    /// * `width` - Coded frame width in pixels.
    /// * `height` - Coded frame height in pixels.
    ///
    /// # Returns
    ///
    /// AVD-friendly NV12 frame layout.
    pub fn nv12(width: u32, height: u32) -> Self {
        let y_stride = align_up_u32(width, 64);
        Self {
            width,
            height,
            y_stride,
            uv_stride: y_stride,
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
        let luma = self.y_stride * height;
        let chroma = luma / 2;
        let meta =
            (self.width.next_power_of_two() * self.height.next_power_of_two() / 32).max(0x100);
        [
            luma,
            luma + meta,
            luma + meta + chroma,
            luma + meta + chroma / 2,
        ]
    }

    /// Return the AVD RVRA scratch size for this frame layout.
    ///
    /// # Returns
    ///
    /// Required RVRA scratch bytes.
    pub fn rvra_len(&self) -> usize {
        align_up_usize(self.rvra_offsets()[2] as usize, 0x4000)
    }

    /// Return the AVD VP9 SPS scratch size for this frame layout.
    ///
    /// # Returns
    ///
    /// Required SPS scratch bytes.
    pub fn sps_scratch_len(&self) -> usize {
        6 * 0x8000
    }
}

/// VP9 stream parameters needed by the AVD frontend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vp9StreamParameters {
    /// VP9 profile.
    pub profile: u8,
    /// Component bit depth.
    pub bit_depth: u8,
    /// Decoded coded width.
    pub width: u32,
    /// Decoded coded height.
    pub height: u32,
    /// Display/render width.
    pub render_width: u32,
    /// Display/render height.
    pub render_height: u32,
    /// Log2 tile columns.
    pub tile_cols_log2: u8,
    /// Log2 tile rows.
    pub tile_rows_log2: u8,
}

impl Vp9StreamParameters {
    /// Build stream parameters from stateless VP9 frame parameters.
    ///
    /// # Arguments
    ///
    /// * `frame` - Userspace-provided VP9 frame parameters.
    ///
    /// # Returns
    ///
    /// VP9 stream parameters accepted by the current AVD path.
    pub fn from_stateless_frame(
        frame: &ScarletVideoVp9FrameParams,
    ) -> Result<Self, Vp9FrontendError> {
        let width = u32::from(frame.frame_width_minus_1) + 1;
        let height = u32::from(frame.frame_height_minus_1) + 1;
        let render_width = u32::from(frame.render_width_minus_1) + 1;
        let render_height = u32::from(frame.render_height_minus_1) + 1;
        if width == 0 || height == 0 || width > 4096 || height > 4096 {
            return Err(Vp9FrontendError::InvalidDimensions);
        }
        if render_width == 0 || render_height == 0 || render_width > width || render_height > height
        {
            return Err(Vp9FrontendError::InvalidDimensions);
        }
        if frame.profile != 0 || frame.bit_depth != 8 {
            return Err(Vp9FrontendError::UnsupportedFrame);
        }
        if frame.tile_cols_log2 > 6 || frame.tile_rows_log2 > 2 {
            return Err(Vp9FrontendError::UnsupportedFrame);
        }
        Ok(Self {
            profile: frame.profile,
            bit_depth: frame.bit_depth,
            width,
            height,
            render_width,
            render_height,
            tile_cols_log2: frame.tile_cols_log2,
            tile_rows_log2: frame.tile_rows_log2,
        })
    }

    /// Build the NV12 output layout used by the Scarlet video ABI.
    ///
    /// # Returns
    ///
    /// NV12 frame layout with AVD-friendly aligned strides.
    pub fn nv12_layout(&self) -> AvdVp9FrameLayout {
        AvdVp9FrameLayout::nv12(self.width, self.height)
    }
}

/// Device-visible addresses of AVD VP9 session work areas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvdVp9Workspace {
    /// Instruction FIFO memory.
    pub instruction_fifo_dma_addr: u64,
    /// VP9 probability table memory.
    pub probabilities_dma_addr: u64,
    /// VP9 pps0 tile memory.
    pub pps0_tile_dma_addr: u64,
    /// VP9 pps1 tile memory ring.
    pub pps1_tile_dma_addrs: [u64; 8],
    /// VP9 pps2 tile memory pair.
    pub pps2_tile_dma_addrs: [u64; 2],
    /// VP9 SPS tile memory base.
    pub sps_tile_dma_addr: u64,
    /// Current decoded frame RVRA addresses.
    pub current_rvra_dma_addrs: [u64; 4],
    /// Last/golden/alternate reference RVRA addresses.
    pub reference_rvra_dma_addrs: [[u64; 4]; 3],
}

/// Previously decoded VP9 reference picture visible to the AVD command stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AvdVp9ReferencePicture {
    /// Stateless reference timestamp identifying this decoded picture.
    pub timestamp: u64,
    /// RVRA scratch addresses for this reference picture.
    pub rvra_dma_addrs: [u64; 4],
}

/// VP9 decode request lowered for the Apple AVD command path.
#[derive(Clone, Copy, Debug)]
pub struct Vp9DecodeRequest {
    /// Video session identifier.
    pub session_id: u64,
    /// Driver-local frame number.
    pub frame_number: u32,
    /// Input frame byte stream.
    pub input: AvdDmaRange,
    /// Output NV12 frame buffer.
    pub output: AvdDmaRange,
    /// Decoded frame layout.
    pub layout: AvdVp9FrameLayout,
    /// Userspace-provided VP9 stateless parameters.
    pub params: ScarletVideoVp9StatelessParams,
}

impl Vp9DecodeRequest {
    /// Build a decode request from stateless VP9 controls and DMA buffers.
    ///
    /// # Arguments
    ///
    /// * `session_id` - Video session identifier.
    /// * `frame_number` - Driver-local frame number.
    /// * `params` - Userspace-provided stateless VP9 parameters.
    /// * `input` - Device-visible input range.
    /// * `output` - Device-visible output range.
    /// * `layout` - Expected decoded frame layout.
    ///
    /// # Returns
    ///
    /// VP9 decode request ready for AVD command lowering.
    pub fn from_stateless(
        session_id: u64,
        frame_number: u32,
        params: &ScarletVideoVp9StatelessParams,
        input: AvdDmaRange,
        output: AvdDmaRange,
        layout: AvdVp9FrameLayout,
    ) -> Result<Self, Vp9FrontendError> {
        if layout.width == 0 || layout.height == 0 {
            return Err(Vp9FrontendError::InvalidDimensions);
        }
        validate_tiles(
            &params.frame,
            &params.tiles.tiles,
            params.tiles.tile_count,
            input.len,
        )?;
        Ok(Self {
            session_id,
            frame_number,
            input,
            output,
            layout,
            params: *params,
        })
    }
}

/// AVD v3-style instruction stream produced from one VP9 frame.
pub struct AvdVp9InstructionStream {
    words: [u32; AVD_VP9_MAX_INSTRUCTION_WORDS],
    len: usize,
    overflowed: bool,
}

impl AvdVp9InstructionStream {
    fn new() -> Self {
        Self {
            words: [0; AVD_VP9_MAX_INSTRUCTION_WORDS],
            len: 0,
            overflowed: false,
        }
    }

    fn push_word(&mut self, value: u32) {
        if let Some(slot) = self.words.get_mut(self.len) {
            *slot = value;
            self.len += 1;
        } else {
            self.overflowed = true;
        }
    }

    /// Generate an AVD VP9 instruction stream.
    ///
    /// The command sequence follows the public `eiln/avd` v3 VP9 HAL model.
    /// Userspace supplies compressed-header probability state and tile ranges;
    /// this builder only validates and lowers them to AVD command words.
    ///
    /// # Arguments
    ///
    /// * `request` - VP9 decode request being submitted.
    /// * `stream` - Current VP9 stream parameters.
    /// * `workspace` - Device-visible workspace addresses.
    /// * `references` - Last, golden, and alternate reference pictures.
    ///
    /// # Returns
    ///
    /// Encoded instruction stream words.
    pub fn build(
        request: &Vp9DecodeRequest,
        stream: &Vp9StreamParameters,
        workspace: &AvdVp9Workspace,
        references: &[Option<AvdVp9ReferencePicture>; 3],
    ) -> Self {
        let mut words = Self::new();
        let frame = &request.params.frame;
        let key_frame = frame.flags & SCARLET_VIDEO_VP9_FRAME_FLAG_KEY_FRAME != 0;
        let inst_fifo_slot = request.frame_number % 7;
        let coded_hw = pack_hw(stream.width, stream.height);
        let y_addr = request.output.dma_addr;
        let uv_addr =
            request.output.dma_addr + request.layout.y_stride as u64 * request.layout.height as u64;

        push(
            &mut words,
            0x2b00_0000 | 0xfff000 | 0x100 | (inst_fifo_slot * 0x10),
            "cm3_cmd_inst_fifo_start",
        );

        let mut start = 0x1000 | 0x02e0;
        if key_frame {
            start |= 0x2000;
        }
        push(&mut words, 0x2db0_0000 | start, "hdr_30_cmd_start_hdr");
        push(&mut words, 0x0200_0000, "hdr_34_const_20");
        push(&mut words, coded_hw, "hdr_28_height_width_shift3");
        push(&mut words, 0, "cm3_dma_config_0");
        push(&mut words, coded_hw, "hdr_38_height_width_shift3");

        let mut txfm = 0x0100_0000 | 0x1000 | 0x800;
        txfm |= (u32::from(frame.tx_mode.min(SCARLET_VIDEO_VP9_TX_MODE_SELECT)) & 3) << 7;
        if frame.tx_mode == SCARLET_VIDEO_VP9_TX_MODE_SELECT {
            txfm |= 1;
        }
        push(&mut words, txfm, "hdr_2c_txfm_mode");
        push(&mut words, make_flags1(frame), "hdr_40_flags1_pt1");
        for _ in 0..8 {
            push(&mut words, 0, "hdr_scaling_list_zero");
        }

        push(&mut words, 0x0002_0000, "cm3_dma_config_1");
        push(&mut words, 0x0402_0002, "cm3_dma_config_2");
        push(&mut words, 0x0202_0202, "cm3_dma_config_3");
        push(&mut words, 0x240, "hdr_e0_const_240");
        push(
            &mut words,
            (workspace.probabilities_dma_addr >> 8) as u32,
            "hdr_104_probs_addr_lsb8",
        );

        push(
            &mut words,
            (workspace.pps0_tile_dma_addr >> 8) as u32,
            "hdr_118_pps0_tile_addr_lsb8",
        );
        let pps1_index = ((request.frame_number / 128) as usize + 1) % 8;
        push(
            &mut words,
            (workspace.pps1_tile_dma_addrs[pps1_index] >> 8) as u32,
            "hdr_108_pps1_tile_addr_lsb8",
        );
        push(
            &mut words,
            (workspace.pps1_tile_dma_addrs[pps1_index] >> 8) as u32,
            "hdr_108_pps1_tile_addr_lsb8",
        );
        let pps2_a = (request.frame_number as usize) & 1;
        let pps2_b = pps2_a ^ 1;
        push(
            &mut words,
            (workspace.pps2_tile_dma_addrs[pps2_a] >> 8) as u32,
            "hdr_110_pps2_tile_addr_lsb8",
        );
        push(
            &mut words,
            (workspace.pps2_tile_dma_addrs[pps2_b] >> 8) as u32,
            "hdr_110_pps2_tile_addr_lsb8",
        );

        push(
            &mut words,
            u32::from(frame.quantization.base_q_idx) * 0x8000,
            "hdr_4c_base_q_idx",
        );
        push(&mut words, 0x0020_ffff, "hdr_44_flags1_pt2");
        push(
            &mut words,
            u32::from(frame.loop_filter.level) * 0x4000,
            "hdr_48_loop_filter_level",
        );

        push(&mut words, 0x0402_0002, "cm3_dma_config_4");
        push(&mut words, 0x0402_0002, "cm3_dma_config_5");
        push(&mut words, 0, "cm3_dma_config_6");

        let sps_unit = 0x8000u64;
        push(
            &mut words,
            ((workspace.sps_tile_dma_addr + 0 * sps_unit) >> 8) as u32,
            "hdr_e8_sps0_tile_addr_lsb8",
        );
        push(
            &mut words,
            ((workspace.sps_tile_dma_addr + sps_unit) >> 8) as u32,
            "hdr_e8_sps0_tile_addr_lsb8",
        );
        push(&mut words, 0, "hdr_e8_sps0_tile_addr_lsb8");
        push(
            &mut words,
            ((workspace.sps_tile_dma_addr + 3 * sps_unit) >> 8) as u32,
            "hdr_f4_sps1_tile_addr_lsb8",
        );
        push(
            &mut words,
            ((workspace.sps_tile_dma_addr + 4 * sps_unit) >> 8) as u32,
            "hdr_f4_sps1_tile_addr_lsb8",
        );
        push(
            &mut words,
            ((workspace.sps_tile_dma_addr + 6 * sps_unit) >> 8) as u32,
            "hdr_f4_sps1_tile_addr_lsb8",
        );

        push(&mut words, 0x0007_0007, "cm3_dma_config_7");
        push_rvra(
            &mut words,
            workspace.current_rvra_dma_addrs,
            "hdr_11c_curr_rvra_addr_lsb7",
        );
        push(
            &mut words,
            ((workspace.sps_tile_dma_addr + 5 * sps_unit) >> 8) as u32,
            "hdr_f4_sps1_tile_addr_lsb8",
        );

        push(&mut words, (y_addr >> 8) as u32, "hdr_168_y_addr_lsb8");
        push(
            &mut words,
            height_width_align(stream.width),
            "hdr_170_width_align",
        );
        push(&mut words, (uv_addr >> 8) as u32, "hdr_16c_uv_addr_lsb8");
        push(
            &mut words,
            height_width_align(stream.width),
            "hdr_174_width_align",
        );
        push(&mut words, 0, "hdr_zero");
        push(&mut words, coded_hw, "cm3_height_width");

        if !key_frame {
            stream_refs(&mut words, coded_hw, references);
        }
        stream_tiles(&mut words, request);

        words
    }

    /// Return encoded instruction words.
    ///
    /// # Returns
    ///
    /// Instruction word slice.
    pub fn words(&self) -> &[u32] {
        &self.words[..self.len]
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
    pub fn write_le_bytes(&self, dst: &mut [u8]) -> Result<usize, Vp9FrontendError> {
        if self.overflowed {
            return Err(Vp9FrontendError::InstructionStreamTooLarge);
        }
        let byte_len = self.len * core::mem::size_of::<u32>();
        if byte_len > dst.len() {
            return Err(Vp9FrontendError::InstructionStreamTooLarge);
        }
        for (index, word) in self.words().iter().enumerate() {
            let offset = index * 4;
            dst[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
        }
        Ok(byte_len)
    }
}

fn validate_tiles(
    frame: &ScarletVideoVp9FrameParams,
    tiles: &[ScarletVideoVp9Tile; SCARLET_VIDEO_VP9_MAX_TILES],
    tile_count: u32,
    input_len: usize,
) -> Result<(), Vp9FrontendError> {
    let expected = (1usize << frame.tile_cols_log2) * (1usize << frame.tile_rows_log2);
    let tile_count = tile_count as usize;
    if tile_count == 0 || tile_count != expected || tile_count > SCARLET_VIDEO_VP9_MAX_TILES {
        return Err(Vp9FrontendError::InvalidTiles);
    }
    for tile in tiles.iter().take(tile_count) {
        let offset = tile.offset as usize;
        let size = tile.size as usize;
        let end = offset
            .checked_add(size)
            .ok_or(Vp9FrontendError::InvalidTiles)?;
        if size == 0 || end > input_len {
            return Err(Vp9FrontendError::InvalidTiles);
        }
    }
    Ok(())
}

fn make_flags1(frame: &ScarletVideoVp9FrameParams) -> u32 {
    let mut flags = 0;
    flags |= bit(0, true);
    flags |= bit(
        14,
        frame.flags & SCARLET_VIDEO_VP9_FRAME_FLAG_ERROR_RESILIENT == 0,
    );
    flags |= bit(
        15,
        frame.flags & SCARLET_VIDEO_VP9_FRAME_FLAG_SHOW_FRAME != 0,
    );
    if frame.flags & SCARLET_VIDEO_VP9_FRAME_FLAG_KEY_FRAME == 0 {
        flags |= bit(19, true);
        flags |= bit(21, true);
        if frame.interpolation_filter != SCARLET_VIDEO_VP9_INTERP_FILTER_SWITCHABLE {
            if frame.interpolation_filter == SCARLET_VIDEO_VP9_INTERP_FILTER_EIGHTTAP {
                flags |= bit(16, true);
            } else if frame.interpolation_filter == SCARLET_VIDEO_VP9_INTERP_FILTER_EIGHTTAP_SHARP {
                flags |= bit(17, true);
            }
        }
        flags |= bit(
            18,
            frame.interpolation_filter == SCARLET_VIDEO_VP9_INTERP_FILTER_SWITCHABLE,
        );
    }
    flags |= bit(4, true);
    flags |= bit(8, frame.refresh_frame_flags & (1 << 1) != 0);
    flags |= bit(9, frame.refresh_frame_flags & (1 << 0) != 0);
    flags
}

fn stream_refs(
    words: &mut AvdVp9InstructionStream,
    coded_hw: u32,
    references: &[Option<AvdVp9ReferencePicture>; 3],
) {
    push(words, 0x0007_0007, "cm3_dma_config_7");
    push(words, 0x0007_0007, "cm3_dma_config_8");
    push(words, 0x0007_0007, "cm3_dma_config_9");

    for reference in references {
        push(words, 0x0100_0000, "hdr_9c_ref_100");
        push(words, coded_hw, "hdr_70_ref_height_width");
        push(words, 0x4000_4000, "hdr_7c_ref_align");
        let rvra = (*reference)
            .map(|reference| reference.rvra_dma_addrs)
            .unwrap_or([0; 4]);
        push(words, (rvra[0] >> 7) as u32, "hdr_138_ref_rvra0_addr_lsb7");
        push(words, (rvra[1] >> 7) as u32, "hdr_144_ref_rvra1_addr_lsb7");
        push(words, (rvra[2] >> 7) as u32, "hdr_150_ref_rvra2_addr_lsb7");
        push(words, (rvra[3] >> 7) as u32, "hdr_15c_ref_rvra3_addr_lsb7");
    }
}

fn stream_tiles(words: &mut AvdVp9InstructionStream, request: &Vp9DecodeRequest) {
    let tile_count = request.params.tiles.tile_count as usize;
    for (index, tile) in request
        .params
        .tiles
        .tiles
        .iter()
        .take(tile_count)
        .enumerate()
    {
        let tile_addr = request.input.dma_addr + tile.offset as u64;
        push(words, 0x2d80_0000, "cm3_cmd_set_slice_data");
        push(words, tile_addr as u32, "til_ab4_tile_addr_low");
        push(words, tile.size, "til_ab8_tile_size");
        push(
            words,
            0x2a00_0000 | (index as u32 * 4),
            "cm3_cmd_tile_index",
        );
        let dims = if tile_count == 1 {
            1
        } else {
            ((index as u32) << 24)
                | (((u32::from(tile.row) + 1) * 8 - 1) << 12)
                | ((u32::from(tile.col) + 1) * 4 - 1)
        };
        push(words, dims, "til_ac0_tile_dims");
        let end = if index + 1 == tile_count {
            0x400
        } else {
            0xfff000
        };
        push(words, 0x2b00_0000 | end, "cm3_cmd_inst_fifo_end");
    }
}

fn push_rvra(words: &mut AvdVp9InstructionStream, addrs: [u64; 4], _name: &'static str) {
    for addr in addrs {
        push(words, (addr >> 7) as u32, "rvra_addr_lsb7");
    }
}

fn push(words: &mut AvdVp9InstructionStream, value: u32, _name: &'static str) {
    words.push_word(value);
}

fn bit(position: u32, value: bool) -> u32 {
    if value { 1 << position } else { 0 }
}

fn pack_hw(width: u32, height: u32) -> u32 {
    (((height - 1) & 0xffff) << 16) | ((width - 1) & 0xffff)
}

fn height_width_align(width: u32) -> u32 {
    (((align_up_u32(width, 64) >> 6) << 2).max(0xc)) & 0xffff
}

fn align_up_u32(value: u32, align: u32) -> u32 {
    if align == 0 {
        value
    } else {
        value.div_ceil(align) * align
    }
}

fn align_up_usize(value: usize, align: usize) -> usize {
    if align == 0 {
        value
    } else {
        value.div_ceil(align) * align
    }
}
