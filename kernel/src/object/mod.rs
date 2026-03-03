//! Kernel object management system
//!
//! This module provides a unified abstraction for all kernel-managed resources
//! including files, pipes, devices, and other IPC mechanisms.

pub mod capability;
pub mod handle;
pub mod introspection;

use crate::fs::FileObject;
use crate::ipc::counter::{Counter, CounterObject};
use crate::ipc::event::{EventChannelObject, EventSubscriptionObject};
use crate::ipc::pipe::PipeObject;
use crate::ipc::shared_memory::SharedMemoryObject;
use crate::ipc::StreamIpcOps;
use alloc::sync::Arc;
use capability::{CloneOps, ControlOps, MemoryMappingOps, Selectable, StreamOps};

#[cfg(feature = "network")]
use crate::network::SocketObject;

#[cfg(feature = "hypervisor")]
use crate::hypervisor;

/// Unified representation of all kernel-managed resources
///
/// Note: Debug is not implemented for KernelObject because it contains
/// trait objects that may not implement Debug. Use introspection methods instead.
pub enum KernelObject {
    File(Arc<dyn FileObject>),
    Pipe(Arc<dyn PipeObject>),
    Counter(Arc<dyn CounterObject>),
    EventChannel(Arc<EventChannelObject>),
    EventSubscription(Arc<EventSubscriptionObject>),
    #[cfg(feature = "network")]
    Socket(Arc<dyn SocketObject>),
    SharedMemory(Arc<dyn SharedMemoryObject>),
    #[cfg(feature = "hypervisor")]
    HypervisorVm(hypervisor::VmRef),
    #[cfg(feature = "hypervisor")]
    HypervisorVcpu(hypervisor::VcpuRef),
    // Future variants will be added here:
    // MessageQueue(Arc<dyn MessageQueueObject>),
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

    /// Create a KernelObject from a SocketObject
    #[cfg(feature = "network")]
    pub fn from_socket_object(socket: Arc<dyn SocketObject>) -> Self {
        KernelObject::Socket(socket)
    }

    /// Create a KernelObject from a SharedMemoryObject
    pub fn from_shared_memory_object(shared_memory: Arc<dyn SharedMemoryObject>) -> Self {
        KernelObject::SharedMemory(shared_memory)
    }

