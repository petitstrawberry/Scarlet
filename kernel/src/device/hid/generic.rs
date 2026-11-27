//! Descriptor-driven standard input fallback. A composite device may expose
//! multiple application collections through a single transport and report ID.
use super::{HidDeviceInfo, HidInstance};
use crate::device::{
    input::event_device::{
        EventDevice, INPUT_CAP_ABS, INPUT_CAP_KEY, INPUT_CAP_REL, InputDeviceKind,
        InputDeviceMetadata,
    },
    manager::DeviceManager,
};
use alloc::{boxed::Box, string::ToString, sync::Arc, vec::Vec};
mod decode;
mod descriptor;
use decode::Input;
use descriptor::{Descriptor, Kind};

pub(super) use descriptor::valid_descriptor;

struct Endpoint {
    input: Input,
    device: Arc<EventDevice>,
    registration: usize,
}

impl Endpoint {
    fn publish(input: Input) -> Option<Self> {
        let (kind, name) = match input.app.kind {
            Kind::Keyboard => (InputDeviceKind::Keyboard, "keyboard"),
            Kind::Mouse => (InputDeviceKind::Mouse, "mouse"),
            Kind::Gamepad | Kind::Joystick => (InputDeviceKind::Gamepad, "gamepad"),
        };
        let mut caps = 0;
        if !input.app.buttons.is_empty() {
            caps |= INPUT_CAP_KEY;
        }
        if !input.app.axes.is_empty() {
            caps |= INPUT_CAP_ABS;
        }
        if input.app.relative {
            caps |= INPUT_CAP_REL;
        }
        let mut metadata = InputDeviceMetadata::new(kind, caps);
        for a in &input.app.axes {
            metadata = metadata.with_absolute_axis(a.code, a.min, a.max).ok()?;
        }
        let node = if kind == InputDeviceKind::Gamepad {
            let metadata = metadata.with_gamepad_buttons(&input.app.buttons);
            let (_, device) = EventDevice::logical_gamepad(metadata).ok()?;
            device.push_events(&input.snapshot());
            device
        } else {
            EventDevice::removable_input(name, metadata)
        };
        let device = Arc::new(node);
        let registration = DeviceManager::get_manager()
            .register_device_with_name(device.get_name().to_string(), device.clone());
        Some(Self {
            input,
            device,
            registration,
        })
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.device.push_events(&self.input.reset(false));
        DeviceManager::get_manager().unregister_device(self.registration);
        self.device.detach_gamepad();
    }
}

struct StandardInput {
    numbered: bool,
    input_bits: [u16; 256],
    endpoints: Vec<Endpoint>,
}

impl HidInstance for StandardInput {
    fn input_report(&mut self, report: &[u8]) -> Result<(), &'static str> {
        let (id, data) = if self.numbered {
            let Some((&id, data)) = report.split_first() else {
                for e in &mut self.endpoints {
                    e.device.push_events(&e.input.reset(true));
                }
                return Ok(());
            };
            (id, data)
        } else {
            (0, report)
        };
        let bits = self.input_bits[usize::from(id)];
        if bits == 0 {
            return Ok(());
        }
        let payload = (data.len() == usize::from(bits).div_ceil(8)).then_some(data);
        for e in &mut self.endpoints {
            let events = e.input.report(id, payload);
            e.device.push_events(&events);
        }
        Ok(())
    }

    fn allow_raw_output(&self) -> bool {
        true
    }
}

pub(super) fn probe(info: &HidDeviceInfo) -> Option<Box<dyn HidInstance>> {
    let descriptor = Descriptor::parse(&info.report_descriptor)?;
    let endpoints = descriptor
        .apps
        .into_iter()
        .map(|app| Endpoint::publish(Input::new(app)))
        .collect::<Option<Vec<_>>>()?;
    Some(Box::new(StandardInput {
        numbered: descriptor.numbered,
        input_bits: descriptor.input_bits,
        endpoints,
    }))
}

#[cfg(all(test, not(target_os = "none")))]
mod tests;
