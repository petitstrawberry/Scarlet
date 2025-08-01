//! Event-based Inter-Process Communication
//! 
//! This module provides a unified event system for Scarlet OS that handles
//! different types of event delivery mechanisms:
//! - Immediate: Force delivery regardless of receiver state
//! - Notification: One-way, best-effort delivery
//! - Subscription: Channel-based pub/sub delivery
//! - Group: Broadcast delivery to multiple targets

use alloc::{string::String, vec::Vec, format};
use hashbrown::HashMap;
use spin::Mutex;

/// Type alias for task identifiers
pub type TaskId = u32;
/// Type alias for group identifiers
pub type GroupId = u32;
/// Type alias for session identifiers
pub type SessionId = u32;

/// Event delivery operations trait
pub trait EventOps {
    /// Send an event
    fn send_event(&self, event: Event) -> Result<(), EventError>;
    
    /// Register an event handler
    fn register_handler(&self, filter: EventFilter, handler: EventHandler) -> Result<(), EventError>;
    
    /// Subscribe to a channel
    fn subscribe_channel(&self, channel: &str) -> Result<(), EventError>;
    
    /// Unsubscribe from a channel
    fn unsubscribe_channel(&self, channel: &str) -> Result<(), EventError>;
    
    /// Join a task group
    fn join_group(&self, group_id: GroupId) -> Result<(), EventError>;
    
    /// Leave a task group
    fn leave_group(&self, group_id: GroupId) -> Result<(), EventError>;
    
    /// Configure delivery settings
    fn configure_delivery(&self, config: DeliveryConfig) -> Result<(), EventError>;
}

/// Event structure containing all event information
#[derive(Debug, Clone)]
pub struct Event {
    /// Event type (includes delivery characteristics)
    pub event_type: EventType,
    
    /// Delivery target
    pub target: EventTarget,
    
    /// Event payload data
    pub payload: EventPayload,
    
    /// Event metadata
    pub metadata: EventMetadata,
}

/// Event types with embedded delivery characteristics
#[derive(Debug, Clone)]
pub enum EventType {
    /// Immediate delivery: Force delivery regardless of receiver state
    /// Used for process control, emergency signals, etc.
    Immediate {
        event_id: u32,
        priority: EventPriority,
    },
    
    /// Notification delivery: One-way, lightweight, best-effort
    /// Used for status updates, general notifications, etc.
    Notification {
        notification_id: u32,
        priority: EventPriority,
    },
    
    /// Subscription delivery: Requires prior subscription, channel-based
    /// Used for pub/sub patterns, user events, etc.
    Subscription {
        channel_id: String,
        create_channel_if_missing: bool,
    },
    
    /// Group delivery: Broadcast to multiple targets
    /// Used for system-wide notifications, group communications, etc.
    Group {
        group_type: GroupType,
        reliable_delivery: bool,
    },
}

/// Group delivery types
#[derive(Debug, Clone)]
pub enum GroupType {
    /// Specific task group
    TaskGroup(GroupId),
    
    /// All tasks in the system
    AllTasks,
    
    /// Session-based group
    Session(SessionId),
    
    /// Custom named group
    Custom(String),
}

/// Event delivery targets
#[derive(Debug, Clone)]
pub enum EventTarget {
    /// Specific task
    Task(u32), // Using u32 for TaskId
    
    /// Task group
    Group(GroupId),
    
    /// Channel (for subscription-based delivery)
    Channel(String),
    
    /// Broadcast to all
    Broadcast,
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
    /// Any immediate event
    AnyImmediate,
    
    /// Any notification
    AnyNotification,
    
    /// Any subscription
    AnySubscription,
    
    /// Any group event
    AnyGroup,
    
    /// Specific immediate event
    Immediate(u32),
    
    /// Specific notification
    Notification(u32),
}

/// Event handler
pub enum EventHandler {
    /// Function pointer
    Function(fn(Event)),
    
    /// Forward to another target
    Forward(EventTarget),
    
    /// Default system action
    Default,
}

// Custom Debug implementation to handle the non-Debug closure
impl core::fmt::Debug for EventHandler {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EventHandler::Function(_) => write!(f, "Function(<function>)"),
            EventHandler::Forward(target) => write!(f, "Forward({:?})", target),
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
        }
    }
    
    /// Get the global EventManager instance
    pub fn get() -> &'static EventManager {
        static INSTANCE: spin::once::Once<EventManager> = spin::once::Once::new();
        INSTANCE.call_once(|| EventManager::new())
    }
}

impl EventOps for EventManager {
    fn send_event(&self, event: Event) -> Result<(), EventError> {
        match event.event_type.clone() {
            EventType::Immediate { event_id, priority } => {
                self.deliver_immediate(event, event_id, priority)
            }
            
            EventType::Notification { notification_id, priority } => {
                self.deliver_notification(event, notification_id, priority)
            }
            
            EventType::Subscription { channel_id, create_channel_if_missing } => {
                self.deliver_to_channel(event, &channel_id, create_channel_if_missing)
            }
            
            EventType::Group { group_type, reliable_delivery } => {
                self.deliver_to_group(event, &group_type, reliable_delivery)
            }
        }
    }
    
