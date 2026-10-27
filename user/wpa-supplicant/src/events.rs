//! Coalesce IPC and radio notifications onto one selectable socket. Native
//! multi-handle poll still uses timed rechecks, so the actor waits on one handle.
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};
use scarlet_os::{
    poll::{self, POLLIN, PollHandle},
    socket::Socket,
    time::monotonic_time_ns,
    wireless::{EVENT_OVERFLOW, EVENT_REMOVED, WirelessSession},
};
use std::thread;

pub struct Events {
    reader: Socket,
    writer: Socket,
    pending: AtomicBool,
    watching: AtomicBool,
}
impl Events {
    pub fn new() -> Self {
        let (reader, writer) = Socket::pair().expect("wirelessd event socket");
        Self {
            reader,
            writer,
            pending: AtomicBool::new(false),
            watching: AtomicBool::new(false),
        }
    }
    pub fn notify(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            // At most one byte is outstanding, so producer notification cannot
            // fill the socket while the actor waits for a radio operation.
            self.writer
                .as_stream()
                .expect("wirelessd event stream")
                .write_all(&[1])
                .expect("wirelessd event publication");
        }
    }
    /// Clear the old wake before inspecting queues/inboxes. Producers racing
    /// this clear either appear in that inspection or leave a new readable byte.
    pub fn drain(&self) {
        let mut handles = [PollHandle::new(self.reader.as_raw() as u32, POLLIN)];
        if poll::poll(&mut handles, 0).expect("wirelessd event query") != 0 {
            let mut byte = [0];
            self.reader
                .as_stream()
                .expect("wirelessd event stream")
                .read_exact(&mut byte)
                .expect("wirelessd event read");
            self.pending.store(false, Ordering::Release);
        }
    }
    pub fn wait(&self, deadline: Option<u64>) {
        let timeout = deadline.map_or(-1, |end| {
            end.saturating_sub(monotonic_time_ns()).min(i64::MAX as u64) as i64
        });
        let mut handles = [PollHandle::new(self.reader.as_raw() as u32, POLLIN)];
        poll::poll(&mut handles, timeout).expect("wirelessd event wait");
    }
    pub fn watch_radio(self: &Arc<Self>) {
        if self.watching.load(Ordering::Acquire) {
            return;
        }
        let Ok(mut session) = WirelessSession::open("/dev/wifi0") else {
            return;
        };
        if session.subscribe().is_err() {
            return;
        }
        self.watching.store(true, Ordering::Release);
        let events = self.clone();
        thread::spawn(move || {
            let mut handles = [PollHandle::new(session.handle().as_raw() as u32, POLLIN)];
            'watch: loop {
                if poll::poll(&mut handles, -1).is_err() {
                    break;
                }
                loop {
                    match session.next_event() {
                        Ok(Some(event)) => {
                            if event.kind == EVENT_REMOVED {
                                break 'watch;
                            }
                            if event.kind == EVENT_OVERFLOW {
                                if session.resync().is_err() {
                                    break 'watch;
                                }
                                break;
                            }
                        }
                        Ok(None) => break,
                        Err(_) => break 'watch,
                    }
                }
                events.notify();
            }
            events.watching.store(false, Ordering::Release);
            events.notify();
        });
    }
}
