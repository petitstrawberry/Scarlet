//! Event Channel and Subscription objects for KernelObject integration
//! 
//! This module provides handle-based event IPC objects that integrate
//! with the KernelObject/HandleTable pattern.

use alloc::{string::String, vec::Vec, sync::Arc, collections::VecDeque, format};
use spin::Mutex;

use crate::object::capability::{CloneOps, EventIpcOps};
use crate::object::KernelObject;
use crate::ipc::{Event, EventType, event::EventPriority};
use crate::sync::Waker;
use crate::task::mytask;
use crate::arch::get_cpu;

/// Event errors for KernelObject integration
#[derive(Debug, Clone)]
pub enum EventError {
    /// Channel is closed
    ChannelClosed,
    
    /// Subscription is closed
    SubscriptionClosed,
    
    /// No events available (non-blocking receive)
    NoEventsAvailable,
    
    /// Queue is full
    QueueFull,
    
    /// Invalid filter
    InvalidFilter,
    
    /// Permission denied
    PermissionDenied,
    
    /// Other error
    Other(String),
}

/// Channel statistics
#[derive(Debug, Clone)]
pub struct ChannelStats {
    pub subscriber_count: usize,
    pub events_published: u64,
    pub events_delivered: u64,
    pub events_dropped: u64,
}

/// Event filter for subscriptions
#[derive(Debug, Clone)]
pub struct EventFilter {
    pub event_types: Option<Vec<String>>,
    pub priority_threshold: Option<EventPriority>,
    pub source_filter: Option<u32>, // TaskId
}

/// Event channel object trait - represents a named event channel
pub trait EventChannelObject: EventIpcOps + CloneOps {
    /// Get the channel name/identifier
    fn channel_name(&self) -> &str;
    
    /// Publish an event to this channel
    fn publish(&self, event: Event) -> Result<(), EventError>;
    
    /// Get subscriber count
    fn subscriber_count(&self) -> usize;
    
    /// Check if the channel is active (has subscribers or publishers)
    fn is_active(&self) -> bool;
    
    /// Get channel statistics
    fn get_stats(&self) -> ChannelStats;
}

/// Event subscription object trait - represents a subscription to events
pub trait EventSubscriptionObject: EventIpcOps + CloneOps {
    /// Receive the next event (blocking or non-blocking)
    fn receive_event(&self, blocking: bool) -> Result<Event, EventError>;
    
    /// Check if events are available
    fn has_pending_events(&self) -> bool;
    
    /// Get the number of pending events
    fn pending_count(&self) -> usize;
    
    /// Get subscription filter
    fn get_filter(&self) -> Option<EventFilter>;
    
    /// Update subscription filter
    fn set_filter(&self, filter: Option<EventFilter>) -> Result<(), EventError>;
    
    /// Get the channel name this subscription is for
    fn channel_name(&self) -> &str;
}

/// Internal state of an event channel
struct ChannelState {
    /// Channel name
    name: String,
    /// List of active subscriptions (weak references)
    subscriptions: Vec<Arc<Mutex<SubscriptionState>>>,
    /// Channel statistics
    stats: ChannelStats,
    /// Whether the channel is closed
    closed: bool,
}

/// Internal state of an event subscription
struct SubscriptionState {
    /// Channel name
    channel_name: String,
    /// Event queue for this subscription
    event_queue: VecDeque<Event>,
    /// Maximum queue size
    max_queue_size: usize,
    /// Event filter
    filter: Option<EventFilter>,
    /// Whether this subscription is active
    active: bool,
    /// Waker for blocking receive operations
    waker: crate::sync::waker::Waker,
}

/// Concrete implementation of EventChannelObject
pub struct EventChannel {
    /// Shared channel state
    state: Arc<Mutex<ChannelState>>,
    /// Cached channel name for efficient access
    name: String,
}

/// Concrete implementation of EventSubscriptionObject
pub struct EventSubscription {
    /// Reference to subscription state
    state: Arc<Mutex<SubscriptionState>>,
    /// Reference to channel state (for publishing)
    channel_state: Arc<Mutex<ChannelState>>,
    /// Cached channel name for efficient access
    channel_name: String,
}

