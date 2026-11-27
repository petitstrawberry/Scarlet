//! One native report stream per registered HID device. Binding and report
//! dispatch belong to the HID bus. Every open gets an independent input cursor.
use super::{HidDeviceInfo, HidTransport, HidWake};
use crate::{
    arch::Trapframe,
    device::{Device, DeviceCapability, DeviceType, char::CharDevice, manager::DeviceManager},
    library::std::usercopy::{copy_from_user, copy_to_user},
    object::capability::{
        ControlOps, MemoryMappingInfo, MemoryMappingOps, ReadyInterest, ReadySet,
        SelectWaitOutcome, Selectable, StreamError,
    },
    shield::authorization::{Management, permits_management},
    sync::{IrqSpinLock, ReadinessRegistration, WaitResult, Waker},
    task::{CurrentTaskRef, mytask},
};
use alloc::{collections::VecDeque, sync::Arc, vec::Vec};
use core::{
    any::Any,
    mem::size_of,
    sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
};
use scarlet_abi::hidraw as abi;

const INPUT_DEPTH: usize = 32;
const OUTPUT_DEPTH: usize = 8;
static USED_SLOTS: AtomicU32 = AtomicU32::new(0);
const _: () = assert!(abi::SLOTS == u32::BITS as usize);

struct Queues {
    info: abi::Info,
    input: VecDeque<Vec<u8>>,
    end: u64,
    output: VecDeque<Vec<u8>>,
    // The registration owns the transport wake. Detached readers retain only data.
    transport_wake: Option<HidWake>,
}

impl Queues {
    fn set_name(&mut self, name: &[u8]) {
        let count = name.len().min(abi::MAX_NAME);
        self.info.name.fill(0);
        self.info.name[..count].copy_from_slice(&name[..count]);
        self.info.name_len = count as u16;
    }

    fn writable(&self) -> bool {
        self.transport_wake.is_some()
            && self.info.flags & abi::CAN_OUTPUT != 0
            && self.output.len() < OUTPUT_DEPTH
    }

    fn read(&self, cursor: &AtomicU64, buf: &mut [u8]) -> Result<usize, StreamError> {
        let next = cursor.load(Ordering::Relaxed);
        let unread = self.end.wrapping_sub(next);
        if unread > self.input.len() as u64 {
            cursor.store(
                self.end.wrapping_sub(self.input.len() as u64),
                Ordering::Relaxed,
            );
            return Err(StreamError::NoSpace);
        }
        if unread == 0 {
            return Err(if self.transport_wake.is_some() {
                StreamError::WouldBlock
            } else {
                StreamError::EndOfStream
            });
        }
        let report = &self.input[self.input.len() - unread as usize];
        let dest = buf
            .get_mut(..report.len())
            .ok_or(StreamError::InvalidArgument)?;
        dest.copy_from_slice(report);
        cursor.store(next.wrapping_add(1), Ordering::Relaxed);
        Ok(report.len())
    }
}

struct Shared {
    queues: IrqSpinLock<Queues>,
    descriptor: Vec<u8>,
    changed: Waker,
}

pub(super) struct RawDevice {
    shared: Arc<Shared>,
    registration: usize,
    slot_bit: u32,
}

