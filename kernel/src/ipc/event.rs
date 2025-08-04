//! Event-based Inter-Process Communication
//! 
//! This module provides a unified event system for Scarlet OS that handles
//! different types of event delivery mechanisms:
//! - Immediate: Force delivery regardless of receiver state
//! - Notification: One-way, best-effort delivery
//! - Subscription: Channel-based pub/sub delivery
//! - Group: Broadcast delivery to multiple targets

use alloc::{string::String, vec::Vec, sync::Arc, format};
use hashbrown::HashMap;
use spin::Mutex;

/// Type alias for task identifiers
pub type TaskId = u32;
/// Type alias for group identifiers
pub type GroupId = u32;
/// Type alias for session identifiers
pub type SessionId = u32;

/// Event structure containing all event information
#[derive(Debug, Clone)]
pub struct Event {
    /// Event type (contains all delivery and targeting information)
    pub event_type: EventType,
    
    /// Event payload data
    pub payload: EventPayload,
    
    /// Event metadata
    pub metadata: EventMetadata,
}

/// Event types with embedded delivery characteristics and targeting
#[derive(Debug, Clone)]
pub enum EventType {
    /// Direct task communication (1:1)
    /// Used for process control, signals, direct notifications
    Direct {
        target: TaskId,
        event_id: u32,
        priority: EventPriority,
        reliable: bool,
    },
    
    /// Channel-based communication (1:many, pub/sub)
    /// Used for event distribution, notifications, pub/sub patterns
    Channel {
        channel_id: String,
        create_if_missing: bool,
        priority: EventPriority,
    },
    
    /// Group broadcast (1:many, membership-based)
    /// Used for group notifications, session broadcasts, process groups
    Group {
        group_target: GroupTarget,
        priority: EventPriority,
        reliable: bool,
    },
    
    /// System-wide broadcast (1:all)
    /// Used for system-wide notifications, shutdown signals
    Broadcast {
        event_id: u32,
        priority: EventPriority,
        reliable: bool,
    },
}

/// Group targeting options
#[derive(Debug, Clone)]
pub enum GroupTarget {
    /// Specific task group
    TaskGroup(GroupId),
    
    /// All tasks in the system
    AllTasks,
    
    /// Session-based group
    Session(SessionId),
    
    /// Custom named group
    Custom(String),
}
/// Event payload data
#[derive(Debug, Clone)]
pub enum EventPayload {
    /// No data
    Empty,
    
    /// Integer value
    Integer(i64),
    
    /// Byte array
    Bytes(Vec<u8>),
    
    /// String data
    String(String),
    
    /// Custom binary data
    Custom(Vec<u8>),
}

/// Event metadata
#[derive(Debug, Clone)]
pub struct EventMetadata {
    /// Sender task ID
    pub sender: Option<u32>,
    
    /// Event priority
    pub priority: EventPriority,
    
    /// Timestamp
    pub timestamp: u64,
    
    /// Unique event ID
    pub event_id: u64,
}

impl EventMetadata {
    /// Create new metadata with current timestamp
    pub fn new() -> Self {
        Self {
            sender: None, // Will be filled by EventManager
            priority: EventPriority::Normal,
            timestamp: 0, // TODO: integrate with timer system
            event_id: generate_event_id(),
        }
    }
}

/// Event priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EventPriority {
    Low = 1,
    Normal = 2,
    High = 3,
    Critical = 4,
}

/// Event filter for handler registration
#[derive(Debug, Clone)]
pub enum EventFilter {
    /// All events
    All,
    
    /// Specific event type
    EventType(EventTypeFilter),
    
    /// Specific event ID
    EventId(u32),
    
    /// Specific channel
    Channel(String),
    
    /// Specific sender
    Sender(u32),
    
    /// Custom filter function
    Custom(fn(&Event) -> bool),
}

/// Event type filter
#[derive(Debug, Clone)]
pub enum EventTypeFilter {
    /// Any direct event
    AnyDirect,
    
    /// Any channel event
    AnyChannel,
    
    /// Any group event
    AnyGroup,
    
    /// Any broadcast event
    AnyBroadcast,
    
    /// Specific direct event
    Direct(u32),
    
    /// Specific channel
    Channel(String),
    
    /// Specific group
    Group(GroupId),
    
    /// Specific broadcast
    Broadcast(u32),
}

/// Event handler
pub enum EventHandler {
    /// Function pointer
    Function(fn(Event)),
    
