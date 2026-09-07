//! SBus transport and service mirror for the authoritative input environment.

use super::input_environment::{self, Snapshot};
use core::time::Duration;
use sbus_client::{Argument, Connection, Message};
use std::println;
use std::string::String;
use std::sync::Mutex;
use std::thread;
use std::vec::Vec;

/// SBus name already owned by the Scarlet window server.
pub const SBUS_SERVICE: &str = "org.scarlet-os.sws";
/// Object path exposing the input environment.
pub const SBUS_PATH: &str = "/org/scarlet/InputEnvironment";
/// Interface exposing the input environment.
pub const SBUS_INTERFACE: &str = "org.scarlet.InputEnvironment";
/// Method returning the current full snapshot.
pub const SBUS_GET_STATE: &str = "GetState";
/// Signal emitted after an effective snapshot change.
pub const SBUS_STATE_CHANGED: &str = "StateChanged";

const SBUS_REGISTRATION_TIMEOUT_MS: u64 = 1_000;
const SBUS_RECEIVE_TIMEOUT_MS: u64 = 100;
const SBUS_RECONNECT_DELAY_MS: u64 = 250;

static PENDING_SBUS_SIGNAL: Mutex<Option<Snapshot>> = Mutex::new(None);

fn sbus_arguments(snapshot: Snapshot) -> Vec<Argument> {
    vec![
        Argument::UInt(snapshot.generation),
        Argument::UInt(snapshot.known_flags),
        Argument::UInt(snapshot.state_flags),
        Argument::UInt(snapshot.capability_flags),
    ]
}

/// Queue the latest SBus mirror signal after an authoritative change.
///
/// # Arguments
///
/// * `snapshot` - New full snapshot to publish.
pub fn queue_state_changed(snapshot: Snapshot) {
    *PENDING_SBUS_SIGNAL
        .lock()
        .expect("SWS input-environment signal mutex poisoned") = Some(snapshot);
}

struct MethodCall {
    path: String,
    interface: String,
    method: String,
    args: Vec<Argument>,
}

trait Transport {
    fn emit_state_changed(&mut self, args: Vec<Argument>) -> Result<(), ()>;
    fn receive_method(&mut self, timeout_ms: u64) -> Result<Option<MethodCall>, ()>;
    fn send_method_return(&mut self, args: Vec<Argument>) -> Result<(), ()>;
    fn send_method_error(&mut self, name: &str, message: &str) -> Result<(), ()>;
}

struct ConnectionTransport<'a> {
    connection: &'a mut Connection,
}

impl Transport for ConnectionTransport<'_> {
    fn emit_state_changed(&mut self, args: Vec<Argument>) -> Result<(), ()> {
        self.connection
            .emit_signal(
                SBUS_SERVICE,
                SBUS_PATH,
                SBUS_INTERFACE,
                SBUS_STATE_CHANGED,
                args,
            )
            .map_err(|_| ())
    }

    fn receive_method(&mut self, timeout_ms: u64) -> Result<Option<MethodCall>, ()> {
        let message = self
            .connection
            .receive_message_timeout(timeout_ms)
            .map_err(|_| ())?;
        let Some(Message::CallMethod {
            path,
            interface,
            method,
            args,
            ..
        }) = message
        else {
            return Ok(None);
        };
        Ok(Some(MethodCall {
            path,
            interface,
            method,
            args,
        }))
    }

    fn send_method_return(&mut self, args: Vec<Argument>) -> Result<(), ()> {
        self.connection.send_method_return(0, args).map_err(|_| ())
    }

    fn send_method_error(&mut self, name: &str, message: &str) -> Result<(), ()> {
        self.connection
            .send_method_error(0, name, message)
            .map_err(|_| ())
    }
}

