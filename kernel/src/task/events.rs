//! Task Event System
//! 
//! This module provides an abstract event/notification system for tasks.
//! It serves as the kernel abstraction layer for OS-specific concepts like
//! POSIX signals, Windows events, or other notification mechanisms.
//! 
//! The design is intentionally generic to support multiple ABI implementations
//! without being tied to any specific OS paradigm.

extern crate alloc;

use alloc::{boxed::Box, collections::BTreeMap, sync::Arc, vec::Vec, string::String};
use spin::Mutex;
use core::any::Any;

/// Global task event registry
/// Maps task ID to their event handlers
static TASK_EVENT_HANDLERS: Mutex<BTreeMap<usize, Arc<TaskEventHandler>>> = Mutex::new(BTreeMap::new());

/// Global event channel registry for pub/sub pattern
/// Maps channel name to subscriber list
static EVENT_CHANNELS: Mutex<BTreeMap<String, Vec<usize>>> = Mutex::new(BTreeMap::new());

/// Global process group registry
/// Maps process group ID to task list
static PROCESS_GROUPS: Mutex<BTreeMap<usize, Vec<usize>>> = Mutex::new(BTreeMap::new());

/// Abstract task event types
/// 
/// These represent common event categories that most operating systems support
/// in some form. ABI modules can map their specific signals/events to these
/// abstract types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TaskEventType {
    /// Process termination request (SIGTERM equivalent)
    Terminate,
    /// Immediate termination (SIGKILL equivalent)  
    Kill,
    /// Interrupt from terminal (SIGINT equivalent)
    Interrupt,
    /// Suspend execution (SIGSTOP equivalent)
    Suspend,
    /// Resume execution (SIGCONT equivalent)  
    Resume,
    /// Child process state change (SIGCHLD equivalent)
    ChildStateChange,
    /// Alarm/timer expiration (SIGALRM equivalent)
    Timer,
    /// User-defined event (SIGUSR1/SIGUSR2 equivalent)
    User(u32),
    /// I/O ready notification (SIGIO equivalent)
    IoReady,
    /// Pipe/channel broken (SIGPIPE equivalent)
    PipeBroken,
    /// Window size change (SIGWINCH equivalent)
    WindowChange,
    /// Custom ABI-specific event
    Custom(u32),
}

/// Event delivery action
/// 
/// Defines what should happen when an event is delivered to a task
#[derive(Clone)]
pub enum EventAction {
    /// Ignore the event
    Ignore,
    /// Terminate the task
    Terminate,
    /// Suspend the task
    Suspend,
    /// Resume the task
    Resume,
    /// Call a custom handler function
    Handler(EventHandlerFn),
    /// Default system behavior for this event type
    Default,
}

/// Custom event handler function type
/// 
/// ABI modules can register custom handlers that will be called
/// when events are delivered.
pub type EventHandlerFn = Arc<dyn Fn(&TaskEvent) + Send + Sync>;

/// Task event data
/// 
/// Contains information about an event being delivered to a task
#[derive(Debug)]
pub struct TaskEvent {
    /// Type of event
    pub event_type: TaskEventType,
    /// Source task ID (if applicable)
    pub source_task: Option<usize>,
    /// Target task ID
    pub target_task: usize,
    /// Additional event-specific data
    pub data: Option<Box<dyn Any + Send + Sync>>,
    /// Timestamp when event was created
    pub timestamp: u64,
}

impl TaskEvent {
    /// Create a new task event
    pub fn new(event_type: TaskEventType, target_task: usize) -> Self {
        Self {
            event_type,
            source_task: None,
            target_task,
            data: None,
            timestamp: crate::timer::get_tick(),
        }
    }
    
    /// Create a new task event with source
    pub fn new_with_source(event_type: TaskEventType, source_task: usize, target_task: usize) -> Self {
        Self {
            event_type,
            source_task: Some(source_task),
            target_task,
            data: None,
            timestamp: crate::timer::get_tick(),
        }
    }
    
    /// Add custom data to the event
    pub fn with_data<T: Any + Send + Sync>(mut self, data: T) -> Self {
        self.data = Some(Box::new(data));
        self
    }
}

/// Task event handler
/// 
/// Manages event delivery and handling for a specific task
pub struct TaskEventHandler {
    /// Event action mappings
    actions: Mutex<BTreeMap<TaskEventType, EventAction>>,
    /// Pending events queue
    pending_events: Mutex<Vec<TaskEvent>>,
    /// Whether event handling is currently blocked
    blocked: Mutex<bool>,
    /// Custom event type counter for ABI-specific events
    next_custom_event: Mutex<u32>,
}

