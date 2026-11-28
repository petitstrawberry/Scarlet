//! iOS registration; the realtime callback and PCM ownership stay in Rust.
#[path = "../../../shared/hosted/audio/mod.rs"]
mod shared;
use scarlet::device::audio::{
    AUDIO_DEVICE_KIND_UNKNOWN, AudioDeviceInfo, register_playback_device_with_info,
};
use shared::{CoreAudioPlayback, SessionActivation, SessionHooks};
use std::sync::{Arc, OnceLock};

static OUTPUT: OnceLock<Arc<CoreAudioPlayback>> = OnceLock::new();
unsafe extern "C" {
    fn scarlet_ios_audio_session_activate(resuming: bool) -> i32;
    fn scarlet_ios_audio_session_deactivate();
}
scarlet::late_initcall!(register_output);
fn register_output() {
    let hooks = SessionHooks {
        activate: |resuming| match unsafe { scarlet_ios_audio_session_activate(resuming) } {
            0 => SessionActivation::Ready,
            1 => SessionActivation::Deferred,
            _ => SessionActivation::Failed,
        },
        deactivate: || unsafe { scarlet_ios_audio_session_deactivate() },
    };
    match CoreAudioPlayback::attach(0, 18, Some(hooks)) {
        Ok(output) => {
            let output = Arc::new(output);
            if OUTPUT.set(output.clone()).is_ok() {
                register_playback_device_with_info(
                    output,
                    AudioDeviceInfo::new(
                        AUDIO_DEVICE_KIND_UNKNOWN,
                        "coreaudio",
                        "iOS Default Output",
                    ),
                );
            }
        }
        Err(error) => eprintln!("scarlet: cannot register iOS audio: {error}"),
    }
}

/// Retain registration before guest boot, without activating AVAudioSession.
#[unsafe(no_mangle)]
pub extern "C" fn scarlet_ios_audio_prepare() {
    std::hint::black_box(&__LATE_INITCALL__);
}
#[unsafe(no_mangle)]
pub extern "C" fn scarlet_ios_audio_pause() {
    if let Some(output) = OUTPUT.get() {
        output.pause_host();
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn scarlet_ios_audio_resume() {
    if let Some(output) = OUTPUT.get() {
        output.resume_host();
    }
}
