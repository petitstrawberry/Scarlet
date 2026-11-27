//! Published gamepads broadcast complete snapshots. Readers share bounded history
//! but own their positions, overflow recovery, and nonblocking mode.
use super::*;
use crate::sync::waker::WaitResult;
use core::sync::atomic::AtomicBool;

#[derive(Default)]
pub(super) struct History {
    end: u64,
    latest_len: usize,
}

impl EventQueue {
    pub(super) fn push_snapshot(&mut self, frame: &[(u16, u16, i32)], time: u64) -> bool {
        let Some((&(0, SYN_REPORT, 0), controls)) = frame.split_last() else {
            return false;
        };
        if frame.len() > EVENT_QUEUE_CAPACITY
            || controls.is_empty()
            || controls.iter().any(|&(ty, _, _)| !matches!(ty, 1 | 3))
        {
            return false;
        }
        let history = self.history.as_mut().expect("gamepad snapshot history");
        let excess = (self.events.len() + frame.len()).saturating_sub(EVENT_QUEUE_CAPACITY);
        self.events.drain(..excess);
        self.events
            .extend(frame.iter().map(|&(type_, code, value)| InputEvent {
                time,
                type_,
                code,
                value,
            }));
        history.end += frame.len() as u64;
        history.latest_len = frame.len();
        true
    }
}

struct Cursor {
    next: u64,
    pending: VecDeque<InputEvent>,
}

impl Cursor {
    fn snapshot(&mut self, q: &EventQueue, dropped: bool) {
        let history = q.history.as_ref().unwrap();
        let start = q.events.len() - history.latest_len;
        if dropped {
            let time = q.events[start].time;
            for code in [SYN_DROPPED, SYN_REPORT] {
                self.pending.push_back(InputEvent {
                    time,
                    type_: 0,
                    code,
                    value: 0,
                });
            }
        }
        self.pending.extend(q.events.range(start..).copied());
        self.next = history.end;
    }

    fn read(&mut self, q: &EventQueue, buffer: &mut [u8]) -> usize {
        let end = q.history.as_ref().unwrap().end;
        let first = end - q.events.len() as u64;
        let mut bytes = 0;
        for record in buffer.chunks_exact_mut(InputEvent::size()) {
            if self.pending.is_empty() && self.next < first {
                self.snapshot(q, true);
            }
            let event = if let Some(event) = self.pending.pop_front() {
                event
            } else if self.next < end {
                let event = q.events[(self.next - first) as usize];
                self.next += 1;
                event
            } else {
                break;
            };
            encode_event(event, record);
            bytes += record.len();
        }
        bytes
    }
}

pub(super) struct Reader {
    source: Arc<EventDevice>,
    cursor: IrqSpinLock<Cursor>,
    nonblocking: AtomicBool,
}

impl Reader {
    pub(super) fn new(source: Arc<EventDevice>, q: &EventQueue) -> Self {
        let mut cursor = Cursor {
            next: 0,
            pending: VecDeque::new(),
        };
        cursor.snapshot(q, false);
        Self {
            source,
            cursor: IrqSpinLock::new(cursor),
            nonblocking: AtomicBool::new(false),
        }
    }

    fn readable(&self) -> bool {
        let q = self.source.queue.lock();
        let cursor = self.cursor.lock();
        q.detached() || !cursor.pending.is_empty() || cursor.next < q.history.as_ref().unwrap().end
    }
}

impl Device for Reader {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }
    fn name(&self) -> &'static str {
        "gamepad-reader"
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
    fn capabilities(&self) -> &'static [crate::device::DeviceCapability] {
        self.source.capabilities()
    }
}

impl CharDevice for Reader {
    fn read_byte(&self) -> Option<u8> {
        None
    }
    fn read(&self, buf: &mut [u8]) -> usize {
        self.try_read(buf).unwrap_or(0)
    }
    fn try_read(&self, buf: &mut [u8]) -> Result<usize, StreamError> {
        if buf.is_empty() {
            return Ok(0);
        }
        if buf.len() < InputEvent::size() {
            return Err(StreamError::InvalidArgument);
        }
        loop {
            {
                let q = self.source.queue.lock();
                let count = self.cursor.lock().read(&q, buf);
                if count != 0 {
                    return Ok(count);
                }
                if q.detached() {
                    return Err(StreamError::EndOfStream);
                }
            }
            if self.is_nonblocking() {
                return Err(StreamError::WouldBlock);
            }
            let task = crate::task::mytask().ok_or(StreamError::WouldBlock)?;
            if self.source.waker.wait_with_condition(
                task.get_id(),
                task.get_trapframe(),
                None,
                0,
                || self.readable(),
            ) == WaitResult::Interrupted
            {
                return Err(StreamError::Interrupted);
            }
        }
    }
    fn try_read_at(&self, _: u64, buf: &mut [u8]) -> Result<usize, StreamError> {
        self.try_read(buf)
    }
    fn write_byte(&self, _: u8) -> Result<(), &'static str> {
        Err("Gamepad reader is read only")
    }
    fn write(&self, _: &[u8]) -> Result<usize, &'static str> {
        Err("Gamepad reader is read only")
    }
    fn can_read(&self) -> bool {
        self.readable()
    }
    fn can_write(&self) -> bool {
        false
    }
}

