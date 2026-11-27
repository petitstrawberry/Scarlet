//! Publish logical gamepads from the common Bluetooth raw Joy-Con endpoints.
mod joycon_layout;

use joycon_layout::{
    ABS, CENTER, Decoder, Event, HAT_X, HAT_Y, Id, KEY, Layout, MAX, Sample, Seat,
};
use scarlet_abi::fs::VFS_O_WRONLY;
use scarlet_abi::gamepad::{
    Axis, GamepadSpec, SCTL_GAMEPAD_CREATE, SCTL_JOYCON_CLAIM_MAPPER, SET_JOYCON_PLAYER,
};
use scarlet_os::handle::HandleError;
use scarlet_os::handle::capability::StreamError;
use scarlet_os::poll::{self, POLLIN, PollHandle};
use scarlet_os::{Handle, InputDevice};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const RESCAN: Duration = Duration::from_millis(250);
const RECORD: usize = 16;

fn descriptor(frame: &[Event]) -> GamepadSpec {
    let mut spec = GamepadSpec::default();
    for &(ty, code, _) in frame {
        match ty {
            KEY => {
                spec.buttons[usize::from(spec.button_count)] = code;
                spec.button_count += 1;
            }
            ABS => {
                let (minimum, maximum, neutral) = match code {
                    HAT_X | HAT_Y => (-1, 1, 0),
                    _ => (0, MAX, CENTER),
                };
                spec.axes[usize::from(spec.axis_count)] = Axis {
                    code,
                    minimum,
                    maximum,
                    neutral,
                    reserved: 0,
                };
                spec.axis_count += 1;
            }
            _ => {}
        }
    }
    spec
}