    fn register_handler(&self, _filter: EventFilter, _handler: EventHandler) -> Result<(), EventError> {
        // TODO: Implement handler registration
        Ok(())
    }
    
    fn subscribe_channel(&self, channel: &str) -> Result<(), EventError> {
        // TODO: Get current task ID from task system
        let current_task_id = 1; // Placeholder
        
        let mut subscriptions = self.subscriptions.lock();
        let channel_subscribers = subscriptions.entry(format!("{}", channel)).or_insert_with(Vec::new);
        
        if !channel_subscribers.contains(&current_task_id) {
            channel_subscribers.push(current_task_id);
        }
        
        Ok(())
    }
    
    fn unsubscribe_channel(&self, channel: &str) -> Result<(), EventError> {
        let current_task_id = 1; // TODO: Get from task system
        
        let mut subscriptions = self.subscriptions.lock();
        if let Some(channel_subscribers) = subscriptions.get_mut(channel) {
            channel_subscribers.retain(|&task_id| task_id != current_task_id);
        }
        
        Ok(())
    }
    
    fn join_group(&self, group_id: GroupId) -> Result<(), EventError> {
        let current_task_id = 1; // TODO: Get from task system
        
        let mut groups = self.groups.lock();
        let group_members = groups.entry(group_id).or_insert_with(Vec::new);
        
        if !group_members.contains(&current_task_id) {
            group_members.push(current_task_id);
        }
        
        Ok(())
    }
    
    fn leave_group(&self, group_id: GroupId) -> Result<(), EventError> {
        let current_task_id = 1; // TODO: Get from task system
        
        let mut groups = self.groups.lock();
        if let Some(group_members) = groups.get_mut(&group_id) {
            group_members.retain(|&task_id| task_id != current_task_id);
        }
        
        Ok(())
    }
    
    fn configure_delivery(&self, config: DeliveryConfig) -> Result<(), EventError> {
        let current_task_id = 1; // TODO: Get from task system
        
        let mut configs = self.configs.lock();
        configs.insert(current_task_id, config);
        
        Ok(())
    }
}

impl EventManager {
    /// Deliver immediate event (force delivery)
    fn deliver_immediate(&self, event: Event, _event_id: u32, _priority: EventPriority) -> Result<(), EventError> {
        match &event.target {
            EventTarget::Task(task_id) => {
                self.deliver_to_task(*task_id, event)
            }
            _ => Err(EventError::InvalidConfiguration),
        }
    }
    
    /// Deliver notification (best-effort)
    fn deliver_notification(&self, event: Event, _notification_id: u32, _priority: EventPriority) -> Result<(), EventError> {
        match &event.target {
            EventTarget::Task(task_id) => {
                self.deliver_to_task(*task_id, event)
            }
            _ => Err(EventError::InvalidConfiguration),
        }
    }
    
