//! sbus client library
//!
//! Provides a high-level client interface for connecting to sbusd and
//! sending/receiving messages.

#![no_std]

extern crate alloc;

extern crate scarlet_std as std;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use sbus::DEFAULT_SOCKET_PATH;
use std::io::{Read, Write};
use std::socket::Socket;

// Re-export sbus types for convenience
pub use sbus::{Argument, Message, MessageHeader};

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
}

/// Connection to sbusd
pub struct Connection {
    socket: Socket,
    next_serial: u32,
}

impl Connection {
    /// Connect to sbusd
    pub fn connect() -> Result<Self, Error> {
        Self::connect_to_path(DEFAULT_SOCKET_PATH)
    }

    /// Connect to sbusd at a specific path
    pub fn connect_to_path(path: &str) -> Result<Self, Error> {
        std::println!("sbus-client: Creating socket...");
        let mut socket = Socket::new().map_err(|_| Error::ConnectionFailed)?;

        std::println!("sbus-client: Connecting to {}...", path);
        if let Err(_) = socket.connect(path) {
            std::println!("sbus-client: Connect failed");
            return Err(Error::ConnectionFailed);
        }
        std::println!("sbus-client: Connected successfully");

        // Send HELLO message
        let hello = Message::Hello {
            client_name: "sbus-client".to_string(),
        };

        std::println!("sbus-client: Sending HELLO...");
        let bytes = hello
            .to_bytes()
            .map_err(|_| Error::ProtocolError("Failed to serialize HELLO"))?;
        Self::write_all(&mut socket, &bytes).map_err(|_| Error::IoError)?;
        std::println!("sbus-client: HELLO sent ({} bytes)", bytes.len());

        Ok(Connection {
            socket,
            next_serial: 1,
        })
    }

    /// Register a service
    pub fn register_service(&mut self, bus_name: &str) -> Result<(), Error> {
        std::println!("sbus-client: Registering service: {}", bus_name);
        let msg = Message::RegisterService {
            bus_name: bus_name.to_string(),
        };

        let bytes = msg
            .to_bytes()
            .map_err(|_| Error::ProtocolError("Failed to serialize REGISTER_SERVICE"))?;
        std::println!(
            "sbus-client: Sending RegisterService ({} bytes)...",
            bytes.len()
        );
        Self::write_all(&mut self.socket, &bytes).map_err(|_| Error::IoError)?;
        std::println!("sbus-client: RegisterService sent, waiting for response...");

        // Wait for acknowledgment
        self.wait_for_response().map_err(|_| Error::IoError)?;
        std::println!("sbus-client: Got response for RegisterService");

        Ok(())
    }

    /// Unregister a service
    pub fn unregister_service(&mut self, bus_name: &str) -> Result<(), Error> {
        let msg = Message::UnregisterService {
            bus_name: bus_name.to_string(),
        };

        let bytes = msg
            .to_bytes()
            .map_err(|_| Error::ProtocolError("Failed to serialize UNREGISTER_SERVICE"))?;
        Self::write_all(&mut self.socket, &bytes).map_err(|_| Error::IoError)?;

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
        let serial = self.next_serial;
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
        Self::write_all(&mut self.socket, &bytes).map_err(|_| Error::IoError)?;

        // Wait for response
        let response = self.wait_for_response().map_err(|_| Error::IoError)?;

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

    /// Emit a signal
    pub fn emit_signal(
        &mut self,
        sender: &str,
        path: &str,
        interface: &str,
        signal: &str,
        args: Vec<Argument>,
    ) -> Result<(), Error> {
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
        Self::write_all(&mut self.socket, &bytes).map_err(|_| Error::IoError)?;

        Ok(())
    }

    /// Receive any incoming message
    pub fn receive_message(&mut self) -> Result<Message, Error> {
        self.wait_for_response()
    }

    /// Send a method return response
    pub fn send_method_return(&mut self, serial: u32, result: Vec<Argument>) -> Result<(), Error> {
        let msg = Message::MethodReturn { serial, result };
        let bytes = msg
            .to_bytes()
            .map_err(|_| Error::ProtocolError("Failed to serialize METHOD_RETURN"))?;
        Self::write_all(&mut self.socket, &bytes).map_err(|_| Error::IoError)?;
        Ok(())
    }

    /// Send a method error response
    pub fn send_method_error(
        &mut self,
        serial: u32,
        error_name: &str,
        message: &str,
    ) -> Result<(), Error> {
        let msg = Message::MethodError {
            serial,
            error_name: error_name.to_string(),
            message: message.to_string(),
        };
        let bytes = msg
            .to_bytes()
            .map_err(|_| Error::ProtocolError("Failed to serialize METHOD_ERROR"))?;
        Self::write_all(&mut self.socket, &bytes).map_err(|_| Error::IoError)?;
        Ok(())
    }

    /// Wait for a response message
    fn wait_for_response(&mut self) -> Result<Message, Error> {
        let mut buffer = [0u8; 4096];
        let mut read_buffer = Vec::new();

        std::println!("sbus-client: wait_for_response: reading header...");

        // Read header
        while read_buffer.len() < 16 {
            match self.socket.read(&mut buffer) {
                Ok(0) => {
                    std::println!("sbus-client: wait_for_response: read returned 0 (EOF)");
                    return Err(Error::IoError);
                }
                Ok(n) => {
                    std::println!(
                        "sbus-client: wait_for_response: read {} bytes (total: {})",
                        n,
                        read_buffer.len() + n
                    );
                    read_buffer.extend_from_slice(&buffer[..n]);
                }
                Err(_e) => {
                    std::println!("sbus-client: wait_for_response: read error, retrying...");
                    continue;
                }
            }
        }

        let mut header_bytes = [0u8; 16];
        header_bytes.copy_from_slice(&read_buffer[0..16]);
        let header = MessageHeader::from_le_bytes(header_bytes);

        let total_len = 16 + header.payload_length as usize;

        // Read remaining payload
        while read_buffer.len() < total_len {
            match self.socket.read(&mut buffer) {
                Ok(0) => {
                    return Err(Error::IoError);
                }
                Ok(n) => {
                    read_buffer.extend_from_slice(&buffer[..n]);
                }
                Err(_e) => {
                    continue;
                }
            }
        }

        sbus::from_bytes(read_buffer).map_err(|_| Error::ProtocolError("Failed to parse message"))
    }

    /// Write all bytes to socket
    fn write_all(socket: &mut Socket, bytes: &[u8]) -> Result<(), Error> {
        let mut written = 0;
        while written < bytes.len() {
            match socket.write(&bytes[written..]) {
                Ok(0) => return Err(Error::IoError),
                Ok(n) => written += n,
                Err(_) => return Err(Error::IoError),
            }
        }
        Ok(())
    }
}