    /// Forward to another task
    ForwardToTask(TaskId),
    
    /// Forward to a channel
    ForwardToChannel(String),
    
    /// Default system action
    Default,
}

// Custom Debug implementation to handle the non-Debug closure
impl core::fmt::Debug for EventHandler {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EventHandler::Function(_) => write!(f, "Function(<function>)"),
            EventHandler::ForwardToTask(task_id) => write!(f, "ForwardToTask({})", task_id),
            EventHandler::ForwardToChannel(channel) => write!(f, "ForwardToChannel({})", channel),
            EventHandler::Default => write!(f, "Default"),
        }
    }
}

/// Delivery configuration
#[derive(Debug, Clone)]
pub struct DeliveryConfig {
    /// Buffer size for queued events
    pub buffer_size: usize,
    
    /// Delivery timeout in milliseconds
    pub timeout_ms: Option<u64>,
    
    /// Retry count on failure
    pub retry_count: u32,
    
    /// Failure policy
    pub failure_policy: FailurePolicy,
}

impl Default for DeliveryConfig {
    fn default() -> Self {
        Self {
            buffer_size: 1024,
            timeout_ms: Some(5000),
            retry_count: 3,
            failure_policy: FailurePolicy::Log,
        }
    }
}

/// Failure handling policy
#[derive(Debug, Clone)]
pub enum FailurePolicy {
    /// Ignore failures
    Ignore,
    
    /// Log failures
    Log,
    
    /// Notify sender of failure
    NotifySender,
    
    /// Generate system event
    SystemEvent,
}

/// Event system errors
#[derive(Debug, Clone)]
pub enum EventError {
    /// Target not found
    TargetNotFound,
    
    /// Permission denied
    PermissionDenied,
    
    /// Delivery failed
    DeliveryFailed,
    
    /// Buffer full
    BufferFull,
    
    /// Operation timeout
    Timeout,
    
    /// Invalid configuration
    InvalidConfiguration,
    
    /// Channel not found
    ChannelNotFound,
    
    /// Group not found
    GroupNotFound,
    
    /// Other error
    Other(String),
}

/// Event configuration for delivery settings
#[derive(Debug, Clone)]
pub struct EventConfig {
    /// Default buffer size
    pub default_buffer_size: usize,
    
    /// Timeout settings
    pub default_timeout_ms: u64,
    
    /// Maximum number of channels
    pub max_channels: usize,
    
    /// Maximum number of groups
    pub max_groups: usize,
}

impl Default for EventConfig {
    fn default() -> Self {
        Self {
            default_buffer_size: 64,
            default_timeout_ms: 1000,
            max_channels: 1024,
            max_groups: 256,
        }
    }
}

/// Event Manager - Main implementation of the event system
pub struct EventManager {
    /// Event handlers by task
    handlers: Mutex<HashMap<u32, Vec<(EventFilter, EventHandler)>>>,
    
    /// Channel subscriptions
    subscriptions: Mutex<HashMap<String, Vec<u32>>>,
    
    /// Task group memberships
    groups: Mutex<HashMap<GroupId, Vec<u32>>>,
    
    /// Delivery configurations per task
    configs: Mutex<HashMap<u32, DeliveryConfig>>,
    
    /// Event queue for async delivery
    event_queue: Mutex<Vec<Event>>,
    
    /// Next event ID
    next_event_id: Mutex<u64>,
    
    /// Handle-based channel registry for KernelObject integration
    handle_channels: Mutex<HashMap<String, Arc<crate::ipc::event_objects::EventChannel>>>,
}

impl EventManager {
    /// Create a new EventManager
    pub fn new() -> Self {
        Self {
            handlers: Mutex::new(HashMap::new()),
            subscriptions: Mutex::new(HashMap::new()),
            groups: Mutex::new(HashMap::new()),
            configs: Mutex::new(HashMap::new()),
            event_queue: Mutex::new(Vec::new()),
            next_event_id: Mutex::new(1),
            handle_channels: Mutex::new(HashMap::new()),
        }
    }
    