    /// Deliver to channel subscribers
    fn deliver_to_channel(&self, event: Event, channel_id: &str, create_if_missing: bool) -> Result<(), EventError> {
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
    fn deliver_to_group(&self, event: Event, group_type: &GroupType, _reliable: bool) -> Result<(), EventError> {
        match group_type {
            GroupType::TaskGroup(group_id) => {
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
            GroupType::AllTasks => {
                // TODO: Deliver to all tasks in the system
                // This would require integration with the task manager
                Err(EventError::Other(format!("AllTasks delivery not implemented")))
            }
            _ => Err(EventError::Other(format!("Group type not implemented"))),
        }
    }
    
    /// Deliver event to a specific task
    fn deliver_to_task(&self, _task_id: u32, event: Event) -> Result<(), EventError> {
        // TODO: This needs integration with the task system
        // For now, just add to event queue
        let mut queue = self.event_queue.lock();
        queue.push(event);
        Ok(())
    }
}

/// Convenience functions for creating events
impl Event {
    /// Create a new immediate event
    pub fn new_immediate(target: u32, event_id: u32) -> Self {
        Self {
            event_type: EventType::Immediate {
                event_id,
                priority: EventPriority::High,
            },
            target: EventTarget::Task(target),
            payload: EventPayload::Empty,
            metadata: EventMetadata::new(),
        }
    }
    
    /// Create a new notification event
    pub fn new_notification(target: u32, notification_id: u32) -> Self {
        Self {
            event_type: EventType::Notification {
                notification_id,
                priority: EventPriority::Normal,
            },
            target: EventTarget::Task(target),
            payload: EventPayload::Empty,
            metadata: EventMetadata::new(),
        }
    }
    
    /// Create a new channel event
    pub fn new_channel_event(channel: &str, payload: EventPayload) -> Self {
        Self {
            event_type: EventType::Subscription {
                channel_id: format!("{}", channel),
                create_channel_if_missing: false,
            },
            target: EventTarget::Channel(format!("{}", channel)),
            payload,
            metadata: EventMetadata::new(),
        }
    }
    
    /// Create a new group broadcast event
    pub fn new_group_broadcast(group_type: GroupType, payload: EventPayload) -> Self {
        Self {
            event_type: EventType::Group {
                group_type: group_type.clone(),
                reliable_delivery: false,
            },
            target: match group_type {
                GroupType::TaskGroup(id) => EventTarget::Group(id),
                _ => EventTarget::Broadcast,
            },
            payload,
            metadata: EventMetadata::new(),
        }
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
    EventManager::get()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Test creating basic events
    #[test_case]
    fn test_event_creation() {
        // Test immediate event creation
        let immediate_event = Event::new_immediate(1, event_ids::EVENT_TERMINATE);
        assert!(matches!(immediate_event.event_type, EventType::Immediate { .. }));
        assert!(matches!(immediate_event.target, EventTarget::Task(1)));
        assert!(matches!(immediate_event.payload, EventPayload::Empty));

        // Test notification event creation
        let notification_event = Event::new_notification(2, event_ids::NOTIFICATION_TASK_COMPLETED);
        assert!(matches!(notification_event.event_type, EventType::Notification { .. }));
        assert!(matches!(notification_event.target, EventTarget::Task(2)));

        // Test channel event creation
        let channel_event = Event::new_channel_event("test_channel", EventPayload::String("test".into()));
        assert!(matches!(channel_event.event_type, EventType::Subscription { .. }));
        assert!(matches!(channel_event.target, EventTarget::Channel(_)));
        assert!(matches!(channel_event.payload, EventPayload::String(_)));

        // Test group broadcast event creation
        let group_event = Event::new_group_broadcast(
            GroupType::TaskGroup(100), 
            EventPayload::Integer(42)
        );
        assert!(matches!(group_event.event_type, EventType::Group { .. }));
        assert!(matches!(group_event.target, EventTarget::Group(100)));
        assert!(matches!(group_event.payload, EventPayload::Integer(42)));
    }

    /// Test event manager singleton
    #[test_case]
    fn test_event_manager_singleton() {
        let manager1 = EventManager::get();
        let manager2 = EventManager::get();
        
        // Should return the same instance
        assert!(core::ptr::eq(manager1, manager2));
    }

    /// Test event subscription operations
    #[test_case]
    fn test_event_subscription() {
        let manager = EventManager::get();
        
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
        let manager = EventManager::get();
        
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
        let manager = EventManager::get();
        
        // Test immediate event sending
        let immediate_event = Event::new_immediate(1, event_ids::EVENT_TERMINATE);
        let result = manager.send_event(immediate_event);
        assert!(result.is_ok());
        
        // Test notification event sending
        let notification_event = Event::new_notification(2, event_ids::NOTIFICATION_TASK_COMPLETED);
        let result = manager.send_event(notification_event);
        assert!(result.is_ok());
        
        // Test channel event sending - first subscribe to channel
        let _ = manager.subscribe_channel("test_channel");
        let channel_event = Event::new_channel_event("test_channel", EventPayload::Bytes(vec![1, 2, 3]));
        let result = manager.send_event(channel_event);
        assert!(result.is_ok());
        
        // Test channel event with create_if_missing=true
        let channel_event_with_create = Event {
            event_type: EventType::Subscription {
                channel_id: "new_channel".into(),
                create_channel_if_missing: true,
            },
            target: EventTarget::Channel("new_channel".into()),
            payload: EventPayload::String("test".into()),
            metadata: EventMetadata::new(),
        };
        let result = manager.send_event(channel_event_with_create);
        assert!(result.is_ok());
        
        // Test group event sending
        let group_event = Event::new_group_broadcast(
            GroupType::AllTasks, 
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
            matches!(event.event_type, EventType::Immediate { .. })
        });
        
        // Create test event
        let test_event = Event::new_immediate(1, event_ids::EVENT_TERMINATE);
        
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
        
        // Test forward handler
        let _forward_handler = EventHandler::Forward(EventTarget::Task(42));
        
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
        let _task_group = GroupType::TaskGroup(100);
        let _all_tasks = GroupType::AllTasks;
        let _session = GroupType::Session(200);
        let _custom = GroupType::Custom("custom_group".into());
        
        // Test cloning
        let original = GroupType::Custom("test".into());
        let cloned = original.clone();
        
        if let (GroupType::Custom(orig), GroupType::Custom(clone)) = (original, cloned) {
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
        let manager = EventManager::get();
        
        // Test sending to non-existent channel without create_if_missing
        let channel_event = Event::new_channel_event("nonexistent_channel", EventPayload::Empty);
        let result = manager.send_event(channel_event);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), EventError::ChannelNotFound));
        
        // Test sending to non-existent group
        let group_event = Event::new_group_broadcast(
            GroupType::TaskGroup(9999), 
            EventPayload::Empty
        );
        let result = manager.send_event(group_event);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), EventError::GroupNotFound));
        
        // Test AllTasks delivery (not implemented)
        let all_tasks_event = Event::new_group_broadcast(
            GroupType::AllTasks, 
            EventPayload::Empty
        );
        let result = manager.send_event(all_tasks_event);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), EventError::Other(_)));
    }
}
