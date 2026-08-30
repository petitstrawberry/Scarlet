//! Kernel object management system
//!
//! This module provides a unified abstraction for all kernel-managed resources
//! including files, pipes, devices, and other IPC mechanisms.

pub mod capability;
pub mod handle;
pub mod introspection;
pub mod timer;

use crate::device::gpu::GpuObject;
use crate::fs::FileObject;
use crate::ipc::StreamIpcOps;
use crate::ipc::counter::{Counter, CounterObject};
use crate::ipc::event::{EventChannelObject, EventSubscriptionObject};
use crate::ipc::pipe::PipeObject;
use crate::ipc::shared_memory::SharedMemoryObject;
use crate::object::timer::{Timer, TimerObject};
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
    Timer(Arc<dyn TimerObject>),
    EventChannel(Arc<EventChannelObject>),
    EventSubscription(Arc<EventSubscriptionObject>),
    #[cfg(feature = "network")]
    Socket(SocketHandle),
    SharedMemory(Arc<dyn SharedMemoryObject>),
    /// GPU child capability object with optional control, mapping, and readiness capabilities.
    Gpu(Arc<dyn GpuObject>),
    #[cfg(feature = "hypervisor")]
    HypervisorVm(hypervisor::VmRef),
    #[cfg(feature = "hypervisor")]
    HypervisorVcpu(hypervisor::VcpuRef),
    // Future variants will be added here:
    // MessageQueue(Arc<dyn MessageQueueObject>),
    // CharDevice(Arc<dyn CharDevice>),
}

/// Logical ownership wrapper for a socket kernel object.
///
/// Owning wrappers represent a user-visible handle, a pending fork/dup copy,
/// or a handle queued for IPC transfer. Temporary wrappers keep the socket
/// allocation alive for an in-flight syscall without delaying final handle
/// close side effects.
#[cfg(feature = "network")]
pub struct SocketHandle {
    socket: Arc<dyn SocketObject>,
    owns_reference: bool,
}

#[cfg(feature = "network")]
impl SocketHandle {
    /// Create a logical owning socket reference.
    ///
    /// # Arguments
    ///
    /// * `socket` - Socket object owned by the new reference.
    ///
    /// # Returns
    ///
    /// A wrapper that closes the socket when the last logical owner is
    /// released.
    fn new(socket: Arc<dyn SocketObject>) -> Self {
        crate::network::NetworkManager::get_manager().retain_socket_handle_reference(&socket);
        Self {
            socket,
            owns_reference: true,
        }
    }

    /// Duplicate this logical socket reference.
    ///
    /// # Returns
    ///
    /// A new owning reference suitable for fork, dup, or IPC transfer.
    fn duplicate(&self) -> Self {
        Self::new(Arc::clone(&self.socket))
    }

    /// Clone this socket only for temporary kernel access.
    ///
    /// # Returns
    ///
    /// A non-owning wrapper that does not delay final handle close.
    fn temporary_clone(&self) -> Self {
        Self {
            socket: Arc::clone(&self.socket),
            owns_reference: false,
        }
    }

    /// Promote a temporary wrapper to logical ownership.
    ///
    /// This is idempotent and is used when a kernel object enters a handle
    /// table through an API that may have received a temporary access clone.
    fn ensure_owning(&mut self) {
        if !self.owns_reference {
            crate::network::NetworkManager::get_manager()
                .retain_socket_handle_reference(&self.socket);
            self.owns_reference = true;
        }
    }

    /// Borrow the underlying socket `Arc`.
    ///
    /// # Returns
    ///
    /// The allocation-level socket reference without changing logical
    /// ownership.
    pub(crate) fn as_arc(&self) -> &Arc<dyn SocketObject> {
        &self.socket
    }

    /// Consume this wrapper and return an allocation-level socket clone.
    ///
    /// # Returns
    ///
    /// A temporary `Arc` to the underlying socket. Consuming an owning wrapper
    /// still releases its logical ownership.
    fn into_arc(self) -> Arc<dyn SocketObject> {
        Arc::clone(&self.socket)
    }
}

#[cfg(feature = "network")]
impl core::ops::Deref for SocketHandle {
    type Target = dyn SocketObject;

    fn deref(&self) -> &Self::Target {
        self.socket.as_ref()
    }
}

#[cfg(feature = "network")]
impl AsRef<dyn SocketObject + 'static> for SocketHandle {
    fn as_ref(&self) -> &(dyn SocketObject + 'static) {
        self.socket.as_ref()
    }
}

