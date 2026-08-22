//! sbus client library
//!
//! Provides a high-level client interface for connecting to sbusd and
//! sending/receiving messages.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use sbus::DEFAULT_SOCKET_PATH;

#[cfg(feature = "std")]
use scarlet_os::poll::{POLLERR, POLLHUP, POLLIN, PollHandle, poll};
#[cfg(feature = "std")]
use scarlet_os::socket::Socket;
#[cfg(not(feature = "std"))]
use scarlet_std::io::{Read, Write};
#[cfg(not(feature = "std"))]
use scarlet_std::poll::{POLLERR, POLLHUP, POLLIN, PollHandle, poll};
#[cfg(not(feature = "std"))]
use scarlet_std::socket::Socket;

// Re-export sbus types for convenience
pub use sbus::{Argument, Message, MessageHeader};

/// Largest sbus frame accepted by this client, including the fixed header.
const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;
/// Default upper bound for service-registration acknowledgments.
const DEFAULT_REGISTRATION_TIMEOUT_MS: u64 = 5_000;

#[cfg(feature = "std")]
fn monotonic_time_ns() -> u64 {
    scarlet_os::time::monotonic_time_ns()
}

#[cfg(not(feature = "std"))]
fn monotonic_time_ns() -> u64 {
    use scarlet_std::syscall::{Syscall, syscall0};

    syscall0(Syscall::MonotonicTime) as u64
}

/// Connection error types
#[derive(Debug)]
pub enum Error {
    /// Failed to connect to sbusd
    ConnectionFailed,
    /// Socket I/O error
    IoError,
    /// Protocol error
    ProtocolError(&'static str),
    /// Service not found
    ServiceNotFound,
    /// Method call failed
    MethodFailed(String),
    /// Timed out waiting for a response
    TimedOut,
    /// The connection cannot be reused after an incomplete method call.
    ConnectionPoisoned,
}

/// Connection to sbusd
pub struct Connection {
    socket: Socket,
    next_serial: u32,
    receive_buffer: Vec<u8>,
    pending_messages: VecDeque<Message>,
    poisoned: bool,
}

impl Connection {
    /// Connect to sbusd
    pub fn connect() -> Result<Self, Error> {
        Self::connect_to_path(DEFAULT_SOCKET_PATH)
    }

    /// Connect to sbusd at a specific path
    pub fn connect_to_path(path: &str) -> Result<Self, Error> {
        let mut socket = Socket::new().map_err(|_| Error::ConnectionFailed)?;

        if socket.connect(path).is_err() {
            return Err(Error::ConnectionFailed);
        }

        // Send HELLO message
        let hello = Message::Hello {
            client_name: "sbus-client".to_string(),
        };

        let bytes = hello
            .to_bytes()
            .map_err(|_| Error::ProtocolError("Failed to serialize HELLO"))?;
        Self::write_all(&mut socket, &bytes).map_err(|_| Error::IoError)?;

        Ok(Connection {
            socket,
            next_serial: 1,
            receive_buffer: Vec::new(),
            pending_messages: VecDeque::new(),
            poisoned: false,
        })
    }

    /// Register a service with the default bounded acknowledgment wait.
    ///
    /// # Arguments
    ///
    /// * `bus_name` - Bus name to register.
    ///
    /// # Returns
    ///
    /// `Ok(())` after sbusd acknowledges the registration, or
    /// [`Error::TimedOut`] if it does not respond before the default deadline.
    pub fn register_service(&mut self, bus_name: &str) -> Result<(), Error> {
        self.register_service_timeout(bus_name, DEFAULT_REGISTRATION_TIMEOUT_MS)
    }

