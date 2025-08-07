//! Event-based Inter-Process Communication
//! 
//! This module provides a unified event system for Scarlet OS that handles
//! different types of event delivery mechanisms:
//! - Immediate: Force delivery regardless of receiver state
//! - Notification: One-way, best-effort delivery
//! - Subscription: Channel-based pub/sub delivery
//! - Group: Broadcast delivery to multiple targets

use alloc::{string::String, vec::Vec, sync::Arc, format, collections::VecDeque};
use hashbrown::HashMap;
use alloc::collections::BTreeMap;
use spin::Mutex;

/// Type alias for task identifiers
pub type TaskId = u32;
/// Type alias for group identifiers
pub type GroupId = u32;
/// Type alias for session identifiers
pub type SessionId = u32;

/// Event structure containing all event information
/// 
/// # Design Philosophy
/// 
/// This design separates **delivery mechanism** from **event content**:
/// - `delivery`: HOW the event is delivered (direct, channel, group, broadcast)
/// - `content`: WHAT the event represents (signal, message, notification)
/// - `payload`: Additional data carried with the event
/// - `metadata`: System-level tracking information
#[derive(Debug, Clone)]
pub struct Event {
    /// Event delivery mechanism (routing and targeting)
    pub delivery: EventDelivery,
    
    /// Event content (what this event represents)
    pub content: EventContent,
    
    /// Event payload data (additional data)
    pub payload: EventPayload,
    
    /// Event metadata (system tracking)
    pub metadata: EventMetadata,
}

/// Event delivery mechanisms
/// 
/// Defines HOW an event is delivered, independent of WHAT the event represents
#[derive(Debug, Clone)]
pub enum EventDelivery {
    /// Direct task communication (1:1)
    Direct {
        target: TaskId,
        priority: EventPriority,
        reliable: bool,
    },
    
    /// Channel-based communication (1:many, pub/sub)
    Channel {
        channel_id: String,
        create_if_missing: bool,
        priority: EventPriority,
    },
    
    /// Group broadcast (1:many, membership-based)
    Group {
        group_target: GroupTarget,
        priority: EventPriority,
        reliable: bool,
    },
    
    /// System-wide broadcast (1:all)
    Broadcast {
        priority: EventPriority,
        reliable: bool,
    },
}

/// Event content types
/// 
/// Defines WHAT the event represents, independent of HOW it's delivered
#[derive(Debug, Clone)]
pub enum EventContent {
    /// Process control events (equivalent to signals, but OS-agnostic)
    ProcessControl(ProcessControlType),
    
    /// Application-level message with type
    Message {
        message_type: u32,
        category: MessageCategory,
    },
    
    /// System notification
    Notification(NotificationType),
    
    /// Custom event defined by ABI or application
    Custom {
        namespace: String,  // e.g., "linux", "xv6", "user_app_123"
        event_id: u32,
    },
}

/// Process control event types
/// 
/// These represent universal process control operations that exist across
/// different operating systems (Linux signals, Windows events, etc.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessControlType {
    Terminate,          // Graceful termination
    Kill,              // Force termination
    Stop,              // Suspend execution
    Continue,          // Resume execution
    Interrupt,         // User interrupt (Ctrl+C)
    Quit,              // Quit with core dump
    Hangup,            // Terminal hangup
    ChildExit,         // Child process exited
    PipeBroken,        // Broken pipe
    Alarm,             // Timer alarm
    IoReady,           // I/O ready
    User1,             // User-defined 1
    User2,             // User-defined 2
    // Add more as needed
}

/// Message categories (for structured communication)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageCategory {
    Control,           // Control messages
    Data,              // Data messages
    Status,            // Status updates
    Error,             // Error notifications
    Custom(u8),        // Custom category (0-255)
}