impl TaskEventHandler {
    /// Create a new task event handler
    pub fn new(_task_id: usize) -> Self {
        Self {
            actions: Mutex::new(BTreeMap::new()),
            pending_events: Mutex::new(Vec::new()),
            blocked: Mutex::new(false),
            next_custom_event: Mutex::new(1),
        }
    }
    
    /// Set action for an event type
    pub fn set_action(&self, event_type: TaskEventType, action: EventAction) {
        let mut actions = self.actions.lock();
        actions.insert(event_type, action);
    }
    
    /// Get action for an event type
    pub fn get_action(&self, event_type: TaskEventType) -> EventAction {
        let actions = self.actions.lock();
        actions.get(&event_type).cloned().unwrap_or_else(|| Self::get_default_action(event_type))
    }

    /// Get the default action for an event type
    fn get_default_action(event_type: TaskEventType) -> EventAction {
        match event_type {
            TaskEventType::Terminate => EventAction::Terminate,
            TaskEventType::Kill => EventAction::Terminate,
            TaskEventType::Interrupt => EventAction::Terminate,
            TaskEventType::Suspend => EventAction::Suspend,
            TaskEventType::Resume => EventAction::Resume,
            TaskEventType::ChildStateChange => EventAction::Ignore,
            TaskEventType::Timer => EventAction::Default,
            TaskEventType::User(_) => EventAction::Ignore,
            TaskEventType::IoReady => EventAction::Default,
            TaskEventType::PipeBroken => EventAction::Terminate,
            TaskEventType::WindowChange => EventAction::Ignore,
            TaskEventType::Custom(_) => EventAction::Default,
        }
    }
    
    /// Register a new custom event type
    pub fn register_custom_event(&self) -> TaskEventType {
        let mut counter = self.next_custom_event.lock();
        let event_id = *counter;
        *counter += 1;
        TaskEventType::Custom(event_id)
    }
    
    /// Queue an event for delivery
    pub fn queue_event(&self, event: TaskEvent) {
        let mut pending = self.pending_events.lock();
        pending.push(event);
    }
    
    /// Process pending events
    /// 
    /// This should be called by the scheduler when the task is about to run
    pub fn process_pending_events(&self) -> Vec<EventAction> {
        let blocked = *self.blocked.lock();
        if blocked {
            return Vec::new();
        }
        
        let mut pending = self.pending_events.lock();
        let events = pending.drain(..).collect::<Vec<_>>();
        drop(pending);
        
        let mut actions = Vec::new();
        let action_map = self.actions.lock();
        
        for event in events {
            let action = action_map.get(&event.event_type).cloned().unwrap_or_else(|| {
                // Default actions for standard events when not explicitly set
                match event.event_type {
                    TaskEventType::Terminate => EventAction::Terminate,
                    TaskEventType::Kill => EventAction::Terminate,
                    TaskEventType::Interrupt => EventAction::Terminate,
                    TaskEventType::Suspend => EventAction::Suspend,
                    TaskEventType::Resume => EventAction::Resume,
                    TaskEventType::ChildStateChange => EventAction::Ignore,
                    TaskEventType::Timer => EventAction::Default,
                    TaskEventType::User(_) => EventAction::Ignore,
                    TaskEventType::IoReady => EventAction::Default,
                    TaskEventType::PipeBroken => EventAction::Terminate,
                    TaskEventType::WindowChange => EventAction::Ignore,
                    TaskEventType::Custom(_) => EventAction::Default,
                }
            });
            
            match action {
                EventAction::Handler(handler) => {
                    // Call custom handler
                    handler(&event);
                },
                other_action => {
                    actions.push(other_action);
                }
            }
        }
        
        actions
    }
    
    /// Block event delivery
    pub fn block_events(&self) {
        let mut blocked = self.blocked.lock();
        *blocked = true;
    }
    
    /// Unblock event delivery
    pub fn unblock_events(&self) {
        let mut blocked = self.blocked.lock();
        *blocked = false;
    }
    
    /// Check if events are blocked
    pub fn are_events_blocked(&self) -> bool {
        *self.blocked.lock()
    }
}

/// Event delivery target specification
#[derive(Debug, Clone)]
pub enum EventTarget {
    /// Send to specific task
    Task(usize),
    /// Send to all tasks in process group
    ProcessGroup(usize),
    /// Send to all tasks (broadcast)
    Broadcast,
    /// Send to specific list of tasks (multicast)
    TaskList(Vec<usize>),
    /// Send to all subscribers of named channel
    Channel(String),
}