    /// Get the global EventManager instance
    pub fn get_manager() -> &'static EventManager {
        static INSTANCE: spin::once::Once<EventManager> = spin::once::Once::new();
        INSTANCE.call_once(|| EventManager::new())
    }
    
    /// Create or get an event channel as a KernelObject handle
    /// 
    /// This method creates an EventChannel that can be inserted into a HandleTable,
    /// providing consistent resource management with other kernel objects.
    pub fn create_channel(&self, name: String) -> crate::object::KernelObject {
        let mut channels = self.handle_channels.lock();
        
        let channel = channels
            .entry(name.clone())
            .or_insert_with(|| {
                Arc::new(crate::ipc::event_objects::EventChannel::new(name.clone()))
            })
            .clone();
        
        crate::object::KernelObject::from_event_channel_object(channel)
    }
    
    /// Create a subscription to a channel as a KernelObject handle
    /// 
    /// This method creates an EventSubscription that can be inserted into a HandleTable,
    /// allowing tasks to receive events through the standard handle interface.
    pub fn create_subscription(&self, channel_name: String) -> Result<crate::object::KernelObject, EventError> {
        let channels = self.handle_channels.lock();
        
        let channel = channels.get(&channel_name)
            .ok_or(EventError::ChannelNotFound)?;
        
        let subscription = channel.create_subscription(Some(1024)); // Default queue size
        
        Ok(crate::object::KernelObject::from_event_subscription_object(Arc::new(subscription)))
    }
    
    // === Core Event Operations ===
    
    /// Send an event
    pub fn send_event(&self, event: Event) -> Result<(), EventError> {
        match event.event_type.clone() {
            EventType::Direct { target, event_id, priority, reliable } => {
                self.deliver_direct(event, target, event_id, priority, reliable)
            }
            
            EventType::Channel { channel_id, create_if_missing, priority } => {
                self.deliver_to_channel(event, &channel_id, create_if_missing, priority)
            }
            
            EventType::Group { group_target, priority, reliable } => {
                self.deliver_to_group(event, &group_target, priority, reliable)
            }
            
            EventType::Broadcast { event_id, priority, reliable } => {
                self.deliver_broadcast(event, event_id, priority, reliable)
            }
        }
    }
    
    /// Register an event handler
    pub fn register_handler(&self, _filter: EventFilter, _handler: EventHandler) -> Result<(), EventError> {
        // TODO: Implement handler registration
        Ok(())
    }
    
    /// Subscribe to a channel
    pub fn subscribe_channel(&self, channel: &str) -> Result<(), EventError> {
        // TODO: Get current task ID from task system
        let current_task_id = 1; // Placeholder
        
        let mut subscriptions = self.subscriptions.lock();
        let channel_subscribers = subscriptions.entry(format!("{}", channel)).or_insert_with(Vec::new);
        
        if !channel_subscribers.contains(&current_task_id) {
            channel_subscribers.push(current_task_id);
        }
        
        Ok(())
    }
    
    /// Unsubscribe from a channel
    pub fn unsubscribe_channel(&self, channel: &str) -> Result<(), EventError> {
        let current_task_id = 1; // TODO: Get from task system
        
        let mut subscriptions = self.subscriptions.lock();
        if let Some(channel_subscribers) = subscriptions.get_mut(channel) {
            channel_subscribers.retain(|&task_id| task_id != current_task_id);
        }
        
        Ok(())
    }
    
    /// Join a task group
    pub fn join_group(&self, group_id: GroupId) -> Result<(), EventError> {
        let current_task_id = 1; // TODO: Get from task system
        
        let mut groups = self.groups.lock();
        let group_members = groups.entry(group_id).or_insert_with(Vec::new);
        
        if !group_members.contains(&current_task_id) {
            group_members.push(current_task_id);
        }
        
        Ok(())
    }
    
    /// Leave a task group
    pub fn leave_group(&self, group_id: GroupId) -> Result<(), EventError> {
        let current_task_id = 1; // TODO: Get from task system
        
        let mut groups = self.groups.lock();
        if let Some(group_members) = groups.get_mut(&group_id) {
            group_members.retain(|&task_id| task_id != current_task_id);
        }
        
        Ok(())
    }
    
    /// Configure delivery settings
    pub fn configure_delivery(&self, config: DeliveryConfig) -> Result<(), EventError> {
        let current_task_id = 1; // TODO: Get from task system
        
        let mut configs = self.configs.lock();
        configs.insert(current_task_id, config);
        
        Ok(())
    }
    
    // === Internal Event Delivery Methods ===
    
    /// Deliver direct event to specific task
    fn deliver_direct(&self, event: Event, target: TaskId, _event_id: u32, _priority: EventPriority, _reliable: bool) -> Result<(), EventError> {
        self.deliver_to_task(target, event)
    }
    
    /// Deliver to channel subscribers
    fn deliver_to_channel(&self, event: Event, channel_id: &str, create_if_missing: bool, _priority: EventPriority) -> Result<(), EventError> {
        let subscriptions = self.subscriptions.lock();
        
        if let Some(subscribers) = subscriptions.get(channel_id) {
            for &task_id in subscribers {
                let _ = self.deliver_to_task(task_id, event.clone());
            }
            Ok(())
        } else if create_if_missing {
            // Create empty channel
            drop(subscriptions);
            let mut subscriptions = self.subscriptions.lock();
            subscriptions.insert(format!("{}", channel_id), Vec::new());
            Ok(())
        } else {
            Err(EventError::ChannelNotFound)
        }
    }
    
    /// Deliver to group members
    fn deliver_to_group(&self, event: Event, group_target: &GroupTarget, _priority: EventPriority, _reliable: bool) -> Result<(), EventError> {
        match group_target {
            GroupTarget::TaskGroup(group_id) => {
                let groups = self.groups.lock();
                if let Some(members) = groups.get(group_id) {
                    for &task_id in members {
                        let _ = self.deliver_to_task(task_id, event.clone());
                    }
                    Ok(())
                } else {
                    Err(EventError::GroupNotFound)
                }
            }
            GroupTarget::AllTasks => {
                // TODO: Deliver to all tasks in the system
                // This would require integration with the task manager
                Err(EventError::Other(format!("AllTasks delivery not implemented")))
            }
            _ => Err(EventError::Other(format!("Group target not implemented"))),
        }
    }
    
    /// Deliver broadcast event to all tasks
    fn deliver_broadcast(&self, event: Event, _event_id: u32, _priority: EventPriority, _reliable: bool) -> Result<(), EventError> {
        // TODO: Deliver to all tasks in the system
        // This would require integration with the task manager
        let _ = event; // Suppress unused warning for now
        Err(EventError::Other(format!("Broadcast delivery not implemented")))
    }
    
    /// Deliver event to a specific task
    pub fn deliver_to_task(&self, task_id: u32, event: Event) -> Result<(), EventError> {
        if let Some(task) = crate::sched::scheduler::get_scheduler().get_task_by_id(task_id as usize) {
            // Delegate to ABI module
            if let Some(abi) = &task.abi {
                abi.handle_event(event, task_id)
                    .map_err(|_| EventError::DeliveryFailed)
            } else {
                // Ignore if ABI module is not set
                Ok(())
            }
        } else {
            Err(EventError::TargetNotFound)
        }
    }
}

