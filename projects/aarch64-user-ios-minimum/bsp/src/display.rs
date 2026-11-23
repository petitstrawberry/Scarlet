//! Connect a UIKit view to Scarlet's ordinary display endpoint.

#[path = "../../../shared/hosted/scanout.rs"]
mod scanout;

use scarlet::device::{
    graphics::{FramebufferConfig, PixelFormat},
    input::{
        event_device::{EventDevice, INPUT_CAP_INTERNAL, InputDeviceKind, InputDeviceMetadata},
        event_types::{EV_SW, EV_SYN},
        switch_codes::SW_TABLET_MODE,
        syn_codes::SYN_REPORT,
    },
    manager::DeviceManager,
};
use std::sync::{Arc, Mutex, OnceLock};

pub(crate) type Damage = extern "C" fn();

struct Output {
    frames: Arc<Mutex<scanout::Frames>>,
    bytes: usize,
}

static OUTPUT: OnceLock<Output> = OnceLock::new();

#[repr(C)]
pub struct Configuration {
    pub width: u32,
    pub height: u32,
    pub tablet: bool,
    pub damage: Damage,
}

impl Configuration {
    pub fn register(&self, manager: &DeviceManager) -> Result<(), &'static str> {
        let metadata = InputDeviceMetadata::new(InputDeviceKind::Switch, INPUT_CAP_INTERNAL)
            .with_switch(SW_TABLET_MODE)?;
        let posture = Arc::new(EventDevice::new_with_metadata("switch", metadata));
        posture.push_events(&[
            (EV_SW, SW_TABLET_MODE, i32::from(self.tablet)),
            (EV_SYN, SYN_REPORT, 0),
        ]);
        manager.register_device_with_name(posture.get_name().into(), posture);

        if self.width == 0 || self.height == 0 {
            return Ok(());
        }
        if self.width > 4096 || self.height > 4096 {
            return Err("iOS display dimensions exceed the scanout limit");
        }
        let config = FramebufferConfig::new(self.width, self.height, PixelFormat::BGRA8888);
        let frames = Arc::new(Mutex::new(scanout::Frames::new(&config)));
        let damage = self.damage;
        let scanout = scanout::Scanout::new(
            "ios-display",
            "Scarlet iOS display",
            config.clone(),
            frames.clone(),
            Arc::new(move || damage()),
        )?;
        OUTPUT
            .set(Output {
                frames,
                bytes: config.size(),
            })
            .map_err(|_| "iOS scanout is already registered")?;
        manager.register_device(Arc::new(scanout));
        Ok(())
    }
}

/// Copy current damage to the caller's persistent, tightly packed BGRA buffer.
///
/// # Safety
/// The buffer must be writable for `length` bytes and exclusively borrowed
/// until this call returns. UIKit keeps the same buffer until the next frame.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scarlet_ios_copy_frame(buffer: *mut u8, length: usize) -> bool {
    let Some(output) = OUTPUT.get() else {
        return false;
    };
    if buffer.is_null() || length != output.bytes {
        return false;
    }
    let pixels = unsafe { std::slice::from_raw_parts_mut(buffer, length) };
    output.frames.lock().unwrap().take(pixels).is_some()
}