fn encode(frame: &[Event]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(frame.len() * RECORD);
    for &(ty, code, value) in frame {
        bytes.extend_from_slice(&0u64.to_ne_bytes());
        bytes.extend_from_slice(&ty.to_ne_bytes());
        bytes.extend_from_slice(&code.to_ne_bytes());
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}

trait Producer: Sized {
    fn create(spec: &GamepadSpec) -> Result<Self, HandleError>;
    fn publish(&self, frame: &[Event]) -> Result<(), StreamError>;
}

impl Producer for Handle {
    fn create(spec: &GamepadSpec) -> Result<Self, HandleError> {
        let handle = Handle::open("/dev/uinput", VFS_O_WRONLY as usize)?;
        // SAFETY: CREATE synchronously copies this initialized repr(C) descriptor.
        unsafe { handle.control(SCTL_GAMEPAD_CREATE, std::ptr::from_ref(spec) as usize) }?;
        Ok(handle)
    }

    fn publish(&self, frame: &[Event]) -> Result<(), StreamError> {
        let bytes = encode(frame);
        let stream = self.as_stream().map_err(|_| StreamError::Unsupported)?;
        if stream.write(&bytes)? == bytes.len() {
            Ok(())
        } else {
            Err(StreamError::IoError)
        }
    }
}

struct Output<P> {
    producer: Option<P>,
    last: Vec<Event>,
    retry: Option<Instant>,
}

fn publish<P: Producer>(outputs: &mut BTreeMap<Layout, Output<P>>, seat: &Seat, now: Instant) {
    // Last close releases and unregisters every obsolete layout before any
    // replacement is visible. The kernel owns that cleanup on daemon exit too.
    outputs.retain(|layout, _| seat.pads.contains_key(layout));
    for &layout in seat.pads.keys() {
        let output = outputs.entry(layout).or_insert_with(|| Output {
            producer: None,
            last: Vec::new(),
            retry: None,
        });
        if output.retry.is_some_and(|due| now < due) {
            continue;
        }
        let frame = seat.snapshot(layout);
        if frame == output.last {
            continue;
        }
        if output.producer.is_none() {
            match P::create(&descriptor(&frame)) {
                Ok(p) => output.producer = Some(p),
                Err(e) => {
                    if output.retry.is_none() {
                        eprintln!("joycond: create {layout:?}: {e:?}");
                    }
                    output.retry = Some(now + RESCAN);
                    continue;
                }
            }
        }
        match output.producer.as_ref().unwrap().publish(&frame) {
            Ok(()) => {
                output.last = frame;
                output.retry = None;
            }
            Err(e) => {
                if output.retry.is_none() {
                    eprintln!("joycond: publish {layout:?}: {e:?}");
                }
                output.producer.take();
                output.last.clear();
                output.retry = Some(now + RESCAN);
            }
        }
    }
}

#[derive(Default)]
struct PlayerLight {
    sent: Option<u8>,
    retry: Option<Instant>,
}

impl PlayerLight {
    fn assign(
        &mut self,
        player: u8,
        now: Instant,
        send: impl FnOnce(u8) -> Result<i32, HandleError>,
    ) {
        if self.sent == Some(player) || self.retry.is_some_and(|due| now < due) {
            return;
        }
        match send(player) {
            Ok(_) => {
                self.sent = Some(player);
                self.retry = None;
            }
            Err(e) => {
                if self.retry.is_none() {
                    eprintln!("joycond: set player {player}: {e:?}");
                }
                self.retry = Some(now + RESCAN);
            }
        }
    }
}

struct Raw {
    device: InputDevice,
    decoder: Decoder,
    light: PlayerLight,
}

impl Raw {
    fn open(id: Id) -> Result<Self, HandleError> {
        let device = InputDevice::open(&id.path())?;
        device.set_nonblocking(true)?;
        Ok(Self {
            device,
            decoder: Decoder::default(),
            light: PlayerLight::default(),
        })
    }

    fn drain(&mut self, id: Id, frames: &mut Vec<(u64, Id, Sample)>) -> bool {
        // One read can hold the kernel's entire 256-record history. Bound the
        // drain so a busy half cannot starve another half or device discovery.
        let mut buf = [0u8; 256 * RECORD];
        for _ in 0..2 {
            match self.device.read(&mut buf) {
                Ok(n) if n > 0 && n <= buf.len() && n % RECORD == 0 => {
                    for record in buf[..n].chunks_exact(RECORD) {
                        let ty = u16::from_ne_bytes(record[8..10].try_into().unwrap());
                        let code = u16::from_ne_bytes(record[10..12].try_into().unwrap());
                        let value = i32::from_ne_bytes(record[12..16].try_into().unwrap());
                        if let Some(state) = self.decoder.event(id, (ty, code, value)) {
                            let time = u64::from_ne_bytes(record[..8].try_into().unwrap());
                            frames.push((time, id, state));
                        }
                    }
                }
                Err(StreamError::WouldBlock | StreamError::Interrupted) => return true,
                _ => return false,
            }
        }
        true
    }
}

type TimedSample = (u64, Id, Sample);

/// Poll is a wake hint. Read every nonblocking descriptor before deciding
/// whether two shoulder presses overlapped. Reports arriving during this pass
/// belong to the next cut, including any from a reader already drained.
fn collect_through<R>(
    readers: &mut BTreeMap<Id, R>,
    pending: &mut Vec<TimedSample>,
    cutoff: u64,
    mut drain: impl FnMut(&mut R, Id, &mut Vec<TimedSample>) -> bool,
) -> (Vec<TimedSample>, Vec<Id>) {
    let mut gone = Vec::new();
    for (&id, reader) in readers {
        if !drain(reader, id, pending) {
            gone.push(id);
        }
    }
    pending.sort_by_key(|&(time, _, _)| time);
    // Timestamps have microsecond precision. Defer the cutoff tick itself.
    let later = pending.split_off(pending.partition_point(|&(time, _, _)| time < cutoff));
    let ready = std::mem::replace(pending, later);
    gone.retain(|id| !pending.iter().any(|(_, source, _)| source == id));
    (ready, gone)
}

#[derive(Default)]
struct Daemon {
    raw: BTreeMap<Id, Raw>,
    seat: Seat,
    outputs: BTreeMap<Layout, Output<Handle>>,
}

impl Daemon {
    fn discover(&mut self) {
        for slot in 0..8 {
            for id in [Id::Left(slot), Id::Right(slot)] {
                if let std::collections::btree_map::Entry::Vacant(entry) = self.raw.entry(id)
                    && let Ok(raw) = Raw::open(id)
                {
                    entry.insert(raw);
                }
            }
        }
    }

    fn sync(&mut self) {
        let now = Instant::now();
        publish(&mut self.outputs, &self.seat, now);
        for (id, raw) in &mut self.raw {
            if let Some((_, &player)) = self.seat.pads.iter().find(|(pad, _)| pad.contains(*id)) {
                raw.light.assign(player, now, |p| {
                    // SAFETY: the raw descriptor takes an immediate scalar in 1..=4.
                    unsafe {
                        raw.device
                            .as_handle()
                            .control(SET_JOYCON_PLAYER, usize::from(p))
                    }
                });
            }
        }
    }

    fn run(&mut self) -> ! {
        let mut rescan = Instant::now();
        let mut pending = Vec::<TimedSample>::new();
        loop {
            if Instant::now() >= rescan {
                self.discover();
                self.sync();
                rescan = Instant::now() + RESCAN;
            }
            let mut fds: Vec<_> = self
                .raw
                .values()
                .map(|raw| PollHandle::new(raw.device.as_handle().as_raw() as u32, POLLIN))
                .collect();
            // Empty poll sleeps until discovery is due. Activity on a live
            // half wakes immediately, without delaying the next hotplug scan.
            let mut wait = rescan.saturating_duration_since(Instant::now());
            if let Some(&(time, _, _)) = pending.first() {
                let now = scarlet_os::time::monotonic_time_ns();
                wait = wait.min(Duration::from_nanos(
                    time.saturating_add(1000).saturating_sub(now),
                ));
            }
            if poll::poll(&mut fds, wait.as_nanos() as i64).is_err() {
                std::thread::sleep(RESCAN);
                continue;
            }
            let cutoff = scarlet_os::time::monotonic_time_ns();
            let (frames, gone) = collect_through(&mut self.raw, &mut pending, cutoff, Raw::drain);
            for (_, id, state) in frames {
                self.seat.update(id, state);
                self.sync();
            }
            if !gone.is_empty() {
                for id in gone {
                    self.raw.remove(&id);
                    self.seat.remove(id);
                }
                self.sync();
            }
        }
    }
}

/// A dedicated physical-seat claim follows the last close, including process
/// exit. Ordinary uinput producers and the filesystem are independent of it.
fn claim_instance() -> Result<Handle, String> {
    let handle = Handle::open("/dev/uinput", VFS_O_WRONLY as usize)
        .map_err(|e| format!("open Joy-Con mapper control: {e:?}"))?;
    // SAFETY: this control takes the immediate scalar zero on an empty producer.
    unsafe { handle.control(SCTL_JOYCON_CLAIM_MAPPER, 0) }
        .map_err(|e| format!("claim Joy-Con mapper: {e:?}"))?;
    Ok(handle)
}

fn main() -> Result<(), String> {
    let _instance = claim_instance()?;
    Daemon::default().run()
}

#[cfg(test)]
#[path = "joycond_tests.rs"]
mod tests;
