//! Per-open producer for a named logical gamepad. Both creation and injection
//! check the current caller, including after a producer handle is transferred.
use super::{
    InputEvent,
    event_device::{EventDevice, INPUT_CAP_KEY, InputDeviceKind, InputDeviceMetadata},
};
use crate::{
    arch::Trapframe,
    device::{Device, DeviceCapability, DeviceType, char::CharDevice, manager::DeviceManager},
    library::std::usercopy::copy_from_user,
    object::capability::{
        ControlOps, MemoryMappingInfo, MemoryMappingOps, ReadyInterest, ReadySet,
        SelectWaitOutcome, Selectable,
    },
    shield::authorization::{Management, permits_management},
    sync::Mutex,
    task::{CurrentTaskRef, mytask},
};
use alloc::{sync::Arc, vec::Vec};
use core::{any::Any, mem::size_of};
use scarlet_abi::gamepad::{Axis, GamepadSpec, SCTL_GAMEPAD_CREATE};

type Event = (u16, u16, i32);
const SYN: Event = (0, 0, 0);

struct Layout {
    spec: GamepadSpec,
    neutral: Vec<Event>,
}

impl Layout {
    fn new(spec: GamepadSpec) -> Result<(Self, InputDeviceMetadata), &'static str> {
        let nb = usize::from(spec.button_count);
        let na = usize::from(spec.axis_count);
        if nb > spec.buttons.len() || na > spec.axes.len() || nb + na == 0 {
            return Err("Invalid gamepad control count");
        }
        if spec.buttons[nb..].iter().any(|&v| v != 0)
            || spec.axes[na..].iter().any(|&a| a != Axis::default())
        {
            return Err("Unused gamepad entries must be zero");
        }
        let buttons = &spec.buttons[..nb];
        let mut neutral = Vec::with_capacity(nb + na + 1);
        for (i, &code) in buttons.iter().enumerate() {
            if !matches!(code, 0x120..=0x13f | 0x220..=0x223 | 0x2c0..=0x2e7)
                || buttons[..i].contains(&code)
            {
                return Err("Invalid or duplicate gamepad button");
            }
            neutral.push((1, code, 0));
        }
        let caps = if nb == 0 { 0 } else { INPUT_CAP_KEY };
        let mut metadata =
            InputDeviceMetadata::new(InputDeviceKind::Gamepad, caps).with_gamepad_buttons(buttons);
        for a in &spec.axes[..na] {
            if a.reserved != 0
                || !matches!(a.code, 0..=0x0a | 0x10..=0x17)
                || !(a.minimum..=a.maximum).contains(&a.neutral)
            {
                return Err("Invalid gamepad axis");
            }
            metadata = metadata.with_absolute_axis(a.code, a.minimum, a.maximum)?;
            neutral.push((3, a.code, a.neutral));
        }
        neutral.push(SYN);
        Ok((Self { spec, neutral }, metadata))
    }

    fn decode(&self, bytes: &[u8]) -> Result<Vec<Event>, &'static str> {
        if bytes.len() != self.neutral.len() * InputEvent::size() {
            return Err("Expected one complete gamepad snapshot");
        }
        let nb = usize::from(self.spec.button_count);
        let axes = &self.spec.axes[..usize::from(self.spec.axis_count)];
        let mut seen = 0u64;
        let mut frame = Vec::with_capacity(self.neutral.len());
        for (i, record) in bytes.chunks_exact(InputEvent::size()).enumerate() {
            let ty = u16::from_ne_bytes(record[8..10].try_into().unwrap());
            let code = u16::from_ne_bytes(record[10..12].try_into().unwrap());
            let value = i32::from_ne_bytes(record[12..16].try_into().unwrap());
            let event = (ty, code, value);
            if i == self.neutral.len() - 1 {
                if event != SYN {
                    return Err("Expected final SYN_REPORT");
                }
            } else {
                let index = match ty {
                    1 if matches!(value, 0 | 1) => {
                        self.spec.buttons[..nb].iter().position(|&c| c == code)
                    }
                    3 => axes
                        .iter()
                        .position(|a| a.code == code && (a.minimum..=a.maximum).contains(&value))
                        .map(|i| nb + i),
                    _ => None,
                }
                .ok_or("Undeclared gamepad control or invalid value")?;
                let bit = 1u64 << index;
                if seen & bit != 0 {
                    return Err("Duplicate control in gamepad snapshot");
                }
                seen |= bit;
            }
            frame.push(event);
        }
        Ok(frame)
    }
}

