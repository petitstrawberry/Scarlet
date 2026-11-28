//! macOS registration for the shared Apple PCM backend.
#[path = "../../../shared/hosted/audio/mod.rs"]
mod shared;
use scarlet::{
    device::audio::{
        AUDIO_DEVICE_KIND_UNKNOWN, AudioDeviceInfo, register_playback_device_with_info,
    },
    interrupt::InterruptId,
};
use std::sync::Arc;

pub const COMPLETION_IRQ: InterruptId = 18;

// The late initcall runs after PID 1 and the external IRQ controller exist.
scarlet::late_initcall!(register_default_output);

/// Retain the BSP's late initcall in the native executable.
pub fn link_boot_output() {
    std::hint::black_box(&__LATE_INITCALL__);
}

fn register_default_output() {
    match shared::CoreAudioPlayback::attach(0, COMPLETION_IRQ, None) {
        Ok(output) => {
            register_playback_device_with_info(
                Arc::new(output),
                AudioDeviceInfo::new(
                    AUDIO_DEVICE_KIND_UNKNOWN,
                    "coreaudio",
                    "macOS Default Output",
                ),
            );
        }
        Err(error) => scarlet::println!("[coreaudio] Could not register default output: {}", error),
    }
}
