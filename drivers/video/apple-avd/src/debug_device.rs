use alloc::{format, string::String, sync::Arc, vec::Vec};
use core::{any::Any, fmt::Write};

use scarlet::{
    arch::{self, Trapframe},
    device::{
        Device, DeviceType,
        char::CharDevice,
        iommu::IommuMapFlags,
        manager::DeviceManager,
        video::{
            SCARLET_VIDEO_FORMAT_H264, SCARLET_VIDEO_FRAME_HEADER_LEN,
            SCARLET_VIDEO_H264_DECODE_PARAM_FLAG_IDR,
            SCARLET_VIDEO_H264_PPS_FLAG_DEBLOCKING_FILTER_CONTROL_PRESENT,
            SCARLET_VIDEO_H264_SPS_FLAG_DIRECT_8X8_INFERENCE,
            SCARLET_VIDEO_H264_SPS_FLAG_FRAME_MBS_ONLY, ScarletVideoH264DecodeParams,
            ScarletVideoH264Pps, ScarletVideoH264ScalingMatrix, ScarletVideoH264SliceParams,
            ScarletVideoH264Sps, ScarletVideoH264StatelessParams, VideoBackendDecodeRequest,
            VideoBackendH264StatelessRequest, VideoDecodeBackend,
        },
    },
    environment::PAGE_SIZE,
    mem::page::ContiguousPages,
    object::capability::{
        ControlOps, MemoryMappingInfo, MemoryMappingOps,
        selectable::{ReadyInterest, ReadySet, SelectWaitOutcome, Selectable},
    },
    sync::Mutex,
};

use crate::{AVD_DMA_GRANULE, AVD_MAPPED_INPUT_BYTES, AVD_MAPPED_OUTPUT_BYTES, get_apple_avd};

const DEBUG_BUFFER_BYTES: usize = AVD_MAPPED_INPUT_BYTES + AVD_MAPPED_OUTPUT_BYTES;
const DEBUG_BUFFER_PAGES: usize = DEBUG_BUFFER_BYTES / PAGE_SIZE;
const SAMPLE_H264_TIMESTAMP: u64 = 1;

const SAMPLE_H264_AU: &[u8] = &[
    0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xc0, 0x1e, 0xdd, 0xec, 0x04, 0x40, 0x00, 0x00, 0x03, 0x00,
    0x40, 0x00, 0x00, 0x03, 0x00, 0xa3, 0xc5, 0x8b, 0xe0, 0x00, 0x00, 0x00, 0x01, 0x68, 0xce, 0x0f,
    0xc8, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x3a, 0x26, 0x28, 0x00, 0x09, 0x02, 0xe0,
];

fn sample_h264_stateless_params() -> ScarletVideoH264StatelessParams {
    ScarletVideoH264StatelessParams {
        sps: ScarletVideoH264Sps {
            profile_idc: 66,
            constraint_set_flags: 0xc0,
            level_idc: 30,
            seq_parameter_set_id: 0,
            chroma_format_idc: 1,
            bit_depth_luma_minus8: 0,
            bit_depth_chroma_minus8: 0,
            log2_max_frame_num_minus4: 0,
            pic_order_cnt_type: 2,
            log2_max_pic_order_cnt_lsb_minus4: 0,
            max_num_ref_frames: 0,
            num_ref_frames_in_pic_order_cnt_cycle: 0,
            pic_width_in_mbs_minus1: 0,
            pic_height_in_map_units_minus1: 0,
            flags: SCARLET_VIDEO_H264_SPS_FLAG_FRAME_MBS_ONLY
                | SCARLET_VIDEO_H264_SPS_FLAG_DIRECT_8X8_INFERENCE,
            ..Default::default()
        },
        pps: ScarletVideoH264Pps {
            pic_parameter_set_id: 0,
            seq_parameter_set_id: 0,
            num_slice_groups_minus1: 0,
            num_ref_idx_l0_default_active_minus1: 0,
            num_ref_idx_l1_default_active_minus1: 0,
            weighted_bipred_idc: 0,
            pic_init_qp_minus26: -3,
            pic_init_qs_minus26: 0,
            chroma_qp_index_offset: 0,
            second_chroma_qp_index_offset: 0,
            flags: SCARLET_VIDEO_H264_PPS_FLAG_DEBLOCKING_FILTER_CONTROL_PRESENT,
        },
        scaling_matrix: ScarletVideoH264ScalingMatrix::default(),
        slice_params: ScarletVideoH264SliceParams {
            header_bit_size: 24,
            nal_offset: 36,
            nal_len: 10,
            first_mb_in_slice: 0,
            slice_type: 7,
            pic_parameter_set_id: 0,
            slice_qp_delta: -3,
            disable_deblocking_filter_idc: 1,
            ..Default::default()
        },
        decode_params: ScarletVideoH264DecodeParams {
            nal_ref_idc: 3,
            frame_num: 0,
            flags: SCARLET_VIDEO_H264_DECODE_PARAM_FLAG_IDR,
            ..Default::default()
        },
        ..Default::default()
    }
}