impl RawDevice {
    pub(super) fn publish(
        device: &HidDeviceInfo,
        allow_output: bool,
        wake: HidWake,
    ) -> Result<Self, &'static str> {
        if device.report_descriptor.len() > abi::MAX_DESCRIPTOR {
            return Err("HID report descriptor exceeds raw interface limit");
        }
        let mut info = abi::Info {
            descriptor_len: device.report_descriptor.len() as u32,
            max_output_len: device.max_output_len.min(abi::MAX_REPORT) as u32,
            ..abi::Info::default()
        };
        if let Some(id) = &device.identity {
            info.flags |= abi::HAS_IDENTITY;
            info.vendor_id_source = id.vendor_id_source;
            info.vendor_id = id.vendor_id;
            info.product_id = id.product_id;
            info.version = id.version;
        }
        match &device.transport {
            HidTransport::Usb {
                bus,
                address,
                interface,
            } => {
                info.transport = abi::USB;
                info.usb_bus = *bus;
                info.usb_address = *address;
                info.usb_interface = *interface;
            }
            HidTransport::Bluetooth { controller, peer } => {
                info.transport = abi::BLUETOOTH;
                info.controller = *controller;
                info.peer = *peer;
            }
        }
        if allow_output && info.max_output_len > 0 {
            info.flags |= abi::CAN_OUTPUT;
        }
        let mut queues = Queues {
            info,
            input: VecDeque::with_capacity(INPUT_DEPTH),
            end: 0,
            output: VecDeque::with_capacity(OUTPUT_DEPTH),
            transport_wake: Some(wake),
        };
        queues.set_name(&device.name);
        let shared = Arc::new(Shared {
            queues: IrqSpinLock::new(queues),
            descriptor: device.report_descriptor.clone(),
            changed: Waker::new_interruptible("hidraw"),
        });
        let previous = USED_SLOTS
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                let bit = 1u32.checked_shl(used.trailing_ones())?;
                Some(used | bit)
            })
            .map_err(|_| "All raw HID slots are occupied")?;
        let slot = previous.trailing_ones();
        let node = Arc::new(File::new(shared.clone(), 0));
        let registration = DeviceManager::get_manager()
            .register_device_with_name(alloc::format!("hidraw{slot}"), node);
        Ok(Self {
            shared,
            registration,
            slot_bit: 1 << slot,
        })
    }

    pub(super) fn set_name(&self, name: &[u8]) {
        self.shared.queues.lock().set_name(name);
    }

    pub(super) fn push_input(&self, report: &[u8]) {
        let mut q = self.shared.queues.lock();
        if report.is_empty() || report.len() > abi::MAX_REPORT {
            // Mark an unrepresentable report as a gap for every current reader.
            q.input.clear();
        } else {
            if q.input.len() == INPUT_DEPTH {
                q.input.pop_front();
            }
            q.input.push_back(report.to_vec());
        }
        q.end = q.end.wrapping_add(1);
        drop(q);
        self.shared.changed.wake_all();
    }

    pub(super) fn take_output(&self) -> Option<Vec<u8>> {
        let mut q = self.shared.queues.lock();
        let was_full = q.output.len() == OUTPUT_DEPTH;
        let report = q.output.pop_front();
        drop(q);
        if was_full {
            self.shared.changed.wake_all();
        }
        report
    }
}

impl Drop for RawDevice {
    fn drop(&mut self) {
        let wake = {
            let mut q = self.shared.queues.lock();
            q.output.clear();
            q.transport_wake.take()
        };
        self.shared.changed.wake_all();
        DeviceManager::get_manager().unregister_device(self.registration);
        USED_SLOTS.fetch_and(!self.slot_bit, Ordering::Relaxed);
        drop(wake);
    }
}

fn seat_caller() -> Result<CurrentTaskRef, &'static str> {
    mytask()
        .filter(|task| permits_management(task, Management::PhysicalSeat))
        .ok_or("Raw HID requires physical-seat management")
}

struct File {
    shared: Arc<Shared>,
    cursor: AtomicU64,
    nonblocking: AtomicBool,
}

impl File {
    fn new(shared: Arc<Shared>, next: u64) -> Self {
        Self {
            shared,
            cursor: AtomicU64::new(next),
            nonblocking: AtomicBool::new(false),
        }
    }
}

impl Device for File {
    fn open(self: Arc<Self>) -> Result<Arc<dyn Device>, &'static str> {
        drop(seat_caller()?);
        let q = self.shared.queues.lock();
        if q.transport_wake.is_none() {
            return Err("HID device detached");
        }
        Ok(Arc::new(Self::new(self.shared.clone(), q.end)))
    }
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }
    fn capabilities(&self) -> &'static [DeviceCapability] {
        &[DeviceCapability::PhysicalSeat]
    }
    fn name(&self) -> &'static str {
        "hidraw"
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