/// Convenience functions for creating events
impl Event {
    /// Create a new event with specified type and payload
    pub fn new(event_type: EventType, payload: EventPayload) -> Self {
        Self {
            event_type,
            payload,
            metadata: EventMetadata::new(),
        }
    }
    
    /// Create a direct event to a specific task
    pub fn direct(target: TaskId, event_id: u32, priority: EventPriority, reliable: bool, payload: EventPayload) -> Self {
        Self::new(
            EventType::Direct { target, event_id, priority, reliable },
            payload,
        )
    }
    
    /// Create a channel event
    pub fn channel(channel_id: String, create_if_missing: bool, priority: EventPriority, payload: EventPayload) -> Self {
        Self::new(
            EventType::Channel { channel_id, create_if_missing, priority },
            payload,
        )
    }
    
    /// Create a group event
    pub fn group(group_target: GroupTarget, priority: EventPriority, reliable: bool, payload: EventPayload) -> Self {
        Self::new(
            EventType::Group { group_target, priority, reliable },
            payload,
        )
    }
    
    /// Create a broadcast event
    pub fn broadcast(event_id: u32, priority: EventPriority, reliable: bool, payload: EventPayload) -> Self {
        Self::new(
            EventType::Broadcast { event_id, priority, reliable },
            payload,
        )
    }
    
    // Convenience methods for common use cases
    
    /// Create immediate event for a specific task
    pub fn immediate_to_task(task_id: u32, event_id: u32) -> Self {
        Self::direct(task_id, event_id, EventPriority::High, true, EventPayload::Empty)
    }
    
    /// Create notification event for a specific task
    pub fn notification_to_task(task_id: u32, notification_id: u32) -> Self {
        Self::direct(task_id, notification_id, EventPriority::Normal, false, EventPayload::Empty)
    }
    