/// Get or create task event handler
pub fn get_task_event_handler(task_id: usize) -> Arc<TaskEventHandler> {
    let mut handlers = TASK_EVENT_HANDLERS.lock();
    
    if let Some(handler) = handlers.get(&task_id) {
        handler.clone()
    } else {
        let handler = Arc::new(TaskEventHandler::new(task_id));
        handlers.insert(task_id, handler.clone());
        handler
    }
}

/// Send an event to a task
pub fn send_task_event(event: TaskEvent) -> Result<(), &'static str> {
    let target_task = event.target_task;
    let handler = get_task_event_handler(target_task);
    handler.queue_event(event);
    
    // Wake up the target task if it's sleeping
    crate::task::get_task_waker(target_task).wake_all();
    
    Ok(())
}

/// Send an event to all children of a task
pub fn send_event_to_children(parent_task_id: usize, event_type: TaskEventType) -> Result<(), &'static str> {
    // Get parent task to access children list
    if let Some(parent_task) = crate::sched::scheduler::get_scheduler().get_task_by_id(parent_task_id) {
        let children = parent_task.get_children().clone();
        
        for child_id in children {
            let event = TaskEvent::new_with_source(event_type, parent_task_id, child_id);
            send_task_event(event)?;
        }
    }
    
    Ok(())
}

/// Generic event sending function that supports multiple delivery targets
pub fn send_event_to_target(event_type: TaskEventType, target: EventTarget, source_task: Option<usize>) -> Result<usize, &'static str> {
    let mut sent_count = 0;
    
    match target {
        EventTarget::Task(task_id) => {
            let event = if let Some(source) = source_task {
                TaskEvent::new_with_source(event_type, source, task_id)
            } else {
                TaskEvent::new(event_type, task_id)
            };
            
            send_task_event(event)?;
            sent_count = 1;
        },
        
        EventTarget::ProcessGroup(group_id) => {
            let groups = PROCESS_GROUPS.lock();
            if let Some(task_list) = groups.get(&group_id) {
                for &task_id in task_list {
                    let event = if let Some(source) = source_task {
                        TaskEvent::new_with_source(event_type, source, task_id)
                    } else {
                        TaskEvent::new(event_type, task_id)
                    };
                    
                    send_task_event(event)?;
                    sent_count += 1;
                }
            }
        },
        
        EventTarget::Broadcast => {
            // For broadcast, we'll iterate through the handler registry
            // This gives us all active tasks with event handlers
            let handlers = TASK_EVENT_HANDLERS.lock();
            let task_ids: Vec<usize> = handlers.keys().cloned().collect();
            drop(handlers);
            
            for task_id in task_ids {
                // Don't send to source task (avoid self-signal by default)
                if Some(task_id) == source_task {
                    continue;
                }
                
                let event = if let Some(source) = source_task {
                    TaskEvent::new_with_source(event_type, source, task_id)
                } else {
                    TaskEvent::new(event_type, task_id)
                };
                
                send_task_event(event)?;
                sent_count += 1;
            }
        },
        
        EventTarget::TaskList(task_list) => {
            for task_id in task_list {
                let event = if let Some(source) = source_task {
                    TaskEvent::new_with_source(event_type, source, task_id)
                } else {
                    TaskEvent::new(event_type, task_id)
                };
                
                send_task_event(event)?;
                sent_count += 1;
            }
        },
        
        EventTarget::Channel(channel_name) => {
            let channels = EVENT_CHANNELS.lock();
            if let Some(subscribers) = channels.get(&channel_name) {
                for &task_id in subscribers {
                    let event = if let Some(source) = source_task {
                        TaskEvent::new_with_source(event_type, source, task_id)
                    } else {
                        TaskEvent::new(event_type, task_id)
                    };
                    
                    send_task_event(event)?;
                    sent_count += 1;
                }
            }
        },
    }
    
    Ok(sent_count)
}

/// Remove task event handler when task exits
pub fn cleanup_task_events(task_id: usize) {
    let mut handlers = TASK_EVENT_HANDLERS.lock();
    handlers.remove(&task_id);
}

/// Get pending event count for a task
pub fn get_pending_event_count(task_id: usize) -> usize {
    if let Some(handler) = TASK_EVENT_HANDLERS.lock().get(&task_id) {
        handler.pending_events.lock().len()
    } else {
        0
    }
}

/// Convenience functions for ABI modules to interact with the event system
/// 
/// These functions provide a higher-level interface for common event operations
/// that ABI modules typically need to perform.

