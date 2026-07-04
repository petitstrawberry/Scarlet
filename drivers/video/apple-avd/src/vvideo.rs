use alloc::{format, string::String, sync::Arc, vec::Vec};
use core::any::Any;

use scarlet::{
    arch::Trapframe,
    device::{
        Device, DeviceType,
        char::CharDevice,
        manager::DeviceManager,
        video::{
            SCARLET_VIDEO_FORMAT_H264, ScarletVideoBufferInfo, ScarletVideoSessionDequeuedFrame,
            ScarletVideoSessionInfo, ScarletVideoSessionSubmit, ScarletVideoSubmit,
            VVIDEO_CREATE_SESSION, VVIDEO_DEQUEUE, VVIDEO_DEQUEUE_SESSION, VVIDEO_DESTROY_SESSION,
            VVIDEO_GET_BUFFER, VVIDEO_SUBMIT, VVIDEO_SUBMIT_SESSION, VideoBackendDecodeRequest,
            VideoDecodeBackend,
        },
    },
    environment::PAGE_SIZE,
    library::std::usercopy::{copy_from_user, copy_to_user},
    mem::page::ContiguousPages,
    object::capability::{
        ControlOps, MemoryMappingInfo, MemoryMappingOps,
        selectable::{ReadyInterest, ReadySet, SelectWaitOutcome, Selectable},
    },
    sync::Mutex,
    task::mytask,
};

use crate::{AVD_MAPPED_INPUT_BYTES, AVD_MAPPED_OUTPUT_BYTES};

const DEFAULT_STREAM_ID: u32 = 1;
const MAPPED_OUTPUT_OFFSET: usize = AVD_MAPPED_INPUT_BYTES;
const MAPPED_BUFFER_BYTES: usize = MAPPED_OUTPUT_OFFSET + AVD_MAPPED_OUTPUT_BYTES;
const MAPPED_BUFFER_PAGES: usize = MAPPED_BUFFER_BYTES / PAGE_SIZE;

static VVIDEO_REGISTERED: Mutex<bool> = Mutex::new(false);

pub(crate) fn register_avd_vvideo_device(backend: Arc<dyn VideoDecodeBackend>) {
    let mut registered = VVIDEO_REGISTERED.lock();
    if *registered {
        scarlet::early_println!("[apple-avd] /dev/vvideo0 already registered, skipping");
        return;
    }

    let device = Arc::new(AppleAvdVvideoDevice::new(backend));
    DeviceManager::get_manager().register_device_with_name(String::from("vvideo0"), device);
    *registered = true;
    scarlet::early_println!("[apple-avd] registered /dev/vvideo0 stub frontend");
}

struct AppleAvdVvideoDevice {
    backend: Arc<dyn VideoDecodeBackend>,
    mapped_buffer: Mutex<Option<ContiguousPages>>,
    last_error: Mutex<Option<&'static str>>,
    next_timestamp: Mutex<u64>,
}

impl AppleAvdVvideoDevice {
    fn new(backend: Arc<dyn VideoDecodeBackend>) -> Self {
        Self {
            backend,
            mapped_buffer: Mutex::new(None),
            last_error: Mutex::new(None),
            next_timestamp: Mutex::new(1),
        }
    }