struct Publication {
    device: Arc<EventDevice>,
    registration: usize,
    layout: Layout,
}

impl Publication {
    fn create(spec: GamepadSpec) -> Result<(i32, Self), &'static str> {
        let (layout, metadata) = Layout::new(spec)?;
        let (slot, device) = EventDevice::logical_gamepad(metadata)?;
        let device = Arc::new(device);
        device.push_events(&layout.neutral);
        let registration = DeviceManager::get_manager()
            .register_device_with_name(device.get_name().into(), device.clone());
        Ok((
            slot as i32,
            Self {
                device,
                registration,
                layout,
            },
        ))
    }
}

impl Drop for Publication {
    fn drop(&mut self) {
        self.device.push_events(&self.layout.neutral);
        DeviceManager::get_manager().unregister_device(self.registration);
        self.device.detach_gamepad();
    }
}

enum Uinput {
    Root,
    Producer(Mutex<Option<Publication>>),
}

fn authorized_caller() -> Result<CurrentTaskRef, &'static str> {
    mytask()
        .filter(|task| permits_management(task, Management::PhysicalSeat))
        .ok_or("Physical-seat management permission denied")
}

impl Device for Uinput {
    fn open(self: Arc<Self>) -> Result<Arc<dyn Device>, &'static str> {
        drop(authorized_caller()?);
        Ok(Arc::new(Self::Producer(Mutex::new(None))))
    }
    fn capabilities(&self) -> &'static [DeviceCapability] {
        &[DeviceCapability::PhysicalSeat]
    }
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }
    fn name(&self) -> &'static str {
        "uinput"
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

impl ControlOps for Uinput {
    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        alloc::vec![(SCTL_GAMEPAD_CREATE, "Create a logical gamepad")]
    }

    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        let Self::Producer(state) = self else {
            return Err("Open a gamepad producer first");
        };
        let task = authorized_caller()?;
        if command != SCTL_GAMEPAD_CREATE {
            return Err("Unknown gamepad producer control");
        }
        let mut spec = GamepadSpec::default();
        // SAFETY: repr(C), no padding, integer fields only. The mutable slice
        // covers exactly this initialized record and is used only for the copy.
        let bytes = unsafe {
            core::slice::from_raw_parts_mut(
                core::ptr::from_mut(&mut spec).cast::<u8>(),
                size_of::<GamepadSpec>(),
            )
        };
        copy_from_user(&task, arg, bytes).map_err(|_| "Invalid gamepad descriptor pointer")?;
        let mut live = state.lock();
        if live.is_some() {
            return Err("Gamepad creation requires an empty producer");
        }
        let (slot, publication) = Publication::create(spec)?;
        *live = Some(publication);
        Ok(slot)
    }
}

impl CharDevice for Uinput {
    fn read_byte(&self) -> Option<u8> {
        None
    }
    fn read(&self, _: &mut [u8]) -> usize {
        0
    }
    fn write_byte(&self, _: u8) -> Result<(), &'static str> {
        Err("Expected complete gamepad snapshot")
    }
    fn write(&self, bytes: &[u8]) -> Result<usize, &'static str> {
        drop(authorized_caller()?);
        let Self::Producer(state) = self else {
            return Err("Open a gamepad producer first");
        };
        let live = state.lock();
        let Some(pad) = &*live else {
            return Err("Create a gamepad before injecting events");
        };
        let frame = pad.layout.decode(bytes)?;
        pad.device.push_events(&frame);
        Ok(bytes.len())
    }
    fn can_read(&self) -> bool {
        false
    }
    fn can_write(&self) -> bool {
        matches!(self, Self::Producer(_))
    }
}

impl MemoryMappingOps for Uinput {
    fn get_mapping_info(&self, _: usize, _: usize) -> Result<MemoryMappingInfo, &'static str> {
        Err("Gamepad producer has no mapping")
    }
    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for Uinput {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        ReadySet {
            write: interest.write && self.can_write(),
            ..ReadySet::none()
        }
    }
    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        _: &mut Trapframe,
        _: Option<u64>,
        _: u64,
    ) -> SelectWaitOutcome {
        if self.current_ready(interest).write {
            SelectWaitOutcome::Ready
        } else {
            SelectWaitOutcome::TimedOut
        }
    }
}