/// Send a termination signal to a task
pub fn send_terminate_event(target_task: usize, source_task: Option<usize>) -> Result<(), &'static str> {
    let event = if let Some(source) = source_task {
        TaskEvent::new_with_source(TaskEventType::Terminate, source, target_task)
    } else {
        TaskEvent::new(TaskEventType::Terminate, target_task)
    };
    send_task_event(event)
}

/// Send a kill signal to a task (non-ignorable termination)
pub fn send_kill_event(target_task: usize, source_task: Option<usize>) -> Result<(), &'static str> {
    let event = if let Some(source) = source_task {
        TaskEvent::new_with_source(TaskEventType::Kill, source, target_task)
    } else {
        TaskEvent::new(TaskEventType::Kill, target_task)
    };
    send_task_event(event)
}

/// Send an interrupt signal to a task (Ctrl+C equivalent)
pub fn send_interrupt_event(target_task: usize, source_task: Option<usize>) -> Result<(), &'static str> {
    let event = if let Some(source) = source_task {
        TaskEvent::new_with_source(TaskEventType::Interrupt, source, target_task)
    } else {
        TaskEvent::new(TaskEventType::Interrupt, target_task)
    };
    send_task_event(event)
}

/// Send a suspend signal to a task
pub fn send_suspend_event(target_task: usize, source_task: Option<usize>) -> Result<(), &'static str> {
    let event = if let Some(source) = source_task {
        TaskEvent::new_with_source(TaskEventType::Suspend, source, target_task)
    } else {
        TaskEvent::new(TaskEventType::Suspend, target_task)
    };
    send_task_event(event)
}

/// Send a resume signal to a task
pub fn send_resume_event(target_task: usize, source_task: Option<usize>) -> Result<(), &'static str> {
    let event = if let Some(source) = source_task {
        TaskEvent::new_with_source(TaskEventType::Resume, source, target_task)
    } else {
        TaskEvent::new(TaskEventType::Resume, target_task)
    };
    send_task_event(event)
}

/// Send a user-defined event to a task
pub fn send_user_event(user_id: u32, target_task: usize, source_task: Option<usize>, data: Option<Box<dyn Any + Send + Sync>>) -> Result<(), &'static str> {
    let mut event = if let Some(source) = source_task {
        TaskEvent::new_with_source(TaskEventType::User(user_id), source, target_task)
    } else {
        TaskEvent::new(TaskEventType::User(user_id), target_task)
    };
    
    if let Some(event_data) = data {
        event.data = Some(event_data);
    }
    
    send_task_event(event)
}

/// Send a timer event to a task
pub fn send_timer_event(target_task: usize, timer_id: u32) -> Result<(), &'static str> {
    let event = TaskEvent::new(TaskEventType::Timer, target_task).with_data(timer_id);
    send_task_event(event)
}

/// Send a pipe broken event to a task
pub fn send_pipe_broken_event(target_task: usize) -> Result<(), &'static str> {
    let event = TaskEvent::new(TaskEventType::PipeBroken, target_task);
    send_task_event(event)
}

/// Send an I/O ready event to a task
pub fn send_io_ready_event(target_task: usize, fd: i32) -> Result<(), &'static str> {
    let event = TaskEvent::new(TaskEventType::IoReady, target_task).with_data(fd);
    send_task_event(event)
}

/// Block events for a task (useful during critical sections)
pub fn block_task_events(task_id: usize) {
    let handler = get_task_event_handler(task_id);
    handler.block_events();
}

/// Unblock events for a task
pub fn unblock_task_events(task_id: usize) {
    let handler = get_task_event_handler(task_id);
    handler.unblock_events();
}

/// Check if a task has pending events
pub fn has_pending_events(task_id: usize) -> bool {
    get_pending_event_count(task_id) > 0
}

/// Register a custom event handler for a specific event type
pub fn register_event_handler(task_id: usize, event_type: TaskEventType, handler: EventHandlerFn) {
    let event_handler = get_task_event_handler(task_id);
    event_handler.set_action(event_type, EventAction::Handler(handler));
}

/// Set event action for a task (ignore, terminate, etc.)
pub fn set_event_action(task_id: usize, event_type: TaskEventType, action: EventAction) {
    let event_handler = get_task_event_handler(task_id);
    event_handler.set_action(event_type, action);
}

/// Get a new custom event type for ABI-specific use
pub fn register_custom_event_type(task_id: usize) -> TaskEventType {
    let handler = get_task_event_handler(task_id);
    handler.register_custom_event()
}

// ===== Channel Management Functions =====

