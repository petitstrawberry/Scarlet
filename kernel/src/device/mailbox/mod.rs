//! Generic mailbox controller abstractions for kernel device drivers.
//!
//! The mailbox subsystem exposes provider-neutral controller and channel traits
//! for firmware-described message-passing hardware such as Apple ASC mailboxes.

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;

/// Mailbox operation errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxError {
    /// Referenced mailbox controller was not found.
    ControllerNotFound,
    /// Requested channel specifier is invalid for the controller.
    InvalidChannel,
    /// Channel is busy and cannot accept the operation.
    Busy,
    /// No message is available to receive.
    Empty,
    /// Operation timed out before completion.
    Timeout,
    /// Hardware access failed.
    HardwareError,
    /// Operation is not supported by this controller or channel.
    NotSupported,
}

/// Consumer-supplied mailbox specifier.
///
/// The specifier contains a firmware phandle for the controller plus the
/// controller-specific cells from the `mailboxes` property.
#[derive(Debug, Clone)]
pub struct MailboxSpec {
    /// Firmware phandle identifying the mailbox controller node.
    pub controller_phandle: u32,
    /// Provider-specific specifier cells after the controller phandle.
    pub cells: Vec<u32>,
}

/// Controller-local mailbox channel identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailboxChannelId(pub u32);

/// Fixed-size mailbox word message.
///
/// The layout matches Apple ASC's four-word message format. Controllers that
/// require byte-stream payloads should wrap this type or expose a separate API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailboxMessage {
    /// Message words. Only the first [`Self::len`] entries are valid payload.
    pub words: [u64; 4],
    /// Number of valid words in [`Self::words`].
    pub len: u8,
}

impl MailboxMessage {
    /// Build a single-word message.
    ///
    /// # Arguments
    ///
    /// * `word` - Word to place in the first message slot.
    ///
    /// # Returns
    ///
    /// A mailbox message with one valid word.
    pub const fn one(word: u64) -> Self {
        Self {
            words: [word, 0, 0, 0],
            len: 1,
        }
    }
}

/// Client callback interface for mailbox channel events.
pub trait MailboxClient: Send + Sync {
    /// Notify the client that a channel has data ready to receive.
    ///
    /// # Arguments
    ///
    /// * `channel` - Channel with a pending inbound message.
    fn rx_ready(&self, channel: MailboxChannelId);

    /// Notify the client that transmit work completed on a channel.
    ///
    /// # Arguments
    ///
    /// * `channel` - Channel whose transmit work completed.
    fn tx_done(&self, channel: MailboxChannelId);

    /// Notify the client that an asynchronous channel error occurred.
    ///
    /// # Arguments
    ///
    /// * `channel` - Channel that reported the error.
    /// * `error` - Error reported by the controller or channel.
    fn error(&self, channel: MailboxChannelId, error: MailboxError);
}

/// One mailbox channel allocated from a controller.
pub trait MailboxChannel: Send + Sync {
    /// Return the channel identifier.
    ///
    /// # Returns
    ///
    /// Controller-local channel identifier.
    fn id(&self) -> MailboxChannelId;

    /// Try to send a message without blocking.
    ///
    /// # Arguments
    ///
    /// * `message` - Message to submit to the controller.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the message was accepted.
    fn try_send(&self, message: &MailboxMessage) -> Result<(), MailboxError>;

    /// Try to receive one pending message without blocking.
    ///
    /// # Returns
    ///
    /// `Ok(Some(message))` when a message was available, `Ok(None)` when empty.
    fn try_recv(&self) -> Result<Option<MailboxMessage>, MailboxError>;

    /// Send a message, waiting up to a bounded timeout.
    ///
    /// # Arguments
    ///
    /// * `message` - Message to submit to the controller.
    /// * `timeout_us` - Maximum wait time in microseconds.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the message was accepted before the timeout.
    fn send_timeout(&self, message: &MailboxMessage, timeout_us: u64) -> Result<(), MailboxError>;

    /// Set or clear the client callback sink for this channel.
    ///
    /// # Arguments
    ///
    /// * `client` - Callback sink to install, or `None` to clear callbacks.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the client was updated.
    fn set_client(&self, client: Option<Arc<dyn MailboxClient>>) -> Result<(), MailboxError>;

    /// Poll the channel for pending completions or inbound messages.
    ///
    /// # Returns
    ///
    /// `Ok(())` when polling completed successfully.
    fn poll(&self) -> Result<(), MailboxError>;
}

/// Mailbox controller registered by firmware phandle.
pub trait MailboxController: Send + Sync {
    /// Return the controller name.
    ///
    /// # Returns
    ///
    /// Static controller name used for diagnostics.
    fn name(&self) -> &'static str;

    /// Request a mailbox channel from this controller.
    ///
    /// # Arguments
    ///
    /// * `spec` - Firmware mailbox specifier for this controller.
    /// * `client` - Optional callback sink to install before returning.
    ///
    /// # Returns
    ///
    /// A reference-counted mailbox channel on success.
    fn request_channel(
        &self,
        spec: &MailboxSpec,
        client: Option<Arc<dyn MailboxClient>>,
    ) -> Result<Arc<dyn MailboxChannel>, MailboxError>;

    /// Release a channel previously returned by [`MailboxController::request_channel`].
    ///
    /// # Arguments
    ///
    /// * `channel` - Controller-local channel identifier to release.
    fn release_channel(&self, channel: MailboxChannelId);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::IrqSpinLock;
    use alloc::collections::VecDeque;
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

    struct FakeClient {
        rx_count: AtomicUsize,
        tx_count: AtomicUsize,
        error_count: AtomicUsize,
    }

