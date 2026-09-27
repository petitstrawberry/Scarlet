//! Native virtual interrupt event control records (independent of Linux KVM).

pub const VM_CREATE_IRQ_EVENT: u32 = 0x04;
pub const VM_REMOVE_IRQ_EVENT: u32 = 0x05;
/// Hold a level until guest deactivation, then request device re-evaluation.
pub const IRQ_EVENT_RESAMPLE: u32 = 1;
pub const IRQ_EVENT_NONBLOCK: u32 = 2;

/// CREATE inputs: vcpu, interrupt, flags; remaining fields must be zero.
/// Outputs: owning trigger/resample handles (resample is zero for an edge event).
/// REMOVE inputs: vcpu, interrupt, trigger; resample is ignored and stays owned
/// by the caller. Interrupt is a controller-local ID, e.g. an AArch64 SPI INTID.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VmIrqEvent {
    pub vcpu: u32,
    pub interrupt: u32,
    pub flags: u32,
    pub trigger: u32,
    pub resample: u32,
    pub reserved: u32,
}

const _: () = {
    assert!(core::mem::size_of::<VmIrqEvent>() == 24);
    assert!(core::mem::align_of::<VmIrqEvent>() == 4);
};