    /// Create channel event (simple)
    pub fn new_channel_event(channel: &str, payload: EventPayload) -> Self {
        Self::channel(channel.into(), false, EventPriority::Normal, payload)
    }
    
    /// Create group broadcast event (simple)
    pub fn new_group_broadcast(group_target: GroupTarget, payload: EventPayload) -> Self {
        Self::group(group_target, EventPriority::Normal, false, payload)
    }
    
    /// Create immediate broadcast event
    pub fn immediate_broadcast(event_id: u32) -> Self {
        Self::broadcast(event_id, EventPriority::High, true, EventPayload::Empty)
    }
    
    /// Create notification for a group
    pub fn notification_to_group(group_id: GroupId, _notification_id: u32) -> Self {
        Self::group(GroupTarget::TaskGroup(group_id), EventPriority::Normal, false, EventPayload::Empty)
    }
}

/// Generate unique event ID
fn generate_event_id() -> u64 {
    static COUNTER: Mutex<u64> = Mutex::new(1);
    let mut counter = COUNTER.lock();
    let id = *counter;
    *counter += 1;
    id
}

/// Event ID constants for common events
pub mod event_ids {
    /// Process control events
    pub const EVENT_TERMINATE: u32 = 1;
    pub const EVENT_FORCE_TERMINATE: u32 = 2;
    pub const EVENT_STOP: u32 = 3;
    pub const EVENT_CONTINUE: u32 = 4;
    pub const EVENT_CHILD_EXIT: u32 = 5;
    pub const SYSTEM_SHUTDOWN: u32 = 6;
    
    /// Notification events
    pub const NOTIFICATION_TASK_COMPLETED: u32 = 100;
    pub const NOTIFICATION_MEMORY_LOW: u32 = 101;
    pub const NOTIFICATION_DEVICE_CONNECTED: u32 = 102;
    pub const NOTIFICATION_DEVICE_DISCONNECTED: u32 = 103;
    
    /// Base ranges for custom events
    pub const EVENT_CUSTOM_BASE: u32 = 10000;
    pub const EVENT_USER_BASE: u32 = 20000;
}

/// Global function to get the event manager
pub fn get_event_manager() -> &'static EventManager {
    EventManager::get_manager()
}

/// Example usage functions demonstrating KernelObject integration
pub mod handle_based_examples {
    use super::*;
    use crate::object::handle::{HandleTable, HandleMetadata, HandleType, AccessMode};
    
    /// Create an event channel and add it to a task's handle table
    pub fn create_channel_handle(handle_table: &mut HandleTable, channel_name: String) -> Result<crate::object::handle::Handle, &'static str> {
        let manager = EventManager::get_manager();
        let channel_obj = manager.create_channel(channel_name);
        
        let metadata = HandleMetadata {
            handle_type: HandleType::EventChannel,
            access_mode: AccessMode::ReadWrite,
            special_semantics: None,
        };
        
