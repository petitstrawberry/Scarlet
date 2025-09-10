//! Event-based IPC capabilities for kernel objects

use alloc::vec::Vec;
use crate::ipc::event::{Event, EventFilter};

/// Event sending capability
/// 
/// Objects that can send events to other objects or channels.
pub trait EventSender: Send + Sync {
    /// Send an event
    /// 
    /// # Arguments
    /// * `event` - The event to send
    /// 
    /// # Returns
    /// * `Ok(())` on successful send
    /// * `Err(error_message)` on failure
    fn send_event(&self, event: Event) -> Result<(), &'static str>;
}

/// Event receiving capability
/// 
/// Objects that can receive and poll for events.
pub trait EventReceiver: Send + Sync {
    /// Check if events are pending
    /// 
    /// # Returns
    /// * `true` if events are available, `false` otherwise
    fn has_pending_events(&self) -> bool;
}

/// Event subscription management capability
/// 
/// Objects that can manage event filters and subscriptions.
pub trait EventSubscriber: Send + Sync {
    /// Register an event filter with a handler ID
    /// 
    /// # Arguments
    /// * `filter` - Event filter to register
    /// * `handler_id` - Unique identifier for this handler
    /// 
    /// # Returns
    /// * `Ok(())` on successful registration
    /// * `Err(error_message)` on failure
    fn register_filter(&self, filter: EventFilter, handler_id: usize) -> Result<(), &'static str>;
    
    /// Unregister an event filter by handler ID
    /// 
    /// # Arguments
    /// * `handler_id` - Handler ID to unregister
    /// 
    /// # Returns
    /// * `Ok(())` on successful unregistration
    /// * `Err(error_message)` on failure
    fn unregister_filter(&self, handler_id: usize) -> Result<(), &'static str>;
    
    /// Get registered filters and their handler IDs
    /// 
    /// # Returns
    /// * `Vec<(usize, EventFilter)>` - List of (handler_id, filter) pairs
    fn get_filters(&self) -> Vec<(usize, EventFilter)>;
}
