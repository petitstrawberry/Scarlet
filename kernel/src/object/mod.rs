//! Kernel object management system
//! 
//! This module provides a unified abstraction for all kernel-managed resources
//! including files, pipes, devices, and other IPC mechanisms.

pub mod capability;
pub mod introspection;
pub mod handle;

use alloc::{sync::Arc, vec::Vec};
use crate::fs::FileObject;
use crate::ipc::pipe::PipeObject;
use crate::ipc::event::{EventChannelObject, EventSubscriptionObject};
use crate::ipc::StreamIpcOps;
use capability::{StreamOps, CloneOps, ControlOps, MemoryMappingOps};

/// Unified representation of all kernel-managed resources
pub enum KernelObject {
    File(Arc<dyn FileObject>),
    Pipe(Arc<dyn PipeObject>),
    EventChannel(Arc<EventChannelObject>),
    EventSubscription(Arc<EventSubscriptionObject>),
    // Future variants will be added here:
    // MessageQueue(Arc<dyn MessageQueueObject>),
    // SharedMemory(Arc<dyn SharedMemoryObject>),
    // Socket(Arc<dyn SocketObject>),
    // CharDevice(Arc<dyn CharDevice>),
}

impl KernelObject {
    /// Create a KernelObject from a FileObject
    pub fn from_file_object(file_object: Arc<dyn FileObject>) -> Self {
        KernelObject::File(file_object)
    }
    
    /// Create a KernelObject from a PipeObject
    pub fn from_pipe_object(pipe_object: Arc<dyn PipeObject>) -> Self {
        KernelObject::Pipe(pipe_object)
    }

    /// Create a KernelObject from an EventChannelObject
    pub fn from_event_channel_object(event_channel: Arc<EventChannelObject>) -> Self {
        KernelObject::EventChannel(event_channel)
    }

    /// Create a KernelObject from an EventSubscriptionObject
    pub fn from_event_subscription(event_subscription: Arc<EventSubscriptionObject>) -> Self {
        KernelObject::EventSubscription(event_subscription)
    }
    
    /// Try to get StreamOps capability
    pub fn as_stream(&self) -> Option<&dyn StreamOps> {
        match self {
            KernelObject::File(file_object) => {
                // FileObject automatically implements StreamOps
                let stream_ops: &dyn StreamOps = file_object.as_ref();
                Some(stream_ops)
            }
            KernelObject::Pipe(pipe_object) => {
                // PipeObject automatically implements StreamOps
                let stream_ops: &dyn StreamOps = pipe_object.as_ref();
                Some(stream_ops)
            }
            KernelObject::EventChannel(_) => {
                // Event channels don't provide stream operations
                None
            }
            KernelObject::EventSubscription(_) => {
                // Event subscriptions don't provide stream operations
                None
            }
        }
    }
    
    /// Try to get StreamIpcOps capability for IPC stream operations
    pub fn as_stream_ipc(&self) -> Option<&dyn StreamIpcOps> {
        match self {
            KernelObject::File(_) => {
                // Files don't provide IPC stream operations
                None
            }
            KernelObject::Pipe(pipe_object) => {
                // PipeObject implements StreamIpcOps
                let stream_ipc_ops: &dyn StreamIpcOps = pipe_object.as_ref();
                Some(stream_ipc_ops)
            }
            KernelObject::EventChannel(_) => {
                // Event channels don't provide stream IPC operations
                None
            }
            KernelObject::EventSubscription(_) => {
                // Event subscriptions don't provide stream IPC operations
                None
            }
        }
    }
    
    /// Try to get FileObject that provides file-like operations and stream capabilities
    pub fn as_file(&self) -> Option<&dyn FileObject> {
        match self {
            KernelObject::File(file_object) => {
                // FileObject automatically implements StreamOps
                let file_ops: &dyn FileObject = file_object.as_ref();
                Some(file_ops)
            }
            KernelObject::Pipe(_) => {
                // Pipes don't provide file operations
                None
            }
            KernelObject::EventChannel(_) => {
                // Event channels don't provide file operations
                None
            }
            KernelObject::EventSubscription(_) => {
                // Event subscriptions don't provide file operations
                None
            }
        }
    }
    
    /// Try to get PipeObject that provides pipe-specific operations
    pub fn as_pipe(&self) -> Option<&dyn PipeObject> {
        match self {
            KernelObject::File(_) => {
                // Files don't provide pipe operations
                None
            }
            KernelObject::Pipe(pipe_object) => {
                let pipe_ops: &dyn PipeObject = pipe_object.as_ref();
                Some(pipe_ops)
            }
            KernelObject::EventChannel(_) => {
                // Event channels don't provide pipe operations
                None
            }
            KernelObject::EventSubscription(_) => {
                // Event subscriptions don't provide pipe operations
                None
            }
        }
    }
    