    fn buffer_info(&self) -> Result<ScarletVideoBufferInfo, &'static str> {
        self.ensure_mapped_buffer()?;
        Ok(ScarletVideoBufferInfo {
            mmap_offset: 0,
            mmap_len: MAPPED_BUFFER_BYTES as u64,
            input_offset: 0,
            input_len: AVD_MAPPED_INPUT_BYTES as u32,
            output_offset: MAPPED_OUTPUT_OFFSET as u64,
            output_len: AVD_MAPPED_OUTPUT_BYTES as u32,
        })
    }

    fn ensure_mapped_buffer(&self) -> Result<(), &'static str> {
        let mut mapped_buffer = self.mapped_buffer.lock();
        if mapped_buffer.is_none() {
            *mapped_buffer = ContiguousPages::new(MAPPED_BUFFER_PAGES);
        }
        if mapped_buffer.is_some() {
            Ok(())
        } else {
            Err("apple-avd-vvideo: mmap buffer allocation failed")
        }
    }

    fn next_timestamp(&self) -> u64 {
        let mut next = self.next_timestamp.lock();
        let timestamp = *next;
        *next = next.wrapping_add(1);
        timestamp
    }

    fn submit_mapped(
        &self,
        stream_id: u32,
        coded_format: u32,
        input_len: usize,
        timestamp: u64,
    ) -> Result<(), &'static str> {
        if stream_id != DEFAULT_STREAM_ID {
            return Err("apple-avd-vvideo: invalid stream id");
        }
        if coded_format != SCARLET_VIDEO_FORMAT_H264 {
            return Err("apple-avd-vvideo: only H.264 is supported");
        }
        if input_len == 0 {
            return Err("apple-avd-vvideo: input is empty");
        }
        if input_len > AVD_MAPPED_INPUT_BYTES {
            return Err("apple-avd-vvideo: input exceeds mapped buffer");
        }

        self.ensure_mapped_buffer()?;
        let (input_dma_addr, output_dma_addr) = {
            let mapped_buffer = self.mapped_buffer.lock();
            let buffer = mapped_buffer
                .as_ref()
                .ok_or("apple-avd-vvideo: mmap buffer missing")?;
            (
                buffer.as_paddr() as u64,
                (buffer.as_paddr() + MAPPED_OUTPUT_OFFSET) as u64,
            )
        };
        let timestamp = if timestamp == 0 {
            self.next_timestamp()
        } else {
            timestamp
        };
        let request = VideoBackendDecodeRequest {
            stream_id,
            coded_format,
            input_dma_addr,
            input_len: input_len as u32,
            output_dma_addr,
            output_len: AVD_MAPPED_OUTPUT_BYTES as u32,
            timestamp,
        };
        self.backend.submit_decode(&request)
    }

    fn handle_get_buffer(&self, arg: usize) -> Result<i32, &'static str> {
        let info = self.buffer_info()?;
        write_user_value(arg, &info)?;
        Ok(0)
    }

    fn handle_create_session(&self, arg: usize) -> Result<i32, &'static str> {
        let mut info: ScarletVideoSessionInfo = read_user_value(arg)?;
        let stream_id = if info.stream_id == 0 {
            self.backend.create_session(SCARLET_VIDEO_FORMAT_H264)?
        } else {
            info.stream_id
        };
        if stream_id != DEFAULT_STREAM_ID {
            return Err("apple-avd-vvideo: invalid stream id");
        }
        info.stream_id = stream_id;
        info.padding = 0;
        info.buffer = self.buffer_info()?;
        write_user_value(arg, &info)?;
        Ok(0)
    }

    fn handle_destroy_session(&self, arg: usize) -> Result<i32, &'static str> {
        let info: ScarletVideoSessionInfo = read_user_value(arg)?;
        self.backend.destroy_session(info.stream_id)?;
        *self.next_timestamp.lock() = 1;
        Ok(0)
    }

    fn handle_submit(&self, arg: usize) -> Result<i32, &'static str> {
        let submit: ScarletVideoSubmit = read_user_value(arg)?;
        match self.submit_mapped(
            DEFAULT_STREAM_ID,
            submit.coded_format,
            submit.input_len as usize,
            submit.timestamp,
        ) {
            Ok(()) => {
                *self.last_error.lock() = None;
                Ok(0)
            }
            Err(e) => {
                *self.last_error.lock() = Some(e);
                Err(e)
            }
        }
    }

    fn handle_submit_session(&self, arg: usize) -> Result<i32, &'static str> {
        let submit: ScarletVideoSessionSubmit = read_user_value(arg)?;
        match self.submit_mapped(
            submit.stream_id,
            submit.coded_format,
            submit.input_len as usize,
            submit.timestamp,
        ) {
            Ok(()) => {
                *self.last_error.lock() = None;
                Ok(0)
            }
            Err(e) => {
                *self.last_error.lock() = Some(e);
                Err(e)
            }
        }
    }

    fn handle_dequeue(&self, arg: usize) -> Result<i32, &'static str> {
        let Some(decoded) = self.backend.dequeue_frame(DEFAULT_STREAM_ID)? else {
            return Ok(0);
        };
        write_user_value(arg, &decoded.frame)?;
        Ok(1)
    }

    fn handle_dequeue_session(&self, arg: usize) -> Result<i32, &'static str> {
        let mut dequeued: ScarletVideoSessionDequeuedFrame = read_user_value(arg)?;
        let stream_id = if dequeued.stream_id == 0 {
            DEFAULT_STREAM_ID
        } else {
            dequeued.stream_id
        };
        let Some(decoded) = self.backend.dequeue_frame(stream_id)? else {
            return Ok(0);
        };
        dequeued.stream_id = decoded.stream_id;
        dequeued.padding = 0;
        dequeued.frame = decoded.frame;
        write_user_value(arg, &dequeued)?;
        Ok(1)
    }

    fn status_line(&self) -> String {
        let caps = self.backend.capabilities();
        let last_error = self.last_error.lock().unwrap_or("none");
        format!(
            "apple-avd vvideo backend={} h264={} sessions={} input={} output={} last_error={}\n",
            self.backend.name(),
            caps.supports_h264,
            caps.max_sessions,
            caps.mapped_input_len,
            caps.mapped_output_len,
            last_error
        )
    }
}