    /// Register a service with a caller-provided acknowledgment deadline.
    ///
    /// # Arguments
    ///
    /// * `bus_name` - Bus name to register.
    /// * `timeout_ms` - Maximum time to wait for sbusd's response.
    ///
    /// # Returns
    ///
    /// `Ok(())` after acknowledgment, or [`Error::TimedOut`] when the deadline
    /// expires. A timed-out connection is poisoned because the late response
    /// can no longer be associated safely with a later request.
    pub fn register_service_timeout(
        &mut self,
        bus_name: &str,
        timeout_ms: u64,
    ) -> Result<(), Error> {
        self.ensure_usable()?;
        let msg = Message::RegisterService {
            bus_name: bus_name.to_string(),
        };

        let bytes = msg
            .to_bytes()
            .map_err(|_| Error::ProtocolError("Failed to serialize REGISTER_SERVICE"))?;
        self.send_bytes(&bytes)?;

        // A service becomes visible before sbusd sends this acknowledgment, so
        // asynchronous traffic may race ahead of it. Preserve that traffic for
        // receive_message() and wait specifically for the method response.
        let response = match self.wait_for_method_response_timeout(timeout_ms) {
            Ok(Some(response)) => response,
            Ok(None) => {
                self.poisoned = true;
                return Err(Error::TimedOut);
            }
            Err(error) => {
                self.poisoned = true;
                return Err(error);
            }
        };
        Self::parse_method_response(response).map(|_| ())
    }

    /// Unregister a service
    pub fn unregister_service(&mut self, bus_name: &str) -> Result<(), Error> {
        self.ensure_usable()?;
        let msg = Message::UnregisterService {
            bus_name: bus_name.to_string(),
        };

        let bytes = msg
            .to_bytes()
            .map_err(|_| Error::ProtocolError("Failed to serialize UNREGISTER_SERVICE"))?;
        self.send_bytes(&bytes)?;

        Ok(())
    }

    /// Call a method on a service
    pub fn call_method(
        &mut self,
        destination: &str,
        path: &str,
        interface: &str,
        method: &str,
        args: Vec<Argument>,
    ) -> Result<Vec<Argument>, Error> {
        self.send_method_call(destination, path, interface, method, args)?;
        let response = match self.wait_for_method_response() {
            Ok(response) => response,
            Err(error) => {
                self.poisoned = true;
                return Err(error);
            }
        };
        Self::parse_method_response(response)
    }

    /// Call a method on a service with a bounded response wait.
    ///
    /// # Arguments
    ///
    /// * `destination` - Destination service bus name.
    /// * `path` - Destination object path.
    /// * `interface` - Interface that owns the method.
    /// * `method` - Method name to invoke.
    /// * `args` - Method arguments.
    /// * `timeout_ms` - Maximum time to wait for a complete response frame.
    ///
    /// # Returns
    ///
    /// The returned arguments, or [`Error::TimedOut`] if no complete response
    /// frame arrives before the deadline.
    pub fn call_method_timeout(
        &mut self,
        destination: &str,
        path: &str,
        interface: &str,
        method: &str,
        args: Vec<Argument>,
        timeout_ms: u64,
    ) -> Result<Vec<Argument>, Error> {
        self.send_method_call(destination, path, interface, method, args)?;
        let response = match self.wait_for_method_response_timeout(timeout_ms) {
            Ok(Some(response)) => response,
            Ok(None) => {
                self.poisoned = true;
                return Err(Error::TimedOut);
            }
            Err(error) => {
                self.poisoned = true;
                return Err(error);
            }
        };
        Self::parse_method_response(response)
    }

    /// Emit a signal
    pub fn emit_signal(
        &mut self,
        sender: &str,
        path: &str,
        interface: &str,
        signal: &str,
        args: Vec<Argument>,
    ) -> Result<(), Error> {
        self.ensure_usable()?;
        let msg = Message::Signal {
            sender: sender.to_string(),
            path: path.to_string(),
            interface: interface.to_string(),
            signal: signal.to_string(),
            args,
        };

        let bytes = msg
            .to_bytes()
            .map_err(|_| Error::ProtocolError("Failed to serialize SIGNAL"))?;
        self.send_bytes(&bytes)?;

        Ok(())
    }

    /// Receive any incoming message
    pub fn receive_message(&mut self) -> Result<Message, Error> {
        self.ensure_usable()?;
        if let Some(message) = self.pending_messages.pop_front() {
            return Ok(message);
        }
        self.wait_for_response()
    }

    /// Receive an incoming message, waiting for at most `timeout_ms`.
    ///
    /// This keeps a listener thread from becoming permanently blocked while
    /// its owning application is shutting down.
    pub fn receive_message_timeout(&mut self, timeout_ms: u64) -> Result<Option<Message>, Error> {
        self.ensure_usable()?;
        if let Some(message) = self.pending_messages.pop_front() {
            return Ok(Some(message));
        }
        self.wait_for_response_timeout(timeout_ms)
    }

