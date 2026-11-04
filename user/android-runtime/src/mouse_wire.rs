use scarlet_os::{Handle, handle::capability::StreamError};
use std::collections::VecDeque;

pub const RECORD_BYTES: usize = 40;
pub const QUEUE_RECORDS: usize = 256;

const MAGIC: u32 = 0x314d_4153;
const VERSION: u16 = 1;
const KIND_POSITION: u16 = 1;
const KIND_BUTTON: u16 = 2;
const KIND_SCROLL: u16 = 3;
const KIND_CANCEL: u16 = 4;

#[derive(Clone, Copy)]
pub struct Modifiers(u32);

impl Modifiers {
    pub const SHIFT: u32 = 1 << 0;
    pub const CONTROL: u32 = 1 << 1;
    pub const ALT: u32 = 1 << 2;
    pub const META: u32 = 1 << 3;
    pub const CAPS_LOCK: u32 = 1 << 4;
    pub const NUM_LOCK: u32 = 1 << 5;
    pub const SCROLL_LOCK: u32 = 1 << 6;

    pub const fn new(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }
}

impl Default for Modifiers {
    fn default() -> Self {
        Self(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Position {
        x: i32,
        y: i32,
    },
    Button {
        x: i32,
        y: i32,
        code: u16,
        value: i32,
    },
    Scroll {
        x: i32,
        y: i32,
        code: u16,
        value: i32,
    },
    Cancel {
        x: i32,
        y: i32,
    },
}

#[derive(Clone, Copy)]
struct Record([u8; RECORD_BYTES]);

impl Record {
    fn position(x: i32, y: i32, modifiers: Modifiers) -> Self {
        Self::new(KIND_POSITION, 0, 0, x, y, modifiers)
    }

    fn button(x: i32, y: i32, code: u16, value: i32, modifiers: Modifiers) -> Self {
        Self::new(KIND_BUTTON, code, value, x, y, modifiers)
    }

    fn scroll(x: i32, y: i32, code: u16, value: i32, modifiers: Modifiers) -> Self {
        Self::new(KIND_SCROLL, code, value, x, y, modifiers)
    }

    fn cancel(x: i32, y: i32, modifiers: Modifiers) -> Self {
        Self::new(KIND_CANCEL, 0, 0, x, y, modifiers)
    }

    fn new(kind: u16, code: u16, value: i32, x: i32, y: i32, modifiers: Modifiers) -> Self {
        let mut bytes = [0u8; RECORD_BYTES];
        bytes[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[4..6].copy_from_slice(&VERSION.to_le_bytes());
        bytes[6..8].copy_from_slice(&kind.to_le_bytes());
        bytes[8..12].copy_from_slice(&x.to_le_bytes());
        bytes[12..16].copy_from_slice(&y.to_le_bytes());
        bytes[16..20].copy_from_slice(&u32::from(code).to_le_bytes());
        bytes[20..24].copy_from_slice(&value.to_le_bytes());
        // timestamp_ns deliberately remains zero: the receiver supplies its
        // own uptime when it injects the event into Android.
        bytes[32..36].copy_from_slice(&modifiers.bits().to_le_bytes());
        Self(bytes)
    }

    fn bytes(&self) -> &[u8; RECORD_BYTES] {
        &self.0
    }

    fn kind(&self) -> u16 {
        u16::from_le_bytes([self.0[6], self.0[7]])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WriteError {
    WouldBlock,
    Fatal,
}

trait Writer {
    fn write(&mut self, record: &[u8; RECORD_BYTES]) -> Result<(), WriteError>;
}

struct PipeWriter {
    handle: Handle,
}

impl PipeWriter {
    fn new(handle: Handle) -> Result<Self, &'static str> {
        handle
            .set_nonblocking(true)
            .map_err(|_| "mouse pipe nonblocking mode")?;
        handle
            .as_stream()
            .map_err(|_| "mouse pipe stream capability")?;
        Ok(Self { handle })
    }
}

impl Writer for PipeWriter {
    fn write(&mut self, record: &[u8; RECORD_BYTES]) -> Result<(), WriteError> {
        let stream = self.handle.as_stream().map_err(|_| WriteError::Fatal)?;
        match stream.write(record) {
            Ok(RECORD_BYTES) => Ok(()),
            Ok(_) => Err(WriteError::Fatal),
            Err(StreamError::WouldBlock | StreamError::Interrupted) => Err(WriteError::WouldBlock),
            Err(_) => Err(WriteError::Fatal),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushError {
    Full,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlushError {
    Io,
}

struct MouseQueue<W: Writer> {
    writer: Option<W>,
    pending: VecDeque<Record>,
}

impl<W: Writer> MouseQueue<W> {
    fn new(writer: W) -> Self {
        Self {
            writer: Some(writer),
            pending: VecDeque::with_capacity(QUEUE_RECORDS),
        }
    }

    fn disabled() -> Self {
        Self {
            writer: None,
            pending: VecDeque::new(),
        }
    }

    fn is_enabled(&self) -> bool {
        self.writer.is_some()
    }

    fn push(&mut self, event: Event, modifiers: Modifiers) -> Result<(), PushError> {
        if !self.is_enabled() {
            return Err(PushError::Closed);
        }
        let record = match event {
            Event::Position { x, y } => Record::position(x, y, modifiers),
            Event::Button { x, y, code, value } => Record::button(x, y, code, value, modifiers),
            Event::Scroll { x, y, code, value } => Record::scroll(x, y, code, value, modifiers),
            Event::Cancel { x, y } => Record::cancel(x, y, modifiers),
        };

        if record.kind() == KIND_POSITION
            && let Some(last) = self.pending.back_mut()
            && last.kind() == KIND_POSITION
        {
            *last = record;
            return Ok(());
        }
        if self.pending.len() == QUEUE_RECORDS {
            self.fail_closed();
            return Err(PushError::Full);
        }
        self.pending.push_back(record);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), FlushError> {
        let Some(writer) = self.writer.as_mut() else {
            return Ok(());
        };
        loop {
            let Some(record) = self.pending.front() else {
                return Ok(());
            };
            match writer.write(record.bytes()) {
                Ok(()) => {
                    self.pending.pop_front();
                }
                Err(WriteError::WouldBlock) => return Ok(()),
                Err(WriteError::Fatal) => {
                    self.fail_closed();
                    return Err(FlushError::Io);
                }
            }
        }
    }

    fn fail_closed(&mut self) {
        self.pending.clear();
        self.writer = None;
    }
}

pub struct MouseProducer {
    queue: MouseQueue<PipeWriter>,
}

impl MouseProducer {
    pub fn new(handle: Handle) -> Result<Self, &'static str> {
        Ok(Self {
            queue: MouseQueue::new(PipeWriter::new(handle)?),
        })
    }

    pub fn disabled() -> Self {
        Self {
            queue: MouseQueue::disabled(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.queue.is_enabled()
    }

    pub fn push(&mut self, event: Event, modifiers: Modifiers) -> Result<(), PushError> {
        self.queue.push(event, modifiers)
    }

    pub fn flush(&mut self) -> Result<(), FlushError> {
        self.queue.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode {
        Ready,
        Blocked,
        Partial,
    }

    struct FakeState {
        mode: Mode,
        records: Vec<[u8; RECORD_BYTES]>,
        lengths: Vec<usize>,
        closed: bool,
    }

    struct FakeWriter(Rc<RefCell<FakeState>>);

    impl Writer for FakeWriter {
        fn write(&mut self, record: &[u8; RECORD_BYTES]) -> Result<(), WriteError> {
            let mut state = self.0.borrow_mut();
            state.lengths.push(record.len());
            match state.mode {
                Mode::Ready => {
                    state.records.push(*record);
                    Ok(())
                }
                Mode::Blocked => Err(WriteError::WouldBlock),
                Mode::Partial => Err(WriteError::Fatal),
            }
        }
    }

    impl Drop for FakeWriter {
        fn drop(&mut self) {
            self.0.borrow_mut().closed = true;
        }
    }

    fn fake(mode: Mode) -> (Rc<RefCell<FakeState>>, MouseQueue<FakeWriter>) {
        let state = Rc::new(RefCell::new(FakeState {
            mode,
            records: Vec::new(),
            lengths: Vec::new(),
            closed: false,
        }));
        (state.clone(), MouseQueue::new(FakeWriter(state)))
    }

    #[test]
    fn wire_layout_is_little_endian_and_exactly_forty_bytes() {
        let bytes = Record::button(
            -7,
            1_234,
            0x111,
            1,
            Modifiers::new(
                Modifiers::SHIFT
                    | Modifiers::CONTROL
                    | Modifiers::ALT
                    | Modifiers::META
                    | Modifiers::CAPS_LOCK
                    | Modifiers::NUM_LOCK
                    | Modifiers::SCROLL_LOCK,
            ),
        )
        .0;
        assert_eq!(bytes.len(), RECORD_BYTES);
        assert_eq!(&bytes[0..4], b"SAM1");
        assert_eq!(u16::from_le_bytes(bytes[4..6].try_into().unwrap()), VERSION);
        assert_eq!(
            u16::from_le_bytes(bytes[6..8].try_into().unwrap()),
            KIND_BUTTON
        );
        assert_eq!(i32::from_le_bytes(bytes[8..12].try_into().unwrap()), -7);
        assert_eq!(i32::from_le_bytes(bytes[12..16].try_into().unwrap()), 1_234);
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 0x111);
        assert_eq!(i32::from_le_bytes(bytes[20..24].try_into().unwrap()), 1);
        assert_eq!(&bytes[24..32], &[0; 8]);
        assert_eq!(u32::from_le_bytes(bytes[32..36].try_into().unwrap()), 0x7f);
        assert_eq!(&bytes[36..40], &[0; 4]);
    }

    #[test]
    fn blocked_queue_preserves_order_and_drains_without_new_input() {
        let (state, mut queue) = fake(Mode::Blocked);
        queue
            .push(Event::Position { x: 1, y: 2 }, Modifiers::default())
            .unwrap();
        queue
            .push(Event::Position { x: 3, y: 4 }, Modifiers::default())
            .unwrap();
        queue
            .push(
                Event::Button {
                    x: 3,
                    y: 4,
                    code: 0x110,
                    value: 1,
                },
                Modifiers::default(),
            )
            .unwrap();
        queue
            .push(
                Event::Button {
                    x: 3,
                    y: 4,
                    code: 0x110,
                    value: 0,
                },
                Modifiers::default(),
            )
            .unwrap();
        queue.flush().unwrap();
        assert!(state.borrow().records.is_empty());

        state.borrow_mut().mode = Mode::Ready;
        queue.flush().unwrap();
        let state = state.borrow();
        assert_eq!(state.records.len(), 3);
        assert!(!state.lengths.is_empty());
        assert!(state.lengths.iter().all(|length| *length == RECORD_BYTES));
        assert_eq!(
            i32::from_le_bytes(state.records[0][8..12].try_into().unwrap()),
            3
        );
        assert_eq!(
            i32::from_le_bytes(state.records[0][12..16].try_into().unwrap()),
            4
        );
        assert_eq!(state.records[1][6], KIND_BUTTON as u8);
        assert_eq!(
            i32::from_le_bytes(state.records[1][20..24].try_into().unwrap()),
            1
        );
        assert_eq!(
            i32::from_le_bytes(state.records[2][20..24].try_into().unwrap()),
            0
        );
    }

    #[test]
    fn queue_pressure_closes_writer_and_short_write_fails_closed() {
        let (state, mut queue) = fake(Mode::Blocked);
        for value in 0..QUEUE_RECORDS {
            queue
                .push(
                    Event::Button {
                        x: 1,
                        y: 2,
                        code: 0x110,
                        value: value as i32,
                    },
                    Modifiers::default(),
                )
                .unwrap();
        }
        assert_eq!(
            queue.push(
                Event::Button {
                    x: 1,
                    y: 2,
                    code: 0x111,
                    value: 1,
                },
                Modifiers::default(),
            ),
            Err(PushError::Full)
        );
        assert!(!queue.is_enabled());
        assert!(state.borrow().closed);

        let (state, mut queue) = fake(Mode::Partial);
        queue
            .push(Event::Cancel { x: 0, y: 0 }, Modifiers::default())
            .unwrap();
        assert_eq!(queue.flush(), Err(FlushError::Io));
        assert!(!queue.is_enabled());
        assert_eq!(state.borrow().lengths, vec![RECORD_BYTES]);
        assert!(state.borrow().closed);
    }
}
