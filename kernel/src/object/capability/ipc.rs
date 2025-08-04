//! Simplified Event-based IPC operations capability for kernel objects

use alloc::{string::String, vec::Vec};

/// Event-based IPC operations capability
/// 
/// Simplified interface for event-driven IPC mechanisms.
pub trait EventIpcOps: Send + Sync {
    /// Subscribe to an event channel
    /// 
    /// # Arguments
    /// * `channel_name` - Name of the channel to subscribe to
    /// 
    /// # Returns
    /// * `Ok(())` on successful subscription
    /// * `Err(error_message)` on failure
    fn subscribe_channel(&self, channel_name: String) -> Result<(), &'static str>;
    
    /// Unsubscribe from an event channel
    /// 
    /// # Arguments
    /// * `channel_name` - Name of the channel to unsubscribe from
    /// 
    /// # Returns
    /// * `Ok(())` on successful unsubscription
    /// * `Err(error_message)` on failure
    fn unsubscribe_channel(&self, channel_name: &str) -> Result<(), &'static str>;
    
    /// Get list of subscribed channels
    /// 
    /// # Returns
    /// * `Vec<String>` - List of channel names this object is subscribed to
    fn get_subscribed_channels(&self) -> Vec<String>;
    
    /// Get task ID if this object represents a task
    /// 
    /// # Returns
    /// * `Some(task_id)` if this object represents a task
    /// * `None` otherwise
    fn get_task_id(&self) -> Option<usize>;
}
