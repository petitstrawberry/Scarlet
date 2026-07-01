//! Generic remote processor abstractions for kernel device drivers.
//!
//! The remoteproc subsystem exposes provider-neutral lifecycle, firmware memory,
//! crash handling, and service messaging traits for firmware-described
//! coprocessors such as Apple RTKit-managed devices.

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;

/// Remote processor operation errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteprocError {
    /// Operation is invalid for the current lifecycle state.
    InvalidState,
    /// Firmware information is missing.
    FirmwareMissing,
    /// Firmware loading failed.
    LoadFailed,
    /// Processor boot failed.
    BootFailed,
    /// Processor shutdown failed.
    ShutdownFailed,
    /// Processor or service has crashed.
    Crashed,
    /// Requested service was not found.
    ServiceNotFound,
    /// Service transport failed.
    TransportError,
    /// Operation is not supported by this processor or service.
    NotSupported,
    /// Processor or service is busy and cannot satisfy the operation.
    Busy,
}

/// Remote processor lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteprocState {
    /// Processor is powered off or otherwise unavailable.
    Offline,
    /// Firmware is being loaded or has been staged for boot.
    Loading,
    /// Processor is booted and services may be available.
    Running,
    /// Processor is suspended and may be resumed.
    Suspended,
    /// Processor has crashed and needs recovery.
    Crashed,
}

/// Physical memory region used by remote processor firmware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteprocMemoryRegion {
    /// Physical base address of the firmware memory region.
    pub paddr: usize,
    /// Size of the firmware memory region in bytes.
    pub size: usize,
}

/// Firmware description for a remote processor.
#[derive(Debug, Clone)]
pub struct RemoteprocFirmware {
    /// Firmware memory regions discovered from firmware tables.
    pub regions: Vec<RemoteprocMemoryRegion>,
}

/// Identifier for a service exposed by a remote processor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RemoteprocServiceId(pub u32);

/// Fixed-size remote processor service message.
///
/// Apple RTKit endpoints are services. Other coprocessors may expose services
/// differently. The fixed-size message matches `MailboxMessage` for parity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteprocMessage {
    /// Message words. Only the first [`Self::len`] entries are valid payload.
    pub words: [u64; 4],
    /// Number of valid words in [`Self::words`].
    pub len: u8,
}

impl RemoteprocMessage {
    /// Build a single-word remoteproc message.
    ///
    /// # Arguments
    ///
    /// * `word` - Word to place in the first message slot.
    ///
    /// # Returns
    ///
    /// A remoteproc message with one valid word.
    pub const fn one(word: u64) -> Self {
        Self {
            words: [word, 0, 0, 0],
            len: 1,
        }
    }
}

/// Callback interface for processor-level crash notifications.
pub trait RemoteprocCrashHandler: Send + Sync {
    /// Notify the handler that a remote processor crashed.
    ///
    /// # Arguments
    ///
    /// * `remoteproc` - Service identifier associated with the crash source.
    /// * `reason` - Provider-specific crash reason code.
    fn crashed(&self, remoteproc: RemoteprocServiceId, reason: u32);
}

/// Callback interface for remote processor service events.
pub trait RemoteprocServiceClient: Send + Sync {
    /// Notify the client that a service has data ready to receive.
    ///
    /// # Arguments
    ///
    /// * `service` - Service with a pending inbound message.
    fn message_received(&self, service: RemoteprocServiceId);

    /// Notify the client that a service has crashed.
    ///
    /// # Arguments
    ///
    /// * `service` - Service that reported the crash.
    fn service_crashed(&self, service: RemoteprocServiceId);
}

/// One service exposed by a remote processor.
pub trait RemoteprocService: Send + Sync {
    /// Return the service identifier.
    ///
    /// # Returns
    ///
    /// Provider-local service identifier.
    fn id(&self) -> RemoteprocServiceId;

    /// Return the service name.
    ///
    /// # Returns
    ///
    /// Static name used for logging and diagnostics.
    fn name(&self) -> &'static str;

    /// Send a message to this service.
    ///
    /// # Arguments
    ///
    /// * `message` - Message to submit to the service transport.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the message was accepted.
    fn send(&self, message: &RemoteprocMessage) -> Result<(), RemoteprocError>;

    /// Try to receive one pending message without blocking.
    ///
    /// # Returns
    ///
    /// `Ok(Some(message))` when a message was available, `Ok(None)` when empty.
    fn try_recv(&self) -> Result<Option<RemoteprocMessage>, RemoteprocError>;

    /// Set or clear the service callback sink.
    ///
    /// # Arguments
    ///
    /// * `client` - Callback sink to install, or `None` to clear callbacks.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the client was updated.
    fn set_client(
        &self,
        client: Option<Arc<dyn RemoteprocServiceClient>>,
    ) -> Result<(), RemoteprocError>;
}