pub(crate) fn register_avd_debug_device(avd_id: u32, backend: Arc<dyn VideoDecodeBackend>) {
    let Some(avd) = get_apple_avd(avd_id) else {
        scarlet::early_println!(
            "[apple-avd] debug device skipped because avd{} is not registered",
            avd_id
        );
        return;
    };
    let device = Arc::new(AppleAvdDebugDevice::new(avd_id, avd, backend));
    DeviceManager::get_manager().register_device_with_name(format!("avd{}", avd_id), device);
    scarlet::early_println!("[apple-avd] registered /dev/avd{} debug device", avd_id);
}

struct AppleAvdDebugDevice {
    avd_id: u32,
    avd: Arc<Mutex<crate::AppleAvd>>,
    backend: Arc<dyn VideoDecodeBackend>,
    last_report: Mutex<String>,
    decode_buffer: Mutex<Option<ContiguousPages>>,
    decode_stream: Mutex<Option<u32>>,
    decode_pending: Mutex<bool>,
}

impl AppleAvdDebugDevice {
    fn new(
        avd_id: u32,
        avd: Arc<Mutex<crate::AppleAvd>>,
        backend: Arc<dyn VideoDecodeBackend>,
    ) -> Self {
        Self {
            avd_id,
            avd,
            backend,
            last_report: Mutex::new(String::from(
                "write one of: info, fw-ping, dart-test, decode-one, poll-decode, trace, clear-trace\n",
            )),
            decode_buffer: Mutex::new(None),
            decode_stream: Mutex::new(None),
            decode_pending: Mutex::new(false),
        }
    }