/// Subscribe a task to an event channel
pub fn subscribe_to_channel(task_id: usize, channel_name: String) -> Result<(), &'static str> {
    let mut channels = EVENT_CHANNELS.lock();
    let subscribers = channels.entry(channel_name).or_insert_with(Vec::new);
    
    if !subscribers.contains(&task_id) {
        subscribers.push(task_id);
    }
    
    Ok(())
}

/// Unsubscribe a task from an event channel
pub fn unsubscribe_from_channel(task_id: usize, channel_name: &str) -> Result<(), &'static str> {
    let mut channels = EVENT_CHANNELS.lock();
    if let Some(subscribers) = channels.get_mut(channel_name) {
        subscribers.retain(|&id| id != task_id);
        
        // Remove empty channels
        if subscribers.is_empty() {
            channels.remove(channel_name);
        }
    }
    
    Ok(())
}

/// Get list of all subscribers to a channel
pub fn get_channel_subscribers(channel_name: &str) -> Vec<usize> {
    let channels = EVENT_CHANNELS.lock();
    channels.get(channel_name).cloned().unwrap_or_default()
}

/// Get list of all available channels
pub fn list_channels() -> Vec<String> {
    let channels = EVENT_CHANNELS.lock();
    channels.keys().cloned().collect()
}

// ===== Process Group Management Functions =====

/// Add a task to a process group
pub fn add_task_to_process_group(task_id: usize, group_id: usize) -> Result<(), &'static str> {
    let mut groups = PROCESS_GROUPS.lock();
    let group_members = groups.entry(group_id).or_insert_with(Vec::new);
    
    if !group_members.contains(&task_id) {
        group_members.push(task_id);
    }
    
    Ok(())
}

/// Remove a task from a process group
pub fn remove_task_from_process_group(task_id: usize, group_id: usize) -> Result<(), &'static str> {
    let mut groups = PROCESS_GROUPS.lock();
    if let Some(group_members) = groups.get_mut(&group_id) {
        group_members.retain(|&id| id != task_id);
        
        // Remove empty groups
        if group_members.is_empty() {
            groups.remove(&group_id);
        }
    }
    
    Ok(())
}

/// Get all tasks in a process group
pub fn get_process_group_members(group_id: usize) -> Vec<usize> {
    let groups = PROCESS_GROUPS.lock();
    groups.get(&group_id).cloned().unwrap_or_default()
}

/// Get list of all process groups
pub fn list_process_groups() -> Vec<usize> {
    let groups = PROCESS_GROUPS.lock();
    groups.keys().cloned().collect()
}

// ===== Enhanced Convenience Functions =====

/// Broadcast an event to all tasks
pub fn broadcast_event(event_type: TaskEventType, source_task: Option<usize>) -> Result<usize, &'static str> {
    send_event_to_target(event_type, EventTarget::Broadcast, source_task)
}

/// Send event to multiple specific tasks
pub fn multicast_event(event_type: TaskEventType, task_list: Vec<usize>, source_task: Option<usize>) -> Result<usize, &'static str> {
    send_event_to_target(event_type, EventTarget::TaskList(task_list), source_task)
}

/// Send event to all tasks in a process group
pub fn send_event_to_process_group(event_type: TaskEventType, group_id: usize, source_task: Option<usize>) -> Result<usize, &'static str> {
    send_event_to_target(event_type, EventTarget::ProcessGroup(group_id), source_task)
}

/// Publish event to all subscribers of a channel
pub fn publish_to_channel(event_type: TaskEventType, channel_name: String, source_task: Option<usize>) -> Result<usize, &'static str> {
    send_event_to_target(event_type, EventTarget::Channel(channel_name), source_task)
}

/// Cleanup function to remove task from all channels and groups when it exits
pub fn cleanup_task_from_all_groups_and_channels(task_id: usize) {
    // Remove from all channels
    let mut channels = EVENT_CHANNELS.lock();
    let channel_names: Vec<String> = channels.keys().cloned().collect();
    for channel_name in channel_names {
        if let Some(subscribers) = channels.get_mut(&channel_name) {
            subscribers.retain(|&id| id != task_id);
            if subscribers.is_empty() {
                channels.remove(&channel_name);
            }
        }
    }
    drop(channels);
    
    // Remove from all process groups
    let mut groups = PROCESS_GROUPS.lock();
    let group_ids: Vec<usize> = groups.keys().cloned().collect();
    for group_id in group_ids {
        if let Some(members) = groups.get_mut(&group_id) {
            members.retain(|&id| id != task_id);
            if members.is_empty() {
                groups.remove(&group_id);
            }
        }
    }
    drop(groups);
    
    // Also cleanup existing event handlers
    cleanup_task_events(task_id);
}