    /// Send a method return response
    pub fn send_method_return(&mut self, serial: u32, result: Vec<Argument>) -> Result<(), Error> {
        self.ensure_usable()?;
        let msg = Message::MethodReturn { serial, result };
        let bytes = msg
            .to_bytes()
            .map_err(|_| Error::ProtocolError("Failed to serialize METHOD_RETURN"))?;
        self.send_bytes(&bytes)?;
        Ok(())
    }

    /// Send a method error response
    pub fn send_method_error(
        &mut self,
        serial: u32,
        error_name: &str,
        message: &str,
    ) -> Result<(), Error> {
        self.ensure_usable()?;
        let msg = Message::MethodError {
            serial,
            error_name: error_name.to_string(),
            message: message.to_string(),
        };
        let bytes = msg
            .to_bytes()
            .map_err(|_| Error::ProtocolError("Failed to serialize METHOD_ERROR"))?;
        self.send_bytes(&bytes)?;
        Ok(())
    }

    fn ensure_usable(&self) -> Result<(), Error> {
        Self::ensure_not_poisoned(self.poisoned)
    }

    fn ensure_not_poisoned(poisoned: bool) -> Result<(), Error> {
        if poisoned {
            Err(Error::ConnectionPoisoned)
        } else {
            Ok(())
        }
    }

    /// Wait for a response message
    fn wait_for_response(&mut self) -> Result<Message, Error> {
        loop {
            if let Some(message) = Self::take_buffered_message(&mut self.receive_buffer)? {
                return Ok(message);
            }
            self.read_into_receive_buffer()?;
        }
    }

    /// Wait for a complete response message until a monotonic deadline.
    fn wait_for_response_timeout(&mut self, timeout_ms: u64) -> Result<Option<Message>, Error> {
        let timeout_ns = timeout_ms.saturating_mul(1_000_000);
        let deadline_ns = monotonic_time_ns().saturating_add(timeout_ns);
        let mut first_poll = true;

        self.wait_for_response_until(deadline_ns, &mut first_poll)
    }

    /// Wait for one wire message without extending the supplied deadline.
    fn wait_for_response_until(
        &mut self,
        deadline_ns: u64,
        first_poll: &mut bool,
    ) -> Result<Option<Message>, Error> {
        if let Some(message) = Self::take_buffered_message(&mut self.receive_buffer)? {
            return Ok(Some(message));
        }

        loop {
            let now_ns = monotonic_time_ns();
            let remaining_ns = deadline_ns.saturating_sub(now_ns);
            if remaining_ns == 0 && !*first_poll {
                return Ok(None);
            }
            *first_poll = false;

            let poll_timeout_ns = remaining_ns.min(i64::MAX as u64) as i64;
            let mut poll_handle =
                PollHandle::new(self.socket.as_raw() as u32, POLLIN | POLLERR | POLLHUP);
            let ready = poll(core::slice::from_mut(&mut poll_handle), poll_timeout_ns)
                .map_err(|_| Error::IoError)?;
            if ready == 0 {
                return Ok(None);
            }

            self.read_into_receive_buffer()?;
            if let Some(message) = Self::take_buffered_message(&mut self.receive_buffer)? {
                return Ok(Some(message));
            }
        }
    }

    /// Wait for a method response while retaining asynchronous messages.
    fn wait_for_method_response(&mut self) -> Result<Message, Error> {
        loop {
            let message = self.wait_for_response()?;
            if let Some(response) =
                Self::defer_unless_method_response(&mut self.pending_messages, message)
            {
                return Ok(response);
            }
        }
    }

    /// Wait for a method response without letting intervening messages renew
    /// the timeout deadline.
    fn wait_for_method_response_timeout(
        &mut self,
        timeout_ms: u64,
    ) -> Result<Option<Message>, Error> {
        let timeout_ns = timeout_ms.saturating_mul(1_000_000);
        let deadline_ns = monotonic_time_ns().saturating_add(timeout_ns);
        let mut first_poll = true;

        loop {
            let Some(message) = self.wait_for_response_until(deadline_ns, &mut first_poll)? else {
                return Ok(None);
            };
            if let Some(response) =
                Self::defer_unless_method_response(&mut self.pending_messages, message)
            {
                return Ok(Some(response));
            }
        }
    }