impl CharDevice for File {
    fn read_byte(&self) -> Option<u8> {
        let mut byte = [0];
        self.try_read(&mut byte).ok().map(|_| byte[0])
    }
    fn read(&self, buf: &mut [u8]) -> usize {
        self.try_read(buf).unwrap_or(0)
    }
    fn try_read(&self, buf: &mut [u8]) -> Result<usize, StreamError> {
        loop {
            let task = seat_caller().map_err(|_| StreamError::PermissionDenied)?;
            if buf.is_empty() {
                return Ok(0);
            }
            match self.shared.queues.lock().read(&self.cursor, buf) {
                Err(StreamError::WouldBlock) if !self.is_nonblocking() => {}
                Err(StreamError::EndOfStream) => return Ok(0),
                result => return result,
            }
            if self.shared.changed.wait_with_condition(
                task.get_id(),
                task.get_trapframe(),
                None,
                0,
                || self.is_nonblocking() || self.can_read(),
            ) == WaitResult::Interrupted
            {
                return Err(StreamError::Interrupted);
            }
        }
    }
    fn try_read_at(&self, _: u64, buf: &mut [u8]) -> Result<usize, StreamError> {
        self.try_read(buf)
    }
    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        self.write(&[byte]).map(|_| ())
    }
    fn write(&self, report: &[u8]) -> Result<usize, &'static str> {
        drop(seat_caller()?);
        let notify = {
            let mut q = self.shared.queues.lock();
            let wake = q
                .transport_wake
                .as_ref()
                .ok_or("HID device detached")?
                .clone();
            if q.info.flags & abi::CAN_OUTPUT == 0 {
                return Err("Raw HID output unavailable");
            }
            if report.is_empty() || report.len() > q.info.max_output_len as usize {
                return Err("Invalid HID output report length");
            }
            if q.output.len() == OUTPUT_DEPTH {
                return Err("HID output would block");
            }
            q.output.push_back(report.to_vec());
            wake
        };
        notify();
        Ok(report.len())
    }
    fn can_read(&self) -> bool {
        let q = self.shared.queues.lock();
        q.transport_wake.is_none() || self.cursor.load(Ordering::Relaxed) != q.end
    }
    fn can_write(&self) -> bool {
        self.shared.queues.lock().writable()
    }
}

impl ControlOps for File {
    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        alloc::vec![
            (abi::INFO, "Get HID identity"),
            (abi::GET_DESCRIPTOR, "Get HID report descriptor")
        ]
    }
    fn control(&self, cmd: u32, arg: usize) -> Result<i32, &'static str> {
        let task = seat_caller()?;
        match cmd {
            abi::INFO => {
                let info = self.shared.queues.lock().info;
                // SAFETY: Info is repr(C), fully initialized and has no padding.
                let bytes = unsafe {
                    core::slice::from_raw_parts(
                        core::ptr::from_ref(&info).cast(),
                        size_of::<abi::Info>(),
                    )
                };
                copy_to_user(&task, arg, bytes).map_err(|_| "Invalid HID info buffer")?;
                Ok(0)
            }
            abi::GET_DESCRIPTOR => {
                let mut bytes = [0; size_of::<abi::Transfer>()];
                copy_from_user(&task, arg, &mut bytes)
                    .map_err(|_| "Invalid HID descriptor request")?;
                let address = usize::try_from(u64::from_ne_bytes(bytes[..8].try_into().unwrap()))
                    .map_err(|_| "Invalid HID buffer address")?;
                let len = u32::from_ne_bytes(bytes[8..12].try_into().unwrap()) as usize;
                let descriptor = &self.shared.descriptor;
                if bytes[12..] != [0; 4] || len < descriptor.len() {
                    return Err("Invalid HID descriptor request");
                }
                copy_to_user(&task, address, descriptor)
                    .map_err(|_| "Invalid HID descriptor buffer")?;
                Ok(descriptor.len() as i32)
            }
            abi::GET_FEATURE | abi::SET_FEATURE => Err("HID Feature transfer unsupported"),
            scarlet_abi::SCTL_SOCKET_SET_NONBLOCK => {
                self.set_nonblocking(arg != 0);
                Ok(0)
            }
            scarlet_abi::SCTL_SOCKET_GET_NONBLOCK => Ok(i32::from(self.is_nonblocking())),
            _ => Err("Unknown raw HID control"),
        }
    }
}

