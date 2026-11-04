//! Anonymous input channels. Only the producer can inject events; only the
//! receiver can consume them. Publication in a View is a manager decision.
use super::{InputEvent, event_device::*};
use crate::{
    arch::Trapframe,
    fs::{FileMetadata, FilePermission, FileType},
    object::{
        KernelObject,
        capability::{
            ControlOps, FileObject, MemoryMappingOps, ReadyInterest, ReadySet, SelectWaitOutcome,
            Selectable, StreamError, StreamOps,
        },
    },
    sync::{IrqSpinLock, Waker, waker::WaitResult},
};
use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use core::{
    any::Any,
    sync::atomic::{AtomicBool, Ordering},
};

struct Channel {
    device: EventDevice,
    values: IrqSpinLock<BTreeMap<(u16, u16), i32>>,
    kind: u32,
    width: i32,
    height: i32,
    producer_alive: AtomicBool,
    receiver_alive: AtomicBool,
    waker: Arc<Waker>,
}
pub struct InputEndpoint {
    channel: Arc<Channel>,
    producer: bool,
    nonblocking: AtomicBool,
}
impl InputEndpoint {
    /// Create keyboard (kind 1, dimensions zero) or direct single-contact
    /// touchscreen (kind 4, dimensions 2..=65536) endpoints.
    pub fn pair(kind: u32, width: u32, height: u32) -> Result<(Arc<Self>, Arc<Self>), StreamError> {
        let metadata = match kind {
            1 if width == 0 && height == 0 => {
                InputDeviceMetadata::new(InputDeviceKind::Keyboard, INPUT_CAP_KEY)
            }
            4 if (2..=65536).contains(&width) && (2..=65536).contains(&height) => {
                InputDeviceMetadata::new(
                    InputDeviceKind::Touchscreen,
                    INPUT_CAP_KEY | INPUT_CAP_DIRECT_TOUCH,
                )
                .with_absolute_axis(0, 0, width as i32 - 1)
                .unwrap()
                .with_absolute_axis(1, 0, height as i32 - 1)
                .unwrap()
            }
            _ => return Err(StreamError::InvalidArgument),
        };
        let channel = Arc::new(Channel {
            device: EventDevice::new_with_metadata("input", metadata),
            values: IrqSpinLock::new(BTreeMap::new()),
            kind,
            width: width as i32,
            height: height as i32,
            producer_alive: AtomicBool::new(true),
            receiver_alive: AtomicBool::new(true),
            waker: Arc::new(Waker::new_interruptible("virtual-input")),
        });
        Ok((
            Arc::new(Self {
                channel: channel.clone(),
                producer: true,
                nonblocking: AtomicBool::new(false),
            }),
            Arc::new(Self {
                channel,
                producer: false,
                nonblocking: AtomicBool::new(false),
            }),
        ))
    }
    fn valid(&self, type_: u16, code: u16, value: i32) -> bool {
        match (type_, code) {
            (0, 0) => value == 0,
            (1, code) if self.channel.kind == 1 => {
                (1..=0xff).contains(&code) && (0..=2).contains(&value)
            }
            (1, 0x14a) if self.channel.kind == 4 => (0..=1).contains(&value),
            (3, 0) if self.channel.kind == 4 => (0..self.channel.width).contains(&value),
            (3, 1) if self.channel.kind == 4 => (0..self.channel.height).contains(&value),
            _ => false,
        }
    }
}
impl Drop for InputEndpoint {
    fn drop(&mut self) {
        if self.producer {
            self.channel.producer_alive.store(false, Ordering::Release);
        } else {
            self.channel.receiver_alive.store(false, Ordering::Release);
        }
        self.channel.waker.wake_all();
    }
}
impl StreamOps for InputEndpoint {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        if self.producer {
            return Err(StreamError::PermissionDenied);
        }
        if buffer.is_empty() {
            return Ok(0);
        }
        if buffer.len() < InputEvent::size() {
            return Err(StreamError::InvalidArgument);
        }
        let count = self.channel.device.drain_records(buffer);
        if count != 0 || !self.channel.producer_alive.load(Ordering::Acquire) {
            Ok(count)
        } else {
            Err(StreamError::WouldBlock)
        }
    }
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        if !self.producer {
            return Err(StreamError::PermissionDenied);
        }
        if !self.channel.receiver_alive.load(Ordering::Acquire) {
            return Err(StreamError::BrokenPipe);
        }
        if buffer.is_empty() {
            return Ok(0);
        }
        // Each write is a complete frame. Validate all records before changing
        // the shared queue; producer timestamps never cross the boundary.
        if buffer.len() > 128 * InputEvent::size()
            || !buffer.len().is_multiple_of(InputEvent::size())
        {
            return Err(StreamError::InvalidArgument);
        }
        let mut events = Vec::new();
        for record in buffer.chunks_exact(InputEvent::size()) {
            let type_ = u16::from_ne_bytes(record[8..10].try_into().unwrap());
            let code = u16::from_ne_bytes(record[10..12].try_into().unwrap());
            let value = i32::from_ne_bytes(record[12..16].try_into().unwrap());
            if !self.valid(type_, code, value) {
                return Err(StreamError::InvalidArgument);
            }
            events.push((type_, code, value));
        }
        if events.last() != Some(&(0, 0, 0)) {
            return Err(StreamError::InvalidArgument);
        }
        {
            let mut values = self.channel.values.lock();
            for &(type_, code, value) in &events {
                if type_ != 0 {
                    values.insert(
                        (type_, code),
                        if type_ == 1 && value == 2 { 1 } else { value },
                    );
                }
            }
            self.channel.device.push_events(&events);
        }
        self.channel.waker.wake_all();
        Ok(buffer.len())
    }
}
impl ControlOps for InputEndpoint {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        if self.producer {
            return Err("Producer has no device query capability");
        }
        if command == 0x5353_0007 {
            self.set_nonblocking(arg != 0);
            return Ok(0);
        }
        if matches!(
            command,
            SCTL_INPUT_HAS_EVENT_CODE | SCTL_INPUT_GET_EVENT_VALUE
        ) {
            let packed = u32::try_from(arg).map_err(|_| "Invalid input event selector")?;
            let type_ = (packed >> 16) as u16;
            let code = packed as u16;
            let supported = self.valid(type_, code, 0);
            if command == SCTL_INPUT_HAS_EVENT_CODE {
                return Ok(i32::from(supported));
            }
            if !supported {
                return Err("Unsupported input event code");
            }
            return Ok(*self.channel.values.lock().get(&(type_, code)).unwrap_or(&0));
        }
        self.channel.device.control(command, arg)
    }
}
impl MemoryMappingOps for InputEndpoint {
    fn get_mapping_info(
        &self,
        _: usize,
        _: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("Input channels cannot be mapped")
    }
}
impl Selectable for InputEndpoint {
    fn subscribe_readiness(&self, _interest: ReadyInterest, waiter: &Arc<Waker>)
        -> Option<Vec<crate::sync::ReadinessRegistration>> {
        // Injection and either endpoint's close notify this shared source.
        Some(alloc::vec![self.channel.waker.subscribe_readiness(waiter)])
    }
    fn is_hung_up(&self) -> bool {
        if self.producer {
            !self.channel.receiver_alive.load(Ordering::Acquire)
        } else {
            !self.channel.producer_alive.load(Ordering::Acquire)
        }
    }
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        if self.producer {
            ReadySet {
                read: false,
                write: interest.write && self.channel.receiver_alive.load(Ordering::Acquire),
                except: !self.channel.receiver_alive.load(Ordering::Acquire),
            }
        } else {
            let closed = !self.channel.producer_alive.load(Ordering::Acquire);
            let mut ready = self.channel.device.current_ready(interest);
            ready.read |= interest.read && closed;
            ready.except = closed;
            ready
        }
    }
    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        tf: &mut Trapframe,
        timeout: Option<u64>,
        _: u64,
    ) -> SelectWaitOutcome {
        let ready = || {
            let r = self.current_ready(interest);
            r.read || r.write || r.except
        };
        if ready() {
            return SelectWaitOutcome::Ready;
        }
        let Some(task) = crate::task::mytask() else {
            return SelectWaitOutcome::TimedOut;
        };
        let result = match timeout {
            Some(ns) => self
                .channel
                .waker
                .clone()
                .wait_with_timeout_result_until_owned(task.get_id(), tf, ns, ready),
            None => self
                .channel
                .waker
                .clone()
                .wait_result_until_owned(task.get_id(), tf, ready),
        };
        match result {
            WaitResult::Interrupted => SelectWaitOutcome::Interrupted,
            WaitResult::TimedOut => SelectWaitOutcome::TimedOut,
            _ => SelectWaitOutcome::Ready,
        }
    }
    fn set_nonblocking(&self, enabled: bool) {
        self.nonblocking.store(enabled, Ordering::Release);
    }
    fn is_nonblocking(&self) -> bool {
        self.nonblocking.load(Ordering::Acquire)
    }
}
impl FileObject for InputEndpoint {
    fn allows_execution(&self) -> bool {
        false
    }
    fn seek(&self, _: crate::object::capability::file::SeekFrom) -> Result<u64, StreamError> {
        Err(StreamError::SeekError)
    }
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(FileMetadata {
            file_type: FileType::Pipe,
            size: 0,
            permissions: FilePermission {
                read: !self.producer,
                write: self.producer,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: 0,
            link_count: 1,
        })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Native ABI: (kind, width, height, output: *mut [i32; 2]); output is
/// [producer, receiver]. Both handles are removed if publication fails.
pub fn sys_input_channel_create(tf: &mut Trapframe) -> usize {
    let Some(task) = crate::task::mytask() else {
        return usize::MAX;
    };
    let args = [tf.get_arg(0), tf.get_arg(1), tf.get_arg(2), tf.get_arg(3)];
    tf.increment_pc_next(&task);
    if args[..3].iter().any(|arg| *arg > u32::MAX as usize) {
        return usize::MAX;
    }
    let Ok((producer, receiver)) =
        InputEndpoint::pair(args[0] as u32, args[1] as u32, args[2] as u32)
    else {
        return usize::MAX;
    };
    use crate::object::handle::{AccessMode, HandleMetadata, HandleType};
    let metadata = |access_mode| HandleMetadata {
        handle_type: HandleType::IpcChannel,
        access_mode,
        special_semantics: None,
    };
    let Ok(send) = task.handle_table.insert_with_metadata(
        KernelObject::from_file_object(producer),
        metadata(AccessMode::WriteOnly),
    ) else {
        return usize::MAX;
    };
    let Ok(receive) = task.handle_table.insert_with_metadata(
        KernelObject::from_file_object(receiver),
        metadata(AccessMode::ReadOnly),
    ) else {
        task.handle_table.remove(send);
        return usize::MAX;
    };
    let mut bytes = [0; 8];
    bytes[..4].copy_from_slice(&send.to_le_bytes());
    bytes[4..].copy_from_slice(&receive.to_le_bytes());
    if crate::library::std::usercopy::copy_to_user(&task, args[3], &bytes).is_err() {
        task.handle_table.remove(send);
        task.handle_table.remove(receive);
        return usize::MAX;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    fn frame(events: &[(u16, u16, i32)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for &(type_, code, value) in events {
            bytes.extend_from_slice(&u64::MAX.to_ne_bytes());
            bytes.extend_from_slice(&type_.to_ne_bytes());
            bytes.extend_from_slice(&code.to_ne_bytes());
            bytes.extend_from_slice(&value.to_ne_bytes());
        }
        bytes
    }
    #[test_case]
    fn input_endpoints_separate_authority_and_preserve_frames_and_close() {
        let (send, receive) = InputEndpoint::pair(1, 0, 0).unwrap();
        let bytes = frame(&[(1, 30, 1), (0, 0, 0)]);
        assert!(matches!(
            receive.write(&bytes),
            Err(StreamError::PermissionDenied)
        ));
        assert!(matches!(
            send.read(&mut [0; 16]),
            Err(StreamError::PermissionDenied)
        ));
        assert!(send.control(SCTL_INPUT_GET_KIND, 0).is_err());
        assert_eq!(receive.control(SCTL_INPUT_GET_KIND, 0), Ok(1));
        assert!(matches!(
            receive.read(&mut [0; 16]),
            Err(StreamError::WouldBlock)
        ));
        assert!(matches!(
            send.write(&bytes[..16]),
            Err(StreamError::InvalidArgument)
        ));
        assert!(!receive.current_ready(ReadyInterest::read()).read);
        assert_eq!(send.write(&bytes).unwrap(), 32);
        drop(send);
        assert!(receive.is_hung_up());
        let mut output = [0; 48];
        assert_eq!(receive.read(&mut output).unwrap(), 32);
        assert_ne!(&output[..8], &u64::MAX.to_ne_bytes());
        assert_eq!(&output[8..16], &bytes[8..16]);
        assert_eq!(receive.read(&mut output).unwrap(), 0);
    }
    #[test_case]
    fn input_touch_frames_validate_atomically_and_use_existing_overflow_recovery() {
        assert!(InputEndpoint::pair(4, 1, 100).is_err());
        let (send, receive) = InputEndpoint::pair(4, 100, 200).unwrap();
        assert!(matches!(
            send.write(&frame(&[(3, 0, 5), (3, 1, 200), (0, 0, 0)])),
            Err(StreamError::InvalidArgument)
        ));
        assert!(!receive.current_ready(ReadyInterest::read()).read);
        let bytes = frame(&[(3, 0, 99), (3, 1, 199), (1, 0x14a, 1), (0, 0, 0)]);
        for _ in 0..65 {
            send.write(&bytes).unwrap();
        }
        let mut output = vec![0; 4096];
        let count = receive.read(&mut output).unwrap();
        assert_eq!(count, 32);
        assert_eq!(u16::from_ne_bytes(output[10..12].try_into().unwrap()), 3);
        assert_eq!(u16::from_ne_bytes(output[26..28].try_into().unwrap()), 0);
        assert_eq!(
            receive.control(SCTL_INPUT_GET_EVENT_VALUE, (1 << 16) | 0x14a),
            Ok(1)
        );
        assert_eq!(receive.control(SCTL_INPUT_GET_EVENT_VALUE, 3 << 16), Ok(99));
        assert_eq!(
            receive.control(SCTL_INPUT_HAS_EVENT_CODE, (3 << 16) | 2),
            Ok(0)
        );
        receive.control(0x5353_0007, 1).unwrap();
        assert!(receive.is_nonblocking());
        drop(receive);
        assert!(matches!(send.write(&bytes), Err(StreamError::BrokenPipe)));
    }
}
