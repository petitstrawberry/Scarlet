# Virtual interrupt events

Scarlet connects native `Counter` objects to virtual interrupt lines through
`kernel/src/hypervisor/irq.rs`. The Linux KVM adapter translates eventfds and
GSIs into these connections; the common layer has no Linux descriptor types.

## Native API

`scarlet_os::hypervisor` (also re-exported by Scarlet's `std::hypervisor`)
provides:

```rust,ignore
let event = create_irq_event(vm_handle, 0, 40, IRQ_EVENT_RESAMPLE | IRQ_EVENT_NONBLOCK)?;
// event.trigger and event.resample are newly allocated, owning Counter handles.
// Adopt each exactly once with Handle::from_raw, then use normal stream/select APIs.
// Write the native-endian u64 value 1 to trigger when the device requests an IRQ.
// On resample readability, read eight bytes and recheck the device's IRQ condition.
// If still asserted, write 1 to trigger again.
remove_irq_event(vm_handle, &event)?;
// Close both owned handles after disconnecting.
```

`VM_CREATE_IRQ_EVENT` (`0x04`) and `VM_REMOVE_IRQ_EVENT` (`0x05`) use the
24-byte `scarlet_abi::hypervisor::VmIrqEvent` record. CREATE takes `vcpu`,
`interrupt`, and `flags`; other fields must be zero. It returns `trigger` and,
with `IRQ_EVENT_RESAMPLE`, `resample`. Without resampling, `resample` is zero
and is not an owned handle. `IRQ_EVENT_NONBLOCK` applies to both counters.
Unknown flags and nonzero reserved fields are rejected.

REMOVE identifies a binding by its route and trigger counter's shared identity,
so duplicated handles also work. It neither closes the handles nor consumes
subsequent trigger writes. Disconnect explicitly before closing handles; VM
handle closure currently does not release the global VM manager's ownership.

Only vCPU 0 is supported. On AArch64, `interrupt` is a raw SPI INTID in
`32..256`, not a Linux GSI or a packed `KVM_IRQ_LINE` value. RISC-V supports
edge notifications using its existing virtual interrupt injection path;
resampling is rejected until guest completion can be detected there.

## Level interrupt lifecycle

1. A trigger notification asserts the virtual line. Registration also consumes
   any trigger notifications already queued.
2. AArch64 marks the software VGIC list register for EOI maintenance. The
   current platform uses maintenance PPI 25.
3. Guest deactivation consumes that delivery and lowers the line before
   signalling the resample counter. Priority drop alone in EOImode=1 does not
   complete it; a pending LR or host cancellation does not complete it either.
4. The VMM rechecks the device condition and triggers another interrupt if needed.

Sources sharing one level line receive resample notifications together.
Disconnecting one source preserves other sources' assertions. Connection
generations prevent an old delivery from completing a newly bound source.
Previously captured trigger callbacks cannot inject after disconnect. A resample
notification already committed before a concurrent disconnect can still finish.

Kernel notifications never block on a full counter: saturation coalesces the
notification while leaving the counter readable. Trigger notifications are
drained without blocking, including for semaphore counters. Edge and resampling
sources cannot be mixed on one route.

## KVM adapter

`KVM_IRQFD_FLAG_RESAMPLE` attaches `resamplefd` to the common level-event path.
`KVM_IRQFD_FLAG_DEASSIGN` disconnects using `(eventfd identity, GSI)` and ignores
`resamplefd`. `KVM_CAP_IRQFD_RESAMPLE` (82) is reported on AArch64 only, through
both system and VM `KVM_CHECK_EXTENSION` ioctls. Trigger and resample counters
must differ, and a trigger cannot be registered twice on the same VM.

AArch64 currently maps `GSI + 32` to SPI INTID, accepting GSI 0 through 223.
This does not implement `KVM_SET_GSI_ROUTING`, MSI routing, or SMP targeting.
See the [Linux KVM IRQFD specification](https://docs.kernel.org/virt/kvm/api.html#kvm-irqfd).

## Validation

Kernel tests cover shared lines, rearming, counter saturation, subscription
removal, stale callbacks, connection generations, LR exhaustion, deactivation
versus host cancellation, and KVM registration/capability/DEASSIGN handling.
These tests exercise the state transitions; an actual guest's GIC acknowledge,
EOI/DIR, and maintenance interrupt delivery still require an integration run.