impl MemoryMappingOps for File {
    fn get_mapping_info(&self, _: usize, _: usize) -> Result<MemoryMappingInfo, &'static str> {
        Err("Raw HID has no mapping")
    }
    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for File {
    fn is_hung_up(&self) -> bool {
        self.shared.queues.lock().transport_wake.is_none()
    }
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let q = self.shared.queues.lock();
        let detached = q.transport_wake.is_none();
        ReadySet {
            read: interest.read && (detached || self.cursor.load(Ordering::Relaxed) != q.end),
            write: interest.write && q.writable(),
            except: interest.except && detached,
        }
    }
    fn subscribe_readiness(
        &self,
        _: ReadyInterest,
        waiter: &Arc<Waker>,
    ) -> Option<Vec<ReadinessRegistration>> {
        Some(alloc::vec![self.shared.changed.subscribe_readiness(waiter)])
    }
    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        tf: &mut Trapframe,
        timeout: Option<u64>,
        min: u64,
    ) -> SelectWaitOutcome {
        let ready = || {
            let r = self.current_ready(interest);
            r.read || r.write || r.except || self.is_hung_up()
        };
        if ready() {
            return SelectWaitOutcome::Ready;
        }
        let Some(task) = mytask() else {
            return SelectWaitOutcome::TimedOut;
        };
        match self
            .shared
            .changed
            .wait_with_condition(task.get_id(), tf, timeout, min, ready)
        {
            WaitResult::Interrupted => SelectWaitOutcome::Interrupted,
            WaitResult::TimedOut => SelectWaitOutcome::TimedOut,
            WaitResult::Woken => SelectWaitOutcome::Ready,
        }
    }
    fn set_nonblocking(&self, enabled: bool) {
        let was_enabled = self.nonblocking.swap(enabled, Ordering::Relaxed);
        if enabled && !was_enabled {
            self.shared.changed.wake_all();
        }
    }
    fn is_nonblocking(&self) -> bool {
        self.nonblocking.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{Task, TaskType, clear_mock_current_task, set_mock_current_task};
    use core::sync::atomic::AtomicUsize;

    fn caller() -> Arc<Task> {
        let mut task = Task::new("hidraw-test".into(), 0, TaskType::User);
        task.set_id(91102);
        let task = Arc::new(task);
        set_mock_current_task(task.clone());
        task
    }

    fn usb() -> HidDeviceInfo {
        HidDeviceInfo {
            transport: HidTransport::Usb {
                bus: 3,
                address: 5,
                interface: 2,
            },
            name: b"HID peripheral".to_vec(),
            identity: None,
            report_descriptor: alloc::vec![
                5, 1, 9, 4, 0xa1, 1, 0x85, 7, 0x75, 8, 0x95, 1, 0x81, 1, 0x91, 1, 0xc0
            ],
            max_output_len: 2,
        }
    }

    fn open(device: &RawDevice) -> Arc<dyn Device> {
        DeviceManager::get_manager()
            .get_device(device.registration)
            .unwrap()
            .open()
            .unwrap()
    }

    #[test_case]
    fn independent_report_cursors_loss_and_disconnect() {
        let _task = caller();
        let dev = RawDevice::publish(&usb(), false, Arc::new(|| {})).unwrap();
        let first = open(&dev);
        first.set_nonblocking(true);
        let fast = first.as_char_device().unwrap();
        let mut data = [0; 4];
        assert!(matches!(
            fast.try_read_at(0, &mut data),
            Err(StreamError::WouldBlock)
        ));
        dev.push_input(&[7, 0]);
        let second = open(&dev);
        let slow = second.as_char_device().unwrap();
        assert!(!second.is_nonblocking());
        assert!(!slow.can_read());
        assert!(matches!(
            fast.try_read_at(0, &mut data[..1]),
            Err(StreamError::InvalidArgument)
        ));
        assert_eq!(fast.try_read_at(0, &mut data).unwrap(), 2);
        for i in 1..=INPUT_DEPTH as u8 + 2 {
            dev.push_input(&[7, i]);
            assert_eq!(fast.try_read_at(0, &mut data).unwrap(), 2);
            assert_eq!(data[..2], [7, i]);
        }
        assert!(matches!(
            slow.try_read_at(0, &mut data),
            Err(StreamError::NoSpace)
        ));
        assert!(!slow.can_write());
        assert!(slow.write(&[7, 1]).is_err());
        assert!(dev.take_output().is_none());
        let bit = dev.slot_bit;
        drop(dev);
        let replacement = RawDevice::publish(&usb(), true, Arc::new(|| {})).unwrap();
        assert_eq!(replacement.slot_bit, bit);
        replacement.push_input(&[7, 99]);
        assert!(second.is_hung_up());
        for i in 3..=INPUT_DEPTH as u8 + 2 {
            assert_eq!(slow.try_read_at(0, &mut data).unwrap(), 2);
            assert_eq!(data[..2], [7, i]);
        }
        assert_eq!(slow.try_read_at(0, &mut data), Ok(0));
        assert_eq!(fast.try_read_at(0, &mut data), Ok(0));
        assert!(!open(&replacement).is_hung_up());
        clear_mock_current_task();
    }

    #[test_case]
    fn raw_metadata_descriptor_output_backpressure_and_seat_authority() {
        use super::super::HidIdentity;
        let task = caller();
        let addr = 0x40000;
        task.allocate_data_pages(addr, 1).unwrap();
        let notifications = Arc::new(AtomicUsize::new(0));
        let counter = notifications.clone();
        let mut device = usb();
        device.transport = HidTransport::Bluetooth {
            controller: [1, 2, 3, 4, 5, 6],
            peer: [11, 12, 13, 14, 15, 16],
        };
        device.identity = Some(HidIdentity {
            vendor_id_source: 2,
            vendor_id: 17,
            product_id: 23,
            version: 42,
        });
        let dev = RawDevice::publish(
            &device,
            true,
            Arc::new(move || {
                counter.fetch_add(1, Ordering::Relaxed);
            }),
        )
        .unwrap();
        let fd = open(&dev);
        let raw = fd.as_char_device().unwrap();
        dev.set_name(b"peer");
        assert_eq!(fd.control(abi::INFO, addr), Ok(0));
        let mut info = [0; size_of::<abi::Info>()];
        copy_from_user(&task, addr, &mut info).unwrap();
        assert_eq!(
            u16::from_ne_bytes(info[..2].try_into().unwrap()),
            abi::BLUETOOTH
        );
        assert_eq!(
            u16::from_ne_bytes(info[2..4].try_into().unwrap()),
            abi::HAS_IDENTITY | abi::CAN_OUTPUT
        );
        assert_eq!(u16::from_ne_bytes(info[6..8].try_into().unwrap()), 17);
        assert_eq!(info[26..32], [11, 12, 13, 14, 15, 16]);
        assert_eq!(&info[38..43], b"peer\0");
        let mut transfer = [0; size_of::<abi::Transfer>()];
        transfer[..8].copy_from_slice(&((addr + 512) as u64).to_ne_bytes());
        copy_to_user(&task, addr, &transfer).unwrap();
        assert!(fd.control(abi::GET_DESCRIPTOR, addr).is_err());
        transfer[8..12].copy_from_slice(&(device.report_descriptor.len() as u32).to_ne_bytes());
        copy_to_user(&task, addr, &transfer).unwrap();
        assert_eq!(
            fd.control(abi::GET_DESCRIPTOR, addr),
            Ok(device.report_descriptor.len() as i32)
        );
        let mut desc = alloc::vec![0; device.report_descriptor.len()];
        copy_from_user(&task, addr + 512, &mut desc).unwrap();
        assert_eq!(desc, device.report_descriptor);
        assert!(fd.control(abi::GET_FEATURE, addr).is_err());
        assert!(fd.control(abi::SET_FEATURE, addr).is_err());
        assert!(raw.write(&[7, 1, 2]).is_err());
        for i in 0..OUTPUT_DEPTH {
            assert_eq!(raw.write(&[7, i as u8]), Ok(2));
        }
        assert_eq!(notifications.load(Ordering::Relaxed), OUTPUT_DEPTH);
        assert!(!fd.current_ready(ReadyInterest::write()).write);
        assert!(raw.write(&[7, 99]).is_err());
        assert_eq!(dev.take_output().unwrap(), [7, 0]);
        assert!(fd.current_ready(ReadyInterest::write()).write);
        task.credentials.write().set_authority(0, 0).unwrap();
        assert!(
            DeviceManager::get_manager()
                .get_device(dev.registration)
                .unwrap()
                .open()
                .is_err()
        );
        assert!(fd.control(abi::INFO, addr).is_err());
        dev.push_input(&[7, 23]);
        assert!(matches!(
            raw.try_read_at(0, &mut [0; 2]),
            Err(StreamError::PermissionDenied)
        ));
        assert!(raw.write(&[7, 23]).is_err());
        clear_mock_current_task();
    }
}
