//! Forward app-owned contact snapshots over the hosted interrupt controller.
use scarlet::{
    device::{
        input::{
            abs_codes::{ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_SLOT, ABS_MT_TRACKING_ID},
            event_device::{
                EventDevice, INPUT_CAP_DIRECT_TOUCH, INPUT_CAP_INTERNAL, InputDeviceKind,
                InputDeviceMetadata,
            },
            event_types::{EV_ABS, EV_SYN},
            syn_codes::{SYN_DROPPED, SYN_REPORT},
        },
        manager::DeviceManager,
    },
    hosted,
    interrupt::{InterruptClaim, InterruptId, InterruptManager, InterruptResult, InterruptSource},
    sync::IrqSpinLock,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
};

const TOUCH_IRQ: InterruptId = 21;
const TOUCH_SLOTS: usize = 16;
const TOUCH_MAX: i32 = 65_535;
const PENDING_FRAMES: usize = 32;
const IRQ_BATCH: usize = 8;
const SYNC: (u16, u16, i32) = (EV_SYN, SYN_REPORT, 0);

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Contact {
    id: i32,
    x: i32,
    y: i32,
}

impl Contact {
    const INACTIVE: Self = Self { id: -1, x: 0, y: 0 };
}

type Contacts = [Contact; TOUCH_SLOTS];

#[derive(Clone, Copy)]
struct Frame {
    contacts: Contacts,
    reset: bool,
}

static TOUCH: OnceLock<Arc<TouchInterrupt>> = OnceLock::new();

struct TouchInterrupt {
    device: Arc<EventDevice>,
    pending: Mutex<VecDeque<Frame>>,
    previous: IrqSpinLock<Contacts>,
}

pub fn register(devices: &DeviceManager) -> Result<(), &'static str> {
    let metadata = InputDeviceMetadata::new(
        InputDeviceKind::Touchscreen,
        INPUT_CAP_INTERNAL | INPUT_CAP_DIRECT_TOUCH,
    )
    .with_multitouch_slots(TOUCH_SLOTS)?
    .with_absolute_axis(ABS_MT_POSITION_X, 0, TOUCH_MAX)?
    .with_absolute_axis(ABS_MT_POSITION_Y, 0, TOUCH_MAX)?;
    let device = Arc::new(EventDevice::new_with_metadata("touchscreen", metadata));
    let input = Arc::new(TouchInterrupt {
        device: device.clone(),
        pending: Mutex::new(VecDeque::with_capacity(PENDING_FRAMES)),
        previous: IrqSpinLock::new([Contact::INACTIVE; TOUCH_SLOTS]),
    });

    let interrupts = InterruptManager::global();
    let source: Arc<dyn InterruptSource> = input.clone();
    interrupts
        .register_interrupt_source(TOUCH_IRQ, source.clone())
        .map_err(|_| "Cannot register the iOS touchscreen IRQ")?;
    if interrupts.enable_external_interrupt(TOUCH_IRQ, 0).is_err() {
        interrupts.unregister_interrupt_source(TOUCH_IRQ, &source);
        return Err("Cannot enable the iOS touchscreen IRQ");
    }
    TOUCH
        .set(input)
        .map_err(|_| "Touchscreen already registered")?;
    devices.register_device_with_name(device.get_name().into(), device);
    Ok(())
}

impl TouchInterrupt {
    fn queue(&self, mut frame: Frame) {
        let mut pending = self.pending.lock().unwrap();
        let wake = pending.is_empty();
        if pending.len() == PENDING_FRAMES {
            let last = pending[PENDING_FRAMES - 1];
            let earlier = pending[PENDING_FRAMES - 2];
            let same_slots =
                |a: &Contacts, b: &Contacts| a.iter().zip(b).all(|(a, b)| a.id == b.id);
            if !last.reset
                && !frame.reset
                && same_slots(&last.contacts, &earlier.contacts)
                && same_slots(&last.contacts, &frame.contacts)
            {
                pending.pop_back();
            } else {
                // This batch can no longer preserve contact transitions.
                // Cancel gestures before reintroducing all live contacts.
                pending.clear();
                frame.reset = true;
            }
        }
        pending.push_back(frame);
        drop(pending);
        if wake {
            let _ = hosted::raise_external_interrupt(0, TOUCH_IRQ);
        }
    }

    fn report(&self, frame: Frame) {
        let mut previous = self.previous.lock();
        if !frame.reset && *previous == frame.contacts {
            return;
        }
        let mut events = [SYNC; TOUCH_SLOTS * 4 + 3];
        let mut next = 0;
        if frame.reset {
            events[0] = (EV_SYN, SYN_DROPPED, 0);
            next = 2;
            *previous = [Contact::INACTIVE; TOUCH_SLOTS];
        }
        for (slot, (point, last)) in frame.contacts.iter().zip(previous.iter()).enumerate() {
            if point.id < 0 && last.id < 0 {
                continue;
            }
            events[next] = (EV_ABS, ABS_MT_SLOT, slot as i32);
            events[next + 1] = (EV_ABS, ABS_MT_TRACKING_ID, point.id);
            next += 2;
            if point.id >= 0 {
                events[next] = (EV_ABS, ABS_MT_POSITION_X, point.x);
                events[next + 1] = (EV_ABS, ABS_MT_POSITION_Y, point.y);
                next += 2;
            }
        }
        *previous = frame.contacts;
        drop(previous);
        if next == 2 && frame.reset {
            self.device.push_events(&events[..next]);
        } else {
            self.device.push_events(&events[..=next]);
        }
    }
}

impl InterruptSource for TouchInterrupt {
    fn interrupt_id(&self) -> Option<InterruptId> {
        Some(TOUCH_IRQ)
    }

    fn claim_interrupt(&self) -> InterruptResult<InterruptClaim> {
        let mut frames = [None; IRQ_BATCH];
        let more = {
            let mut pending = self.pending.lock().unwrap();
            if pending.is_empty() {
                return Ok(InterruptClaim::NotMine);
            }
            let count = pending.len().min(IRQ_BATCH);
            for (slot, frame) in frames.iter_mut().zip(pending.drain(..count)) {
                *slot = Some(frame);
            }
            !pending.is_empty()
        };
        for frame in frames.into_iter().flatten() {
            self.report(frame);
        }
        if more {
            hosted::raise_external_interrupt(0, TOUCH_IRQ)?;
        }
        Ok(InterruptClaim::Handled)
    }
}

/// Queue a copied frame from the iOS event loop.
///
/// # Safety
/// `contacts` must refer to `count` readable Contact values for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scarlet_ios_touch(
    contacts: *const Contact,
    count: usize,
    cancelled: bool,
) {
    let Some(touch) = TOUCH.get() else { return };
    if contacts.is_null() || count != TOUCH_SLOTS {
        return;
    }
    let contacts = unsafe { std::slice::from_raw_parts(contacts, count) };
    let mut points = [Contact::INACTIVE; TOUCH_SLOTS];
    for (point, contact) in points.iter_mut().zip(contacts) {
        if contact.id >= 0 {
            *point = Contact {
                id: contact.id,
                x: contact.x.clamp(0, TOUCH_MAX),
                y: contact.y.clamp(0, TOUCH_MAX),
            };
        }
    }
    touch.queue(Frame {
        contacts: points,
        reset: cancelled,
    });
}
