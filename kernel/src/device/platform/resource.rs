//! Platform device resource management module.
//! 
//! This module defines the `PlatformDeviceResource` struct and the `PlatformDeviceResourceType` enum,
//! which represent the resources associated with platform devices.
//! 

/// PlatformDeviceResource struct
/// 
/// This struct represents a resource associated with a platform device.
/// It contains the resource type (memory, I/O, IRQ, or DMA),
/// the starting address, and the ending address of the resource.
pub struct PlatformDeviceResource {
    pub res_type: PlatformDeviceResourceType,
    pub start: usize,
    pub end: usize,
}

/// PlatformDeviceResourceType enum
/// 
/// This enum defines the types of resources that can be associated with a platform device.
/// The types include memory (MEM), I/O (IO), interrupt request (IRQ), and direct memory access (DMA).
/// Each type is represented as a variant of the enum.
#[derive(PartialEq, Eq)]
pub enum PlatformDeviceResourceType {
    MEM,
    IO,
    IRQ,
    DMA,
}