impl EventChannel {
    /// Create a new event channel
    pub fn new(name: String) -> Self {
        let cached_name = name.clone();
        let state = ChannelState {
            name,
            subscriptions: Vec::new(),
            stats: ChannelStats {
                subscriber_count: 0,
                events_published: 0,
                events_delivered: 0,
                events_dropped: 0,
            },
            closed: false,
        };
        
        Self {
            state: Arc::new(Mutex::new(state)),
            name: cached_name,
        }
    }
    
    /// Create a subscription to this channel
    pub fn create_subscription(&self, max_queue_size: Option<usize>) -> EventSubscription {
        let max_queue_size = max_queue_size.unwrap_or(1024);
        
        let channel_name = self.name.clone();
        
        let subscription_state = SubscriptionState {
            channel_name: channel_name.clone(),
            event_queue: VecDeque::with_capacity(max_queue_size),
            max_queue_size,
            filter: None,
            active: true,
            waker: crate::sync::waker::Waker::new_interruptible("event_subscription"),
        };
        
        let subscription_state = Arc::new(Mutex::new(subscription_state));
        
        // Register subscription with channel
        {
            let mut state = self.state.lock();
            state.subscriptions.push(subscription_state.clone());
            state.stats.subscriber_count = state.subscriptions.len();
        }
        
        EventSubscription {
            state: subscription_state,
            channel_state: self.state.clone(),
            channel_name,
        }
    }
}

impl EventChannelObject for EventChannel {
    fn channel_name(&self) -> &str {
        &self.name
    }
    
    fn publish(&self, event: Event) -> Result<(), EventError> {
        let mut state = self.state.lock();
        
        if state.closed {
            return Err(EventError::ChannelClosed);
        }
        
        state.stats.events_published += 1;
        
        // Clean up inactive subscriptions first
        state.subscriptions.retain(|sub| {
            let sub_state = sub.lock();
            sub_state.active
        });
        
        let mut delivered = 0;
        let mut dropped = 0;
        
        // Deliver to all active subscriptions
        for subscription in &state.subscriptions {
            let mut sub_state = subscription.lock();
            
            // Check filter
            if let Some(filter) = &sub_state.filter {
                if !self.event_matches_filter(&event, filter) {
                    continue;
                }
            }
            
            // Add to queue if there's space
            if sub_state.event_queue.len() < sub_state.max_queue_size {
                sub_state.event_queue.push_back(event.clone());
                delivered += 1;
                
                // Wake up all tasks waiting on this subscription
                // Multiple tasks might be waiting on the same subscription
                sub_state.waker.wake_all();
            } else {
                dropped += 1;
            }
        }
        
        state.stats.events_delivered += delivered;
        state.stats.events_dropped += dropped;
        state.stats.subscriber_count = state.subscriptions.len();
        
        Ok(())
    }
    
    fn subscriber_count(&self) -> usize {
        let state = self.state.lock();
        state.subscriptions.len()
    }
    
    fn is_active(&self) -> bool {
        let state = self.state.lock();
        !state.closed && !state.subscriptions.is_empty()
    }
    
    fn get_stats(&self) -> ChannelStats {
        let state = self.state.lock();
        state.stats.clone()
    }
}

impl EventChannel {
    /// Helper method to check if an event matches a filter
    fn event_matches_filter(&self, event: &Event, filter: &EventFilter) -> bool {
        // Check priority threshold
        if let Some(threshold) = &filter.priority_threshold {
            let event_priority = match &event.event_type {
                EventType::Direct { priority, .. } => priority,
                EventType::Channel { priority, .. } => priority,
                EventType::Group { priority, .. } => priority,
                EventType::Broadcast { priority, .. } => priority,
            };
            
            if event_priority < threshold {
                return false;
            }
        }
        
        // Check source filter
        if let Some(source_filter) = filter.source_filter {
            if event.metadata.sender.unwrap_or(0) != source_filter {
                return false;
            }
        }
        
        // Check event types (simplified for now)
        if let Some(event_types) = &filter.event_types {
            let event_type_name = match &event.event_type {
                EventType::Direct { .. } => "direct",
                EventType::Channel { .. } => "channel",
                EventType::Group { .. } => "group",
                EventType::Broadcast { .. } => "broadcast",
            };
            
            if !event_types.iter().any(|t| t == event_type_name) {
                return false;
            }
        }
        
        true
    }
}

