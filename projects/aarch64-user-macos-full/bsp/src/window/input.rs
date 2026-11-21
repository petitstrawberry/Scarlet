//! Broker-side display and input for an optional native window.
use scarlet::device::{
    input::{
        abs_codes::{ABS_X, ABS_Y},
        event_device::{
            EventDevice, INPUT_CAP_KEY, INPUT_CAP_REL, InputDeviceKind, InputDeviceMetadata,
        },
        event_types::{EV_ABS, EV_KEY, EV_REL, EV_SYN},
        key_codes::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT},
        key_values::{KEY_PRESS, KEY_RELEASE, KEY_REPEAT},
        rel_codes::{REL_HWHEEL, REL_WHEEL},
        syn_codes::SYN_REPORT,
    },
    manager::DeviceManager,
};
use std::{collections::BTreeSet, sync::Arc};

const SYNC: (u16, u16, i32) = (EV_SYN, SYN_REPORT, 0);
#[derive(Clone, Copy)]
pub(super) enum PointerButton {
    Left,
    Middle,
    Right,
}

struct WindowInput {
    keyboard: Arc<EventDevice>,
    pointer: Arc<EventDevice>,
    keys: BTreeSet<u16>,
    buttons: BTreeSet<u16>,
    max_x: i32,
    max_y: i32,
}

impl WindowInput {
    fn register(width: u32, height: u32) -> Result<Self, &'static str> {
        let keyboard = Arc::new(EventDevice::new_with_metadata(
            "keyboard",
            InputDeviceMetadata::new(InputDeviceKind::Keyboard, INPUT_CAP_KEY),
        ));
        let max_x = width as i32 - 1;
        let max_y = height as i32 - 1;
        let meta = InputDeviceMetadata::new(InputDeviceKind::Tablet, INPUT_CAP_KEY | INPUT_CAP_REL)
            .with_absolute_axis(ABS_X, 0, max_x)?
            .with_absolute_axis(ABS_Y, 0, max_y)?;
        let pointer = Arc::new(EventDevice::new_with_metadata("tablet", meta));
        let devices = DeviceManager::get_manager();
        devices.register_device_with_name(keyboard.get_name().into(), keyboard.clone());
        devices.register_device_with_name(pointer.get_name().into(), pointer.clone());
        Ok(Self {
            keyboard,
            pointer,
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            max_x,
            max_y,
        })
    }

    pub fn motion(&self, x: i32, y: i32) {
        self.pointer.push_events(&[
            (EV_ABS, ABS_X, x.clamp(0, self.max_x)),
            (EV_ABS, ABS_Y, y.clamp(0, self.max_y)),
            SYNC,
        ]);
    }

    pub fn wheel(&self, x: i32, y: i32) {
        if x != 0 || y != 0 {
            self.pointer
                .push_events(&[(EV_REL, REL_HWHEEL, x), (EV_REL, REL_WHEEL, y), SYNC]);
        }
    }

    pub fn button(&mut self, button: PointerButton, down: bool) {
        let code = match button {
            PointerButton::Left => BTN_LEFT,
            PointerButton::Middle => BTN_MIDDLE,
            PointerButton::Right => BTN_RIGHT,
        };
        Self::key_event(&self.pointer, &mut self.buttons, code, down, false);
    }

    pub fn key(&mut self, usage: u8, down: bool, repeat: bool) {
        if let Some(code) = scarlet::drivers::usb::hid::boot::translate_boot_key(usage) {
            Self::key_event(&self.keyboard, &mut self.keys, code, down, repeat);
        }
    }

    pub fn release_all(&mut self) {
        for (device, keys) in [
            (&self.keyboard, &mut self.keys),
            (&self.pointer, &mut self.buttons),
        ] {
            for code in std::mem::take(keys) {
                device.push_events(&[(EV_KEY, code, KEY_RELEASE), SYNC]);
            }
        }
    }

    fn key_event(
        device: &EventDevice,
        keys: &mut BTreeSet<u16>,
        code: u16,
        down: bool,
        repeat: bool,
    ) {
        let value = match down {
            true if keys.insert(code) => KEY_PRESS,
            true if repeat => KEY_REPEAT,
            false if keys.remove(&code) => KEY_RELEASE,
            _ => return,
        };
        device.push_events(&[(EV_KEY, code, value), SYNC]);
    }
}

use scarlet::interrupt::{
    InterruptClaim, InterruptId, InterruptManager, InterruptResult, InterruptSource,
};
use std::{collections::VecDeque, sync::Mutex};
const IRQ: InterruptId = 21;
#[derive(Clone, Copy)]
pub(super) enum Input {
    Motion(i32, i32),
    Wheel(i32, i32),
    Button(PointerButton, bool),
    Key(u8, bool, bool),
    ReleaseAll,
}
#[derive(Clone, Default)]
pub(super) struct InputSender(Arc<Mutex<VecDeque<Input>>>);
impl InputSender {
    pub fn send(&self, event: Input) {
        let mut queue = self.0.lock().unwrap();
        if matches!(event, Input::Motion(..)) && matches!(queue.back(), Some(Input::Motion(..))) {
            *queue.back_mut().unwrap() = event;
            return;
        }
        let wake = queue.is_empty();
        if queue.len() >= 512 {
            queue.clear();
            queue.push_back(Input::ReleaseAll);
        }
        queue.push_back(event);
        drop(queue);
        if wake {
            let _ = scarlet::hosted::raise_external_interrupt(0, IRQ);
        }
    }
    pub fn register(&self, width: u32, height: u32) -> Result<(), &'static str> {
        let source: Arc<dyn InterruptSource> = Arc::new(InputIrq {
            queue: self.clone(),
            devices: scarlet::sync::IrqSpinLock::new(WindowInput::register(width, height)?),
        });
        let manager = InterruptManager::global();
        manager
            .register_interrupt_source(IRQ, source.clone())
            .map_err(|_| "Cannot register window input IRQ")?;
        if manager.enable_external_interrupt(IRQ, 0).is_err() {
            manager.unregister_interrupt_source(IRQ, &source);
            return Err("Cannot enable window input IRQ");
        }
        Ok(())
    }
}
struct InputIrq {
    queue: InputSender,
    devices: scarlet::sync::IrqSpinLock<WindowInput>,
}
impl InterruptSource for InputIrq {
    fn interrupt_id(&self) -> Option<InterruptId> {
        Some(IRQ)
    }
    fn claim_interrupt(&self) -> InterruptResult<InterruptClaim> {
        let (events, more) = {
            let mut queue = self.queue.0.lock().unwrap();
            if queue.is_empty() {
                return Ok(InterruptClaim::NotMine);
            }
            let count = queue.len().min(32);
            let mut events = [None; 32];
            for (slot, event) in events.iter_mut().zip(queue.drain(..count)) {
                *slot = Some(event);
            }
            (events, !queue.is_empty())
        };
        {
            let mut devices = self.devices.lock();
            for event in events.into_iter().flatten() {
                match event {
                    Input::Motion(x, y) => devices.motion(x, y),
                    Input::Wheel(x, y) => devices.wheel(x, y),
                    Input::Button(b, down) => devices.button(b, down),
                    Input::Key(k, down, repeat) => devices.key(k, down, repeat),
                    Input::ReleaseAll => devices.release_all(),
                }
            }
        }
        if more {
            scarlet::hosted::raise_external_interrupt(0, IRQ)?;
        }
        Ok(InterruptClaim::Handled)
    }
}