    impl FakeClient {
        fn new() -> Self {
            Self {
                rx_count: AtomicUsize::new(0),
                tx_count: AtomicUsize::new(0),
                error_count: AtomicUsize::new(0),
            }
        }
    }

    impl MailboxClient for FakeClient {
        fn rx_ready(&self, channel: MailboxChannelId) {
            let _ = channel;
            self.rx_count.fetch_add(1, Ordering::SeqCst);
        }

        fn tx_done(&self, channel: MailboxChannelId) {
            let _ = channel;
            self.tx_count.fetch_add(1, Ordering::SeqCst);
        }

        fn error(&self, channel: MailboxChannelId, error: MailboxError) {
            let _ = (channel, error);
            self.error_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct FakeChannel {
        id: MailboxChannelId,
        queue: IrqSpinLock<VecDeque<MailboxMessage>>,
        client: IrqSpinLock<Option<Arc<dyn MailboxClient>>>,
    }

    impl FakeChannel {
        fn new(id: MailboxChannelId) -> Self {
            Self {
                id,
                queue: IrqSpinLock::new(VecDeque::new()),
                client: IrqSpinLock::new(None),
            }
        }
    }

    impl MailboxChannel for FakeChannel {
        fn id(&self) -> MailboxChannelId {
            self.id
        }

        fn try_send(&self, message: &MailboxMessage) -> Result<(), MailboxError> {
            self.queue.lock().push_back(*message);
            if let Some(client) = self.client.lock().as_ref() {
                client.tx_done(self.id);
                client.rx_ready(self.id);
            }
            Ok(())
        }

        fn try_recv(&self) -> Result<Option<MailboxMessage>, MailboxError> {
            Ok(self.queue.lock().pop_front())
        }

        fn send_timeout(
            &self,
            message: &MailboxMessage,
            timeout_us: u64,
        ) -> Result<(), MailboxError> {
            let _ = timeout_us;
            self.try_send(message)
        }

        fn set_client(&self, client: Option<Arc<dyn MailboxClient>>) -> Result<(), MailboxError> {
            *self.client.lock() = client;
            Ok(())
        }

        fn poll(&self) -> Result<(), MailboxError> {
            Ok(())
        }
    }

    struct FakeController {
        requested: AtomicUsize,
        released: AtomicBool,
        last_released: AtomicU32,
    }

    impl FakeController {
        fn new() -> Self {
            Self {
                requested: AtomicUsize::new(0),
                released: AtomicBool::new(false),
                last_released: AtomicU32::new(0),
            }
        }
    }

    impl MailboxController for FakeController {
        fn name(&self) -> &'static str {
            "fake-mailbox"
        }

        fn request_channel(
            &self,
            spec: &MailboxSpec,
            client: Option<Arc<dyn MailboxClient>>,
        ) -> Result<Arc<dyn MailboxChannel>, MailboxError> {
            if spec.cells.is_empty() {
                return Err(MailboxError::InvalidChannel);
            }
            self.requested.fetch_add(1, Ordering::SeqCst);
            let channel = Arc::new(FakeChannel::new(MailboxChannelId(spec.cells[0])));
            channel.set_client(client)?;
            Ok(channel)
        }

        fn release_channel(&self, channel: MailboxChannelId) {
            self.released.store(true, Ordering::SeqCst);
            self.last_released.store(channel.0, Ordering::SeqCst);
        }
    }

    #[test_case]
    fn test_mailbox_message_one() {
        let message = MailboxMessage::one(0x1234);
        assert_eq!(message.words, [0x1234, 0, 0, 0]);
        assert_eq!(message.len, 1);
    }

    #[test_case]
    fn test_mailbox_spec_clone() {
        let spec = MailboxSpec {
            controller_phandle: 0x10,
            cells: Vec::from([1, 2]),
        };
        let cloned = spec.clone();
        assert_eq!(cloned.controller_phandle, 0x10);
        assert_eq!(cloned.cells, Vec::from([1, 2]));
    }

    #[test_case]
    fn test_mailbox_controller_request_and_release_channel() {
        let controller = FakeController::new();
        let spec = MailboxSpec {
            controller_phandle: 0x20,
            cells: Vec::from([7]),
        };
        let channel = controller.request_channel(&spec, None).unwrap();
        assert_eq!(channel.id(), MailboxChannelId(7));
        assert_eq!(controller.requested.load(Ordering::SeqCst), 1);

        controller.release_channel(channel.id());
        assert!(controller.released.load(Ordering::SeqCst));
        assert_eq!(controller.last_released.load(Ordering::SeqCst), 7);
    }

    #[test_case]
    fn test_mailbox_channel_send_recv_roundtrip() {
        let channel = FakeChannel::new(MailboxChannelId(3));
        let message = MailboxMessage::one(0xabcd);
        channel.try_send(&message).unwrap();
        assert_eq!(channel.try_recv().unwrap(), Some(message));
        assert_eq!(channel.try_recv().unwrap(), None);
    }

    #[test_case]
    fn test_mailbox_channel_set_client() {
        let channel = FakeChannel::new(MailboxChannelId(4));
        let client = Arc::new(FakeClient::new());
        channel.set_client(Some(client.clone())).unwrap();
        channel.try_send(&MailboxMessage::one(1)).unwrap();

        assert_eq!(client.tx_count.load(Ordering::SeqCst), 1);
        assert_eq!(client.rx_count.load(Ordering::SeqCst), 1);
        assert_eq!(client.error_count.load(Ordering::SeqCst), 0);
    }
}