impl EventSubscriptionObject for EventSubscription {
    fn receive_event(&self, blocking: bool) -> Result<Event, EventError> {
        loop {
            let mut state = self.state.lock();
            
            if !state.active {
                return Err(EventError::SubscriptionClosed);
            }
            
            // Check if there's an event available
            if let Some(event) = state.event_queue.pop_front() {
                return Ok(event);
            }
            
            // No events available
            if blocking {
                // Block the current task using the same pattern as TTY
                if let Some(mut task) = mytask() {
                    let mut cpu = get_cpu();
                    
                    // This will block until an event is available
                    // The task will be woken up when an event is published
                    // Note: state.waker.wait() never returns, but when the task
                    // is woken up, the syscall will be restarted from the beginning
                    state.waker.wait(&mut task, &mut cpu);
                    
                    // This should not be reached as wait() doesn't return
                    // But if we somehow get here, loop back to check for events
                } else {
                    // No current task (kernel context)
                    return Err(EventError::Other("Cannot block in kernel context".into()));
                }
            } else {
                return Err(EventError::NoEventsAvailable);
            }
        }
    }
    
    fn has_pending_events(&self) -> bool {
        let state = self.state.lock();
        !state.event_queue.is_empty()
    }
    
    fn pending_count(&self) -> usize {
        let state = self.state.lock();
        state.event_queue.len()
    }
    
    fn get_filter(&self) -> Option<EventFilter> {
        let state = self.state.lock();
        state.filter.clone()
    }
    
    fn set_filter(&self, filter: Option<EventFilter>) -> Result<(), EventError> {
        let mut state = self.state.lock();
        state.filter = filter;
        Ok(())
    }
    
    fn channel_name(&self) -> &str {
        &self.channel_name
    }
}

impl Drop for EventSubscription {
    fn drop(&mut self) {
        // Mark subscription as inactive
        {
            let mut state = self.state.lock();
            state.active = false;
        }
        
        // Remove from channel's subscription list immediately
        {
            let mut channel_state = self.channel_state.lock();
            let subscription_ptr = Arc::as_ptr(&self.state);
            
            // Remove this subscription from the channel's list
            channel_state.subscriptions.retain(|sub| {
                Arc::as_ptr(sub) != subscription_ptr
            });
            
            // Update subscriber count immediately
            channel_state.stats.subscriber_count = channel_state.subscriptions.len();
        }
    }
}

impl Drop for EventChannel {
    fn drop(&mut self) {
        // Mark channel as closed and notify all subscribers
        let mut state = self.state.lock();
        state.closed = true;
        
        // Notify all active subscriptions that the channel is closed
        for subscription in &state.subscriptions {
            let mut sub_state = subscription.lock();
            if sub_state.active {
                // Add a special "channel closed" event
                let close_event = Event::new(
                    crate::ipc::EventType::Direct {
                        target: 0, // Special marker for channel close
                        event_id: 0xFFFFFFFF, // Special event ID for channel close
                        priority: crate::ipc::event::EventPriority::High,
                        reliable: true,
                    },
                    crate::ipc::EventPayload::String(String::from("CHANNEL_CLOSED"))
                );
                
                // Force add to queue even if full (important system event)
                if sub_state.event_queue.len() >= sub_state.max_queue_size {
                    sub_state.event_queue.pop_front(); // Make space
                }
                sub_state.event_queue.push_back(close_event);
                
                // Wake up waiting tasks
                sub_state.waker.wake_all();
                
                // Mark subscription as inactive
                sub_state.active = false;
            }
        }
        
        // Clear subscriptions
        state.subscriptions.clear();
        state.stats.subscriber_count = 0;
    }
}

// Placeholder implementations for EventIpcOps and CloneOps
// These would need to be properly implemented