    fn run_command(&self, command: &str) -> Result<(), &'static str> {
        let report = match command {
            "" | "info" | "avd-info" => self.render_info(),
            "fw-ping" | "avd-fw-ping" => self.run_fw_ping(),
            "dart-test" | "avd-dart-test" => self.run_dart_test(),
            "decode-one" | "avd-decode-one" => self.run_decode_one()?,
            "poll-decode" => self.poll_decode()?,
            "trace" | "avd-trace" => self.render_trace(),
            "clear-trace" => {
                self.avd.lock().clear_trace();
                String::from("avd trace cleared\n")
            }
            "help" => String::from(
                "commands: info, fw-ping, dart-test, decode-one, poll-decode, trace, clear-trace\n",
            ),
            _ => return Err("apple-avd-debug: unknown command"),
        };
        *self.last_report.lock() = report;
        Ok(())
    }

    fn render_info(&self) -> String {
        let avd = self.avd.lock();
        let snapshot = avd.debug_snapshot();
        let caps = self.backend.capabilities();
        format!(
            concat!(
                "avd{} name={} soc={} mmio={:#x}+{:#x} irq={:?}\n",
                "firmware={} fw_dma={:#x} fw_len={}\n",
                "status={:#x} irq_enable_status1={:#x} mailbox_status={:#x} mailbox_raw={:#x}\n",
                "backend={} sessions={} input={} output={} stateful_h264={} stateful_av1={} stateful_hevc={} stateless_h264={} stateless_vp9={}\n"
            ),
            self.avd_id,
            avd.name(),
            avd.soc_name(),
            avd.paddr(),
            avd.size(),
            avd.irq(),
            avd.firmware_state_name(),
            avd.firmware_dma_addr().unwrap_or(0),
            avd.firmware_image_size().unwrap_or(0),
            snapshot.status,
            snapshot.irq_enable_status1,
            snapshot.mailbox_status,
            snapshot.mailbox_raw,
            self.backend.name(),
            caps.max_sessions,
            caps.mapped_input_len,
            caps.mapped_output_len,
            caps.supports_h264,
            caps.supports_av1,
            caps.supports_hevc,
            caps.supports_stateless_h264,
            self.backend.supports_stateless_vp9()
        )
    }

    fn run_fw_ping(&self) -> String {
        let mut avd = self.avd.lock();
        let before = avd.debug_snapshot();
        let message = avd.poll_firmware_message();
        let after = avd.debug_snapshot();
        format!(
            concat!(
                "fw-ping avd{} state={} message={:?}\n",
                "before status={:#x} irq_enable_status1={:#x} mailbox_status={:#x} mailbox_raw={:#x}\n",
                "after status={:#x} irq_enable_status1={:#x} mailbox_status={:#x} mailbox_raw={:#x}\n"
            ),
            self.avd_id,
            avd.firmware_state_name(),
            message,
            before.status,
            before.irq_enable_status1,
            before.mailbox_status,
            before.mailbox_raw,
            after.status,
            after.irq_enable_status1,
            after.mailbox_status,
            after.mailbox_raw
        )
    }

    fn run_dart_test(&self) -> String {
        let avd = self.avd.lock();
        let granule = avd.dma_context().mapping_granule().max(AVD_DMA_GRANULE);
        let pages = ContiguousPages::new_aligned(granule.div_ceil(PAGE_SIZE), granule)
            .ok_or("apple-avd-debug: DART test allocation failed")
            .map(|pages| {
                let ptr = pages.as_ptr() as *mut u8;
                for offset in 0..granule {
                    // SAFETY: `pages` owns at least `granule` bytes by construction.
                    unsafe {
                        ptr.add(offset)
                            .write_volatile((offset as u8).wrapping_mul(17).wrapping_add(3));
                    }
                }
                arch::clean_dcache_to_poc_range(pages.as_vaddr(), granule);
                pages
            });
        let Ok(pages) = pages else {
            return String::from("dart-test allocation failed\n");
        };
        let mapping = match avd.dma_context().map_phys_owned(
            pages.as_paddr(),
            granule,
            IommuMapFlags::READ | IommuMapFlags::WRITE | IommuMapFlags::COHERENT,
        ) {
            Ok(mapping) => mapping,
            Err(_) => return String::from("dart-test map failed\n"),
        };
        arch::invalidate_dcache_to_poc_range(pages.as_vaddr(), granule);
        // SAFETY: `pages` owns at least `granule` bytes; the first byte is in range.
        let first = unsafe { (pages.as_ptr() as *const u8).read_volatile() };
        // SAFETY: `pages` owns at least `granule` bytes; `granule - 1` is in range
        // because the DART granule is non-zero.
        let last = unsafe {
            (pages.as_ptr() as *const u8)
                .add(granule - 1)
                .read_volatile()
        };
        format!(
            "dart-test ok avd{} paddr={:#x} iova={:#x} len={} first={:#x} last={:#x}\n",
            self.avd_id,
            pages.as_paddr(),
            mapping.dma_addr(),
            granule,
            first,
            last
        )
    }

    fn run_decode_one(&self) -> Result<String, &'static str> {
        if *self.decode_pending.lock() {
            return self.poll_decode();
        }

        self.ensure_decode_buffer()?;
        let stream_id = self.ensure_decode_stream()?;
        {
            let mut buffer_guard = self.decode_buffer.lock();
            let pages = buffer_guard
                .as_mut()
                .ok_or("apple-avd-debug: decode buffer missing")?;
            // SAFETY: `pages` owns the debug mapping and SAMPLE_H264_AU is
            // shorter than the mapped input region.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    SAMPLE_H264_AU.as_ptr(),
                    pages.as_ptr() as *mut u8,
                    SAMPLE_H264_AU.len(),
                );
            }
            arch::clean_dcache_to_poc_range(pages.as_vaddr(), SAMPLE_H264_AU.len());
        }

        let (input_paddr, input_vaddr, output_paddr, output_vaddr) = {
            let buffer_guard = self.decode_buffer.lock();
            let pages = buffer_guard
                .as_ref()
                .ok_or("apple-avd-debug: decode buffer missing")?;
            (
                pages.as_paddr(),
                pages.as_vaddr(),
                pages.as_paddr() + AVD_MAPPED_INPUT_BYTES,
                pages.as_vaddr() + AVD_MAPPED_INPUT_BYTES,
            )
        };
        let request = VideoBackendDecodeRequest {
            stream_id,
            coded_format: SCARLET_VIDEO_FORMAT_H264,
            input_paddr,
            input_vaddr,
            input_len: SAMPLE_H264_AU.len() as u32,
            output_paddr,
            output_vaddr,
            output_offset: AVD_MAPPED_INPUT_BYTES as u64,
            output_len: AVD_MAPPED_OUTPUT_BYTES as u32,
            timestamp: SAMPLE_H264_TIMESTAMP,
        };
        let request = VideoBackendH264StatelessRequest {
            decode: request,
            h264: sample_h264_stateless_params(),
        };
        self.backend.submit_h264_stateless(&request)?;
        *self.decode_pending.lock() = true;
        self.poll_decode()
    }

    fn poll_decode(&self) -> Result<String, &'static str> {
        let Some(stream_id) = *self.decode_stream.lock() else {
            return Ok(String::from("decode-one has not been submitted\n"));
        };
        for _ in 0..4096 {
            if let Some(decoded) = self.backend.dequeue_frame(stream_id)? {
                *self.decode_pending.lock() = false;
                let checksum = self.output_checksum(
                    decoded.frame.payload_offset as usize,
                    decoded.frame.payload_len as usize,
                )?;
                return Ok(format!(
                    "decode-one complete stream={} {}x{} pixel={:#x} payload={} flags={:#x} timestamp={} checksum={:#x}\n",
                    decoded.stream_id,
                    decoded.frame.width,
                    decoded.frame.height,
                    decoded.frame.pixel_format,
                    decoded.frame.payload_len,
                    decoded.frame.flags,
                    decoded.frame.timestamp,
                    checksum
                ));
            }
            core::hint::spin_loop();
        }
        Ok(format!(
            "decode-one pending stream={} input={} output={}\n",
            stream_id,
            SAMPLE_H264_AU.len(),
            AVD_MAPPED_OUTPUT_BYTES
        ))
    }

    fn render_trace(&self) -> String {
        let avd = self.avd.lock();
        let mut report = String::new();
        let _ = writeln!(
            report,
            "avd{} trace events={}",
            self.avd_id,
            avd.trace_entries().len()
        );
        for event in avd.trace_entries() {
            let _ = writeln!(
                report,
                "#{:04} {:?} {:#x} {:#x}",
                event.sequence, event.kind, event.arg0, event.arg1
            );
        }
        report
    }

    fn ensure_decode_buffer(&self) -> Result<(), &'static str> {
        let mut guard = self.decode_buffer.lock();
        if guard.is_none() {
            *guard = ContiguousPages::new_aligned(DEBUG_BUFFER_PAGES, AVD_DMA_GRANULE);
        }
        if guard.is_some() {
            Ok(())
        } else {
            Err("apple-avd-debug: decode buffer allocation failed")
        }
    }

    fn ensure_decode_stream(&self) -> Result<u32, &'static str> {
        let mut guard = self.decode_stream.lock();
        if let Some(stream_id) = *guard {
            return Ok(stream_id);
        }
        let stream_id = self.backend.create_session(SCARLET_VIDEO_FORMAT_H264)?;
        *guard = Some(stream_id);
        Ok(stream_id)
    }

    fn output_checksum(&self, offset: usize, len: usize) -> Result<u32, &'static str> {
        if offset < AVD_MAPPED_INPUT_BYTES + SCARLET_VIDEO_FRAME_HEADER_LEN {
            return Err("apple-avd-debug: decoded payload offset is outside output buffer");
        }
        if offset
            .checked_add(len)
            .filter(|end| *end <= DEBUG_BUFFER_BYTES)
            .is_none()
        {
            return Err("apple-avd-debug: decoded payload exceeds debug buffer");
        }
        let guard = self.decode_buffer.lock();
        let pages = guard
            .as_ref()
            .ok_or("apple-avd-debug: decode buffer missing")?;
        let vaddr = pages.as_vaddr() + offset;
        arch::invalidate_dcache_to_poc_range(vaddr, len);
        // SAFETY: `offset..offset + len` was bounds-checked against the debug
        // buffer, and `pages` owns that entire mapping.
        let bytes = unsafe { core::slice::from_raw_parts(vaddr as *const u8, len) };
        Ok(bytes.iter().fold(0x811c_9dc5u32, |hash, byte| {
            hash.wrapping_mul(0x0100_0193) ^ *byte as u32
        }))
    }

    fn read_report(&self, position: usize, buffer: &mut [u8]) -> usize {
        let report = self.last_report.lock();
        let bytes = report.as_bytes();
        if position >= bytes.len() {
            return 0;
        }
        let count = core::cmp::min(buffer.len(), bytes.len() - position);
        buffer[..count].copy_from_slice(&bytes[position..position + count]);
        count
    }
}

