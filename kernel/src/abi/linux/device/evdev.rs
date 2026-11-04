//! Linux 64-bit evdev wire adapter over native input channel capabilities.
//! UAPI layouts: Linux v6.1 include/uapi/linux/input.h.
use crate::abi::linux::generic::errno;
use crate::{
    arch::Trapframe,
    device::input::event_device::*,
    fs::FileMetadata,
    object::{
        KernelObject,
        capability::{
            ControlOps, FileObject, MemoryMappingInfo, MemoryMappingOps, ReadyInterest, ReadySet,
            SelectWaitOutcome, Selectable, StreamError, StreamOps,
        },
    },
    task::Task,
};
use alloc::{sync::Arc, vec, vec::Vec};
use core::{
    any::Any,
    sync::atomic::{AtomicI32, Ordering},
};

pub struct Evdev {
    inner: Arc<dyn FileObject>,
    clock: AtomicI32,
}
/// An input endpoint can be opened for event reads and configuration without
/// granting the write side of Linux's conventional evdev open mode.
pub fn is_receive_only_input(object: &KernelObject) -> bool {
    object.as_file().is_some_and(is_receive_only_input_file)
}
fn is_receive_only_input_file(inner: &dyn FileObject) -> bool {
    !inner.as_any().is::<Evdev>()
        && inner.control(SCTL_INPUT_HAS_EVENT_CODE, 0) == Ok(1)
        && matches!(inner.control(SCTL_INPUT_GET_KIND, 0), Ok(1 | 4))
}
/// Preserve open-description sharing through dup, fork and descriptor transfer.
/// Only devices declaring exact event-code support can be represented honestly.
pub fn wrap(object: KernelObject) -> KernelObject {
    match object {
        KernelObject::File(inner) if is_receive_only_input_file(&*inner) => {
            KernelObject::from_file_object(Arc::new(Evdev {
                inner,
                clock: AtomicI32::new(0),
            }))
        }
        object => object,
    }
}
impl Evdev {
    fn query(&self, command: u32, arg: usize) -> Result<i32, usize> {
        self.inner.control(command, arg).map_err(|_| errno::EINVAL)
    }
    fn supports(&self, type_: u16, code: u16) -> bool {
        self.query(
            SCTL_INPUT_HAS_EVENT_CODE,
            ((type_ as usize) << 16) | code as usize,
        ) == Ok(1)
    }
    fn value(&self, type_: u16, code: u16) -> Result<i32, usize> {
        self.query(
            SCTL_INPUT_GET_EVENT_VALUE,
            ((type_ as usize) << 16) | code as usize,
        )
    }
    fn bitmap(&self, type_: u16, current: bool) -> Vec<u8> {
        let count = match type_ {
            1 => 768,
            3 => 64,
            _ => 32,
        };
        let mut bits = vec![0; ((count + 63) / 64) * 8];
        for code in 0..count {
            let enabled = if current {
                self.value(type_, code as u16).unwrap_or(0) != 0
            } else if type_ == 0 {
                match code {
                    0 => true,
                    1 => self.supports(1, 30) || self.supports(1, 0x14a),
                    3 => self.supports(3, 0),
                    _ => false,
                }
            } else {
                self.supports(type_, code as u16)
            };
            if enabled {
                bits[code / 8] |= 1 << (code % 8);
            }
        }
        bits
    }
    fn read_ioctl(&self, number: u8, size: usize) -> Result<(Vec<u8>, usize), usize> {
        let (mut bytes, variable) = match number {
            1 if size == 4 => (0x010001u32.to_ne_bytes().to_vec(), false),
            2 if size == 8 => {
                // BUS_VIRTUAL, no claimed physical vendor/product identity.
                let mut id = vec![0; 8];
                id[..2].copy_from_slice(&6u16.to_ne_bytes());
                (id, false)
            }
            6 => {
                let name = if self.query(SCTL_INPUT_GET_KIND, 0)? == 4 {
                    b"Scarlet virtual touchscreen\0".as_slice()
                } else {
                    b"Scarlet virtual keyboard\0".as_slice()
                };
                (name.to_vec(), true)
            }
            7 | 8 => return Err(errno::ENOENT),
            9 => {
                let mut properties = vec![0; 8];
                if self.query(SCTL_INPUT_GET_CAPABILITIES, 0)? as u32 & INPUT_CAP_DIRECT_TOUCH != 0
                {
                    properties[0] |= 2;
                }
                (properties, true)
            }
            0x18 => (self.bitmap(1, true), true),
            // These channels provide no LEDs, sounds or switches.
            0x19 | 0x1a | 0x1b => (vec![0; 8], true),
            0x20..=0x3f => (self.bitmap((number - 0x20) as u16, false), true),
            0x40..=0x7f if size == 24 => {
                let axis = (number - 0x40) as u16;
                if !self.supports(3, axis) {
                    return Err(errno::EINVAL);
                }
                let values = [
                    self.value(3, axis)?,
                    self.query(SCTL_INPUT_GET_ABS_MIN, axis as usize)?,
                    self.query(SCTL_INPUT_GET_ABS_MAX, axis as usize)?,
                    0,
                    0,
                    0,
                ];
                let mut bytes = Vec::new();
                for value in values {
                    bytes.extend_from_slice(&value.to_ne_bytes());
                }
                (bytes, false)
            }
            _ => return Err(errno::ENOTTY),
        };
        if variable {
            bytes.truncate(size);
        }
        let result = if variable { bytes.len() } else { 0 };
        Ok((bytes, result))
    }
    fn ioctl(&self, task: &Task, request: u32, arg: usize) -> Result<usize, usize> {
        let size = ((request >> 16) & 0x3fff) as usize;
        let number = request as u8;
        if (request >> 8) & 0xff != b'E' as u32 {
            return Err(errno::ENOTTY);
        }
        if request == 0x400445a0 {
            let mut bytes = [0; 4];
            crate::library::std::usercopy::copy_from_user(task, arg, &mut bytes)
                .map_err(|_| errno::EFAULT)?;
            let clock = i32::from_ne_bytes(bytes);
            if !matches!(clock, 0 | 1 | 7) {
                return Err(errno::EINVAL);
            }
            self.clock.store(clock, Ordering::Release);
            return Ok(0);
        }
        if request >> 30 != 2 {
            return Err(errno::ENOTTY);
        }
        let (bytes, result) = self.read_ioctl(number, size)?;
        crate::library::std::usercopy::copy_to_user(task, arg, &bytes)
            .map_err(|_| errno::EFAULT)?;
        Ok(result)
    }
}
/// Dispatch only evdev open descriptions; other device ioctl routing is unchanged.
pub fn ioctl(object: &KernelObject, task: &Task, request: u32, arg: usize) -> Option<usize> {
    let device = object.as_file()?.as_any().downcast_ref::<Evdev>()?;
    if (request >> 8) & 0xff != b'E' as u32 {
        return None;
    }
    Some(
        device
            .ioctl(task, request, arg)
            .unwrap_or_else(errno::to_result),
    )
}
impl StreamOps for Evdev {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let count = (buffer.len() / 24).min(256);
        if count == 0 {
            return Err(StreamError::InvalidArgument);
        }
        let clock = self.clock.load(Ordering::Acquire);
        let offset = if clock == 0 {
            crate::time::system_time_ns().ok_or(StreamError::DeviceError)? as i128
                - crate::time::current_time_ns() as i128
        } else {
            0
        };
        let mut native = vec![0; count * 16];
        let amount = self.inner.read(&mut native)?;
        if amount > native.len() || !amount.is_multiple_of(16) {
            return Err(StreamError::IoError);
        }
        for (source, target) in native[..amount]
            .chunks_exact(16)
            .zip(buffer.chunks_exact_mut(24))
        {
            let ns = (u64::from_ne_bytes(source[..8].try_into().unwrap()) as i128 + offset)
                .clamp(0, u64::MAX as i128) as u64;
            target[..8].copy_from_slice(&(ns / 1_000_000_000).to_ne_bytes());
            target[8..16].copy_from_slice(&((ns % 1_000_000_000) / 1000).to_ne_bytes());
            target[16..24].copy_from_slice(&source[8..16]);
        }
        Ok(amount / 16 * 24)
    }
    fn write(&self, _: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::PermissionDenied)
    }
}
impl ControlOps for Evdev {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        self.inner.control(command, arg)
    }
}
impl MemoryMappingOps for Evdev {
    fn get_mapping_info(&self, _: usize, _: usize) -> Result<MemoryMappingInfo, &'static str> {
        Err("Input streams cannot be mapped")
    }
}
impl Selectable for Evdev {
    fn is_hung_up(&self) -> bool {
        self.inner.is_hung_up()
    }
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        self.inner.current_ready(interest)
    }
    fn subscribe_readiness(
        &self,
        interest: ReadyInterest,
        waiter: &Arc<crate::sync::Waker>,
    ) -> Option<Vec<crate::sync::readiness::ReadinessRegistration>> {
        self.inner.subscribe_readiness(interest, waiter)
    }
    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        tf: &mut Trapframe,
        timeout: Option<u64>,
        min: u64,
    ) -> SelectWaitOutcome {
        self.inner.wait_until_ready(interest, tf, timeout, min)
    }
    fn set_nonblocking(&self, enabled: bool) {
        self.inner.set_nonblocking(enabled)
    }
    fn is_nonblocking(&self) -> bool {
        self.inner.is_nonblocking()
    }
}
impl FileObject for Evdev {
    fn allows_execution(&self) -> bool {
        false
    }
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        self.inner.metadata()
    }
    fn seek(&self, _: crate::object::capability::file::SeekFrom) -> Result<u64, StreamError> {
        Err(StreamError::SeekError)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::input::virtual_device::InputEndpoint;
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
    fn evdev_records_are_whole_and_queries_reflect_native_state() {
        let (send, receive) = InputEndpoint::pair(4, 640, 480).unwrap();
        let object = wrap(KernelObject::from_file_object(receive));
        let device = object
            .as_file()
            .unwrap()
            .as_any()
            .downcast_ref::<Evdev>()
            .unwrap();
        device.clock.store(1, Ordering::Release);
        send.write(&frame(&[
            (3, 0, 123),
            (3, 1, 321),
            (1, 0x14a, 1),
            (0, 0, 0),
        ]))
        .unwrap();
        assert!(matches!(
            device.read(&mut [0; 23]),
            Err(StreamError::InvalidArgument)
        ));
        let mut record = [0; 25];
        assert_eq!(device.read(&mut record).unwrap(), 24);
        assert_eq!(u16::from_ne_bytes(record[16..18].try_into().unwrap()), 3);
        assert_eq!(i32::from_ne_bytes(record[20..24].try_into().unwrap()), 123);
        assert!(u64::from_ne_bytes(record[8..16].try_into().unwrap()) < 1_000_000);
        let (axis, ret) = device.read_ioctl(0x40, 24).unwrap();
        assert_eq!(ret, 0);
        assert_eq!(i32::from_ne_bytes(axis[..4].try_into().unwrap()), 123);
        assert_eq!(i32::from_ne_bytes(axis[8..12].try_into().unwrap()), 639);
        assert_eq!(device.read_ioctl(0x42, 24), Err(errno::EINVAL));
        let (keys, ret) = device.read_ioctl(0x18, 96).unwrap();
        assert_eq!(ret, 96);
        assert_ne!(keys[0x14a / 8] & (1 << (0x14a % 8)), 0);
        assert_eq!(device.read_ioctl(9, 1).unwrap(), (vec![2], 1));
        let mut tail = [0; 96];
        assert_eq!(device.read(&mut tail).unwrap(), 72);
        assert!(matches!(
            device.read(&mut tail),
            Err(StreamError::WouldBlock)
        ));
        drop(send);
        assert!(device.is_hung_up());
        assert_eq!(device.read(&mut tail).unwrap(), 0);
    }
    #[test_case]
    fn evdev_ioctl_validates_user_buffers_and_clock_selection() {
        let task = Task::new("evdev-test".into(), 0, crate::task::TaskType::User);
        let base = crate::environment::USER_STACK_END - 2 * crate::environment::PAGE_SIZE;
        task.allocate_stack_pages(base, 2).unwrap();
        let (_, receiver) = InputEndpoint::pair(4, 640, 480).unwrap();
        let object = wrap(KernelObject::from_file_object(receiver));
        let device = object
            .as_file()
            .unwrap()
            .as_any()
            .downcast_ref::<Evdev>()
            .unwrap();
        let pointer = base + crate::environment::PAGE_SIZE - 2;
        assert_eq!(ioctl(&object, &task, 0x80044501, pointer), Some(0));
        let mut version = [0; 4];
        crate::library::std::usercopy::copy_from_user(&task, pointer, &mut version).unwrap();
        assert_eq!(u32::from_ne_bytes(version), 0x010001);
        assert_eq!(
            ioctl(&object, &task, 0x80044501, usize::MAX),
            Some(errno::to_result(errno::EFAULT))
        );
        crate::library::std::usercopy::copy_to_user(&task, pointer, &1i32.to_ne_bytes()).unwrap();
        assert_eq!(ioctl(&object, &task, 0x400445a0, pointer), Some(0));
        assert_eq!(device.clock.load(Ordering::Acquire), 1);
        crate::library::std::usercopy::copy_to_user(&task, pointer, &2i32.to_ne_bytes()).unwrap();
        assert_eq!(
            ioctl(&object, &task, 0x400445a0, pointer),
            Some(errno::to_result(errno::EINVAL))
        );
        assert_eq!(device.clock.load(Ordering::Acquire), 1);
        assert_eq!(
            ioctl(&object, &task, 0x40044590, 0),
            Some(errno::to_result(errno::ENOTTY))
        );
    }
    #[test_case]
    fn evdev_wrap_is_idempotent_and_does_not_expose_producers() {
        let (send, receive) = InputEndpoint::pair(1, 0, 0).unwrap();
        let producer = wrap(KernelObject::from_file_object(send));
        assert!(producer.as_file().unwrap().as_any().is::<InputEndpoint>());
        let device = wrap(wrap(KernelObject::from_file_object(receive)));
        let device = device
            .as_file()
            .unwrap()
            .as_any()
            .downcast_ref::<Evdev>()
            .unwrap();
        let bits = device.read_ioctl(0x21, 96).unwrap().0;
        assert_ne!(bits[30 / 8] & (1 << (30 % 8)), 0);
        assert_eq!(bits[0x110 / 8] & (1 << (0x110 % 8)), 0);
        assert_eq!(device.read_ioctl(6, 7).unwrap(), (b"Scarlet".to_vec(), 7));
        assert_eq!(device.read_ioctl(1, 8), Err(errno::ENOTTY));
    }
}