impl ControlOps for Reader {
    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        self.source.supported_control_commands()
    }

    fn control(&self, cmd: u32, arg: usize) -> Result<i32, &'static str> {
        match cmd {
            0x5353_0007 => {
                self.set_nonblocking(arg != 0);
                Ok(0)
            }
            _ => self.source.control(cmd, arg),
        }
    }
}

impl MemoryMappingOps for Reader {
    fn get_mapping_info(
        &self,
        _: usize,
        _: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("Gamepad reader has no mapping")
    }
}

impl Selectable for Reader {
    fn is_hung_up(&self) -> bool {
        self.source.is_hung_up()
    }
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        ReadySet {
            read: interest.read && self.readable(),
            write: false,
            except: interest.except && self.is_hung_up(),
        }
    }
    fn subscribe_readiness(
        &self,
        interest: ReadyInterest,
        waiter: &Arc<Waker>,
    ) -> Option<Vec<crate::sync::readiness::ReadinessRegistration>> {
        self.source.subscribe_readiness(interest, waiter)
    }
    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        tf: &mut Trapframe,
        timeout: Option<u64>,
        min: u64,
    ) -> SelectWaitOutcome {
        let ready = || (interest.read && self.readable()) || self.is_hung_up();
        if ready() {
            return SelectWaitOutcome::Ready;
        }
        let Some(task) = crate::task::mytask() else {
            return SelectWaitOutcome::TimedOut;
        };
        match self
            .source
            .waker
            .wait_with_condition(task.get_id(), tf, timeout, min, ready)
        {
            WaitResult::Interrupted => SelectWaitOutcome::Interrupted,
            WaitResult::TimedOut => SelectWaitOutcome::TimedOut,
            WaitResult::Woken => SelectWaitOutcome::Ready,
        }
    }
    fn set_nonblocking(&self, enabled: bool) {
        self.nonblocking.store(enabled, Ordering::Relaxed);
    }
    fn is_nonblocking(&self) -> bool {
        self.nonblocking.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(value: i32) -> [(u16, u16, i32); 2] {
        [(3, 0, value), (0, SYN_REPORT, 0)]
    }

    fn next(fd: &dyn Device) -> (u16, u16, i32) {
        let mut buf = [0; InputEvent::size()];
        assert_eq!(
            fd.as_char_device()
                .unwrap()
                .try_read_at(0, &mut buf)
                .unwrap(),
            buf.len()
        );
        (
            u16::from_ne_bytes(buf[8..10].try_into().unwrap()),
            u16::from_ne_bytes(buf[10..12].try_into().unwrap()),
            i32::from_ne_bytes(buf[12..].try_into().unwrap()),
        )
    }

    fn gamepad() -> Arc<EventDevice> {
        let metadata = InputDeviceMetadata::new(InputDeviceKind::Gamepad, 0)
            .with_absolute_axis(0, 0, 200)
            .unwrap();
        Arc::new(EventDevice::logical_gamepad(metadata).unwrap().1)
    }

    #[test_case]
    fn restarted_gamepad_reader_replays_current_state_and_detaches_independently() {
        let pad = gamepad();
        pad.push_events(&frame(17));
        let first = pad.clone().open().unwrap();
        assert_eq!(next(&*first), frame(17)[0]);
        pad.push_events(&frame(44));
        let second = pad.clone().open().unwrap();
        second.set_nonblocking(true);
        assert!(!first.is_nonblocking());
        assert_eq!(next(&*second), frame(44)[0]);
        assert_eq!(next(&*second), frame(44)[1]);
        let mut record = [0; InputEvent::size()];
        assert!(matches!(
            second.as_char_device().unwrap().try_read(&mut record),
            Err(StreamError::WouldBlock)
        ));
        assert_eq!(next(&*first), frame(17)[1]);
        assert_eq!(next(&*first), frame(44)[0]);
        pad.push_events(&frame(0));
        pad.detach_gamepad();
        assert!(pad.clone().open().is_err());
        let replacement = gamepad();
        assert_eq!(replacement.get_name(), pad.get_name());
        replacement.push_events(&frame(88));
        assert_eq!(next(&*second), frame(0)[0]);
        assert_eq!(next(&*second), frame(0)[1]);
        assert!(matches!(
            second.as_char_device().unwrap().try_read(&mut record),
            Err(StreamError::EndOfStream)
        ));
        assert_eq!(next(&*replacement.open().unwrap()), frame(88)[0]);
    }

    #[test_case]
    fn gamepad_overrun_preserves_fast_reader_and_resynchronizes_slow_reader() {
        let pad = gamepad();
        pad.push_events(&frame(0));
        let fast = pad.clone().open().unwrap();
        let slow = pad.clone().open().unwrap();
        for fd in [&fast, &slow] {
            for event in frame(0) {
                assert_eq!(next(&**fd), event);
            }
        }
        for value in 1..=200 {
            pad.push_events(&frame(value));
            for event in frame(value) {
                assert_eq!(next(&*fast), event);
            }
        }
        assert_eq!(next(&*slow), (0, SYN_DROPPED, 0));
        assert_eq!(next(&*slow), (0, SYN_REPORT, 0));
        for event in frame(200) {
            assert_eq!(next(&*slow), event);
        }
    }
}