fn serve_once(transport: &mut impl Transport) -> Result<(), ()> {
    let pending_signal = PENDING_SBUS_SIGNAL
        .lock()
        .expect("SWS input-environment signal mutex poisoned")
        .take();
    if let Some(signal_snapshot) = pending_signal
        && transport
            .emit_state_changed(sbus_arguments(signal_snapshot))
            .is_err()
    {
        let mut pending = PENDING_SBUS_SIGNAL
            .lock()
            .expect("SWS input-environment signal mutex poisoned");
        if pending.is_none() {
            *pending = Some(signal_snapshot);
        }
        return Err(());
    }

    let Some(call) = transport.receive_method(SBUS_RECEIVE_TIMEOUT_MS)? else {
        return Ok(());
    };
    if call.path != SBUS_PATH || call.interface != SBUS_INTERFACE || call.method != SBUS_GET_STATE {
        transport.send_method_error(
            "org.scarlet.InputEnvironment.UnknownMethod",
            "unknown input-environment method",
        )
    } else if call.args.is_empty() {
        transport.send_method_return(sbus_arguments(input_environment::snapshot()))
    } else {
        transport.send_method_error(
            "org.scarlet.InputEnvironment.InvalidArgs",
            "GetState takes no arguments",
        )
    }
}

fn connect_and_register() -> Option<Connection> {
    let mut connection = Connection::connect().ok()?;
    connection
        .register_service_timeout(SBUS_SERVICE, SBUS_REGISTRATION_TIMEOUT_MS)
        .ok()?;
    println!("Successfully registered with sbus as {}", SBUS_SERVICE);
    Some(connection)
}

fn serve_connection(connection: &mut Connection) -> Result<(), ()> {
    serve_once(&mut ConnectionTransport { connection })
}

/// Start the background input-environment SBus service and reconnect loop.
///
/// # Arguments
///
/// * `initial_connection` - Already registered SWS connection, when startup
///   registration succeeded.
pub fn start_service(initial_connection: Option<Connection>) {
    let _ = thread::Builder::new()
        .name("sws-input-environment".into())
        .spawn(move || {
            let mut connection = initial_connection;
            loop {
                if connection.is_none() {
                    connection = connect_and_register();
                    if connection.is_none() {
                        thread::sleep(Duration::from_millis(SBUS_RECONNECT_DELAY_MS));
                        continue;
                    }
                }
                if serve_connection(connection.as_mut().expect("connection just checked")).is_err()
                {
                    connection = None;
                    thread::sleep(Duration::from_millis(SBUS_RECONNECT_DELAY_MS));
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeTransport {
        incoming: Option<MethodCall>,
        emitted: Vec<Vec<Argument>>,
        returned: Vec<Vec<Argument>>,
        errors: Vec<String>,
    }

    impl Transport for FakeTransport {
        fn emit_state_changed(&mut self, args: Vec<Argument>) -> Result<(), ()> {
            self.emitted.push(args);
            Ok(())
        }

        fn receive_method(&mut self, _timeout_ms: u64) -> Result<Option<MethodCall>, ()> {
            Ok(self.incoming.take())
        }

        fn send_method_return(&mut self, args: Vec<Argument>) -> Result<(), ()> {
            self.returned.push(args);
            Ok(())
        }

        fn send_method_error(&mut self, name: &str, _message: &str) -> Result<(), ()> {
            self.errors.push(name.into());
            Ok(())
        }
    }

    fn uint_values(args: &[Argument]) -> Vec<u32> {
        args.iter()
            .map(|argument| match argument {
                Argument::UInt(value) => *value,
                _ => panic!("input-environment SBus argument was not UInt"),
            })
            .collect()
    }

    #[test]
    fn get_state_and_queued_signal_send_the_full_snapshot() {
        let _test_guard = input_environment::TEST_STATE_LOCK
            .lock()
            .expect("input-environment test mutex poisoned");
        let expected = input_environment::snapshot();
        queue_state_changed(expected);
        let mut transport = FakeTransport {
            incoming: Some(MethodCall {
                path: SBUS_PATH.into(),
                interface: SBUS_INTERFACE.into(),
                method: SBUS_GET_STATE.into(),
                args: Vec::new(),
            }),
            ..FakeTransport::default()
        };

        serve_once(&mut transport).expect("fake SBus service iteration should succeed");

        let expected_values = vec![
            expected.generation,
            expected.known_flags,
            expected.state_flags,
            expected.capability_flags,
        ];
        assert_eq!(transport.emitted.len(), 1);
        assert_eq!(uint_values(&transport.emitted[0]), expected_values);
        assert_eq!(transport.returned.len(), 1);
        assert_eq!(uint_values(&transport.returned[0]), expected_values);
        assert!(transport.errors.is_empty());
    }
}
