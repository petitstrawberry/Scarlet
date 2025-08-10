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
    User(u32),         // User-defined control signal (0-65535)
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
#[derive(Debug, Clone, PartialEq)]
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
    /// Task group memberships
    groups: Mutex<HashMap<GroupId, Vec<u32>>>,
    /// Session memberships
    sessions: Mutex<HashMap<SessionId, Vec<u32>>>,
    /// Named/custom group memberships
    named_groups: Mutex<HashMap<String, Vec<u32>>>,
    
    /// Delivery configurations per task
    configs: Mutex<HashMap<u32, DeliveryConfig>>,
    
    /// Task-specific event filters (handler_id, filter)
    task_filters: Mutex<HashMap<u32, Vec<(usize, EventFilter)>>>,
    
    /// Next event ID
    #[allow(dead_code)]
    next_event_id: Mutex<u64>,
    
    /// Channel registry - EventManager only manages channels, channels manage their own subscriptions
    channels: Mutex<HashMap<String, Arc<EventChannelObject>>>,
}

impl EventManager {
    /// Create a new EventManager
    pub fn new() -> Self {
        Self {
            groups: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            named_groups: Mutex::new(HashMap::new()),
            configs: Mutex::new(HashMap::new()),
            task_filters: Mutex::new(HashMap::new()),
            next_event_id: Mutex::new(1),
            channels: Mutex::new(HashMap::new()),
        }
    }
    