#[cfg(feature = "network")]
impl Drop for SocketHandle {
    fn drop(&mut self) {
        if self.owns_reference {
            crate::network::NetworkManager::get_manager()
                .release_socket_handle_reference(&self.socket);
        }
    }
}

/// Strong mapping owner that retains a GPU child object while a VMA exists.
struct GpuMemoryMappingOwner {
    gpu: Arc<dyn GpuObject>,
}

impl MemoryMappingOps for GpuMemoryMappingOwner {
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<capability::MemoryMappingInfo, &'static str> {
        self.gpu
            .as_memory_mappable()
            .ok_or("GPU object lost memory mapping capability")?
            .get_mapping_info(offset, length)
    }

    fn get_mapping_info_with(
        &self,
        offset: usize,
        length: usize,
        is_shared: bool,
    ) -> Result<capability::MemoryMappingInfo, &'static str> {
        self.gpu
            .as_memory_mappable()
            .ok_or("GPU object lost memory mapping capability")?
            .get_mapping_info_with(offset, length, is_shared)
    }

    fn on_mapped(&self, vaddr: usize, paddr: usize, length: usize, offset: usize) {
        if let Some(mapping) = self.gpu.as_memory_mappable() {
            mapping.on_mapped(vaddr, paddr, length, offset);
        }
    }

    fn on_unmapped(&self, vaddr: usize, length: usize) {
        if let Some(mapping) = self.gpu.as_memory_mappable() {
            mapping.on_unmapped(vaddr, length);
        }
    }

    fn supports_mmap(&self) -> bool {
        self.gpu
            .as_memory_mappable()
            .is_some_and(MemoryMappingOps::supports_mmap)
    }

    fn mmap_owner_name(&self) -> alloc::string::String {
        self.gpu.as_memory_mappable().map_or_else(
            || alloc::string::String::from("gpu"),
            MemoryMappingOps::mmap_owner_name,
        )
    }

    fn resolve_fault(
        &self,
        access: &capability::memory_mapping::AccessKind,
        page_idx: usize,
        vm_start: usize,
    ) -> Result<
        capability::memory_mapping::ResolveFaultResult,
        capability::memory_mapping::ResolveFaultError,
    > {
        self.gpu
            .as_memory_mappable()
            .ok_or(capability::memory_mapping::ResolveFaultError::Invalid)?
            .resolve_fault(access, page_idx, vm_start)
    }
}

/// Borrowed view of a kernel object that does not expose `Arc` ownership.
///
/// This wrapper is intended for handle-table access paths that only need to
/// inspect or operate on an object without extending its lifetime. It mirrors
/// the normal capability accessors on [`KernelObject`], but intentionally omits
/// APIs that create strong or weak references.
#[derive(Clone, Copy)]
pub struct KernelObjectRef<'a> {
    object: &'a KernelObject,
}

impl<'a> KernelObjectRef<'a> {
    /// Create a borrowed kernel object view.
    ///
    /// # Arguments
    ///
    /// * `object` - Kernel object borrowed from its owning handle table
    ///
    /// # Returns
    ///
    /// A borrowed view that cannot clone the underlying object.
    pub(crate) const fn new(object: &'a KernelObject) -> Self {
        Self { object }
    }

