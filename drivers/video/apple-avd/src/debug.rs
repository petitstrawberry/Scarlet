use alloc::vec::Vec;

/// Apple AVD driver trace event category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvdTraceKind {
    /// Platform probe or resource discovery.
    Probe,
    /// Firmware staging or state transition.
    Firmware,
    /// AP to CM3 mailbox traffic.
    MailboxTx,
    /// CM3 to AP mailbox traffic.
    MailboxRx,
    /// H.264 decode request submission.
    DecodeSubmit,
    /// Decode completion notification.
    DecodeComplete,
    /// Hardware or firmware fault.
    Fault,
}

/// One in-kernel Apple AVD trace event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvdTraceEvent {
    /// Monotonic sequence number assigned by the trace log.
    pub sequence: u64,
    /// Event category.
    pub kind: AvdTraceKind,
    /// First event-specific value.
    pub arg0: u64,
    /// Second event-specific value.
    pub arg1: u64,
}

/// Fixed-capacity Apple AVD trace log.
pub struct AvdTraceLog {
    entries: Vec<AvdTraceEvent>,
    next_sequence: u64,
    capacity: usize,
}

impl AvdTraceLog {
    /// Create an empty trace log.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Maximum number of events retained.
    ///
    /// # Returns
    ///
    /// Empty trace log retaining at most `capacity` events.
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            next_sequence: 0,
            capacity,
        }
    }

    /// Append one trace event.
    ///
    /// # Arguments
    ///
    /// * `kind` - Event category.
    /// * `arg0` - First event-specific value.
    /// * `arg1` - Second event-specific value.
    pub fn push(&mut self, kind: AvdTraceKind, arg0: u64, arg1: u64) {
        if self.capacity == 0 {
            self.next_sequence = self.next_sequence.wrapping_add(1);
            return;
        }

        if self.entries.len() == self.capacity {
            self.entries.remove(0);
        }

        let event = AvdTraceEvent {
            sequence: self.next_sequence,
            kind,
            arg0,
            arg1,
        };
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.entries.push(event);
    }

    /// Return retained trace events.
    ///
    /// # Returns
    ///
    /// Trace event slice ordered from oldest to newest.
    pub fn entries(&self) -> &[AvdTraceEvent] {
        &self.entries
    }

    /// Return the configured event capacity.
    ///
    /// # Returns
    ///
    /// Maximum number of retained events.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Remove all retained events.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}