impl Device for AppleAvdDebugDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "apple-avd-debug"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_char_device(&self) -> Option<&dyn CharDevice> {
        Some(self)
    }
}

impl CharDevice for AppleAvdDebugDevice {
    fn read_byte(&self) -> Option<u8> {
        None
    }

    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        let bytes = [byte];
        self.write(&bytes).map(|_| ())
    }

    fn read(&self, buffer: &mut [u8]) -> usize {
        self.read_report(0, buffer)
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, &'static str> {
        let command = core::str::from_utf8(buffer)
            .map_err(|_| "apple-avd-debug: command is not UTF-8")?
            .trim();
        self.run_command(command)?;
        Ok(buffer.len())
    }

    fn can_read(&self) -> bool {
        true
    }

    fn can_write(&self) -> bool {
        true
    }

    fn read_at(&self, position: u64, buffer: &mut [u8]) -> Result<usize, &'static str> {
        Ok(self.read_report(position as usize, buffer))
    }
}

impl ControlOps for AppleAvdDebugDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("apple-avd-debug: unsupported control command")
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        Vec::new()
    }
}

impl MemoryMappingOps for AppleAvdDebugDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        Err("apple-avd-debug: mmap is not supported")
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for AppleAvdDebugDevice {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let mut set = ReadySet::none();
        if interest.read {
            set.read = true;
        }
        if interest.write {
            set.write = true;
        }
        set
    }

    fn wait_until_ready(
        &self,
        _interest: ReadyInterest,
        _trapframe: &mut Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::Ready
    }

    fn is_nonblocking(&self) -> bool {
        true
    }
}