impl EventIpcOps for EventChannel {
    fn subscribe_channel(&self, _channel_name: String) -> Result<(), &'static str> {
        Err("Not applicable to EventChannel")
    }
    
    fn unsubscribe_channel(&self, _channel_name: &str) -> Result<(), &'static str> {
        Err("Not applicable to EventChannel")
    }
    
    fn publish_to_channel(&self, _channel_name: String, _event_type: crate::task::events::TaskEventType, _source_task: Option<usize>) -> Result<usize, &'static str> {
        Err("Use publish() method instead")
    }
    
    fn join_process_group(&self, _group_id: usize) -> Result<(), &'static str> {
        Err("Not applicable to EventChannel")
    }
    
    fn leave_process_group(&self, _group_id: usize) -> Result<(), &'static str> {
        Err("Not applicable to EventChannel")
    }
    
    fn send_to_process_group(&self, _group_id: usize, _event_type: crate::task::events::TaskEventType, _source_task: Option<usize>) -> Result<usize, &'static str> {
        Err("Not applicable to EventChannel")
    }
    
    fn send_event(&self, _target: crate::task::events::EventTarget, _event_type: crate::task::events::TaskEventType, _source_task: Option<usize>) -> Result<usize, &'static str> {
        Err("Use publish() method instead")
    }
    
    fn get_subscribed_channels(&self) -> Vec<String> {
        alloc::vec![self.name.clone()]
    }
    
    fn get_joined_process_groups(&self) -> Vec<usize> {
        alloc::vec![]
    }
    
    fn get_task_id(&self) -> Option<usize> {
        None
    }
}

impl EventIpcOps for EventSubscription {
    fn subscribe_channel(&self, _channel_name: String) -> Result<(), &'static str> {
        Err("Already subscribed to a channel")
    }
    
    fn unsubscribe_channel(&self, _channel_name: &str) -> Result<(), &'static str> {
        // Mark as inactive
        let mut state = self.state.lock();
        state.active = false;
        Ok(())
    }
    
    fn publish_to_channel(&self, _channel_name: String, _event_type: crate::task::events::TaskEventType, _source_task: Option<usize>) -> Result<usize, &'static str> {
        Err("Use EventChannel for publishing")
    }
    
    fn join_process_group(&self, _group_id: usize) -> Result<(), &'static str> {
        Err("Not applicable to EventSubscription")
    }
    
    fn leave_process_group(&self, _group_id: usize) -> Result<(), &'static str> {
        Err("Not applicable to EventSubscription")
    }
    
    fn send_to_process_group(&self, _group_id: usize, _event_type: crate::task::events::TaskEventType, _source_task: Option<usize>) -> Result<usize, &'static str> {
        Err("Not applicable to EventSubscription")
    }
    
    fn send_event(&self, _target: crate::task::events::EventTarget, _event_type: crate::task::events::TaskEventType, _source_task: Option<usize>) -> Result<usize, &'static str> {
        Err("Not applicable to EventSubscription")
    }
    
    fn get_subscribed_channels(&self) -> Vec<String> {
        alloc::vec![self.channel_name.clone()]
    }
    
    fn get_joined_process_groups(&self) -> Vec<usize> {
        alloc::vec![]
    }
    
    fn get_task_id(&self) -> Option<usize> {
        None
    }
}

impl CloneOps for EventChannel {
    fn custom_clone(&self) -> KernelObject {
        // Clone the EventChannel object
        let cloned = EventChannel {
            state: self.state.clone(),
            name: self.name.clone(),
        };
        KernelObject::EventChannel(Arc::new(cloned))
    }
}

impl CloneOps for EventSubscription {
    fn custom_clone(&self) -> KernelObject {
        // Clone the EventSubscription object
        let cloned = EventSubscription {
            state: self.state.clone(),
            channel_state: self.channel_state.clone(),
            channel_name: self.channel_name.clone(),
        };
        KernelObject::EventSubscription(Arc::new(cloned))
    }
}

impl EventChannel {
    /// Optimized version: Wake up exactly one task if queue was empty
    /// This prevents unnecessary context switches when multiple tasks are waiting
    fn wake_subscribers_optimized(&self, subscription: &Arc<Mutex<SubscriptionState>>, was_empty: bool) {
        if was_empty {
            // Queue was empty, so waiting tasks need to be woken up
            let sub_state = subscription.lock();
            if sub_state.waker.waiting_count() > 0 {
                // Only wake one task since they will all get the same event anyway
                sub_state.waker.wake_one();
            }
        }
        // If queue wasn't empty, waiting tasks will eventually get events without waking
    }
}
