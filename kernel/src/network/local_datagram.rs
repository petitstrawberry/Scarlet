//! Bounded, atomic messages on the native local socket implementation.
use super::*;
const MAX_MESSAGES: usize = 64;
const MAX_BYTES: usize = 128 * 1024;
pub(crate) const MAX_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_WATCHERS: usize = 256;
#[derive(Clone)]
pub(crate) struct Message {
    pub end_of_input: bool,
    pub data: Vec<u8>,
    pub handles: Vec<(KernelObject, HandleMetadata)>,
    pub sender: SocketCredentials,
    pub source: Option<String>,
}
struct State {
    messages: VecDeque<Message>,
    bytes: usize,
    handles: usize,
    closed: bool,
    write_shutdown: bool,
    watchers: Vec<Weak<LocalSocket>>,
}
pub(super) struct Inbox {
    state: IrqRwSpinLock<State>,
    pub event: Waker,
}
impl Inbox {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: IrqRwSpinLock::new(State {
                messages: VecDeque::new(),
                bytes: 0,
                handles: 0,
                closed: false,
                write_shutdown: false,
                watchers: Vec::new(),
            }),
            event: Waker::new_interruptible("local_datagram"),
        })
    }
    fn fits(state: &State, bytes: usize, handles: usize) -> bool {
        state.messages.len() < MAX_MESSAGES
            && state.bytes.saturating_add(bytes) <= MAX_BYTES
            && state.handles.saturating_add(handles) <= MAX_HANDLE_QUEUE_SIZE
    }
    fn changed(&self) {
        self.event.wake_all();
        let watchers: Vec<_> = self
            .state
            .read()
            .watchers
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        for watcher in watchers {
            watcher.datagrams.event.wake_all();
        }
    }
    pub(super) fn watch(&self, socket: &LocalSocket) -> Result<(), SocketError> {
        let weak = socket.self_weak.read().clone();
        if weak.strong_count() == 0 {
            return Err(SocketError::InvalidOperation);
        }
        let mut state = self.state.write();
        state.watchers.retain(|w| w.strong_count() != 0);
        if state.watchers.iter().any(|w| Weak::ptr_eq(w, &weak)) {
            return Ok(());
        }
        if state.watchers.len() >= MAX_WATCHERS {
            return Err(SocketError::WouldBlock);
        }
        state.watchers.push(weak);
        Ok(())
    }
    pub fn close(&self) {
        let messages = {
            let mut state = self.state.write();
            state.closed = true;
            state.bytes = 0;
            state.handles = 0;
            core::mem::take(&mut state.messages)
        };
        self.changed();
        drop(messages);
    }
}
impl LocalSocket {
    pub(crate) fn is_datagram(&self) -> bool {
        self.socket_type == SocketType::Datagram
    }
    fn datagram_target(
        &self,
        address: &SocketAddress,
    ) -> Result<Arc<dyn SocketObject>, SocketError> {
        match address {
            SocketAddress::Local(address) => NetworkManager::get_manager()
                .lookup_named_socket(&local_socket_registry_name(address))
                .map_err(|_| SocketError::ConnectionRefused),
            SocketAddress::Unspecified => self
                .upgrade_peer()
                .map(|socket| socket as Arc<dyn SocketObject>)
                .ok_or(if *self.state.read() == SocketState::Connected {
                    SocketError::ConnectionRefused
                } else {
                    SocketError::DestinationRequired
                }),
            _ => Err(SocketError::InvalidAddress),
        }
    }
    pub(super) fn connect_datagram(&self, address: &SocketAddress) -> Result<(), SocketError> {
        if *self.state.read() == SocketState::Closed {
            return Err(SocketError::NotConnected);
        }
        if matches!(address, SocketAddress::Unspecified) {
            *self.peer_socket.write() = None;
            *self.peer_addr.write() = None;
            *self.peer_credentials.write() = None;
            *self.peer_security_label.write() = None;
            *self.state.write() = if self.local_addr.read().is_some() {
                SocketState::Bound
            } else {
                SocketState::Unconnected
            };
            self.datagrams.event.wake_all();
            return Ok(());
        }
        let target = self.datagram_target(address)?;
        let peer = LocalSocket::from_socket_object(target.as_ref())
            .ok_or(SocketError::ProtocolMismatch)?;
        if !peer.is_datagram() {
            return Err(SocketError::ProtocolMismatch);
        }
        if *peer.state.read() == SocketState::Closed || peer.datagrams.state.read().closed {
            return Err(SocketError::ConnectionRefused);
        }
        peer.datagrams.watch(self)?;
        *self.peer_socket.write() = Some(peer.self_weak.read().clone());
        *self.peer_addr.write() = peer.local_addr.read().clone();
        *self.peer_credentials.write() = None;
        *self.peer_security_label.write() = None;
        *self.state.write() = SocketState::Connected;
        self.datagrams.event.wake_all();
        Ok(())
    }
    pub(crate) fn send_datagram(
        &self,
        data: Vec<u8>,
        handles: Vec<(KernelObject, HandleMetadata)>,
        address: &SocketAddress,
        nonblocking: bool,
    ) -> Result<usize, SocketError> {
        if data.len() > MAX_MESSAGE_BYTES || handles.len() > MAX_HANDLE_QUEUE_SIZE {
            return Err(SocketError::MessageTooLarge);
        }
        if *self.state.read() == SocketState::Closed {
            return Err(SocketError::NotConnected);
        }
        if self.datagrams.state.read().write_shutdown {
            return Err(SocketError::BrokenPipe);
        }
        let target = self.datagram_target(address)?;
        let peer = LocalSocket::from_socket_object(target.as_ref())
            .ok_or(SocketError::ProtocolMismatch)?;
        if !peer.is_datagram() {
            return Err(SocketError::ProtocolMismatch);
        }
        if *peer.state.read() == SocketState::Connected {
            let only = peer.upgrade_peer().ok_or(SocketError::ConnectionRefused)?;
            if !core::ptr::eq(only.as_ref(), self) {
                return Err(SocketError::ConnectionRefused);
            }
        }
        peer.datagrams.watch(self)?;
        let length = data.len();
        let handle_count = handles.len();
        let mut message = Some(Message {
            end_of_input: false,
            data,
            handles,
            sender: Self::current_credentials(false),
            source: self.local_addr.read().clone(),
        });
        loop {
            if self.datagrams.state.read().write_shutdown {
                return Err(SocketError::BrokenPipe);
            }
            let queued = {
                let mut state = peer.datagrams.state.write();
                if state.closed || *self.state.read() == SocketState::Closed {
                    return Err(SocketError::ConnectionRefused);
                }
                if Inbox::fits(&state, length, handle_count) {
                    state.bytes += length;
                    state.handles += handle_count;
                    state.messages.push_back(message.take().unwrap());
                    true
                } else {
                    false
                }
            };
            if queued {
                peer.datagrams.event.wake_all();
                return Ok(length);
            }
            if nonblocking || *self.nonblocking.read() {
                return Err(SocketError::WouldBlock);
            }
            let task = crate::task::mytask().ok_or(SocketError::WouldBlock)?;
            if self.datagrams.event.wait_with_condition(
                task.get_id(),
                task.get_trapframe(),
                None,
                0,
                || {
                    if self.datagrams.state.read().write_shutdown || *self.nonblocking.read() {
                        return true;
                    }
                    let state = peer.datagrams.state.read();
                    state.closed
                        || Inbox::fits(&state, length, handle_count)
                        || *self.state.read() == SocketState::Closed
                },
            ) == WaitResult::Interrupted
            {
                return Err(SocketError::Interrupted);
            }
        }
    }
    pub(crate) fn receive_datagram(
        &self,
        peek: bool,
        nonblocking: bool,
    ) -> Result<Message, SocketError> {
        loop {
            let message = {
                let mut state = self.datagrams.state.write();
                if let Some(message) = state.messages.front() {
                    if peek {
                        return Ok(message.clone());
                    }
                    let message = state.messages.pop_front().unwrap();
                    state.bytes -= message.data.len();
                    state.handles -= message.handles.len();
                    Some(message)
                } else if state.closed {
                    if *self.state.read() == SocketState::Closed {
                        return Err(SocketError::NotConnected);
                    }
                    return Ok(Message {
                        end_of_input: true,
                        data: Vec::new(),
                        handles: Vec::new(),
                        sender: SocketCredentials::unknown(),
                        source: None,
                    });
                } else {
                    None
                }
            };
            if let Some(message) = message {
                self.datagrams.changed();
                return Ok(message);
            }
            if nonblocking || *self.nonblocking.read() {
                return Err(SocketError::WouldBlock);
            }
            let task = crate::task::mytask().ok_or(SocketError::WouldBlock)?;
            if self.datagrams.event.wait_with_condition(
                task.get_id(),
                task.get_trapframe(),
                None,
                0,
                || {
                    let state = self.datagrams.state.read();
                    state.closed || !state.messages.is_empty() || *self.nonblocking.read()
                },
            ) == WaitResult::Interrupted
            {
                return Err(SocketError::Interrupted);
            }
        }
    }
    pub(super) fn shutdown_datagram(&self, how: ShutdownHow) -> Result<(), SocketError> {
        if !matches!(*self.state.read(), SocketState::Connected) {
            return Err(SocketError::NotConnected);
        }
        if matches!(how, ShutdownHow::Read | ShutdownHow::Both) {
            self.datagrams.close();
        }
        if matches!(how, ShutdownHow::Write | ShutdownHow::Both) {
            self.datagrams.state.write().write_shutdown = true;
        }
        self.datagrams.event.wake_all();
        if let Some(peer) = self.upgrade_peer() {
            peer.datagrams.event.wake_all();
        }
        Ok(())
    }
    pub(super) fn datagram_ready(&self, interest: ReadyInterest) -> ReadySet {
        let mut ready = ReadySet::none();
        let state = self.datagrams.state.read();
        ready.read = interest.read && (state.closed || !state.messages.is_empty());
        let write_shutdown = state.write_shutdown || state.closed;
        drop(state);
        if interest.write && write_shutdown {
            ready.write = true;
            return ready;
        }
        if interest.write {
            ready.write = match self.upgrade_peer() {
                Some(peer) => {
                    let state = peer.datagrams.state.read();
                    state.closed || Inbox::fits(&state, 1, 0)
                }
                None => true,
            };
        }
        ready
    }
    pub(super) fn wait_datagram(
        &self,
        interest: ReadyInterest,
        frame: &mut crate::arch::Trapframe,
        timeout: Option<u64>,
        minimum: u64,
    ) -> SelectWaitOutcome {
        let task_id = crate::task::mytask().map(|task| task.get_id()).unwrap_or(0);
        match self
            .datagrams
            .event
            .wait_with_condition(task_id, frame, timeout, minimum, || {
                let ready = self.datagram_ready(interest);
                (interest.read && ready.read) || (interest.write && ready.write)
            }) {
            WaitResult::Interrupted => SelectWaitOutcome::Interrupted,
            WaitResult::TimedOut => SelectWaitOutcome::TimedOut,
            WaitResult::Woken => SelectWaitOutcome::Ready,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pair() -> (Arc<LocalSocket>, Arc<LocalSocket>) {
        LocalSocket::create_connected_pair_with_type(
            SocketType::Datagram,
            "first".into(),
            "second".into(),
        )
    }
    #[test_case]
    fn empty_messages_are_bounded_and_truncation_consumes_one_record() {
        let (sender, receiver) = pair();
        for _ in 0..MAX_MESSAGES {
            assert_eq!(
                sender
                    .send_datagram(Vec::new(), Vec::new(), &SocketAddress::Unspecified, true)
                    .unwrap(),
                0
            );
        }
        assert!(matches!(
            sender.send_datagram(Vec::new(), Vec::new(), &SocketAddress::Unspecified, true),
            Err(SocketError::WouldBlock)
        ));
        assert!(!receiver.receive_datagram(false, true).unwrap().end_of_input);
        sender
            .send_datagram(
                b"first".to_vec(),
                Vec::new(),
                &SocketAddress::Unspecified,
                true,
            )
            .unwrap();
        for _ in 1..MAX_MESSAGES {
            receiver.receive_datagram(false, true).unwrap();
        }
        let mut short = [0; 2];
        assert_eq!(
            receiver
                .recvfrom(
                    &mut short,
                    crate::network::socket::message_flags::FULL_LENGTH
                )
                .unwrap()
                .0,
            5
        );
        assert_eq!(&short, b"fi");
        assert!(matches!(
            receiver.receive_datagram(false, true),
            Err(SocketError::WouldBlock)
        ));
    }
    #[test_case]
    fn datagram_close_does_not_create_stream_eof_and_shutdown_is_directional() {
        let (sender, receiver) = pair();
        sender.shutdown(ShutdownHow::Write).unwrap();
        assert!(matches!(
            sender.send_datagram(b"x".to_vec(), Vec::new(), &SocketAddress::Unspecified, true),
            Err(SocketError::BrokenPipe)
        ));
        receiver
            .send_datagram(
                b"reply".to_vec(),
                Vec::new(),
                &SocketAddress::Unspecified,
                true,
            )
            .unwrap();
        assert_eq!(sender.receive_datagram(false, true).unwrap().data, b"reply");
        sender.close_handle();
        assert!(matches!(
            receiver.receive_datagram(false, true),
            Err(SocketError::WouldBlock)
        ));
        receiver.shutdown(ShutdownHow::Read).unwrap();
        assert!(receiver.receive_datagram(false, true).unwrap().end_of_input);
    }
    #[test_case]
    fn datagram_peek_and_capacity_failures_preserve_whole_records() {
        let (sender, receiver) = pair();
        assert!(matches!(
            sender.send_datagram(
                alloc::vec![0;MAX_MESSAGE_BYTES+1],
                Vec::new(),
                &SocketAddress::Unspecified,
                true
            ),
            Err(SocketError::MessageTooLarge)
        ));
        sender
            .send_datagram(
                b"one".to_vec(),
                Vec::new(),
                &SocketAddress::Unspecified,
                true,
            )
            .unwrap();
        sender
            .send_datagram(
                b"two".to_vec(),
                Vec::new(),
                &SocketAddress::Unspecified,
                true,
            )
            .unwrap();
        assert_eq!(receiver.receive_datagram(true, true).unwrap().data, b"one");
        assert_eq!(receiver.receive_datagram(false, true).unwrap().data, b"one");
        assert_eq!(receiver.receive_datagram(false, true).unwrap().data, b"two");
        assert!(matches!(
            receiver.receive_datagram(false, true),
            Err(SocketError::WouldBlock)
        ));
    }
}