/// Remote processor controller registered by firmware phandle.
pub trait RemoteProcessor: Send + Sync {
    /// Return the processor name.
    ///
    /// # Returns
    ///
    /// Static processor name used for diagnostics.
    fn name(&self) -> &'static str;

    /// Return the current lifecycle state.
    ///
    /// # Returns
    ///
    /// Current remote processor state.
    fn state(&self) -> RemoteprocState;

    /// Load firmware memory regions for the processor.
    ///
    /// # Arguments
    ///
    /// * `firmware` - Firmware memory regions to stage for the processor.
    ///
    /// # Returns
    ///
    /// `Ok(())` when firmware was loaded or staged.
    fn load(&self, firmware: &RemoteprocFirmware) -> Result<(), RemoteprocError>;

    /// Boot the processor after firmware loading.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the processor is running.
    fn boot(&self) -> Result<(), RemoteprocError>;

    /// Shut down the processor.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the processor is offline.
    fn shutdown(&self) -> Result<(), RemoteprocError>;

    /// Suspend the processor.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the processor is suspended.
    fn suspend(&self) -> Result<(), RemoteprocError>;

    /// Resume a suspended processor.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the processor is running again.
    fn resume(&self) -> Result<(), RemoteprocError>;

    /// Register a processor crash handler.
    ///
    /// # Arguments
    ///
    /// * `handler` - Crash callback sink to install for this processor.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the handler was registered.
    fn register_crash_handler(
        &self,
        handler: Arc<dyn RemoteprocCrashHandler>,
    ) -> Result<(), RemoteprocError>;

    /// Look up a service exposed by this processor.
    ///
    /// # Arguments
    ///
    /// * `id` - Service identifier to resolve.
    ///
    /// # Returns
    ///
    /// Service registered for `id`, or `None` when missing.
    fn get_service(&self, id: RemoteprocServiceId) -> Option<Arc<dyn RemoteprocService>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::VecDeque;
    use alloc::collections::btree_map::BTreeMap;
    use core::sync::atomic::{AtomicU32, Ordering};
    use spin::Mutex;

    struct FakeCrashHandler {
        service: AtomicU32,
        reason: AtomicU32,
    }

    impl FakeCrashHandler {
        fn new() -> Self {
            Self {
                service: AtomicU32::new(0),
                reason: AtomicU32::new(0),
            }
        }
    }

    impl RemoteprocCrashHandler for FakeCrashHandler {
        fn crashed(&self, remoteproc: RemoteprocServiceId, reason: u32) {
            self.service.store(remoteproc.0, Ordering::SeqCst);
            self.reason.store(reason, Ordering::SeqCst);
        }
    }

    struct FakeService {
        id: RemoteprocServiceId,
        queue: Mutex<VecDeque<RemoteprocMessage>>,
        client: Mutex<Option<Arc<dyn RemoteprocServiceClient>>>,
    }

    impl FakeService {
        fn new(id: RemoteprocServiceId) -> Self {
            Self {
                id,
                queue: Mutex::new(VecDeque::new()),
                client: Mutex::new(None),
            }
        }
    }

    impl RemoteprocService for FakeService {
        fn id(&self) -> RemoteprocServiceId {
            self.id
        }

        fn name(&self) -> &'static str {
            "fake-service"
        }

        fn send(&self, message: &RemoteprocMessage) -> Result<(), RemoteprocError> {
            self.queue.lock().push_back(*message);
            if let Some(client) = self.client.lock().as_ref() {
                client.message_received(self.id);
            }
            Ok(())
        }

        fn try_recv(&self) -> Result<Option<RemoteprocMessage>, RemoteprocError> {
            Ok(self.queue.lock().pop_front())
        }