    /// Create a KernelObject from a Counter
    pub fn from_counter(counter: Arc<Counter>) -> Self {
        KernelObject::Counter(counter as Arc<dyn CounterObject>)
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
            KernelObject::Counter(counter) => {
                // CounterObject implements StreamOps
                let stream_ops: &dyn StreamOps = counter.as_ref();
                Some(stream_ops)
            }
            #[cfg(feature = "network")]
            KernelObject::Socket(socket) => {
                // SocketObject implements StreamOps
                let stream_ops: &dyn StreamOps = socket.as_ref();
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
            KernelObject::SharedMemory(_) => {
                // Shared memory doesn't provide stream operations
                None
            }
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVm(_) => None,
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVcpu(_) => None,
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
            KernelObject::Counter(_) => {
                // Counter doesn't provide IPC stream operations
                None
            }
            #[cfg(feature = "network")]
            KernelObject::Socket(socket) => {
                // SocketObject implements StreamIpcOps
                let stream_ipc_ops: &dyn StreamIpcOps = socket.as_ref();
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
            KernelObject::SharedMemory(_) => {
                // Shared memory doesn't provide stream IPC operations
                None
            }
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVm(_) => None,
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVcpu(_) => None,
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
            KernelObject::Counter(_) => {
                // Counter doesn't provide file operations
                None
            }
            #[cfg(feature = "network")]
            KernelObject::Socket(_) => {
                // Sockets don't provide file operations
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
            KernelObject::SharedMemory(_) => {
                // Shared memory doesn't provide file operations
                None
            }
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVm(_) => None,
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVcpu(_) => None,
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
            KernelObject::Counter(_) => {
                // Counter doesn't provide pipe operations
                None
            }
            #[cfg(feature = "network")]
            KernelObject::Socket(_) => {
                // Sockets don't provide pipe operations
                None
            }
            KernelObject::EventChannel(_) => {
                // Event channels don't provide pipe operations
                None
            }
            KernelObject::EventSubscription(_) => {
                // Event subscriptions don't provide pipe operations
                None
            }
            KernelObject::SharedMemory(_) => {
                // Shared memory doesn't provide pipe operations
                None
            }
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVm(_) => None,
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVcpu(_) => None,
        }
    }

    /// Try to get SocketObject that provides socket-specific operations
    #[cfg(feature = "network")]
    pub fn as_socket(&self) -> Option<&dyn SocketObject> {
        match self {
            KernelObject::Socket(socket) => {
                let socket_ops: &dyn SocketObject = socket.as_ref();
                Some(socket_ops)
            }
            _ => None,
        }
    }

    /// Try to get SharedMemoryObject that provides shared memory operations
    pub fn as_shared_memory(&self) -> Option<&dyn SharedMemoryObject> {
        match self {
            KernelObject::SharedMemory(shared_memory) => {
                let shmem_ops: &dyn SharedMemoryObject = shared_memory.as_ref();
                Some(shmem_ops)
            }
            _ => None,
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
            KernelObject::Counter(counter) => {
                // CounterObject implements CloneOps
                let cloneable: &dyn CloneOps = counter.as_ref();
                Some(cloneable)
            }
            #[cfg(feature = "network")]
            KernelObject::Socket(_) => {
                // Sockets don't implement CloneOps, use Arc::clone directly
                None
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
            KernelObject::SharedMemory(_) => {
                // Shared memory doesn't implement CloneOps, use Arc::clone directly
                None
            }
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVm(_) => None,
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVcpu(_) => None,
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
            KernelObject::Counter(_) => {
                // Counter doesn't provide control operations
                None
            }
            #[cfg(feature = "network")]
            KernelObject::Socket(socket) => {
                // Try to get control operations through SocketObject trait
                socket.as_control_ops()
            }
            KernelObject::EventChannel(_) => {
                // Event channels don't provide control operations
                None
            }
            KernelObject::EventSubscription(_) => {
                // Event subscriptions don't provide control operations
                None
            }
            KernelObject::SharedMemory(_) => {
                // Shared memory doesn't provide control operations
                None
            }
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVm(vm) => {
                let control_ops: &dyn ControlOps = vm.as_ref();
                Some(control_ops)
            }
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVcpu(vcpu) => {
                let control_ops: &dyn ControlOps = vcpu.as_ref();
                Some(control_ops)
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
            KernelObject::Counter(_) => {
                // Counter doesn't provide memory mapping operations
                None
            }
            #[cfg(feature = "network")]
            KernelObject::Socket(_) => {
                // Sockets don't provide memory mapping operations
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
            KernelObject::SharedMemory(shared_memory) => {
                // SharedMemory implements MemoryMappingOps
                let memory_mapping_ops: &dyn MemoryMappingOps = shared_memory.as_ref();
                Some(memory_mapping_ops)
            }
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVm(_) => None,
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVcpu(_) => None,
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
            KernelObject::Counter(_) => {
                // Counter doesn't provide memory mapping operations
                None
            }
            #[cfg(feature = "network")]
            KernelObject::Socket(_) => {
                // Sockets don't provide memory mapping operations
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
            KernelObject::SharedMemory(shared_memory) => {
                // Create weak reference from the Arc<dyn SharedMemoryObject>
                // SharedMemoryObject implements MemoryMappingOps
                let weak_shmem = Arc::downgrade(shared_memory);
                Some(weak_shmem)
            }
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVm(_) => None,
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVcpu(_) => None,
        }
    }

    /// Try to get EventChannelObject
    pub fn as_event_channel(&self) -> Option<&EventChannelObject> {
        match self {
            KernelObject::EventChannel(event_channel) => {
                let event_channel_obj: &EventChannelObject = event_channel.as_ref();
                Some(event_channel_obj)
            }
            _ => None,
        }
    }

    /// Try to get EventSubscriptionObject
    pub fn as_event_subscription(&self) -> Option<&EventSubscriptionObject> {
        match self {
            KernelObject::EventSubscription(event_subscription) => {
                let event_subscription_obj: &EventSubscriptionObject = event_subscription.as_ref();
                Some(event_subscription_obj)
            }
            _ => None,
        }
    }

    /// Try to get CounterObject
    pub fn as_counter(&self) -> Option<&dyn CounterObject> {
        match self {
            KernelObject::Counter(counter) => {
                let counter_obj: &dyn CounterObject = counter.as_ref();
                Some(counter_obj)
            }
            _ => None,
        }
    }

    /// Try to get Selectable capability for pselect/select readiness
    pub fn as_selectable(&self) -> Option<&dyn Selectable> {
        match self {
            KernelObject::File(file_object) => {
                // FileObject requires Selectable; upcast trait object
                let sel: &dyn Selectable = file_object.as_ref();
                Some(sel)
            }
            KernelObject::Pipe(pipe_object) => pipe_object.as_selectable(),
            KernelObject::Counter(counter) => {
                // CounterObject implements Selectable
                let sel: &dyn Selectable = counter.as_ref();
                Some(sel)
            }
            #[cfg(feature = "network")]
            KernelObject::Socket(socket) => socket.as_selectable(),
            KernelObject::EventChannel(_) => None,
            KernelObject::EventSubscription(_) => None,
            KernelObject::SharedMemory(_) => None,
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVm(_) => None,
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVcpu(_) => None,
        }
    }

    /// Clone the KernelObject at the Arc level only (no state changes).
    ///
    /// Unlike the `Clone` trait implementation which may use `custom_clone()` for
    /// objects like Pipes (incrementing reader/writer counts), this method performs
    /// a simple `Arc::clone()` that only increments the Arc reference count without
    /// modifying the underlying object state.
    ///
    /// Use this when you need a copy of the KernelObject for temporary access
    /// without intending to create a new logical file descriptor (dup semantics).
    pub fn arc_clone(&self) -> Self {
        match self {
            KernelObject::File(file_object) => KernelObject::File(Arc::clone(file_object)),
            KernelObject::Pipe(pipe_object) => KernelObject::Pipe(Arc::clone(pipe_object)),
            KernelObject::Counter(counter) => KernelObject::Counter(Arc::clone(counter)),
            #[cfg(feature = "network")]
            KernelObject::Socket(socket) => KernelObject::Socket(Arc::clone(socket)),
            KernelObject::EventChannel(event_channel) => {
                KernelObject::EventChannel(Arc::clone(event_channel))
            }
            KernelObject::EventSubscription(event_subscription) => {
                KernelObject::EventSubscription(Arc::clone(event_subscription))
            }
            KernelObject::SharedMemory(shared_memory) => {
                KernelObject::SharedMemory(Arc::clone(shared_memory))
            }
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVm(vm) => KernelObject::HypervisorVm(Arc::clone(vm)),
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVcpu(vcpu) => KernelObject::HypervisorVcpu(Arc::clone(vcpu)),
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
                KernelObject::File(file_object) => KernelObject::File(Arc::clone(file_object)),
                KernelObject::Pipe(pipe_object) => KernelObject::Pipe(Arc::clone(pipe_object)),
                KernelObject::Counter(counter) => KernelObject::Counter(Arc::clone(counter)),
                #[cfg(feature = "network")]
                KernelObject::Socket(socket) => KernelObject::Socket(Arc::clone(socket)),
                KernelObject::EventChannel(event_channel) => {
                    KernelObject::EventChannel(Arc::clone(event_channel))
                }
                KernelObject::EventSubscription(event_subscription) => {
                    KernelObject::EventSubscription(Arc::clone(event_subscription))
                }
                KernelObject::SharedMemory(shared_memory) => {
                    KernelObject::SharedMemory(Arc::clone(shared_memory))
                }
                #[cfg(feature = "hypervisor")]
                KernelObject::HypervisorVm(vm) => KernelObject::HypervisorVm(Arc::clone(vm)),
                #[cfg(feature = "hypervisor")]
                KernelObject::HypervisorVcpu(vcpu) => {
                    KernelObject::HypervisorVcpu(Arc::clone(vcpu))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