    fn defer_unless_method_response(
        pending_messages: &mut VecDeque<Message>,
        message: Message,
    ) -> Option<Message> {
        if matches!(
            message,
            Message::MethodReturn { .. } | Message::MethodError { .. }
        ) {
            Some(message)
        } else {
            pending_messages.push_back(message);
            None
        }
    }

    /// Append one socket read to the persistent receive buffer.
    fn read_into_receive_buffer(&mut self) -> Result<(), Error> {
        let mut buffer = [0u8; 4096];
        match Self::read(&mut self.socket, &mut buffer) {
            Ok(0) | Err(()) => Err(Error::IoError),
            Ok(n) => {
                self.receive_buffer.extend_from_slice(&buffer[..n]);
                Ok(())
            }
        }
    }

    /// Remove and decode exactly one complete frame from `buffer`.
    fn take_buffered_message(buffer: &mut Vec<u8>) -> Result<Option<Message>, Error> {
        if buffer.len() < MessageHeader::SIZE {
            return Ok(None);
        }

        let mut header_bytes = [0u8; MessageHeader::SIZE];
        header_bytes.copy_from_slice(&buffer[..MessageHeader::SIZE]);
        let header = MessageHeader::from_le_bytes(header_bytes);
        let total_len = MessageHeader::SIZE
            .checked_add(header.payload_length as usize)
            .ok_or(Error::ProtocolError("Message length overflow"))?;
        if total_len > MAX_FRAME_SIZE {
            return Err(Error::ProtocolError("Message exceeds maximum frame size"));
        }
        if buffer.len() < total_len {
            return Ok(None);
        }

        let remaining = buffer.split_off(total_len);
        let frame = core::mem::replace(buffer, remaining);
        let message =
            sbus::from_bytes(frame).map_err(|_| Error::ProtocolError("Failed to parse message"))?;
        Ok(Some(message))
    }

    /// Serialize and send one method call.
    fn send_method_call(
        &mut self,
        destination: &str,
        path: &str,
        interface: &str,
        method: &str,
        args: Vec<Argument>,
    ) -> Result<(), Error> {
        self.ensure_usable()?;
        let _serial = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1);

        let msg = Message::CallMethod {
            destination: destination.to_string(),
            path: path.to_string(),
            interface: interface.to_string(),
            method: method.to_string(),
            args,
        };

        let bytes = msg
            .to_bytes()
            .map_err(|_| Error::ProtocolError("Failed to serialize CALL_METHOD"))?;
        self.send_bytes(&bytes)
    }

    /// Send one complete frame. Any write failure may have emitted a partial
    /// frame, so the connection must not be used again afterward.
    fn send_bytes(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.ensure_usable()?;
        if Self::write_all(&mut self.socket, bytes).is_err() {
            self.poisoned = true;
            return Err(Error::IoError);
        }
        Ok(())
    }

    /// Convert a response message into a method result.
    fn parse_method_response(response: Message) -> Result<Vec<Argument>, Error> {
        match response {
            Message::MethodReturn { serial: _, result } => Ok(result),
            Message::MethodError {
                serial: _,
                error_name,
                message,
            } => {
                if error_name == "org.scarlet.sbus.ServiceNotFound" {
                    Err(Error::ServiceNotFound)
                } else {
                    Err(Error::MethodFailed(message))
                }
            }
            _ => Err(Error::ProtocolError("Unexpected response type")),
        }
    }

    /// Write all bytes to socket
    fn write_all(socket: &mut Socket, bytes: &[u8]) -> Result<(), Error> {
        let mut written = 0;
        while written < bytes.len() {
            match Self::write(socket, &bytes[written..]) {
                Ok(0) => return Err(Error::IoError),
                Ok(n) => written += n,
                Err(_) => return Err(Error::IoError),
            }
        }
        Ok(())
    }

    #[cfg(feature = "std")]
    fn read(socket: &mut Socket, buffer: &mut [u8]) -> Result<usize, ()> {
        socket
            .as_stream()
            .and_then(|stream| {
                stream
                    .read(buffer)
                    .map_err(|_| scarlet_os::socket::SocketError::SyscallFailed)
            })
            .map_err(|_| ())
    }

    #[cfg(not(feature = "std"))]
    fn read(socket: &mut Socket, buffer: &mut [u8]) -> Result<usize, ()> {
        socket.read(buffer).map_err(|_| ())
    }