        fn set_client(
            &self,
            client: Option<Arc<dyn RemoteprocServiceClient>>,
        ) -> Result<(), RemoteprocError> {
            *self.client.lock() = client;
            Ok(())
        }
    }

    struct FakeProcessor {
        state: Mutex<RemoteprocState>,
        services: Mutex<BTreeMap<RemoteprocServiceId, Arc<dyn RemoteprocService>>>,
        crash_handler: Mutex<Option<Arc<dyn RemoteprocCrashHandler>>>,
    }

    impl FakeProcessor {
        fn new() -> Self {
            Self {
                state: Mutex::new(RemoteprocState::Offline),
                services: Mutex::new(BTreeMap::new()),
                crash_handler: Mutex::new(None),
            }
        }

        fn register_service(&self, service: Arc<dyn RemoteprocService>) {
            self.services.lock().insert(service.id(), service);
        }

        fn trigger_crash(&self, service: RemoteprocServiceId, reason: u32) {
            *self.state.lock() = RemoteprocState::Crashed;
            if let Some(handler) = self.crash_handler.lock().as_ref() {
                handler.crashed(service, reason);
            }
        }
    }

    impl RemoteProcessor for FakeProcessor {
        fn name(&self) -> &'static str {
            "fake-remoteproc"
        }

        fn state(&self) -> RemoteprocState {
            *self.state.lock()
        }

        fn load(&self, firmware: &RemoteprocFirmware) -> Result<(), RemoteprocError> {
            if firmware.regions.is_empty() {
                return Err(RemoteprocError::FirmwareMissing);
            }
            let mut state = self.state.lock();
            if *state != RemoteprocState::Offline {
                return Err(RemoteprocError::InvalidState);
            }
            *state = RemoteprocState::Loading;
            Ok(())
        }

        fn boot(&self) -> Result<(), RemoteprocError> {
            let mut state = self.state.lock();
            if *state != RemoteprocState::Loading {
                return Err(RemoteprocError::InvalidState);
            }
            *state = RemoteprocState::Running;
            Ok(())
        }

        fn shutdown(&self) -> Result<(), RemoteprocError> {
            let mut state = self.state.lock();
            if *state == RemoteprocState::Offline {
                return Err(RemoteprocError::InvalidState);
            }
            *state = RemoteprocState::Offline;
            Ok(())
        }

        fn suspend(&self) -> Result<(), RemoteprocError> {
            let mut state = self.state.lock();
            if *state != RemoteprocState::Running {
                return Err(RemoteprocError::InvalidState);
            }
            *state = RemoteprocState::Suspended;
            Ok(())
        }

        fn resume(&self) -> Result<(), RemoteprocError> {
            let mut state = self.state.lock();
            if *state != RemoteprocState::Suspended {
                return Err(RemoteprocError::InvalidState);
            }
            *state = RemoteprocState::Running;
            Ok(())
        }

        fn register_crash_handler(
            &self,
            handler: Arc<dyn RemoteprocCrashHandler>,
        ) -> Result<(), RemoteprocError> {
            *self.crash_handler.lock() = Some(handler);
            Ok(())
        }

        fn get_service(&self, id: RemoteprocServiceId) -> Option<Arc<dyn RemoteprocService>> {
            self.services.lock().get(&id).cloned()
        }
    }

    #[test_case]
    fn test_remoteproc_message_one() {
        let message = RemoteprocMessage::one(0x1234);
        assert_eq!(message.words, [0x1234, 0, 0, 0]);
        assert_eq!(message.len, 1);
    }

    #[test_case]
    fn test_remoteproc_state_equality() {
        assert_eq!(RemoteprocState::Offline, RemoteprocState::Offline);
        assert_ne!(RemoteprocState::Offline, RemoteprocState::Running);
    }

    #[test_case]
    fn test_remoteproc_service_id_ordering() {
        assert!(RemoteprocServiceId(1) < RemoteprocServiceId(2));
    }

    #[test_case]
    fn test_remote_processor_lifecycle() {
        let processor = FakeProcessor::new();
        let firmware = RemoteprocFirmware {
            regions: Vec::from([RemoteprocMemoryRegion {
                paddr: 0x1000,
                size: 0x100,
            }]),
        };

        assert_eq!(processor.state(), RemoteprocState::Offline);
        assert_eq!(processor.load(&firmware), Ok(()));
        assert_eq!(processor.state(), RemoteprocState::Loading);
        assert_eq!(processor.boot(), Ok(()));
        assert_eq!(processor.state(), RemoteprocState::Running);
        assert_eq!(processor.suspend(), Ok(()));
        assert_eq!(processor.state(), RemoteprocState::Suspended);
        assert_eq!(processor.resume(), Ok(()));
        assert_eq!(processor.state(), RemoteprocState::Running);
        assert_eq!(processor.shutdown(), Ok(()));
        assert_eq!(processor.state(), RemoteprocState::Offline);
    }

    #[test_case]
    fn test_remote_processor_get_service_returns_registered_service() {
        let processor = FakeProcessor::new();
        processor.register_service(Arc::new(FakeService::new(RemoteprocServiceId(7))));

        let service = processor
            .get_service(RemoteprocServiceId(7))
            .expect("registered service missing");
        assert_eq!(service.id(), RemoteprocServiceId(7));
        assert_eq!(service.name(), "fake-service");
    }

    #[test_case]
    fn test_remote_processor_crash_handler_invoked() {
        let processor = FakeProcessor::new();
        let handler = Arc::new(FakeCrashHandler::new());
        processor.register_crash_handler(handler.clone()).unwrap();

        processor.trigger_crash(RemoteprocServiceId(9), 0x55aa);

        assert_eq!(processor.state(), RemoteprocState::Crashed);
        assert_eq!(handler.service.load(Ordering::SeqCst), 9);
        assert_eq!(handler.reason.load(Ordering::SeqCst), 0x55aa);
    }
}