    /// Get the global EventManager instance
    pub fn get_manager() -> &'static EventManager {
        static INSTANCE: spin::once::Once<EventManager> = spin::once::Once::new();
        INSTANCE.call_once(|| EventManager::new())
    }
    
    /// Helper: get the currently running task id, if available
    fn get_current_task_id(&self) -> Option<u32> {
        #[cfg(test)]
        {
            // In unit tests, there is no real scheduler context
            return Some(1);
        }
        #[cfg(not(test))]
        {
            let cpu = crate::arch::get_cpu();
            let cpu_id = cpu.get_cpuid();
            let sched = crate::sched::scheduler::get_scheduler();
            if let Some(task) = sched.get_current_task(cpu_id) {
                return Some(task.get_id() as u32);
            }
            None
        }
    }
    
    /// Create or get an event channel as a KernelObject handle
    /// 
    /// This method creates an EventChannel that can be inserted into a HandleTable,
    /// providing consistent resource management with other kernel objects.
    pub fn create_channel(&self, name: String) -> crate::object::KernelObject {
        let mut channels = self.channels.lock();
        
        let channel = channels
            .entry(name.clone())
            .or_insert_with(|| {
                Arc::new(EventChannelObject::new(name.clone()))
            })
            .clone();
        
        crate::object::KernelObject::EventChannel(channel)
    }
    
    /// Create a subscription to a channel as a KernelObject handle
    /// 
    /// This method creates an EventSubscription that can be inserted into a HandleTable,
    /// allowing tasks to receive events through the standard handle interface.
    pub fn create_subscription(&self, channel_name: String, task_id: u32) -> Result<crate::object::KernelObject, EventError> {
        // Get or create the channel first
        let mut channels = self.channels.lock();
        let channel = channels
            .entry(channel_name.clone())
            .or_insert_with(|| {
                Arc::new(EventChannelObject::new(channel_name.clone()))
            })
            .clone();
        drop(channels);
        
        // Create subscription through the channel
        let subscription = channel.create_subscription(task_id)?;
        Ok(crate::object::KernelObject::EventSubscription(subscription))
    }
    
    /// Send an event
    pub fn send_event(&self, mut event: Event) -> Result<(), EventError> {
        // Auto-fill metadata: sender and timestamp
        if event.metadata.sender.is_none() {
            event.metadata.sender = self.get_current_task_id();
        }
        // Use kernel timer tick as timestamp source
        event.metadata.timestamp = crate::timer::get_tick();

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
    
    /// Register an event filter for a task (without handler id)
    pub fn register_filter(&self, task_id: u32, filter: EventFilter) -> Result<(), EventError> {
        let mut task_filters = self.task_filters.lock();
        let filters = task_filters.entry(task_id).or_insert_with(Vec::new);
        // Assign a simple handler id: next index
        let handler_id = filters.len();
        filters.push((handler_id, filter));
        Ok(())
    }

    /// Register an event filter for a task with explicit handler id
    pub fn register_filter_with_id(&self, task_id: u32, handler_id: usize, filter: EventFilter) -> Result<(), EventError> {
        let mut task_filters = self.task_filters.lock();
        let filters = task_filters.entry(task_id).or_insert_with(Vec::new);
        // If an entry with same handlerId exists, replace it
        if let Some(slot) = filters.iter_mut().find(|(hid, _)| *hid == handler_id) {
            *slot = (handler_id, filter);
        } else {
            filters.push((handler_id, filter));
        }
        Ok(())
    }
    
    /// Unregister a filter by handler id
    pub fn unregister_filter_by_id(&self, task_id: u32, handler_id: usize) -> Result<(), EventError> {
        let mut task_filters = self.task_filters.lock();
        if let Some(filters) = task_filters.get_mut(&task_id) {
            filters.retain(|(hid, _)| *hid != handler_id);
        }
        Ok(())
    }
    
    /// Get a snapshot of filters for a task
    pub fn get_filters_for_task(&self, task_id: u32) -> Vec<(usize, EventFilter)> {
        let task_filters = self.task_filters.lock();
        task_filters.get(&task_id).cloned().unwrap_or_default()
    }
    
    /// Remove all filters for a task
    pub fn clear_filters(&self, task_id: u32) -> Result<(), EventError> {
        let mut task_filters = self.task_filters.lock();
        task_filters.remove(&task_id);
        Ok(())
    }
    
    /// Subscribe to a channel
    pub fn subscribe_channel(&self, channel: &str) -> Result<(), EventError> {
        // Resolve current task ID from scheduler
        let current_task_id = self
            .get_current_task_id()
            .ok_or_else(|| EventError::Other("No current task".into()))?;
        
        // Get or create the channel
        let mut channels = self.channels.lock();
        let channel_obj = channels
            .entry(channel.into())
            .or_insert_with(|| {
                Arc::new(EventChannelObject::new(channel.into()))
            })
            .clone();
        drop(channels);
        
        // Subscribe through the channel (ignore the returned subscription object for backward compatibility)
        let _ = channel_obj.subscribe(current_task_id)?;
        Ok(())
    }
    
    /// Unsubscribe from a channel
    pub fn unsubscribe_channel(&self, channel: &str) -> Result<(), EventError> {
        let current_task_id = self
            .get_current_task_id()
            .ok_or_else(|| EventError::Other("No current task".into()))?;
        
        let channels = self.channels.lock();
        if let Some(channel_obj) = channels.get(channel) {
            channel_obj.unsubscribe(current_task_id)
        } else {
            Err(EventError::ChannelNotFound)
        }
    }
    
    /// Join a task group
    pub fn join_group(&self, group_id: GroupId) -> Result<(), EventError> {
        let current_task_id = self
            .get_current_task_id()
            .ok_or_else(|| EventError::Other("No current task".into()))?;
        
        let mut groups = self.groups.lock();
        let group_members = groups.entry(group_id).or_insert_with(Vec::new);
        
        if !group_members.contains(&current_task_id) {
            group_members.push(current_task_id);
        }
        
        Ok(())
    }
    
    /// Leave a task group
    pub fn leave_group(&self, group_id: GroupId) -> Result<(), EventError> {
        let current_task_id = self
            .get_current_task_id()
            .ok_or_else(|| EventError::Other("No current task".into()))?;
        
        let mut groups = self.groups.lock();
        if let Some(group_members) = groups.get_mut(&group_id) {
            group_members.retain(|&task_id| task_id != current_task_id);
        }
        
        Ok(())
    }
    
    /// Join a session group
    pub fn join_session(&self, session_id: SessionId) -> Result<(), EventError> {
        let current_task_id = self
            .get_current_task_id()
            .ok_or_else(|| EventError::Other("No current task".into()))?;
        let mut sessions = self.sessions.lock();
        let members = sessions.entry(session_id).or_insert_with(Vec::new);
        if !members.contains(&current_task_id) {
            members.push(current_task_id);
        }
        Ok(())
    }

    /// Leave a session group
    pub fn leave_session(&self, session_id: SessionId) -> Result<(), EventError> {
        let current_task_id = self
            .get_current_task_id()
            .ok_or_else(|| EventError::Other("No current task".into()))?;
        let mut sessions = self.sessions.lock();
        if let Some(members) = sessions.get_mut(&session_id) {
            members.retain(|&tid| tid != current_task_id);
        }
        Ok(())
    }

    /// Join a named/custom group
    pub fn join_named_group(&self, name: String) -> Result<(), EventError> {
        let current_task_id = self
            .get_current_task_id()
            .ok_or_else(|| EventError::Other("No current task".into()))?;
        let mut named = self.named_groups.lock();
        let members = named.entry(name).or_insert_with(Vec::new);
        if !members.contains(&current_task_id) {
            members.push(current_task_id);
        }
        Ok(())
    }

    /// Leave a named/custom group
    pub fn leave_named_group(&self, name: &str) -> Result<(), EventError> {
        let current_task_id = self
            .get_current_task_id()
            .ok_or_else(|| EventError::Other("No current task".into()))?;
        let mut named = self.named_groups.lock();
        if let Some(members) = named.get_mut(name) {
            members.retain(|&tid| tid != current_task_id);
        }
        Ok(())
    }
    
    /// Configure delivery settings
    pub fn configure_delivery(&self, config: DeliveryConfig) -> Result<(), EventError> {
        let current_task_id = self
            .get_current_task_id()
            .ok_or_else(|| EventError::Other("No current task".into()))?;
        
        let mut configs = self.configs.lock();
        configs.insert(current_task_id, config);
        
        Ok(())
    }

    /// Get a task's delivery configuration or the default if none is set
    fn get_task_config_or_default(&self, task_id: u32) -> DeliveryConfig {
        self.configs
            .lock()
            .get(&task_id)
            .cloned()
            .unwrap_or_else(DeliveryConfig::default)
    }

    /// Handle delivery failures according to the sender's configured policy
    fn handle_delivery_failure(&self, sender: Option<u32>, err: &EventError, event: &Event) {
        // Determine policy from sender's config if available, else default to Log
        let policy = match sender {
            Some(sid) => self.get_task_config_or_default(sid).failure_policy.clone(),
            None => FailurePolicy::Log,
        };

        match policy {
            FailurePolicy::Ignore => { /* do nothing */ }
            FailurePolicy::Log => {
                crate::early_println!(
                    "[EventManager] Delivery failure: {:?}, sender={:?}, delivery={:?}",
                    err,
                    sender,
                    event.delivery
                );
            }
            FailurePolicy::NotifySender => {
                // Best-effort notify the sender without causing recursive failure handling
                if let Some(sid) = sender {
                    let notice = Event::direct_custom(
                        sid,
                        "system".into(),
                        0x1001,
                        EventPriority::Low,
                        false,
                        EventPayload::String(format!("Delivery failed: {:?}", err)),
                    );
                    let _ = self.deliver_to_task(sid, notice);
                } else {
                    // Fall back to logging when there is no sender
                    crate::early_println!("[EventManager] Delivery failure without sender: {:?}", err);
                }
            }
            FailurePolicy::SystemEvent => {
                // For now, log the failure to avoid recursive broadcasts. Can be expanded later.
                crate::early_println!(
                    "[EventManager] SystemEvent policy: delivery failure: {:?}, sender={:?}",
                    err,
                    sender
                );
            }
        }
    }

    // === Internal Event Delivery Methods ===
    
    /// Deliver direct event to specific task
    fn deliver_direct(&self, event: Event, target: TaskId, _priority: EventPriority, _reliable: bool) -> Result<(), EventError> {
        // Attempt delivery; if reliable, retry according to sender's config
        let mut result = self.deliver_to_task(target, event.clone());
        if result.is_err() && _reliable {
            // Determine retry count from sender's config
            let retries = match event.metadata.sender {
                Some(sender) => self.get_task_config_or_default(sender).retry_count,
                None => DeliveryConfig::default().retry_count,
            };
            let mut attempts = 0u32;
            while attempts < retries {
                // Simple immediate retry (no sleep to keep no_std constraints)
                result = self.deliver_to_task(target, event.clone());
                if result.is_ok() { break; }
                attempts += 1;
            }
            if let Err(ref e) = result {
                self.handle_delivery_failure(event.metadata.sender, e, &event);
            }
        } else if let Err(ref e) = result {
            // Not reliable, still honor failure policy for observability
            self.handle_delivery_failure(event.metadata.sender, e, &event);
        }
        result
    }
    
    /// Deliver to channel subscribers
    fn deliver_to_channel(&self, event: Event, channel_id: &str, create_if_missing: bool, _priority: EventPriority) -> Result<(), EventError> {
        let mut channels = self.channels.lock();
        
        if let Some(channel) = channels.get(channel_id) {
            let channel = channel.clone();
            drop(channels);
            // Broadcast and handle per-target failures
            let subscribers = channel.get_subscribers();
            for task_id in subscribers {
                if let Err(e) = self.deliver_to_task(task_id, event.clone()) {
                    self.handle_delivery_failure(event.metadata.sender, &e, &event);
                }
            }
            Ok(())
        } else if create_if_missing {
            // Create empty channel
            let channel = Arc::new(EventChannelObject::new(channel_id.into()));
            channels.insert(channel_id.into(), channel.clone());
            drop(channels);
            let subscribers = channel.get_subscribers();
            for task_id in subscribers {
                if let Err(e) = self.deliver_to_task(task_id, event.clone()) {
                    self.handle_delivery_failure(event.metadata.sender, &e, &event);
                }
            }
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
                    let targets: alloc::vec::Vec<u32> = members.iter().cloned().collect();
                    drop(groups);
                    for &task_id in &targets {
                        if let Err(e) = self.deliver_to_task(task_id, event.clone()) {
                            self.handle_delivery_failure(event.metadata.sender, &e, &event);
                        }
                    }
                    Ok(())
                } else {
                    Err(EventError::GroupNotFound)
                }
            }
            GroupTarget::AllTasks => {
                let sched = crate::sched::scheduler::get_scheduler();
                let all_ids: alloc::vec::Vec<u32> = sched.get_all_task_ids().into_iter().map(|x| x as u32).collect();
                for tid in all_ids { if let Err(e) = self.deliver_to_task(tid, event.clone()) { self.handle_delivery_failure(event.metadata.sender, &e, &event); } }
                Ok(())
            }
            GroupTarget::Session(session_id) => {
                let sessions = self.sessions.lock();
                if let Some(members) = sessions.get(session_id) {
                    let targets: alloc::vec::Vec<u32> = members.iter().cloned().collect();
                    drop(sessions);
                    for &task_id in &targets { if let Err(e) = self.deliver_to_task(task_id, event.clone()) { self.handle_delivery_failure(event.metadata.sender, &e, &event); } }
                    Ok(())
                } else {
                    Err(EventError::GroupNotFound)
                }
            }
            GroupTarget::Custom(name) => {
                let named = self.named_groups.lock();
                if let Some(members) = named.get(name) {
                    let targets: alloc::vec::Vec<u32> = members.iter().cloned().collect();
                    drop(named);
                    for &task_id in &targets { if let Err(e) = self.deliver_to_task(task_id, event.clone()) { self.handle_delivery_failure(event.metadata.sender, &e, &event); } }
                    Ok(())
                } else {
                    Err(EventError::GroupNotFound)
                }
            }
        }
    }
    
    /// Deliver broadcast event to all tasks
    fn deliver_broadcast(&self, event: Event, _priority: EventPriority, _reliable: bool) -> Result<(), EventError> {
        // broadcast to every task in the system
        let sched = crate::sched::scheduler::get_scheduler();
        let all_ids: alloc::vec::Vec<u32> = sched.get_all_task_ids().into_iter().map(|x| x as u32).collect();
        for tid in all_ids {
            if let Err(e) = self.deliver_to_task(tid, event.clone()) {
                self.handle_delivery_failure(event.metadata.sender, &e, &event);
            }
        }
        Ok(())
    }
    
    /// Deliver event to a specific task
    #[cfg(not(test))]
    pub fn deliver_to_task(&self, task_id: u32, event: Event) -> Result<(), EventError> {
        // Check if the event matches any of the task's filters
        let task_filters = self.task_filters.lock();
        if let Some(filters) = task_filters.get(&task_id) {
            // If task has filters, check if event matches any of them
            if !filters.is_empty() {
                let matches = filters.iter().any(|(_, filter)| filter.matches(&event));
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
            // Enforce buffer size from the target task's config
            let cfg = self.get_task_config_or_default(task_id);
            let mut queue = task.event_queue.lock();
            if queue.len() >= cfg.buffer_size {
                return Err(EventError::BufferFull);
            }
            // Enqueue the event since it passed filtering and buffer check
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

    /// Get a channel by name, if it exists.
    pub fn get_channel(&self, name: &str) -> Option<alloc::sync::Arc<EventChannelObject>> {
        self.channels.lock().get(name).cloned()
    }

    /// Remove a subscription from a channel by name and subscription id.
    pub fn remove_subscription_from_channel(&self, channel_name: &str, subscription_id: &str) -> Result<(), EventError> {
        if let Some(ch) = self.channels.lock().get(channel_name).cloned() {
            ch.remove_subscription(subscription_id)
        } else {
            Err(EventError::ChannelNotFound)
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

/// EventChannel implementation for KernelObject integration
pub struct EventChannelObject {
    name: String,
    /// Channel manages its own subscriptions as EventSubscriptionObjects
    subscriptions: Mutex<HashMap<String, Arc<EventSubscriptionObject>>>,
    #[allow(dead_code)]
    manager_ref: &'static EventManager,
}

impl EventChannelObject {
    pub fn new(name: String) -> Self {
        Self {
            name,
            subscriptions: Mutex::new(HashMap::new()),
            manager_ref: EventManager::get_manager(),
        }
    }
    
    pub fn name(&self) -> &str {
        &self.name
    }
    
    /// Create a new subscription for this channel
    pub fn create_subscription(&self, task_id: u32) -> Result<Arc<EventSubscriptionObject>, EventError> {
        let subscription_id = format!("sub_{}_task_{}", self.name, task_id);
        let subscription = Arc::new(EventSubscriptionObject::new(
            subscription_id.clone(), 
            self.name.clone(), 
            task_id
        ));
        
        let mut subscriptions = self.subscriptions.lock();
        subscriptions.insert(subscription_id, subscription.clone());
        
        Ok(subscription)
    }
    
    /// Remove a subscription from this channel
    pub fn remove_subscription(&self, subscription_id: &str) -> Result<(), EventError> {
        let mut subscriptions = self.subscriptions.lock();
        subscriptions.remove(subscription_id);
        Ok(())
    }
    
    /// Get all subscriptions for this channel
    pub fn get_subscriptions(&self) -> Vec<Arc<EventSubscriptionObject>> {
        self.subscriptions.lock().values().cloned().collect()
    }
    
    /// Get list of current subscriber task IDs
    pub fn get_subscribers(&self) -> Vec<u32> {
        self.subscriptions.lock()
            .values()
            .map(|sub| sub.task_id())
            .collect()
    }
    
    /// Send event to all subscribers of this channel
    pub fn broadcast_to_subscribers(&self, event: Event) -> Result<(), EventError> {
        let subscribers = self.get_subscribers();
        for task_id in subscribers {
            let _ = self.manager_ref.deliver_to_task(task_id, event.clone());
        }
        Ok(())
    }
    
    /// Subscribe a task to this channel (legacy method for backward compatibility)
    pub fn subscribe(&self, task_id: u32) -> Result<Arc<EventSubscriptionObject>, EventError> {
        self.create_subscription(task_id)
    }
    
    /// Unsubscribe a task from this channel (legacy method for backward compatibility)
    pub fn unsubscribe(&self, task_id: u32) -> Result<(), EventError> {
        let mut subscriptions = self.subscriptions.lock();
        subscriptions.retain(|_, sub| sub.task_id() != task_id);
        Ok(())
    }

    /// Get a subscription by its ID
    pub fn get_subscription_by_id(&self, subscription_id: &str) -> Option<Arc<EventSubscriptionObject>> {
        self.subscriptions.lock().get(subscription_id).cloned()
    }
}

/// EventSubscription implementation for KernelObject integration  
pub struct EventSubscriptionObject {
    subscription_id: String,
    channel_name: String,
    task_id: u32,
    /// Local registry of filters keyed by handler ID for this subscription
    filters: Mutex<HashMap<usize, EventFilter>>,
}

impl EventSubscriptionObject {
    pub fn new(subscription_id: String, channel_name: String, task_id: u32) -> Self {
        Self {
            subscription_id,
            channel_name,
            task_id,
            filters: Mutex::new(HashMap::new()),
        }
    }
    
    pub fn subscription_id(&self) -> &str {
        &self.subscription_id
    }
    
    pub fn channel_name(&self) -> &str {
        &self.channel_name
    }
    
    pub fn task_id(&self) -> u32 {
        self.task_id
    }
}

impl crate::object::capability::EventSender for EventChannelObject {
    fn send_event(&self, event: Event) -> Result<(), &'static str> {
        self.manager_ref.send_event(event)
            .map_err(|_| "Failed to send event")?;
        Ok(())
    }
}

impl crate::object::capability::EventReceiver for EventChannelObject {
    fn has_pending_events(&self) -> bool {
        // Check if any subscriber task has pending events for THIS channel specifically
        let subscriber_ids = self.get_subscribers();
        for tid in subscriber_ids {
            if let Some(task) = crate::sched::scheduler::get_scheduler().get_task_by_id(tid as usize) {
                let queue = task.event_queue.lock();
                for (_prio, q) in queue.events.iter() {
                    for ev in q.iter() {
                        if let EventDelivery::Channel { channel_id, .. } = &ev.delivery {
                            if channel_id == &self.name {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        false
    }
}

impl crate::object::capability::EventReceiver for EventSubscriptionObject {
    fn has_pending_events(&self) -> bool {
        // Only consider events delivered to this subscription's channel and matching its local filters
        if let Some(task) = crate::sched::scheduler::get_scheduler().get_task_by_id(self.task_id as usize) {
            let queue = task.event_queue.lock();
            let channel_name = self.channel_name.as_str();
            // Take a snapshot of local filters
            let local_filters: alloc::vec::Vec<EventFilter> = {
                let guard = self.filters.lock();
                guard.values().cloned().collect()
            };
            let use_filters = !local_filters.is_empty();

            for (_prio, q) in queue.events.iter() {
                for ev in q.iter() {
                    if let EventDelivery::Channel { channel_id, .. } = &ev.delivery {
                        if channel_id == channel_name {
                            if use_filters {
                                if local_filters.iter().any(|f| f.matches(ev)) {
                                    return true;
                                }
                            } else {
                                return true; // No local filters => any event on this channel counts
                            }
                        }
                    }
                }
            }
        }
        false
    }
}

impl crate::object::capability::EventSubscriber for EventSubscriptionObject {
    fn register_filter(&self, filter: EventFilter, handler_id: usize) -> Result<(), &'static str> {
        // Store locally only to avoid task-global filter pollution
        self.filters.lock().insert(handler_id, filter);
        Ok(())
    }
    
    fn unregister_filter(&self, handler_id: usize) -> Result<(), &'static str> {
        self.filters.lock().remove(&handler_id);
        Ok(())
    }
    
    fn get_filters(&self) -> Vec<(usize, EventFilter)> {
        // Return a snapshot of local filters
        self.filters
            .lock()
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect()
    }
}

impl crate::object::capability::CloneOps for EventChannelObject {
    fn custom_clone(&self) -> crate::object::KernelObject {
        // Try to return the same Arc registered in EventManager
        let mgr = EventManager::get_manager();
        if let Some(arc) = mgr.channels.lock().get(self.name()).cloned() {
            crate::object::KernelObject::EventChannel(arc)
        } else {
            // Fallback: create or register via manager to ensure registry consistency
            let ko = mgr.create_channel(self.name.clone());
            ko
        }
    }
}

impl crate::object::capability::CloneOps for EventSubscriptionObject {
    fn custom_clone(&self) -> crate::object::KernelObject {
        let mgr = EventManager::get_manager();
        // Resolve channel from manager and then subscription by id
        if let Some(ch) = mgr.channels.lock().get(self.channel_name()).cloned() {
            if let Some(sub) = ch.get_subscription_by_id(self.subscription_id()) {
                return crate::object::KernelObject::EventSubscription(sub);
            }
        }
        // Fallback: create a new subscription object (not ideal, but avoids panic)
        let fallback = alloc::sync::Arc::new(EventSubscriptionObject::new(
            self.subscription_id.clone(),
            self.channel_name.clone(),
            self.task_id,
        ));
        crate::object::KernelObject::EventSubscription(fallback)
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use crate::object::capability::EventSubscriber; // bring trait into scope

    #[test_case]
    fn test_event_creation() {
        let event = Event::new(
            EventDelivery::Direct { 
                target: 123, 
                priority: EventPriority::High, 
                reliable: true 
            },
            EventContent::ProcessControl(ProcessControlType::Terminate),
            EventPayload::Empty,
        );

        match event.delivery {
            EventDelivery::Direct { target, priority, reliable } => {
                assert_eq!(target, 123);
                assert_eq!(priority, EventPriority::High);
                assert_eq!(reliable, true);
            },
            _ => panic!("Wrong delivery type"),
        }

        match event.content {
            EventContent::ProcessControl(ProcessControlType::Terminate) => {},
            _ => panic!("Wrong content type"),
        }

        assert_eq!(event.metadata.priority, EventPriority::High);
    }

    #[test_case]
    fn test_event_convenience_functions() {
        // Test direct process control event
        let event = Event::direct_process_control(
            42, 
            ProcessControlType::Kill, 
            EventPriority::Critical, 
            true
        );
        
        match event.delivery {
            EventDelivery::Direct { target, priority, reliable } => {
                assert_eq!(target, 42);
                assert_eq!(priority, EventPriority::Critical);
                assert_eq!(reliable, true);
            },
            _ => panic!("Wrong delivery type"),
        }

        match event.content {
            EventContent::ProcessControl(ProcessControlType::Kill) => {},
            _ => panic!("Wrong content type"),
        }

        // Test immediate process control event
        let event = Event::immediate_process_control(99, ProcessControlType::Stop);
        match event.delivery {
            EventDelivery::Direct { target, priority, reliable } => {
                assert_eq!(target, 99);
                assert_eq!(priority, EventPriority::High);
                assert_eq!(reliable, true);
            },
            _ => panic!("Wrong delivery type"),
        }
    }

    #[test_case]
    fn test_event_channel_creation() {
        let event = Event::channel(
            "test_channel".to_string(),
            EventContent::Notification(NotificationType::TaskCompleted),
            true,
            EventPriority::Normal,
            EventPayload::Empty,
        );

        match event.delivery {
            EventDelivery::Channel { channel_id, create_if_missing, priority } => {
                assert_eq!(channel_id, "test_channel");
                assert_eq!(create_if_missing, true);
                assert_eq!(priority, EventPriority::Normal);
            },
            _ => panic!("Wrong delivery type"),
        }

        match event.content {
            EventContent::Notification(NotificationType::TaskCompleted) => {},
            _ => panic!("Wrong content type"),
        }
    }

    #[test_case]
    fn test_event_group_creation() {
        let event = Event::group(
            GroupTarget::AllTasks,
            EventContent::Message { 
                message_type: 42, 
                category: MessageCategory::Control 
            },
            EventPriority::Low,
            false,
            EventPayload::Empty,
        );

        match event.delivery {
            EventDelivery::Group { group_target, priority, reliable } => {
                assert_eq!(group_target, GroupTarget::AllTasks);
                assert_eq!(priority, EventPriority::Low);
                assert_eq!(reliable, false);
            },
            _ => panic!("Wrong delivery type"),
        }

        match event.content {
            EventContent::Message { message_type, category } => {
                assert_eq!(message_type, 42);
                assert_eq!(category, MessageCategory::Control);
            },
            _ => panic!("Wrong content type"),
        }
    }

    #[test_case]
    fn test_event_broadcast_creation() {
        let event = Event::broadcast(
            EventContent::Custom { 
                namespace: "test_namespace".to_string(), 
                event_id: 100 
            },
            EventPriority::Normal,
            true,
            EventPayload::Bytes(alloc::vec![1, 2, 3, 4]),
        );

        match event.delivery {
            EventDelivery::Broadcast { priority, reliable } => {
                assert_eq!(priority, EventPriority::Normal);
                assert_eq!(reliable, true);
            },
            _ => panic!("Wrong delivery type"),
        }

        match event.content {
            EventContent::Custom { namespace, event_id } => {
                assert_eq!(namespace, "test_namespace");
                assert_eq!(event_id, 100);
            },
            _ => panic!("Wrong content type"),
        }

        match event.payload {
            EventPayload::Bytes(data) => {
                assert_eq!(data, alloc::vec![1, 2, 3, 4]);
            },
            _ => panic!("Wrong payload type"),
        }
    }

    #[test_case]
    fn test_event_filter_event_type() {
        let filter = EventFilter::EventType(EventTypeFilter::AnyDirect);
        
        let direct_event = Event::direct_process_control(
            123, 
            ProcessControlType::Terminate, 
            EventPriority::High, 
            true
        );
        
        let channel_event = Event::channel(
            "test".to_string(),
            EventContent::ProcessControl(ProcessControlType::Terminate),
            false,
            EventPriority::High,
            EventPayload::Empty,
        );

        assert_eq!(filter.matches(&direct_event), true);
        assert_eq!(filter.matches(&channel_event), false);
    }

    #[test_case]
    fn test_event_filter_channel() {
        let filter = EventFilter::Channel("test_channel".to_string());
        
        let matching_event = Event::channel(
            "test_channel".to_string(),
            EventContent::Notification(NotificationType::TaskCompleted),
            false,
            EventPriority::Normal,
            EventPayload::Empty,
        );
        
        let non_matching_event = Event::channel(
            "other_channel".to_string(),
            EventContent::Notification(NotificationType::TaskCompleted),
            false,
            EventPriority::Normal,
            EventPayload::Empty,
        );

        assert_eq!(filter.matches(&matching_event), true);
        assert_eq!(filter.matches(&non_matching_event), false);
    }

    #[test_case]
    fn test_event_filter_sender() {
        let filter = EventFilter::Sender(42);
        
        let mut matching_event = Event::immediate_process_control(123, ProcessControlType::Terminate);
        matching_event.metadata.sender = Some(42);
        
        let mut non_matching_event = Event::immediate_process_control(123, ProcessControlType::Terminate);
        non_matching_event.metadata.sender = Some(99);

        assert_eq!(filter.matches(&matching_event), true);
        assert_eq!(filter.matches(&non_matching_event), false);
    }

    #[test_case]
    fn test_task_event_queue_basic() {
        let mut queue = TaskEventQueue::new();
        
        assert_eq!(queue.is_empty(), true);
        assert_eq!(queue.len(), 0);
        
        let event = Event::immediate_process_control(123, ProcessControlType::Terminate);
        assert_eq!(queue.enqueue(event.clone()), true);
        
        assert_eq!(queue.is_empty(), false);
        assert_eq!(queue.len(), 1);
        
        let dequeued = queue.dequeue();
        assert!(dequeued.is_some());
        
        assert_eq!(queue.is_empty(), true);
        assert_eq!(queue.len(), 0);
    }

    #[test_case]
    fn test_task_event_queue_priority_ordering() {
        let mut queue = TaskEventQueue::new();
        
        // Add events in non-priority order
        let low_event = Event::direct_process_control(
            1, ProcessControlType::Stop, EventPriority::Low, true
        );
        let critical_event = Event::direct_process_control(
            2, ProcessControlType::Kill, EventPriority::Critical, true
        );
        let high_event = Event::direct_process_control(
            3, ProcessControlType::Terminate, EventPriority::High, true
        );
        let normal_event = Event::direct_process_control(
            4, ProcessControlType::Continue, EventPriority::Normal, true
        );
        
        queue.enqueue(low_event);
        queue.enqueue(critical_event);
        queue.enqueue(high_event);
        queue.enqueue(normal_event);
        
        assert_eq!(queue.len(), 4);
        
        // Should dequeue in priority order: Critical -> High -> Normal -> Low
        let first = queue.dequeue().unwrap();
        assert_eq!(first.metadata.priority, EventPriority::Critical);
        
        let second = queue.dequeue().unwrap();
        assert_eq!(second.metadata.priority, EventPriority::High);
        
        let third = queue.dequeue().unwrap();
        assert_eq!(third.metadata.priority, EventPriority::Normal);
        
        let fourth = queue.dequeue().unwrap();
        assert_eq!(fourth.metadata.priority, EventPriority::Low);
        
        assert_eq!(queue.len(), 0);
    }

    #[test_case]
    fn test_event_manager_creation() {
        let manager = EventManager::new();
        assert!(manager.channels.lock().is_empty());
    }

    #[test_case]
    fn test_process_control_type_variants() {
        // Test all ProcessControlType variants
        let variants = [
            ProcessControlType::Terminate,
            ProcessControlType::Kill,
            ProcessControlType::Stop,
            ProcessControlType::Continue,
            ProcessControlType::Interrupt,
            ProcessControlType::Quit,
            ProcessControlType::Hangup,
            ProcessControlType::ChildExit,
            ProcessControlType::User(0),
        ];
        
        for &variant in &variants {
            let event = Event::immediate_process_control(123, variant);
            match event.content {
                EventContent::ProcessControl(received_variant) => {
                    assert_eq!(received_variant, variant);
                },
                _ => panic!("Wrong content type for variant {:?}", variant),
            }
        }
    }

    #[test_case]
    fn test_notification_type_variants() {
        // Test all NotificationType variants
        let variants = [
            NotificationType::TaskCompleted,
            NotificationType::MemoryLow,
            NotificationType::DeviceConnected,
            NotificationType::DeviceDisconnected,
            NotificationType::FilesystemFull,
            NotificationType::NetworkChange,
        ];
        
        for &variant in &variants {
            let event = Event::notification_to_task(123, variant);
            match event.content {
                EventContent::Notification(received_variant) => {
                    assert_eq!(received_variant, variant);
                },
                _ => panic!("Wrong content type for variant {:?}", variant),
            }
        }
    }

    #[test_case]
    fn test_event_payload_variants() {
        // Test Empty payload
        let empty_event = Event::immediate_process_control(123, ProcessControlType::Terminate);
        match empty_event.payload {
            EventPayload::Empty => {},
            _ => panic!("Expected Empty payload"),
        }
        
        // Test Integer payload
        let integer_event = Event::broadcast(
            EventContent::Custom { 
                namespace: "test".to_string(), 
                event_id: 42 
            },
            EventPriority::Normal,
            false,
            EventPayload::Integer(12345),
        );
        match integer_event.payload {
            EventPayload::Integer(value) => {
                assert_eq!(value, 12345);
            },
            _ => panic!("Expected Integer payload"),
        }
        
        // Test Bytes payload
        let data = alloc::vec![1, 2, 3, 4, 5];
        let bytes_event = Event::broadcast(
            EventContent::Custom { 
                namespace: "test".to_string(), 
                event_id: 43 
            },
            EventPriority::Normal,
            false,
            EventPayload::Bytes(data.clone()),
        );
        match bytes_event.payload {
            EventPayload::Bytes(received_data) => {
                assert_eq!(received_data, data);
            },
            _ => panic!("Expected Bytes payload"),
        }
        
        // Test String payload
        let text = "Hello, World!".to_string();
        let string_event = Event::broadcast(
            EventContent::Custom { 
                namespace: "test".to_string(), 
                event_id: 44 
            },
            EventPriority::Normal,
            false,
            EventPayload::String(text.clone()),
        );
        match string_event.payload {
            EventPayload::String(received_text) => {
                assert_eq!(received_text, text);
            },
            _ => panic!("Expected String payload"),
        }
    }

    #[test_case]
    fn test_process_control_type_variants() {
        // Test all ProcessControlType variants
        let variants = [
            ProcessControlType::Kill,
            ProcessControlType::Terminate,
            ProcessControlType::Stop,
            ProcessControlType::Continue,
            ProcessControlType::Interrupt,
            ProcessControlType::Quit,
            ProcessControlType::Hangup,
            ProcessControlType::User(0),
            ProcessControlType::PipeBroken,
            ProcessControlType::Alarm,
            ProcessControlType::ChildExit,
            ProcessControlType::IoReady,
        ];
        
        for &variant in &variants {
            let event = Event::immediate_process_control(123, variant);
            match event.content {
                EventContent::ProcessControl(received_variant) => {
                    assert_eq!(received_variant, variant);
                },
                _ => panic!("Wrong content type for variant {:?}", variant),
            }
        }
    }

    #[test_case]
    fn test_notification_type_variants_notification() {
        // Test all NotificationType variants
        let variants = [
            NotificationType::TaskCompleted,
            NotificationType::MemoryLow,
            NotificationType::DeviceConnected,
            NotificationType::DeviceDisconnected,
            NotificationType::FilesystemFull,
            NotificationType::NetworkChange,
        ];
        
        for &variant in &variants {
            let event = Event::notification_to_task(123, variant);
            match event.content {
                EventContent::Notification(received_variant) => {
                    assert_eq!(received_variant, variant);
                },
                _ => panic!("Wrong content type for variant {:?}", variant),
            }
        }
    }

    #[test_case]
    fn test_event_payload_variants_extended() {
        // Test Empty payload
        let empty_event = Event::immediate_process_control(123, ProcessControlType::Terminate);
        match empty_event.payload {
            EventPayload::Empty => {},
            _ => panic!("Expected Empty payload"),
        }
        
        // Test Integer payload
        let integer_event = Event::broadcast(
            EventContent::Custom { 
                namespace: "test".to_string(), 
                event_id: 42 
            },
            EventPriority::Normal,
            false,
            EventPayload::Integer(12345),
        );
        match integer_event.payload {
            EventPayload::Integer(value) => {
                assert_eq!(value, 12345);
            },
            _ => panic!("Expected Integer payload"),
        }
        
        // Test Bytes payload
        let data = alloc::vec![1, 2, 3, 4, 5];
        let bytes_event = Event::broadcast(
            EventContent::Custom { 
                namespace: "test".to_string(), 
                event_id: 43 
            },
            EventPriority::Normal,
            false,
            EventPayload::Bytes(data.clone()),
        );
        match bytes_event.payload {
            EventPayload::Bytes(received_data) => {
                assert_eq!(received_data, data);
            },
            _ => panic!("Expected Bytes payload"),
        }
        
        // Test String payload
        let text = "Hello, World!".to_string();
        let string_event = Event::broadcast(
            EventContent::Custom { 
                namespace: "test".to_string(), 
                event_id: 44 
            },
            EventPriority::Normal,
            false,
            EventPayload::String(text.clone()),
        );
        match string_event.payload {
            EventPayload::String(received_text) => {
                assert_eq!(received_text, text);
            },
            _ => panic!("Expected String payload"),
        }
    }

    #[test_case]
    fn test_channel_subscription_management() {
        let manager = EventManager::get_manager();
        // Create channel via manager
        let ch = manager.create_channel("sub_test".to_string());
        let channel = match ch {
            crate::object::KernelObject::EventChannel(arc) => arc,
            _ => panic!("Expected EventChannel"),
        };

        // Initially no subscribers
        assert_eq!(channel.get_subscribers().len(), 0);

        // Subscribe task 1 and 2
        let _s1 = channel.subscribe(1).expect("subscribe(1)");
        let _s2 = channel.subscribe(2).expect("subscribe(2)");
        let mut subs = channel.get_subscribers();
        subs.sort();
        assert_eq!(subs, alloc::vec![1, 2]);

        // Unsubscribe task 1
        channel.unsubscribe(1).expect("unsubscribe(1)");
        let subs = channel.get_subscribers();
        assert_eq!(subs, alloc::vec![2]);

        // Remove remaining via remove_subscription using id
        let ids: alloc::vec::Vec<String> = channel
            .get_subscriptions()
            .into_iter()
            .map(|s| s.subscription_id().to_string())
            .collect();
        for id in ids {
            channel.remove_subscription(&id).unwrap();
        }
        assert_eq!(channel.get_subscribers().len(), 0);
    }

    #[test_case]
    fn test_event_manager_subscription_creation() {
        let manager = EventManager::get_manager();
        // Create subscription through manager, it should create channel if missing
        let ko = manager
            .create_subscription("mgr_sub_test".to_string(), 42)
            .expect("create_subscription");
        let sub = match ko {
            crate::object::KernelObject::EventSubscription(arc) => arc,
            _ => panic!("Expected EventSubscription"),
        };
        assert_eq!(sub.channel_name(), "mgr_sub_test");
        assert_eq!(sub.task_id(), 42);

        // Ensure channel was registered and contains the subscriber
        let channels = manager.channels.lock();
        let ch = channels.get("mgr_sub_test").expect("channel exists").clone();
        drop(channels);
        let subs = ch.get_subscribers();
        assert_eq!(subs, alloc::vec![42]);
    }

    #[test_case]
    fn test_channel_broadcast_no_error() {
        let manager = EventManager::get_manager();
        let ch = manager.create_channel("bc_test".to_string());
        let channel = match ch {
            crate::object::KernelObject::EventChannel(arc) => arc,
            _ => panic!("Expected EventChannel"),
        };
        let _ = channel.subscribe(100).unwrap();
        let _ = channel.subscribe(200).unwrap();

        // Since in cfg(test) deliver_to_task is a stub that returns Ok, we only
        // verify that broadcast completes without error when subscribers exist.
        let ev = Event::new_channel_event(
            "bc_test",
            EventContent::Notification(NotificationType::TaskCompleted),
            EventPayload::Empty,
        );
        channel.broadcast_to_subscribers(ev).expect("broadcast ok");
    }

    #[test_case]
    fn test_subscription_filter_registration() {
        let manager = EventManager::get_manager();
        let ch = manager.create_channel("filter_test".to_string());
        let channel = match ch { crate::object::KernelObject::EventChannel(arc) => arc, _ => panic!("Expected EventChannel"), };
        let sub = channel.subscribe(7).expect("subscribe");

        // Register two filters (local only now)
        sub.register_filter(EventFilter::All, 1).expect("reg1");
        sub.register_filter(EventFilter::Sender(7), 2).expect("reg2");
        let filters = sub.get_filters();
        assert_eq!(filters.len(), 2);

        // Unregister one
        sub.unregister_filter(1).expect("unreg1");
        let filters = sub.get_filters();
        assert_eq!(filters.len(), 1);

        // Manager's global filters should not be polluted by subscription-local filters
        let globals = manager.get_filters_for_task(7);
        assert!(globals.is_empty());
    }

    #[test_case]
    fn test_clone_returns_same_arc_objects() {
        use crate::object::capability::CloneOps;
        let mgr = EventManager::get_manager();
        let ch = match mgr.create_channel("clone_arc".to_string()) { crate::object::KernelObject::EventChannel(arc) => arc, _ => panic!("Expected channel") };
        let sub = ch.subscribe(1234).expect("subscribe");

        // Channel clone should resolve to the same Arc in manager registry
        let ch_clone = match CloneOps::custom_clone(&*ch) { crate::object::KernelObject::EventChannel(arc) => arc, _ => panic!("Expected channel") };
        assert!(alloc::sync::Arc::ptr_eq(&ch, &ch_clone));

        // Subscription clone should resolve to the same Arc if still registered
        let sub_clone = match CloneOps::custom_clone(&*sub) { crate::object::KernelObject::EventSubscription(arc) => arc, _ => panic!("Expected sub") };
        assert!(alloc::sync::Arc::ptr_eq(&sub, &sub_clone));
    }

    #[test_case]
    fn test_group_session_and_named_delivery_no_error() {
        let mgr = EventManager::get_manager();
        // Prepare tasks and register into scheduler
        for i in 0..2 { let task = crate::task::Task::new(format!("g_sess_{}", i), 1, crate::task::TaskType::Kernel); crate::sched::scheduler::get_scheduler().add_task(task, 0); }

        // For tests, get_current_task_id() returns Some(1), so join operations will add task 1
        mgr.join_session(77).expect("join session");
        mgr.join_named_group("teamA".to_string()).expect("join named");

        let ev_sess = Event::group(GroupTarget::Session(77), EventContent::Notification(NotificationType::DeviceConnected), EventPriority::Normal, false, EventPayload::Empty);
        let ev_named = Event::group(GroupTarget::Custom("teamA".to_string()), EventContent::Notification(NotificationType::DeviceDisconnected), EventPriority::Normal, false, EventPayload::Empty);
        assert!(mgr.send_event(ev_sess).is_ok());
        assert!(mgr.send_event(ev_named).is_ok());

        mgr.leave_session(77).ok();
        mgr.leave_named_group("teamA").ok();
    }
}