fn register() {
    DeviceManager::get_manager().register_device_with_name("uinput".into(), Arc::new(Uinput::Root));
}

crate::driver_initcall!(register);

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> GamepadSpec {
        let mut spec = GamepadSpec::default();
        spec.button_count = 1;
        spec.buttons[0] = 0x130;
        spec.axis_count = 1;
        spec.axes[0] = Axis {
            code: 0,
            reserved: 0,
            minimum: -100,
            maximum: 100,
            neutral: 0,
        };
        spec
    }

    fn bytes(events: &[Event]) -> Vec<u8> {
        let mut data = Vec::new();
        for &(ty, code, value) in events {
            data.extend_from_slice(&u64::MAX.to_ne_bytes());
            data.extend_from_slice(&ty.to_ne_bytes());
            data.extend_from_slice(&code.to_ne_bytes());
            data.extend_from_slice(&value.to_ne_bytes());
        }
        data
    }

    #[test_case]
    fn publication_is_limited_to_declared_gamepad_controls() {
        let (layout, _) = Layout::new(spec()).unwrap();
        let valid = [(3, 0, -30), (1, 0x130, 1), SYN];
        assert_eq!(layout.decode(&bytes(&valid)).unwrap(), valid);
        for invalid in [
            [(3, 0, 101), (1, 0x130, 1), SYN],
            [(3, 0, 1), (3, 0, 1), SYN],
            [(3, 0, 1), (1, 0x130, 2), SYN],
            [(3, 0, 1), (1, 30, 1), SYN],
            [(3, 0, 1), (2, 0, 1), SYN],
            [(3, 0, 1), SYN, SYN],
            [(3, 0, 1), (1, 0x130, 1), (0, 3, 0)],
        ] {
            assert!(layout.decode(&bytes(&invalid)).is_err());
        }
        assert!(layout.decode(&bytes(&valid)[1..]).is_err());
        let mut descriptor = spec();
        descriptor.buttons[0] = 30;
        assert!(Layout::new(descriptor).is_err());
        descriptor = spec();
        descriptor.button_count = 33;
        assert!(Layout::new(descriptor).is_err());
        descriptor = spec();
        descriptor.axes[0].neutral = -101;
        assert!(Layout::new(descriptor).is_err());
        descriptor = spec();
        descriptor.axes[0].code = 0x39;
        assert!(Layout::new(descriptor).is_err());
        descriptor = spec();
        descriptor.axes[1] = descriptor.axes[0];
        assert!(Layout::new(descriptor).is_err());
        descriptor.axis_count = 2;
        assert!(Layout::new(descriptor).is_err());
    }

    #[test_case]
    fn publication_drop_neutralizes_unpublishes_and_reuses_slot() {
        use crate::fs::vfs_v2::{drivers::devfs::DevFS, manager::VfsManager};
        use crate::object::KernelObject;
        use crate::task::{Task, TaskType, clear_mock_current_task, set_mock_current_task};
        let mut task = Task::new("gamepad-readers".into(), 0, TaskType::User);
        task.set_id(91011);
        set_mock_current_task(Arc::new(task));
        let (slot, pad) = Publication::create(spec()).unwrap();
        let name = alloc::string::String::from(pad.device.get_name());
        let path = alloc::format!("/{name}");
        let vfs = VfsManager::new_with_root(DevFS::new());
        let open = || {
            let KernelObject::File(fd) = vfs.open(&path, 0).unwrap() else {
                panic!("Expected a gamepad file");
            };
            fd
        };
        let reader = open();
        let app = open();
        let original_id = reader.metadata().unwrap().file_id;
        assert_eq!(original_id, pad.registration as u64);
        app.set_nonblocking(true);
        assert!(!reader.is_nonblocking());
        let mut record = [0; InputEvent::size()];
        // Application and compositor can each be partway through a snapshot.
        app.read(&mut record).unwrap();
        assert_eq!(&record[8..], &bytes(&pad.layout.neutral[..1])[8..]);
        let live = [(1, 0x130, 1), (3, 0, 77), SYN];
        pad.device.push_events(&live);
        let neutral = pad.layout.neutral.clone();
        drop(pad);
        assert!(
            DeviceManager::get_manager()
                .get_device_by_name(&name)
                .is_none()
        );
        assert!(vfs.open(&path, 0).is_err());
        let (reused, replacement) = Publication::create(spec()).unwrap();
        assert_eq!(slot, reused);
        let current = open();
        assert_ne!(original_id, current.metadata().unwrap().file_id);
        assert_eq!(
            current.metadata().unwrap().file_id,
            replacement.registration as u64
        );
        for expected in neutral.iter().chain(&live).chain(&neutral) {
            let mut rec = [0; InputEvent::size()];
            assert_eq!(reader.read(&mut rec).unwrap(), rec.len());
            assert_eq!(&rec[8..], &bytes(&[*expected])[8..]);
        }
        for expected in neutral[1..].iter().chain(&live).chain(&neutral) {
            assert_eq!(app.read(&mut record).unwrap(), record.len());
            assert_eq!(&record[8..], &bytes(&[*expected])[8..]);
        }
        assert!(matches!(
            app.read(&mut record),
            Err(crate::object::capability::StreamError::EndOfStream)
        ));
        assert!(reader.is_hung_up());
        assert!(matches!(
            reader.read(&mut [0; 16]),
            Err(crate::object::capability::StreamError::EndOfStream)
        ));
        assert!(!current.is_hung_up());
        clear_mock_current_task();
    }

    #[test_case]
    fn independent_producer_opens_publish_until_their_last_close() {
        use crate::library::std::usercopy::copy_to_user;
        use crate::task::{clear_mock_current_task, new_user_task, set_mock_current_task};
        let owner = Arc::new(new_user_task("gamepad-producers".into(), 0));
        let address = 0x2000_0000;
        owner.allocate_stack_pages(address, 1).unwrap();
        let descriptor = spec();
        // SAFETY: GamepadSpec is an initialized, padding-free repr(C) ABI record.
        let descriptor_bytes = unsafe {
            core::slice::from_raw_parts(
                core::ptr::from_ref(&descriptor).cast::<u8>(),
                size_of::<GamepadSpec>(),
            )
        };
        copy_to_user(&owner, address, descriptor_bytes).unwrap();
        set_mock_current_task(owner);
        let root = Arc::new(Uinput::Root);
        let first = root.clone().open().unwrap();
        let second = root.open().unwrap();
        assert_eq!(
            first.supported_control_commands(),
            [(SCTL_GAMEPAD_CREATE, "Create a logical gamepad")]
        );
        assert!(first.control(u32::MAX, address).is_err());
        let slot = first.control(SCTL_GAMEPAD_CREATE, address).unwrap();
        assert!(first.control(SCTL_GAMEPAD_CREATE, address).is_err());
        let other_slot = second.control(SCTL_GAMEPAD_CREATE, address).unwrap();
        assert_ne!(slot, other_slot);
        let duplicate = first.clone();
        drop(first);
        let manager = DeviceManager::get_manager();
        let name = alloc::format!("gamepad{slot}");
        assert!(manager.get_device_by_name(&name).is_some());
        let frame = bytes(&[(3, 0, 7), (1, 0x130, 1), SYN]);
        assert_eq!(
            duplicate.as_char_device().unwrap().write(&frame),
            Ok(frame.len())
        );
        drop(duplicate);
        assert!(manager.get_device_by_name(&name).is_none());
        assert_eq!(
            second.as_char_device().unwrap().write(&frame),
            Ok(frame.len())
        );
        drop(second);
        clear_mock_current_task();
    }

    #[test_case]
    fn transferred_producers_recheck_the_current_callers_seat_authority() {
        use crate::task::{clear_mock_current_task, new_user_task, set_mock_current_task};
        let owner = Arc::new(new_user_task("gamepad-owner".into(), 0));
        set_mock_current_task(owner);
        let (_, publication) = Publication::create(spec()).unwrap();
        let producer = Uinput::Producer(Mutex::new(Some(publication)));
        let empty = Arc::new(Uinput::Root).open().unwrap();
        let frame = bytes(&[(3, 0, 7), (1, 0x130, 1), SYN]);
        assert_eq!(producer.write(&frame), Ok(frame.len()));
        let recipient = Arc::new(new_user_task("gamepad-recipient".into(), 0));
        recipient.credentials.write().set_authority(0, 0).unwrap();
        set_mock_current_task(recipient);
        assert!(Arc::new(Uinput::Root).open().is_err());
        assert_eq!(
            empty.control(SCTL_GAMEPAD_CREATE, 0),
            Err("Physical-seat management permission denied")
        );
        assert_eq!(
            producer.write(&frame),
            Err("Physical-seat management permission denied")
        );
        drop(producer);
        clear_mock_current_task();
    }
}