    /// Try to get CloneOps capability
    pub fn as_cloneable(&self) -> Option<&dyn CloneOps> {
        match self {
            KernelObject::File(_) => {
                None // Files do not implement CloneOps, use Arc::clone directly
            }
            KernelObject::Pipe(pipe_object) => {
                // Check if PipeObject implements CloneOps
                let cloneable: &dyn CloneOps = pipe_object.as_ref();
                Some(cloneable)
            }
            KernelObject::EventChannel(event_channel) => {
                // EventChannel implements CloneOps
                let cloneable: &dyn CloneOps = event_channel.as_ref();
                Some(cloneable)
            }
            KernelObject::EventSubscription(event_subscription) => {
                // EventSubscription implements CloneOps
                let cloneable: &dyn CloneOps = event_subscription.as_ref();
                Some(cloneable)
            }
        }
    }
    
    /// Try to get ControlOps capability
    pub fn as_control(&self) -> Option<&dyn ControlOps> {
        match self {
            KernelObject::File(file_object) => {
                // FileObject automatically implements ControlOps
                let control_ops: &dyn ControlOps = file_object.as_ref();
                Some(control_ops)
            }
            KernelObject::Pipe(_) => {
                // Pipes don't provide control operations
                None
            }
            KernelObject::EventChannel(_) => {
                // Event channels don't provide control operations
                None
            }
            KernelObject::EventSubscription(_) => {
                // Event subscriptions don't provide control operations
                None
            }
        }
    }
    
    /// Try to get MemoryMappingOps capability
    pub fn as_memory_mappable(&self) -> Option<&dyn MemoryMappingOps> {
        match self {
            KernelObject::File(file_object) => {
                // FileObject automatically implements MemoryMappingOps
                let memory_mapping_ops: &dyn MemoryMappingOps = file_object.as_ref();
                Some(memory_mapping_ops)
            }
            KernelObject::Pipe(_) => {
                // Pipes don't provide memory mapping operations
                None
            }
            KernelObject::EventChannel(_) => {
                // Event channels don't provide memory mapping operations
                None
            }
            KernelObject::EventSubscription(_) => {
                // Event subscriptions don't provide memory mapping operations
                None
            }
        }
    }

    /// Try to get weak reference to MemoryMappingOps capability
    pub fn as_memory_mappable_weak(&self) -> Option<alloc::sync::Weak<dyn MemoryMappingOps>> {
        match self {
            KernelObject::File(file_object) => {
                // Create weak reference from the Arc<dyn FileObject>
                // FileObject automatically implements MemoryMappingOps
                let weak_file = Arc::downgrade(file_object);
                Some(weak_file)
            }
            KernelObject::Pipe(_) => {
                // Pipes don't provide memory mapping operations
                None
            }
            KernelObject::EventChannel(_) => {
                // Event channels don't provide memory mapping operations
                None
            }
            KernelObject::EventSubscription(_) => {
                // Event subscriptions don't provide memory mapping operations
                None
            }
        }
    }

    /// Try to get EventChannelObject
    pub fn as_event_channel(&self) -> Option<&EventChannelObject> {
        match self {
            KernelObject::EventChannel(event_channel) => {
                let event_channel_obj: &EventChannelObject = event_channel.as_ref();
                Some(event_channel_obj)
            }
            _ => None
        }
    }
    
    /// Try to get EventSubscriptionObject
    pub fn as_event_subscription(&self) -> Option<&EventSubscriptionObject> {
        match self {
            KernelObject::EventSubscription(event_subscription) => {
                let event_subscription_obj: &EventSubscriptionObject = event_subscription.as_ref();
                Some(event_subscription_obj)
            }
            _ => None
        }
    }
}

impl Clone for KernelObject {
    fn clone(&self) -> Self {
        // Try to use CloneOps capability first
        if let Some(cloneable) = self.as_cloneable() {
            cloneable.custom_clone()
        } else {
            // Default: Use Arc::clone for direct cloning
            match self {
                KernelObject::File(file_object) => {
                    KernelObject::File(Arc::clone(file_object))
                }
                KernelObject::Pipe(pipe_object) => {
                    KernelObject::Pipe(Arc::clone(pipe_object))
                }
                KernelObject::EventChannel(event_channel) => {
                    KernelObject::EventChannel(Arc::clone(event_channel))
                }
                KernelObject::EventSubscription(event_subscription) => {
                    KernelObject::EventSubscription(Arc::clone(event_subscription))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;