    #[inline]
    fn object(&self) -> &'a KernelObject {
        self.object
    }

    /// Get the underlying borrowed [`KernelObject`].
    ///
    /// # Returns
    ///
    /// Borrowed kernel object without extending object lifetime.
    pub(crate) fn as_kernel_object(&self) -> &'a KernelObject {
        self.object()
    }

    /// Get a human-readable object type name.
    ///
    /// # Returns
    ///
    /// Static object type name for diagnostics.
    pub fn type_name(&self) -> &'static str {
        match self.object() {
            KernelObject::File(_) => "File",
            KernelObject::Pipe(_) => "Pipe",
            KernelObject::Counter(_) => "Counter",
            KernelObject::Timer(_) => "Timer",
            KernelObject::EventChannel(_) => "EventChannel",
            KernelObject::EventSubscription(_) => "EventSubscription",
            KernelObject::SharedMemory(_) => "SharedMemory",
            KernelObject::Gpu(_) => "Gpu",
            #[cfg(feature = "network")]
            KernelObject::Socket(_) => "Socket",
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVm(_) => "HypervisorVm",
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVcpu(_) => "HypervisorVcpu",
        }
    }

    /// Try to get the hypervisor VM Arc by borrowed reference.
    ///
    /// # Returns
    ///
    /// Borrowed VM Arc if the object is a hypervisor VM.
    #[cfg(feature = "hypervisor")]
    pub(crate) fn as_hypervisor_vm_arc(&self) -> Option<&'a hypervisor::VmRef> {
        match self.object() {
            KernelObject::HypervisorVm(vm) => Some(vm),
            _ => None,
        }
    }

    /// Try to get the hypervisor vCPU object by borrowed reference.
    ///
    /// # Returns
    ///
    /// Borrowed vCPU object if the object is a hypervisor vCPU.
    #[cfg(feature = "hypervisor")]
    pub(crate) fn as_hypervisor_vcpu(&self) -> Option<&'a dyn hypervisor::VcpuObject> {
        match self.object() {
            KernelObject::HypervisorVcpu(vcpu) => Some(vcpu.as_ref()),
            _ => None,
        }
    }

    /// Try to get StreamOps capability.
    ///
    /// # Returns
    ///
    /// Stream capability if the borrowed object supports stream operations.
    pub fn as_stream(&self) -> Option<&'a dyn StreamOps> {
        self.object().as_stream()
    }

    /// Try to get StreamIpcOps capability for IPC stream operations.
    ///
    /// # Returns
    ///
    /// Stream IPC capability if the borrowed object supports IPC stream operations.
    pub fn as_stream_ipc(&self) -> Option<&'a dyn StreamIpcOps> {
        self.object().as_stream_ipc()
    }

    /// Try to get FileObject that provides file-like operations and stream capabilities.
    ///
    /// # Returns
    ///
    /// File capability if the borrowed object is file-like.
    pub fn as_file(&self) -> Option<&'a dyn FileObject> {
        self.object().as_file()
    }

    /// Try to get PipeObject that provides pipe-specific operations.
    ///
    /// # Returns
    ///
    /// Pipe capability if the borrowed object is a pipe.
    pub fn as_pipe(&self) -> Option<&'a dyn PipeObject> {
        self.object().as_pipe()
    }

    /// Try to get SocketObject that provides socket-specific operations.
    ///
    /// # Returns
    ///
    /// Socket capability if the borrowed object is a socket.
    #[cfg(feature = "network")]
    pub fn as_socket(&self) -> Option<&'a dyn SocketObject> {
        self.object().as_socket()
    }

    /// Try to get SharedMemoryObject that provides shared memory operations.
    ///
    /// # Returns
    ///
    /// Shared memory capability if the borrowed object is shared memory.
    pub fn as_shared_memory(&self) -> Option<&'a dyn SharedMemoryObject> {
        self.object().as_shared_memory()
    }

    /// Try to get a GPU child capability object.
    ///
    /// # Returns
    ///
    /// GPU object if this borrowed object is GPU-backed.
    pub fn as_gpu(&self) -> Option<&'a dyn GpuObject> {
        self.object().as_gpu()
    }

    /// Try to get ControlOps capability.
    ///
    /// # Returns
    ///
    /// Control capability if the borrowed object supports control operations.
    pub fn as_control(&self) -> Option<&'a dyn ControlOps> {
        self.object().as_control()
    }

    /// Try to get MemoryMappingOps capability.
    ///
    /// # Returns
    ///
    /// Memory mapping capability if the borrowed object can be mapped.
    pub fn as_memory_mappable(&self) -> Option<&'a dyn MemoryMappingOps> {
        self.object().as_memory_mappable()
    }

    /// Try to get EventChannelObject.
    ///
    /// # Returns
    ///
    /// Event channel object if the borrowed object is an event channel.
    pub fn as_event_channel(&self) -> Option<&'a EventChannelObject> {
        self.object().as_event_channel()
    }

    /// Try to get EventSubscriptionObject.
    ///
    /// # Returns
    ///
    /// Event subscription object if the borrowed object is an event subscription.
    pub fn as_event_subscription(&self) -> Option<&'a EventSubscriptionObject> {
        self.object().as_event_subscription()
    }

    /// Try to get CounterObject.
    ///
    /// # Returns
    ///
    /// Counter capability if the borrowed object is a counter.
    pub fn as_counter(&self) -> Option<&'a dyn CounterObject> {
        self.object().as_counter()
    }

    /// Try to get TimerObject.
    ///
    /// # Returns
    ///
    /// Timer capability if the borrowed object is a timer.
    pub fn as_timer(&self) -> Option<&'a dyn TimerObject> {
        self.object().as_timer()
    }

    /// Try to get Selectable capability for pselect/select readiness.
    ///
    /// # Returns
    ///
    /// Selectable capability if the borrowed object exposes readiness state.
    pub fn as_selectable(&self) -> Option<&'a dyn Selectable> {
        self.object().as_selectable()
    }
}