        handle_table.insert_with_metadata(channel_obj, metadata)
    }
    
    /// Create an event subscription and add it to a task's handle table
    pub fn create_subscription_handle(handle_table: &mut HandleTable, channel_name: String) -> Result<crate::object::handle::Handle, EventError> {
        let manager = EventManager::get_manager();
        let subscription_obj = manager.create_subscription(channel_name)?;
        
        let metadata = HandleMetadata {
            handle_type: HandleType::EventSubscription,
            access_mode: AccessMode::ReadOnly, // Subscriptions are read-only
            special_semantics: None,
        };
        
        handle_table.insert_with_metadata(subscription_obj, metadata)
            .map_err(|_| EventError::Other("Failed to insert subscription handle".into()))
    }
    
    /// Publish an event using a channel handle
    pub fn publish_via_handle(handle_table: &HandleTable, channel_handle: crate::object::handle::Handle, event: Event) -> Result<(), EventError> {
        let kernel_obj = handle_table.get(channel_handle)
            .ok_or(EventError::Other("Invalid handle".into()))?;
        
        let channel = kernel_obj.as_event_channel()
            .ok_or(EventError::Other("Handle is not an event channel".into()))?;
        
        // Convert our EventError to the event_objects::EventError
        channel.publish(event).map_err(|_| EventError::Other("Failed to publish event".into()))
    }
    
    /// Receive an event using a subscription handle
    pub fn receive_via_handle(handle_table: &HandleTable, subscription_handle: crate::object::handle::Handle, blocking: bool) -> Result<Event, EventError> {
        let kernel_obj = handle_table.get(subscription_handle)
            .ok_or(EventError::Other("Invalid handle".into()))?;
        
        let subscription = kernel_obj.as_event_subscription()
            .ok_or(EventError::Other("Handle is not an event subscription".into()))?;
        
        // Convert event_objects::EventError to our EventError
        subscription.receive_event(blocking).map_err(|_| EventError::Other("Failed to receive event".into()))
    }
    
    /// Receive an event using a subscription handle (with blocking support)
    pub fn receive_blocking_via_handle(handle_table: &HandleTable, subscription_handle: crate::object::handle::Handle) -> Result<Event, EventError> {
        // This will block the current task until an event is available
        receive_via_handle(handle_table, subscription_handle, true)
    }
    
    /// Check if a subscription has pending events (non-blocking)
    pub fn has_pending_events_via_handle(handle_table: &HandleTable, subscription_handle: crate::object::handle::Handle) -> Result<bool, EventError> {
        let kernel_obj = handle_table.get(subscription_handle)
            .ok_or(EventError::Other("Invalid handle".into()))?;
        
        let subscription = kernel_obj.as_event_subscription()
            .ok_or(EventError::Other("Handle is not an event subscription".into()))?;
        
        Ok(subscription.has_pending_events())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Test creating basic events
    #[test_case]
    fn test_event_creation() {
        // Test direct event creation
        let direct_event = Event::direct(1, 1001, EventPriority::Normal, true, EventPayload::Empty);
        assert!(matches!(direct_event.event_type, EventType::Direct { target: 1, event_id: 1001, .. }));
        assert!(matches!(direct_event.payload, EventPayload::Empty));

        // Test channel event creation
        let channel_event = Event::channel("test_channel".into(), false, EventPriority::Normal, EventPayload::String("test".into()));
        assert!(matches!(channel_event.event_type, EventType::Channel { ref channel_id, .. } if channel_id == "test_channel"));
        assert!(matches!(channel_event.payload, EventPayload::String(_)));

        // Test group broadcast event creation
        let group_event = Event::group(
            GroupTarget::TaskGroup(100), 
            EventPriority::Normal,
            false,
            EventPayload::Integer(42)
        );
        assert!(matches!(group_event.event_type, EventType::Group { ref group_target, .. } if matches!(group_target, GroupTarget::TaskGroup(100))));
        assert!(matches!(group_event.payload, EventPayload::Integer(42)));
        
        // Test broadcast event creation
        let broadcast_event = Event::broadcast(2001, EventPriority::High, true, EventPayload::String("system message".into()));
        assert!(matches!(broadcast_event.event_type, EventType::Broadcast { event_id: 2001, .. }));
        assert!(matches!(broadcast_event.payload, EventPayload::String(_)));
    }

    /// Test event manager singleton
    #[test_case]
    fn test_event_manager_singleton() {
        let manager1 = EventManager::get_manager();
        let manager2 = EventManager::get_manager();
        
        // Should return the same instance
        assert!(core::ptr::eq(manager1, manager2));
    }

    /// Test event subscription operations
    #[test_case]
    fn test_event_subscription() {
        let manager = EventManager::get_manager();
        
        // Test channel subscription
        let result = manager.subscribe_channel("test_channel");
        assert!(result.is_ok());
        
        // Test unsubscription
        let result = manager.unsubscribe_channel("test_channel");
        assert!(result.is_ok());
        
        // Test group operations
        let result = manager.join_group(42);
        assert!(result.is_ok());
        
        let result = manager.leave_group(42);
        assert!(result.is_ok());
    }

    /// Test delivery configuration
    #[test_case]
    fn test_delivery_configuration() {
        let manager = EventManager::get_manager();
        
        let config = DeliveryConfig {
            buffer_size: 2048,
            timeout_ms: Some(10000),
            retry_count: 5,
            failure_policy: FailurePolicy::NotifySender,
        };
        
        let result = manager.configure_delivery(config);
        assert!(result.is_ok());
    }

    /// Test event sending with different types
    #[test_case]
    fn test_event_sending() {
        let manager = EventManager::get_manager();
        
        // Test direct event sending
        let direct_event = Event::direct(1, 1001, EventPriority::High, true, EventPayload::Empty);
        let result = manager.send_event(direct_event);
        assert!(result.is_ok());
        
        // Test notification event sending
        let notification_event = Event::direct(2, 2001, EventPriority::Normal, false, EventPayload::String("notification".into()));
        let result = manager.send_event(notification_event);
        assert!(result.is_ok());
        
        // Test channel event sending - first subscribe to channel
        let _ = manager.subscribe_channel("test_channel");
        let channel_event = Event::channel("test_channel".into(), false, EventPriority::Normal, EventPayload::Bytes(vec![1, 2, 3]));
        let result = manager.send_event(channel_event);
        assert!(result.is_ok());
        
        // Test channel event with create_if_missing=true
        let channel_event_with_create = Event::channel(
            "new_channel".into(),
            true,
            EventPriority::Normal,
            EventPayload::String("test".into())
        );
        let result = manager.send_event(channel_event_with_create);
        assert!(result.is_ok());
        
        // Test group event sending
        let group_event = Event::group(
            GroupTarget::AllTasks, 
            EventPriority::Normal,
            false,
            EventPayload::String("broadcast_message".into())
        );
        let result = manager.send_event(group_event);
        // This should fail since AllTasks delivery is not implemented yet
        assert!(result.is_err());
    }

    /// Test event metadata generation
    #[test_case]
    fn test_event_metadata() {
        let metadata1 = EventMetadata::new();
        let metadata2 = EventMetadata::new();
        
        // Each metadata should have unique event IDs
        assert_ne!(metadata1.event_id, metadata2.event_id);
        
        // Default values should be correct
        assert_eq!(metadata1.priority, EventPriority::Normal);
        assert!(metadata1.sender.is_none());
    }

    /// Test event priority ordering
    #[test_case]
    fn test_event_priority_ordering() {
        assert!(EventPriority::Critical > EventPriority::High);
        assert!(EventPriority::High > EventPriority::Normal);
        assert!(EventPriority::Normal > EventPriority::Low);
        
        let priorities = vec![
            EventPriority::Low,
            EventPriority::Critical,
            EventPriority::Normal,
            EventPriority::High,
        ];
        
        let mut sorted = priorities.clone();
        sorted.sort();
        
        assert_eq!(sorted, vec![
            EventPriority::Low,
            EventPriority::Normal,
            EventPriority::High,
            EventPriority::Critical,
        ]);
    }

    /// Test event filter functionality
    #[test_case]
    fn test_event_filters() {
        // Test different filter types can be created
        let _filter_all = EventFilter::All;
        let _filter_event_id = EventFilter::EventId(42);
        let _filter_channel = EventFilter::Channel("test".into());
        let _filter_sender = EventFilter::Sender(123);
        
        // Test custom filter
        let custom_filter = EventFilter::Custom(|event| {
            matches!(event.event_type, EventType::Direct { .. })
        });
        
        // Create test event
        let test_event = Event::direct(1, 1001, EventPriority::High, true, EventPayload::Empty);
        
        // Test custom filter (note: this is a simplified test since we can't easily test the function)
        if let EventFilter::Custom(filter_fn) = custom_filter {
            assert!(filter_fn(&test_event));
        }
    }

    /// Test event handler types
    #[test_case]
    fn test_event_handlers() {
        // Test function handler
        fn test_handler(_event: Event) {
            // Handler implementation
        }
        let _handler = EventHandler::Function(test_handler);
        
        // Test forward handlers
        let _forward_to_task = EventHandler::ForwardToTask(42);
        let _forward_to_channel = EventHandler::ForwardToChannel("test_channel".into());
        
        // Test default handler
        let _default_handler = EventHandler::Default;
    }

    /// Test event payload types
    #[test_case]
    fn test_event_payloads() {
        let _empty = EventPayload::Empty;
        let _integer = EventPayload::Integer(-123);
        let _bytes = EventPayload::Bytes(vec![0x01, 0x02, 0x03]);
        let _string = EventPayload::String("test payload".into());
        let _custom = EventPayload::Custom(vec![0xFF, 0xFE, 0xFD]);
        
        // Test cloning
        let original = EventPayload::String("original".into());
        let cloned = original.clone();
        
        if let (EventPayload::String(orig), EventPayload::String(clone)) = (original, cloned) {
            assert_eq!(orig, clone);
        }
    }

    /// Test group types
    #[test_case]
    fn test_group_types() {
        let _task_group = GroupTarget::TaskGroup(100);
        let _all_tasks = GroupTarget::AllTasks;
        let _session = GroupTarget::Session(200);
        let _custom = GroupTarget::Custom("custom_group".into());
        
        // Test cloning
        let original = GroupTarget::Custom("test".into());
        let cloned = original.clone();
        
        if let (GroupTarget::Custom(orig), GroupTarget::Custom(clone)) = (original, cloned) {
            assert_eq!(orig, clone);
        }
    }

    /// Test event ID generation uniqueness
    #[test_case]
    fn test_event_id_uniqueness() {
        let id1 = generate_event_id();
        let id2 = generate_event_id();
        let id3 = generate_event_id();
        
        // All IDs should be unique and incrementing
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert!(id2 > id1);
        assert!(id3 > id2);
    }

    /// Test delivery config defaults
    #[test_case]
    fn test_delivery_config_defaults() {
        let config = DeliveryConfig::default();
        
        assert_eq!(config.buffer_size, 1024);
        assert_eq!(config.timeout_ms, Some(5000));
        assert_eq!(config.retry_count, 3);
        assert!(matches!(config.failure_policy, FailurePolicy::Log));
    }

    /// Test event system constants
    #[test_case]
    fn test_event_constants() {
        // Test that constants are defined correctly
        assert_eq!(event_ids::EVENT_TERMINATE, 1);
        assert_eq!(event_ids::EVENT_FORCE_TERMINATE, 2);
        assert_eq!(event_ids::NOTIFICATION_TASK_COMPLETED, 100);
        assert_eq!(event_ids::EVENT_CUSTOM_BASE, 10000);
        assert_eq!(event_ids::EVENT_USER_BASE, 20000);
        
        // Test that base ranges don't overlap
        assert!(event_ids::EVENT_CUSTOM_BASE > event_ids::NOTIFICATION_DEVICE_DISCONNECTED);
        assert!(event_ids::EVENT_USER_BASE > event_ids::EVENT_CUSTOM_BASE);
    }

    /// Test event error conditions
    #[test_case]
    fn test_event_error_conditions() {
        let manager = EventManager::get_manager();
        
        // Test sending to non-existent channel without create_if_missing
        let channel_event = Event::new_channel_event("nonexistent_channel", EventPayload::Empty);
        let result = manager.send_event(channel_event);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), EventError::ChannelNotFound));
        
        // Test sending to non-existent group
        let group_event = Event::new_group_broadcast(
            GroupTarget::TaskGroup(9999), 
            EventPayload::Empty
        );
        let result = manager.send_event(group_event);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), EventError::GroupNotFound));
        
        // Test AllTasks delivery (not implemented)
        let all_tasks_event = Event::new_group_broadcast(
            GroupTarget::AllTasks, 
            EventPayload::Empty
        );
        let result = manager.send_event(all_tasks_event);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), EventError::Other(_)));
    }

    /// Test unified event creation API
    #[test_case]
    fn test_unified_event_creation() {
        // Test broadcast event creation
        let broadcast_event = Event::broadcast(1001, EventPriority::Critical, true, EventPayload::String("system_shutdown".into()));
        assert!(matches!(broadcast_event.event_type, EventType::Broadcast { event_id: 1001, .. }));
        assert!(matches!(broadcast_event.payload, EventPayload::String(_)));
        
        // Test group notification
        let group_event = Event::notification_to_group(1, 2001);
        assert!(matches!(group_event.event_type, EventType::Group { ref group_target, .. } if matches!(group_target, GroupTarget::TaskGroup(1))));
        assert!(matches!(group_event.payload, EventPayload::Empty));
        
        // Test group broadcast event creation
        let group_broadcast = Event::new_group_broadcast(
            GroupTarget::AllTasks, 
            EventPayload::String("System message".into())
        );
        assert!(matches!(group_broadcast.event_type, EventType::Group { ref group_target, .. } if matches!(group_target, GroupTarget::AllTasks)));
        assert!(matches!(group_broadcast.payload, EventPayload::String(_)));
        
        // Test direct event constructor
        let custom_event = Event::direct(
            42,
            999,
            EventPriority::Critical,
            true,
            EventPayload::Bytes(vec![1, 2, 3, 4])
        );
        assert!(matches!(custom_event.event_type, EventType::Direct { target: 42, event_id: 999, .. }));
        assert!(matches!(custom_event.payload, EventPayload::Bytes(_)));
    }
}