    #[cfg(feature = "std")]
    fn write(socket: &mut Socket, buffer: &[u8]) -> Result<usize, ()> {
        socket
            .as_stream()
            .and_then(|stream| {
                stream
                    .write(buffer)
                    .map_err(|_| scarlet_os::socket::SocketError::SyscallFailed)
            })
            .map_err(|_| ())
    }

    #[cfg(not(feature = "std"))]
    fn write(socket: &mut Socket, buffer: &[u8]) -> Result<usize, ()> {
        socket.write(buffer).map_err(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::{Connection, Error, MAX_FRAME_SIZE, Message, MessageHeader};
    use alloc::collections::VecDeque;
    use alloc::string::String;
    use alloc::vec::Vec;

    fn method_return_frame(serial: u32) -> Vec<u8> {
        Message::MethodReturn {
            serial,
            result: Vec::new(),
        }
        .to_bytes()
        .expect("method return should serialize")
    }

    fn assert_method_return_serial(message: Message, expected_serial: u32) {
        match message {
            Message::MethodReturn { serial, result } => {
                assert_eq!(serial, expected_serial);
                assert!(result.is_empty());
            }
            _ => panic!("expected method return"),
        }
    }

    #[test]
    fn retains_second_coalesced_frame() {
        let first = method_return_frame(11);
        let second = method_return_frame(22);
        let mut buffer = first.clone();
        buffer.extend_from_slice(&second);

        let message = Connection::take_buffered_message(&mut buffer)
            .expect("first frame should parse")
            .expect("first frame should be complete");
        assert_method_return_serial(message, 11);
        assert_eq!(buffer, second);

        let message = Connection::take_buffered_message(&mut buffer)
            .expect("second frame should parse")
            .expect("second frame should be complete");
        assert_method_return_serial(message, 22);
        assert!(buffer.is_empty());
    }

    #[test]
    fn retains_partial_header_and_payload_until_frame_is_complete() {
        let frame = method_return_frame(33);
        let header_split = 7;
        let payload_split = frame.len() - 1;
        let mut buffer = frame[..header_split].to_vec();

        assert!(
            Connection::take_buffered_message(&mut buffer)
                .expect("partial header should not be an error")
                .is_none()
        );
        assert_eq!(buffer, frame[..header_split]);

        buffer.extend_from_slice(&frame[header_split..payload_split]);
        assert!(
            Connection::take_buffered_message(&mut buffer)
                .expect("partial payload should not be an error")
                .is_none()
        );
        assert_eq!(buffer, frame[..payload_split]);

        buffer.extend_from_slice(&frame[payload_split..]);
        let message = Connection::take_buffered_message(&mut buffer)
            .expect("complete frame should parse")
            .expect("frame should now be complete");
        assert_method_return_serial(message, 33);
        assert!(buffer.is_empty());
    }

    #[test]
    fn rejects_oversized_frame_from_header_alone() {
        let header = MessageHeader {
            msg_type: sbus::msg::METHOD_RETURN,
            serial: 0,
            payload_length: (MAX_FRAME_SIZE - MessageHeader::SIZE + 1) as u32,
            flags: 0,
        };
        let mut buffer = header.to_le_bytes().to_vec();

        assert!(matches!(
            Connection::take_buffered_message(&mut buffer),
            Err(Error::ProtocolError("Message exceeds maximum frame size"))
        ));
    }

    #[test]
    fn defers_signal_until_receive_message() {
        let mut pending_messages = VecDeque::new();
        let signal = Message::Signal {
            sender: String::from("org.scarlet.test"),
            path: String::from("/org/scarlet/test"),
            interface: String::from("org.scarlet.test"),
            signal: String::from("Changed"),
            args: Vec::new(),
        };

        assert!(Connection::defer_unless_method_response(&mut pending_messages, signal).is_none());
        let delivered = pending_messages
            .pop_front()
            .expect("queued signal should be delivered");
        assert!(matches!(delivered, Message::Signal { signal, .. } if signal == "Changed"));
    }

    #[test]
    fn poisoned_connection_rejects_future_operations() {
        assert!(matches!(
            Connection::ensure_not_poisoned(true),
            Err(Error::ConnectionPoisoned)
        ));
    }
}
