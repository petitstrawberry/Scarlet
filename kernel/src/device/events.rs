//! Device event system for generic device communication.
//! 
//! This module provides a generic event system that allows devices to communicate
//! with each other without tight coupling.

extern crate alloc;
use core::any::Any;
use alloc::sync::Weak;
use alloc::vec::Vec;
use spin::Mutex;

/// Generic device event trait.
/// 
/// All device events must implement this trait to be handled by the event system.
pub trait DeviceEvent: Send + Sync {
    fn event_type(&self) -> &'static str;
    fn as_any(&self) -> &dyn Any;
}

/// Device event listener trait.
/// 
/// Devices that want to receive events must implement this trait.
pub trait DeviceEventListener: Send + Sync {
    fn on_device_event(&self, event: &dyn DeviceEvent);
    fn interested_in(&self, event_type: &str) -> bool;
}

/// Event capable device trait.
/// 
/// Devices that can emit events must implement this trait.
pub trait EventCapableDevice {
    fn register_event_listener(&self, listener: Weak<dyn DeviceEventListener>);
    fn unregister_event_listener(&self, listener_id: &str);
    fn emit_event(&self, event: &dyn DeviceEvent);
}

/// Generic device event emitter.
/// 
/// This struct provides a generic implementation for event emission.
pub struct DeviceEventEmitter {
    listeners: Mutex<Vec<Weak<dyn DeviceEventListener>>>,
}

impl DeviceEventEmitter {
    pub fn new() -> Self {
        Self {
            listeners: Mutex::new(Vec::new()),
        }
    }
    
    pub fn register_listener(&self, listener: Weak<dyn DeviceEventListener>) {
        let mut listeners = self.listeners.lock();
        listeners.push(listener);
    }
    
    pub fn emit(&self, event: &dyn DeviceEvent) {
        let mut listeners = self.listeners.lock();
        
        // Notify living listeners only and remove dead references
        listeners.retain(|weak_listener| {
            if let Some(listener) = weak_listener.upgrade() {
                if listener.interested_in(event.event_type()) {
                    listener.on_device_event(event);
                }
                true // Keep alive
            } else {
                crate::early_println!("Removing dead listener for event type: {}", event.event_type());
                false // Remove dead reference
            }
        });
    }
}

/// Input event for character devices.
/// 
/// This event is emitted when a character device receives input.
#[derive(Debug)]
pub struct InputEvent {
    pub data: u8,
}

impl DeviceEvent for InputEvent {
    fn event_type(&self) -> &'static str {
        "input"
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Output complete event for character devices.
/// 
/// This event is emitted when a character device completes output.
#[derive(Debug)]
pub struct OutputCompleteEvent;

impl DeviceEvent for OutputCompleteEvent {
    fn event_type(&self) -> &'static str {
        "output_complete"
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Device error event.
/// 
/// This event is emitted when a device encounters an error.
#[derive(Debug)]
pub struct ErrorEvent {
    pub error_code: u32,
}

impl DeviceEvent for ErrorEvent {
    fn event_type(&self) -> &'static str {
        "error"
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Interrupt capable device trait.
/// 
/// Devices that can handle interrupts must implement this trait.
pub trait InterruptCapableDevice: Send + Sync {
    fn handle_interrupt(&self) -> crate::interrupt::InterruptResult<()>;
    fn interrupt_id(&self) -> Option<crate::interrupt::InterruptId>;
}