impl KernelObject {
    /// Ensure this object carries the logical ownership required by a handle slot.
    ///
    /// # Returns
    ///
    /// This method returns no value. Non-socket objects and already-owning
    /// socket objects are unchanged.
    pub(crate) fn ensure_handle_ownership(&mut self) {
        #[cfg(feature = "network")]
        if let KernelObject::Socket(socket) = self {
            socket.ensure_owning();
        }
    }

    /// Get the underlying kernel object by reference.
    pub(crate) fn as_kernel_object(&self) -> &KernelObject {
        self
    }

    /// Get a human-readable object type name.
    pub fn type_name(&self) -> &'static str {
        match self {
            KernelObject::File(_) => "File",
            KernelObject::Pipe(_) => "Pipe",
            KernelObject::Counter(_) => "Counter",
            KernelObject::Timer(_) => "Timer",
            KernelObject::EventChannel(_) => "EventChannel",
            KernelObject::EventSubscription(_) => "EventSubscription",
            KernelObject::SharedMemory(_) => "SharedMemory",
            KernelObject::Gpu(_) => "Gpu",
            #[cfg(feature = "network")]
            KernelObject::Socket(_) => "Socket",
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVm(_) => "HypervisorVm",
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVcpu(_) => "HypervisorVcpu",
        }
    }

    /// Try to get the hypervisor VM Arc by borrowed reference.
    #[cfg(feature = "hypervisor")]
    pub(crate) fn as_hypervisor_vm_arc(&self) -> Option<&hypervisor::VmRef> {
        match self {
            KernelObject::HypervisorVm(vm) => Some(vm),
            _ => None,
        }
    }

    /// Try to get the hypervisor vCPU object by borrowed reference.
    #[cfg(feature = "hypervisor")]
    pub(crate) fn as_hypervisor_vcpu(&self) -> Option<&dyn hypervisor::VcpuObject> {
        match self {
            KernelObject::HypervisorVcpu(vcpu) => Some(vcpu.as_ref()),
            _ => None,
        }
    }

    /// Consume this wrapper and return its socket Arc, if it is a socket.
    #[cfg(feature = "network")]
    pub(crate) fn into_socket_arc(self) -> Option<Arc<dyn SocketObject>> {
        match self {
            KernelObject::Socket(socket) => Some(socket.into_arc()),
            _ => None,
        }
    }

    /// Consume this wrapper and return its event-subscription Arc, if present.
    pub(crate) fn into_event_subscription_arc(self) -> Option<Arc<EventSubscriptionObject>> {
        match self {
            KernelObject::EventSubscription(subscription) => Some(subscription),
            _ => None,
        }
    }

    /// Consume this wrapper and return its hypervisor VM Arc, if present.
    #[cfg(feature = "hypervisor")]
    pub(crate) fn into_hypervisor_vm_arc(self) -> Option<hypervisor::VmRef> {
        match self {
            KernelObject::HypervisorVm(vm) => Some(vm),
            _ => None,
        }
    }

    /// Consume this wrapper and return its hypervisor vCPU Arc, if present.
    #[cfg(feature = "hypervisor")]
    pub(crate) fn into_hypervisor_vcpu_arc(self) -> Option<hypervisor::VcpuRef> {
        match self {
            KernelObject::HypervisorVcpu(vcpu) => Some(vcpu),
            _ => None,
        }
    }

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
        KernelObject::Socket(SocketHandle::new(socket))
    }

    /// Create a KernelObject from a SharedMemoryObject
    pub fn from_shared_memory_object(shared_memory: Arc<dyn SharedMemoryObject>) -> Self {
        KernelObject::SharedMemory(shared_memory)
    }

    /// Create a KernelObject from a GPU child capability object.
    pub fn from_gpu_object(gpu: Arc<dyn GpuObject>) -> Self {
        KernelObject::Gpu(gpu)
    }

    /// Create a KernelObject from a Counter
    pub fn from_counter(counter: Arc<Counter>) -> Self {
        KernelObject::Counter(counter as Arc<dyn CounterObject>)
    }

    /// Create a KernelObject from a Timer
    pub fn from_timer(timer: Arc<Timer>) -> Self {
        KernelObject::Timer(timer as Arc<dyn TimerObject>)
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
            KernelObject::Timer(timer) => {
                let stream_ops: &dyn StreamOps = timer.as_ref();
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
            KernelObject::Gpu(_) => None,
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
            KernelObject::Timer(_) => None,
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
            KernelObject::Gpu(_) => None,
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
            KernelObject::Timer(_) => None,
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
            KernelObject::Gpu(_) => None,
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
            KernelObject::Timer(_) => None,
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
            KernelObject::Gpu(_) => None,
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

    /// Try to get a GPU child capability object.
    ///
    /// # Returns
    ///
    /// GPU object if this object is GPU-backed.
    pub fn as_gpu(&self) -> Option<&dyn GpuObject> {
        match self {
            KernelObject::Gpu(gpu) => Some(gpu.as_ref()),
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
            KernelObject::Timer(_) => None,
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
            KernelObject::Gpu(_) => None,
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
            KernelObject::Timer(_) => None,
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
            KernelObject::Gpu(gpu) => gpu.as_control_ops(),
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
            KernelObject::Timer(_) => None,
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
            KernelObject::Gpu(gpu) => gpu.as_memory_mappable(),
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
            KernelObject::Timer(_) => None,
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
            KernelObject::Gpu(_) => None,
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVm(_) => None,
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVcpu(_) => None,
        }
    }

    /// Try to get strong reference to MemoryMappingOps capability
    pub fn as_memory_mappable_arc(&self) -> Option<Arc<dyn MemoryMappingOps>> {
        match self {
            KernelObject::File(file_object) => {
                Some(Arc::clone(file_object) as Arc<dyn MemoryMappingOps>)
            }
            KernelObject::Pipe(_) => None,
            KernelObject::Counter(_) => None,
            KernelObject::Timer(_) => None,
            #[cfg(feature = "network")]
            KernelObject::Socket(_) => None,
            KernelObject::EventChannel(_) => None,
            KernelObject::EventSubscription(_) => None,
            KernelObject::SharedMemory(shared_memory) => {
                Some(Arc::clone(shared_memory) as Arc<dyn MemoryMappingOps>)
            }
            KernelObject::Gpu(gpu) => gpu.as_memory_mappable().map(|_| {
                Arc::new(GpuMemoryMappingOwner {
                    gpu: Arc::clone(gpu),
                }) as Arc<dyn MemoryMappingOps>
            }),
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

    /// Try to get TimerObject
    pub fn as_timer(&self) -> Option<&dyn TimerObject> {
        match self {
            KernelObject::Timer(timer) => {
                let timer_obj: &dyn TimerObject = timer.as_ref();
                Some(timer_obj)
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
            KernelObject::Timer(timer) => {
                let sel: &dyn Selectable = timer.as_ref();
                Some(sel)
            }
            #[cfg(feature = "network")]
            KernelObject::Socket(socket) => socket.as_selectable(),
            KernelObject::EventChannel(_) => None,
            KernelObject::EventSubscription(_) => None,
            KernelObject::SharedMemory(_) => None,
            KernelObject::Gpu(gpu) => gpu.as_selectable(),
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
            KernelObject::Timer(timer) => KernelObject::Timer(Arc::clone(timer)),
            #[cfg(feature = "network")]
            KernelObject::Socket(socket) => KernelObject::Socket(socket.temporary_clone()),
            KernelObject::EventChannel(event_channel) => {
                KernelObject::EventChannel(Arc::clone(event_channel))
            }
            KernelObject::EventSubscription(event_subscription) => {
                KernelObject::EventSubscription(Arc::clone(event_subscription))
            }
            KernelObject::SharedMemory(shared_memory) => {
                KernelObject::SharedMemory(Arc::clone(shared_memory))
            }
            KernelObject::Gpu(gpu) => KernelObject::Gpu(Arc::clone(gpu)),
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
                KernelObject::Timer(timer) => KernelObject::Timer(Arc::clone(timer)),
                #[cfg(feature = "network")]
                KernelObject::Socket(socket) => KernelObject::Socket(socket.duplicate()),
                KernelObject::EventChannel(event_channel) => {
                    KernelObject::EventChannel(Arc::clone(event_channel))
                }
                KernelObject::EventSubscription(event_subscription) => {
                    KernelObject::EventSubscription(Arc::clone(event_subscription))
                }
                KernelObject::SharedMemory(shared_memory) => {
                    KernelObject::SharedMemory(Arc::clone(shared_memory))
                }
                KernelObject::Gpu(gpu) => KernelObject::Gpu(Arc::clone(gpu)),
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
