//! Event-based IPC operations capability for kernel objects
//! 
//! This module defines the event-based IPC operations capability that enables
//! kernel objects to participate in event-driven inter-process communication
//! through channels, process groups, and event distribution.
//! 
//! Note: This is separate from stream-based IPC (pipes, sockets) which use StreamOps.

use alloc::{string::String, vec::Vec, sync::Arc};
use crate::task::events::{TaskEventType, EventTarget};

/// Event-based IPC operations capability
/// 
/// This trait enables kernel objects to participate in event-driven IPC mechanisms
/// such as event channels, process groups, and broadcast communications.
/// Objects implementing this trait can be used for:
/// - Event channel subscriptions and publishing
/// - Process group membership and communication  
/// - Broadcast event distribution
/// 
/// This is distinct from stream-based IPC mechanisms (pipes, sockets) which use StreamOps.
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
    
    /// Publish an event to a channel
    /// 
    /// # Arguments
    /// * `channel_name` - Name of the channel to publish to
    /// * `event_type` - Type of event to publish
    /// * `source_task` - Optional source task ID
    /// 
    /// # Returns
    /// * `Ok(subscriber_count)` - Number of subscribers that received the event
    /// * `Err(error_message)` on failure
    fn publish_to_channel(&self, channel_name: String, event_type: TaskEventType, source_task: Option<usize>) -> Result<usize, &'static str>;
    
    /// Join a process group
    /// 
    /// # Arguments
    /// * `group_id` - ID of the process group to join
    /// 
    /// # Returns
    /// * `Ok(())` on successful join
    /// * `Err(error_message)` on failure
    fn join_process_group(&self, group_id: usize) -> Result<(), &'static str>;
    
    /// Leave a process group
    /// 
    /// # Arguments
    /// * `group_id` - ID of the process group to leave
    /// 
    /// # Returns
    /// * `Ok(())` on successful leave
    /// * `Err(error_message)` on failure
    fn leave_process_group(&self, group_id: usize) -> Result<(), &'static str>;
    
    /// Send event to a process group
    /// 
    /// # Arguments
    /// * `group_id` - ID of the process group to send to
    /// * `event_type` - Type of event to send
    /// * `source_task` - Optional source task ID
    /// 
    /// # Returns
    /// * `Ok(member_count)` - Number of group members that received the event
    /// * `Err(error_message)` on failure
    fn send_to_process_group(&self, group_id: usize, event_type: TaskEventType, source_task: Option<usize>) -> Result<usize, &'static str>;
    
    /// Send event to a specific target
    /// 
    /// # Arguments
    /// * `target` - Event delivery target (Task, Group, Broadcast, etc.)
    /// * `event_type` - Type of event to send
    /// * `source_task` - Optional source task ID
    /// 
    /// # Returns
    /// * `Ok(recipient_count)` - Number of recipients that received the event
    /// * `Err(error_message)` on failure
    fn send_event(&self, target: EventTarget, event_type: TaskEventType, source_task: Option<usize>) -> Result<usize, &'static str>;
    
    /// Get list of subscribed channels
    /// 
    /// # Returns
    /// * `Vec<String>` - List of channel names this object is subscribed to
    fn get_subscribed_channels(&self) -> Vec<String>;
    
    /// Get list of joined process groups
    /// 
    /// # Returns
    /// * `Vec<usize>` - List of process group IDs this object has joined
    fn get_joined_process_groups(&self) -> Vec<usize>;
    
    /// Get associated task ID for this IPC endpoint
    /// 
    /// # Returns
    /// * `Option<usize>` - Task ID if this IPC endpoint is associated with a specific task
    fn get_task_id(&self) -> Option<usize>;
}

/// Event-based IPC error types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventIpcError {
    /// Channel does not exist
    ChannelNotFound,
    /// Already subscribed to channel
    AlreadySubscribed,
    /// Not subscribed to channel
    NotSubscribed,
    /// Process group does not exist
    GroupNotFound,
    /// Already member of process group
    AlreadyMember,
    /// Not a member of process group
    NotMember,
    /// No task associated with this IPC endpoint
    NoAssociatedTask,
    /// Permission denied
    PermissionDenied,
    /// Resource limit exceeded
    ResourceLimitExceeded,
}

impl EventIpcError {
    /// Convert EventIpcError to error message string
    pub fn as_str(&self) -> &'static str {
        match self {
            EventIpcError::ChannelNotFound => "Channel not found",
            EventIpcError::AlreadySubscribed => "Already subscribed to channel",
            EventIpcError::NotSubscribed => "Not subscribed to channel",
            EventIpcError::GroupNotFound => "Process group not found",
            EventIpcError::AlreadyMember => "Already member of process group",
            EventIpcError::NotMember => "Not a member of process group",
            EventIpcError::NoAssociatedTask => "No task associated with IPC endpoint",
            EventIpcError::PermissionDenied => "Permission denied",
            EventIpcError::ResourceLimitExceeded => "Resource limit exceeded",
        }
    }
}
