//! Deliver host desktop controls through Scarlet's input devices.
use scarlet::{
    device::{
        input::{
            event_device::{
                EventDevice, INPUT_CAP_INTERNAL, INPUT_CAP_KEY, InputDeviceKind,
                InputDeviceMetadata,
            },
            event_types::{EV_KEY, EV_SYN},
            key_codes::KEY_TAB,
            key_values::{KEY_PRESS, KEY_RELEASE},
            syn_codes::SYN_REPORT,
        },
        manager::DeviceManager,
    },
    hosted,
    interrupt::{InterruptClaim, InterruptId, InterruptManager, InterruptResult, InterruptSource},
};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicUsize, Ordering},
};

const BUTTON_IRQ: InterruptId = 22;
const MAX_PENDING_TAPS: usize = 32;
const TAB: [(u16, u16, i32); 4] = [
    (EV_KEY, KEY_TAB, KEY_PRESS),
    (EV_SYN, SYN_REPORT, 0),
    (EV_KEY, KEY_TAB, KEY_RELEASE),
    (EV_SYN, SYN_REPORT, 0),
];

static BUTTONS: OnceLock<Arc<DesktopButtons>> = OnceLock::new();

struct DesktopButtons {
    device: Arc<EventDevice>,
    pending_taps: AtomicUsize,
}

pub(super) fn register(devices: &DeviceManager) -> Result<(), &'static str> {
    let device = Arc::new(EventDevice::new_with_metadata(
        "buttons",
        InputDeviceMetadata::new(InputDeviceKind::Buttons, INPUT_CAP_INTERNAL | INPUT_CAP_KEY),
    ));
    let buttons = Arc::new(DesktopButtons {
        device: device.clone(),
        pending_taps: AtomicUsize::new(0),
    });
    let interrupts = InterruptManager::global();
    let source: Arc<dyn InterruptSource> = buttons.clone();
    interrupts
        .register_interrupt_source(BUTTON_IRQ, source.clone())
        .map_err(|_| "Cannot register the iOS desktop buttons IRQ")?;
    if interrupts.enable_external_interrupt(BUTTON_IRQ, 0).is_err() {
        interrupts.unregister_interrupt_source(BUTTON_IRQ, &source);
        return Err("Cannot enable the iOS desktop buttons IRQ");
    }
    BUTTONS
        .set(buttons)
        .map_err(|_| "iOS desktop buttons are already registered")?;
    devices.register_device_with_name(device.get_name().into(), device);
    Ok(())
}

impl InterruptSource for DesktopButtons {
    fn interrupt_id(&self) -> Option<InterruptId> {
        Some(BUTTON_IRQ)
    }

    fn claim_interrupt(&self) -> InterruptResult<InterruptClaim> {
        let count = self.pending_taps.swap(0, Ordering::Relaxed);
        if count == 0 {
            return Ok(InterruptClaim::NotMine);
        }
        for _ in 0..count {
            self.device.push_events(&TAB);
        }
        Ok(InterruptClaim::Handled)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn scarlet_ios_desktop_tab() {
    let Some(buttons) = BUTTONS.get() else { return };
    let previous =
        buttons
            .pending_taps
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                (count < MAX_PENDING_TAPS).then_some(count + 1)
            });
    if previous == Ok(0) {
        let _ = hosted::raise_external_interrupt(0, BUTTON_IRQ);
    }
}