impl Device for AppleAvdVvideoDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "apple-avd-vvideo"
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

impl CharDevice for AppleAvdVvideoDevice {
    fn read_byte(&self) -> Option<u8> {
        None
    }

    fn write_byte(&self, _byte: u8) -> Result<(), &'static str> {
        Err("apple-avd-vvideo: write a complete access unit")
    }

    fn read(&self, buffer: &mut [u8]) -> usize {
        let status = self.status_line();
        let bytes = status.as_bytes();
        let count = core::cmp::min(buffer.len(), bytes.len());
        buffer[..count].copy_from_slice(&bytes[..count]);
        count
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, &'static str> {
        if buffer.len() > AVD_MAPPED_INPUT_BYTES {
            return Err("apple-avd-vvideo: input exceeds mapped buffer");
        }

        self.ensure_mapped_buffer()?;
        {
            let mut mapped_buffer = self.mapped_buffer.lock();
            let buffer_pages = mapped_buffer
                .as_mut()
                .ok_or("apple-avd-vvideo: mmap buffer missing")?;
            // SAFETY: `buffer_pages` owns at least `AVD_MAPPED_INPUT_BYTES`
            // bytes and `buffer.len()` was checked against that capacity. The
            // source and destination do not overlap.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    buffer.as_ptr(),
                    buffer_pages.as_ptr() as *mut u8,
                    buffer.len(),
                );
            }
        }

        match self.submit_mapped(
            DEFAULT_STREAM_ID,
            SCARLET_VIDEO_FORMAT_H264,
            buffer.len(),
            self.next_timestamp(),
        ) {
            Ok(()) => {
                *self.last_error.lock() = None;
                Ok(buffer.len())
            }
            Err(e) => {
                *self.last_error.lock() = Some(e);
                Err(e)
            }
        }
    }

    fn can_read(&self) -> bool {
        true
    }

    fn can_write(&self) -> bool {
        true
    }

    fn read_at(&self, _position: u64, buffer: &mut [u8]) -> Result<usize, &'static str> {
        Ok(self.read(buffer))
    }
}