/// System notification types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationType {
    TaskCompleted,
    MemoryLow,
    DeviceConnected,
    DeviceDisconnected,
    FilesystemFull,
    NetworkChange,
    SystemShutdown,
    // Add more as needed
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
    
    /// Create new metadata with specified priority
    pub fn with_priority(priority: EventPriority) -> Self {
        Self {
            sender: None, // Will be filled by EventManager
            priority,
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

impl EventFilter {
    /// Check if this filter matches the given event
    pub fn matches(&self, event: &Event) -> bool {
        match self {
            EventFilter::All => true,
            
            EventFilter::EventType(type_filter) => {
                match type_filter {
                    EventTypeFilter::AnyDirect => matches!(event.delivery, EventDelivery::Direct { .. }),
                    EventTypeFilter::AnyChannel => matches!(event.delivery, EventDelivery::Channel { .. }),
                    EventTypeFilter::AnyGroup => matches!(event.delivery, EventDelivery::Group { .. }),
                    EventTypeFilter::AnyBroadcast => matches!(event.delivery, EventDelivery::Broadcast { .. }),
                    
                    EventTypeFilter::Direct(content_id) => {
                        if let EventDelivery::Direct { .. } = &event.delivery {
                            // Check if content matches the expected ID
                            match &event.content {
                                EventContent::ProcessControl(ptype) => {
                                    // Map ProcessControlType to ID for filtering
                                    let type_id = match ptype {
                                        ProcessControlType::Terminate => 1,
                                        ProcessControlType::Kill => 2,
                                        ProcessControlType::Stop => 3,
                                        ProcessControlType::Continue => 4,
                                        ProcessControlType::Interrupt => 7,
                                        _ => 0,
                                    };
                                    type_id == *content_id
                                }
                                EventContent::Custom { event_id, .. } => *event_id == *content_id,
                                _ => false,
                            }
                        } else {
                            false
                        }
                    }
                    
                    EventTypeFilter::Channel(channel_name) => {
                        if let EventDelivery::Channel { channel_id, .. } = &event.delivery {
                            channel_id == channel_name
                        } else {
                            false
                        }
                    }
                    
                    EventTypeFilter::Group(group_id) => {
                        if let EventDelivery::Group { group_target: GroupTarget::TaskGroup(id), .. } = &event.delivery {
                            id == group_id
                        } else {
                            false
                        }
                    }
                    
                    EventTypeFilter::Broadcast(event_id) => {
                        if let EventDelivery::Broadcast { .. } = &event.delivery {
                            // Check if event content matches the broadcast ID
                            match &event.content {
                                EventContent::Custom { event_id: id, .. } => id == event_id,
                                _ => event.metadata.event_id == *event_id as u64,
                            }
                        } else {
                            false
                        }
                    }
                }
            }
            
            EventFilter::EventId(event_id) => {
                // Check event_id in metadata
                event.metadata.event_id == *event_id as u64
            }
            
            EventFilter::Channel(channel_name) => {
                if let EventDelivery::Channel { channel_id, .. } = &event.delivery {
                    channel_id == channel_name
                } else {
                    false
                }
            }
            
            EventFilter::Sender(sender_id) => {
                event.metadata.sender == Some(*sender_id)
            }
            
            EventFilter::Custom(filter_fn) => {
                filter_fn(event)
            }
        }
    }
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

/// Task-specific event queue entry
#[derive(Debug, Clone)]
pub struct TaskEventQueue {
    /// Events sorted by priority (higher priority first)
    events: BTreeMap<EventPriority, VecDeque<Event>>,
    /// Total count of queued events
    total_count: usize,
}

impl TaskEventQueue {
    pub fn new() -> Self {
        Self {
            events: BTreeMap::new(),
            total_count: 0,
        }
    }
    
    /// Add event to queue, returns true if this was the first event (0->1 transition)
    fn enqueue(&mut self, event: Event) -> bool {
        let was_empty = self.total_count == 0;
        let priority = event.metadata.priority;
        
        self.events.entry(priority)
            .or_insert_with(VecDeque::new)
            .push_back(event);
        self.total_count += 1;
        
        was_empty
    }
    
    /// Dequeue highest priority event
    pub fn dequeue(&mut self) -> Option<Event> {
        // Find the highest priority (largest value) that has events
        // BTreeMap iterates in ascending order by default, so we need to find the max
        let priority_to_dequeue = {
            self.events.iter()
                .filter(|(_, queue)| !queue.is_empty())
                .map(|(&priority, _)| priority)
                .max()
        }?;
        
        // Dequeue from the highest priority queue
        if let Some(queue) = self.events.get_mut(&priority_to_dequeue) {
            if let Some(event) = queue.pop_front() {
                self.total_count -= 1;
                if queue.is_empty() {
                    self.events.remove(&priority_to_dequeue);
                }
                return Some(event);
            }
        }
        
        None
    }
    
    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        self.total_count == 0
    }
    
    /// Get total number of queued events
    pub fn len(&self) -> usize {
        self.total_count
    }
}

/// Event Manager - Main implementation of the event system
pub struct EventManager {
    /// Channel subscriptions
    subscriptions: Mutex<HashMap<String, Vec<u32>>>,
    
    /// Task group memberships
    groups: Mutex<HashMap<GroupId, Vec<u32>>>,
    
    /// Delivery configurations per task
    configs: Mutex<HashMap<u32, DeliveryConfig>>,
    
    /// Task-specific event filters
    task_filters: Mutex<HashMap<u32, Vec<EventFilter>>>,
    
    /// Next event ID
    next_event_id: Mutex<u64>,
    
    /// Handle-based channel registry for KernelObject integration
    handle_channels: Mutex<HashMap<String, Arc<crate::ipc::event_objects::EventChannel>>>,
}

impl EventManager {
    /// Create a new EventManager
    pub fn new() -> Self {
        Self {
            subscriptions: Mutex::new(HashMap::new()),
            groups: Mutex::new(HashMap::new()),
            configs: Mutex::new(HashMap::new()),
            task_filters: Mutex::new(HashMap::new()),
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
        match event.delivery.clone() {
            EventDelivery::Direct { target, priority, reliable } => {
                self.deliver_direct(event, target, priority, reliable)
            }
            
            EventDelivery::Channel { channel_id, create_if_missing, priority } => {
                self.deliver_to_channel(event, &channel_id, create_if_missing, priority)
            }
            
            EventDelivery::Group { group_target, priority, reliable } => {
                self.deliver_to_group(event, &group_target, priority, reliable)
            }
            
            EventDelivery::Broadcast { priority, reliable } => {
                self.deliver_broadcast(event, priority, reliable)
            }
        }
    }
    
    /// Register an event filter for a task
    pub fn register_filter(&self, task_id: u32, filter: EventFilter) -> Result<(), EventError> {
        let mut task_filters = self.task_filters.lock();
        let filters = task_filters.entry(task_id).or_insert_with(Vec::new);
        filters.push(filter);
        Ok(())
    }
    
    /// Remove all filters for a task
    pub fn clear_filters(&self, task_id: u32) -> Result<(), EventError> {
        let mut task_filters = self.task_filters.lock();
        task_filters.remove(&task_id);
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
    fn deliver_direct(&self, event: Event, target: TaskId, _priority: EventPriority, _reliable: bool) -> Result<(), EventError> {
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
    fn deliver_broadcast(&self, event: Event, _priority: EventPriority, _reliable: bool) -> Result<(), EventError> {
        // TODO: Deliver to all tasks in the system
        // This would require integration with the task manager
        let _ = event; // Suppress unused warning for now
        Err(EventError::Other(format!("Broadcast delivery not implemented")))
    }
    
    /// Deliver event to a specific task
    #[cfg(not(test))]
    pub fn deliver_to_task(&self, task_id: u32, event: Event) -> Result<(), EventError> {
        // Check if the event matches any of the task's filters
        let task_filters = self.task_filters.lock();
        if let Some(filters) = task_filters.get(&task_id) {
            // If task has filters, check if event matches any of them
            if !filters.is_empty() {
                let matches = filters.iter().any(|filter| filter.matches(&event));
                if !matches {
                    // Event doesn't match any filter, drop it
                    return Ok(());
                }
            }
            // If no filters are registered, allow all events (backward compatibility)
        }
        drop(task_filters); // Release the lock early

        // Get the task and deliver event to its local queue
        if let Some(task) = crate::sched::scheduler::get_scheduler().get_task_by_id(task_id as usize) {
            // Enqueue the event since it passed filtering
            let mut queue = task.event_queue.lock();
            queue.enqueue(event);
            Ok(())
        } else {
            Err(EventError::TargetNotFound)
        }
    }
    #[cfg(test)]
    pub fn deliver_to_task(&self, _task_id: u32, _event: Event) -> Result<(), EventError> {
        // In tests, we simulate event delivery by simply returning success
        // Real integration tests should be done at a higher level with actual Task objects
        Ok(())
    }
    
    /// Dequeue the next highest priority event for a task
    /// This method is deprecated - tasks now process events directly via process_pending_events()
    #[deprecated(note = "Use Task.process_pending_events() instead")]
    pub fn dequeue_event_for_task(&self, task_id: u32) -> Option<Event> {
        if let Some(task) = crate::sched::scheduler::get_scheduler().get_task_by_id(task_id as usize) {
            let mut queue = task.event_queue.lock();
            queue.dequeue()
        } else {
            None
        }
    }
    
    /// Get the number of pending events for a task
    pub fn get_pending_event_count(&self, task_id: u32) -> usize {
        if let Some(task) = crate::sched::scheduler::get_scheduler().get_task_by_id(task_id as usize) {
            let queue = task.event_queue.lock();
            queue.len()
        } else {
            0
        }
    }
    
    /// Check if a task has any pending events
    pub fn has_pending_events(&self, task_id: u32) -> bool {
        if let Some(task) = crate::sched::scheduler::get_scheduler().get_task_by_id(task_id as usize) {
            let queue = task.event_queue.lock();
            !queue.is_empty()
        } else {
            false
        }
    }
}

/// Convenience functions for creating events
impl Event {
    /// Create a new event with delivery, content, and payload
    pub fn new(delivery: EventDelivery, content: EventContent, payload: EventPayload) -> Self {
        // Extract priority from delivery mechanism
        let priority = match &delivery {
            EventDelivery::Direct { priority, .. } => *priority,
            EventDelivery::Channel { priority, .. } => *priority,
            EventDelivery::Group { priority, .. } => *priority,
            EventDelivery::Broadcast { priority, .. } => *priority,
        };
        
        Self {
            delivery,
            content,
            payload,
            metadata: EventMetadata::with_priority(priority),
        }
    }
    
    /// Create a direct process control event to a specific task
    pub fn direct_process_control(target: TaskId, ptype: ProcessControlType, priority: EventPriority, reliable: bool) -> Self {
        Self::new(
            EventDelivery::Direct { target, priority, reliable },
            EventContent::ProcessControl(ptype),
            EventPayload::Empty,
        )
    }
    
    /// Create a direct custom event to a specific task
    pub fn direct_custom(target: TaskId, namespace: String, event_id: u32, priority: EventPriority, reliable: bool, payload: EventPayload) -> Self {
        Self::new(
            EventDelivery::Direct { target, priority, reliable },
            EventContent::Custom { namespace, event_id },
            payload,
        )
    }
    
    /// Create a channel event
    pub fn channel(channel_id: String, content: EventContent, create_if_missing: bool, priority: EventPriority, payload: EventPayload) -> Self {
        Self::new(
            EventDelivery::Channel { channel_id, create_if_missing, priority },
            content,
            payload,
        )
    }
    
    /// Create a group event
    pub fn group(group_target: GroupTarget, content: EventContent, priority: EventPriority, reliable: bool, payload: EventPayload) -> Self {
        Self::new(
            EventDelivery::Group { group_target, priority, reliable },
            content,
            payload,
        )
    }
    
    /// Create a broadcast event
    pub fn broadcast(content: EventContent, priority: EventPriority, reliable: bool, payload: EventPayload) -> Self {
        Self::new(
            EventDelivery::Broadcast { priority, reliable },
            content,
            payload,
        )
    }
    
    // Convenience methods for common use cases
    
    /// Create immediate process control event for a specific task
    pub fn immediate_process_control(task_id: u32, ptype: ProcessControlType) -> Self {
        Self::direct_process_control(task_id, ptype, EventPriority::High, true)
    }
    
    /// Create notification event for a specific task
    pub fn notification_to_task(task_id: u32, ntype: NotificationType) -> Self {
        Self::new(
            EventDelivery::Direct { target: task_id, priority: EventPriority::Normal, reliable: false },
            EventContent::Notification(ntype),
            EventPayload::Empty,
        )
    }
    
    /// Create channel event (simple)
    pub fn new_channel_event(channel: &str, content: EventContent, payload: EventPayload) -> Self {
        Self::channel(channel.into(), content, false, EventPriority::Normal, payload)
    }
    
    /// Create group broadcast event (simple)
    pub fn new_group_broadcast(group_target: GroupTarget, content: EventContent, payload: EventPayload) -> Self {
        Self::group(group_target, content, EventPriority::Normal, false, payload)
    }
    
    /// Create immediate broadcast event
    pub fn immediate_broadcast(content: EventContent) -> Self {
        Self::broadcast(content, EventPriority::High, true, EventPayload::Empty)
    }
    
    /// Create notification for a group
    pub fn notification_to_group(group_id: GroupId, ntype: NotificationType) -> Self {
        Self::group(
            GroupTarget::TaskGroup(group_id), 
            EventContent::Notification(ntype),
            EventPriority::Normal, 
            false, 
            EventPayload::Empty
        )
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
    //! # Scarlet Event ID Design Philosophy
    //! 
    //! Scarlet provides a unified event numbering scheme that allows ABI modules
    //! to map OS-specific signals/events to Scarlet's abstract event types.
    //! 
    //! ## ID Ranges:
    //! - **1-99**: Core system events (universal across all OSes)
    //! - **100-999**: Common notifications (device, memory, etc.)
    //! - **1000-9999**: Reserved for future system expansion
    //! - **10000-19999**: ABI-specific custom events
    //! - **20000+**: User-defined events
    //! 
    //! ## ABI Mapping Examples:
    //! ```
    //! // Linux ABI module maps:
    //! SIGTERM (15) -> EVENT_TERMINATE (1)
    //! SIGKILL (9)  -> EVENT_FORCE_TERMINATE (2)
    //! SIGINT (2)   -> EVENT_INTERRUPT (defined in ABI)
    //! 
    //! // xv6 ABI module maps:
    //! XV6_KILL     -> EVENT_FORCE_TERMINATE (2)
    //! XV6_STOP     -> EVENT_STOP (3)
    //! ```
    
    /// Process control events (1-99)
    /// These are universal concepts that exist in all operating systems
    pub const EVENT_TERMINATE: u32 = 1;        // Graceful termination request
    pub const EVENT_FORCE_TERMINATE: u32 = 2;  // Forced termination (uncatchable)
    pub const EVENT_STOP: u32 = 3;             // Suspend/pause execution
    pub const EVENT_CONTINUE: u32 = 4;         // Resume execution
    pub const EVENT_CHILD_EXIT: u32 = 5;       // Child process exited
    pub const SYSTEM_SHUTDOWN: u32 = 6;        // System shutdown signal
    pub const EVENT_INTERRUPT: u32 = 7;        // User interrupt (Ctrl+C)
    pub const EVENT_QUIT: u32 = 8;             // Quit with core dump
    pub const EVENT_KILL: u32 = 9;             // Alias for FORCE_TERMINATE
    
    /// Process group and session events (10-29)
    pub const EVENT_HANGUP: u32 = 10;          // Terminal hangup
    pub const EVENT_TERMINAL_STOP: u32 = 11;   // Terminal stop signal
    pub const EVENT_TERMINAL_CONTINUE: u32 = 12; // Terminal continue signal
    
    /// I/O and resource events (30-49)
    pub const EVENT_PIPE_BROKEN: u32 = 30;     // Broken pipe
    pub const EVENT_ALARM: u32 = 31;           // Timer alarm
    pub const EVENT_IO_READY: u32 = 32;        // I/O ready for operation
    pub const EVENT_URGENT_DATA: u32 = 33;     // Urgent data available
    
    /// User-defined standard events (50-99)
    pub const EVENT_USER1: u32 = 50;           // User-defined signal 1
    pub const EVENT_USER2: u32 = 51;           // User-defined signal 2
    
    /// System notification events (100-999)
    pub const NOTIFICATION_TASK_COMPLETED: u32 = 100;
    pub const NOTIFICATION_MEMORY_LOW: u32 = 101;
    pub const NOTIFICATION_DEVICE_CONNECTED: u32 = 102;
    pub const NOTIFICATION_DEVICE_DISCONNECTED: u32 = 103;
    pub const NOTIFICATION_FILESYSTEM_FULL: u32 = 104;
    pub const NOTIFICATION_NETWORK_CHANGE: u32 = 105;
    
    /// Hardware and architecture events (200-299)
    pub const EVENT_SEGMENTATION_FAULT: u32 = 200;
    pub const EVENT_ILLEGAL_INSTRUCTION: u32 = 201;
    pub const EVENT_FLOATING_POINT_ERROR: u32 = 202;
    pub const EVENT_BUS_ERROR: u32 = 203;
    
    /// Base ranges for ABI and custom events
    pub const EVENT_RESERVED_MAX: u32 = 9999;      // End of reserved range
    pub const EVENT_CUSTOM_BASE: u32 = 10000;      // Start of ABI-specific events
    pub const EVENT_USER_BASE: u32 = 20000;        // Start of user-defined events
    
    /// Maximum event ID value (for validation)
    pub const EVENT_MAX: u32 = u32::MAX - 1;
}

/// ABI Event Mapping Utilities
/// 
/// These functions help ABI modules map between OS-specific signals/events
/// and Scarlet's universal event ID scheme.
pub mod abi_mapping {
    use super::event_ids;
    
    /// Convert Linux signal number to Scarlet event ID
    /// Used by the Linux ABI module
    pub fn linux_signal_to_event_id(signal: i32) -> Option<u32> {
        match signal {
            1 => Some(event_ids::EVENT_HANGUP),         // SIGHUP
            2 => Some(event_ids::EVENT_INTERRUPT),      // SIGINT
            3 => Some(event_ids::EVENT_QUIT),           // SIGQUIT
            9 => Some(event_ids::EVENT_FORCE_TERMINATE), // SIGKILL
            13 => Some(event_ids::EVENT_PIPE_BROKEN),   // SIGPIPE
            14 => Some(event_ids::EVENT_ALARM),         // SIGALRM
            15 => Some(event_ids::EVENT_TERMINATE),     // SIGTERM
            17 => Some(event_ids::EVENT_CHILD_EXIT),    // SIGCHLD
            18 => Some(event_ids::EVENT_CONTINUE),      // SIGCONT
            19 => Some(event_ids::EVENT_STOP),          // SIGSTOP
            20 => Some(event_ids::EVENT_TERMINAL_STOP), // SIGTSTP
            10 => Some(event_ids::EVENT_USER1),         // SIGUSR1
            12 => Some(event_ids::EVENT_USER2),         // SIGUSR2
            _ => None, // ABI-specific handling required
        }
    }
    
    /// Convert Scarlet event ID to Linux signal number
    /// Used by the Linux ABI module for signal delivery
    pub fn event_id_to_linux_signal(event_id: u32) -> Option<i32> {
        match event_id {
            event_ids::EVENT_HANGUP => Some(1),            // SIGHUP
            event_ids::EVENT_INTERRUPT => Some(2),         // SIGINT
            event_ids::EVENT_QUIT => Some(3),              // SIGQUIT
            event_ids::EVENT_FORCE_TERMINATE => Some(9),   // SIGKILL
            event_ids::EVENT_PIPE_BROKEN => Some(13),      // SIGPIPE
            event_ids::EVENT_ALARM => Some(14),            // SIGALRM
            event_ids::EVENT_TERMINATE => Some(15),        // SIGTERM
            event_ids::EVENT_CHILD_EXIT => Some(17),       // SIGCHLD
            event_ids::EVENT_CONTINUE => Some(18),         // SIGCONT
            event_ids::EVENT_STOP => Some(19),             // SIGSTOP
            event_ids::EVENT_TERMINAL_STOP => Some(20),    // SIGTSTP
            event_ids::EVENT_USER1 => Some(10),            // SIGUSR1
            event_ids::EVENT_USER2 => Some(12),            // SIGUSR2
            _ => None, // No direct Linux equivalent
        }
    }
    
    /// Check if an event ID is in the ABI-specific range
    pub fn is_abi_specific_event(event_id: u32) -> bool {
        event_id >= event_ids::EVENT_CUSTOM_BASE && event_id < event_ids::EVENT_USER_BASE
    }
    
    /// Check if an event ID is user-defined
    pub fn is_user_defined_event(event_id: u32) -> bool {
        event_id >= event_ids::EVENT_USER_BASE
    }
    
    /// Validate event ID range
    pub fn is_valid_event_id(event_id: u32) -> bool {
        event_id > 0 && event_id <= event_ids::EVENT_MAX
    }
}