impl ControlOps for AppleAvdVvideoDevice {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            VVIDEO_GET_BUFFER => self.handle_get_buffer(arg),
            VVIDEO_SUBMIT => self.handle_submit(arg),
            VVIDEO_DEQUEUE => self.handle_dequeue(arg),
            VVIDEO_CREATE_SESSION => self.handle_create_session(arg),
            VVIDEO_SUBMIT_SESSION => self.handle_submit_session(arg),
            VVIDEO_DEQUEUE_SESSION => self.handle_dequeue_session(arg),
            VVIDEO_DESTROY_SESSION => self.handle_destroy_session(arg),
            _ => Err("apple-avd-vvideo: unsupported control command"),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        alloc::vec![
            (VVIDEO_GET_BUFFER, "Get mmap video buffer layout"),
            (VVIDEO_SUBMIT, "Submit mmap-written coded video access unit"),
            (VVIDEO_DEQUEUE, "Dequeue a decoded mmap video frame"),
            (
                VVIDEO_CREATE_SESSION,
                "Create or query mmap video stream session"
            ),
            (
                VVIDEO_SUBMIT_SESSION,
                "Submit mmap-written coded video access unit for a stream"
            ),
            (
                VVIDEO_DEQUEUE_SESSION,
                "Dequeue a decoded mmap video frame for a stream"
            ),
            (VVIDEO_DESTROY_SESSION, "Destroy mmap video stream session"),
        ]
    }
}

impl MemoryMappingOps for AppleAvdVvideoDevice {
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        if offset % PAGE_SIZE != 0 || length % PAGE_SIZE != 0 {
            return Err("apple-avd-vvideo: mmap offset and length must be page-aligned");
        }
        if offset >= MAPPED_BUFFER_BYTES {
            return Err("apple-avd-vvideo: mmap offset exceeds buffer size");
        }
        if length > MAPPED_BUFFER_BYTES - offset {
            return Err("apple-avd-vvideo: mmap length exceeds buffer size");
        }

        self.ensure_mapped_buffer()?;
        let mapped_buffer = self.mapped_buffer.lock();
        let buffer = mapped_buffer
            .as_ref()
            .ok_or("apple-avd-vvideo: mmap buffer missing")?;
        Ok(MemoryMappingInfo::new(
            buffer.as_paddr() + offset,
            0x3,
            true,
        ))
    }

    fn supports_mmap(&self) -> bool {
        true
    }
}

impl Selectable for AppleAvdVvideoDevice {
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

fn read_user_value<T: Copy>(ptr: usize) -> Result<T, &'static str> {
    if ptr == 0 {
        return Err("apple-avd-vvideo: ioctl pointer is null");
    }
    let task = mytask().ok_or("apple-avd-vvideo: no current task for ioctl")?;
    let mut value = core::mem::MaybeUninit::<T>::uninit();
    // SAFETY: `value` is uninitialized storage for `T`; the byte slice covers
    // exactly that storage and is filled before `assume_init`.
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(value.as_mut_ptr() as *mut u8, core::mem::size_of::<T>())
    };
    copy_from_user(task, ptr, bytes).map_err(|_| "apple-avd-vvideo: failed to copy from user")?;
    // SAFETY: `copy_from_user` initialized every byte of `value`.
    Ok(unsafe { value.assume_init() })
}

fn write_user_value<T: Copy>(ptr: usize, value: &T) -> Result<(), &'static str> {
    if ptr == 0 {
        return Err("apple-avd-vvideo: ioctl pointer is null");
    }
    let task = mytask().ok_or("apple-avd-vvideo: no current task for ioctl")?;
    // SAFETY: `value` is valid for `size_of::<T>()` bytes and is only read.
    let bytes = unsafe {
        core::slice::from_raw_parts(value as *const T as *const u8, core::mem::size_of::<T>())
    };
    copy_to_user(task, ptr, bytes).map_err(|_| "apple-avd-vvideo: failed to copy to user")
}
