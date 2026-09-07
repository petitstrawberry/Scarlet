# SHV Implementation Status

This document tracks the implementation status of the Scarlet Hypervisor (SHV).

## Architecture Support

| Feature | RISC-V 64-bit | AArch64 |
|---------|---------------|---------|
| Basic VM creation | ✅ Implemented | ✅ Implemented |
| vCPU creation | ✅ Implemented | ✅ Implemented |
| Stage-2 MMU | ✅ Implemented | ✅ Implemented |
| Guest entry/exit | ✅ Implemented | ✅ Implemented |
| Trap handling | ✅ Implemented | ✅ Implemented |
| CSR/sysreg management | ✅ Implemented | ✅ Implemented |
| Timer virtualization | ✅ Implemented | ✅ Implemented |
| Interrupt injection | ⚠️ Partial | ⚠️ Partial |
| Linux `/dev/kvm` compatibility | ✅ Implemented | ✅ Implemented |

## RISC-V 64-bit (H-extension)

### Core Features

| Feature | Status | Notes |
|---------|--------|-------|
| VM creation (`sys_shv_vm_create`) | ✅ | |
| vCPU creation (`sys_shv_vcpu_create`) | ✅ | |
| vCPU run (`sys_shv_vcpu_run`) | ✅ | |
| Handle-based API | ✅ | Via `HandleControl` |
| Stage-2 page tables | ✅ | Sv48 supported |
| Guest memory slots | ✅ | |
| CSR save/restore | ✅ | VS-mode CSRs |

### Trap Handling

| Feature | Status | Notes |
|---------|--------|-------|
| Guest page faults | ✅ | Auto-maps RAM, exits on MMIO |
| MMIO read/write | ✅ | Decodes load/store instructions |
| ECALL from VS-mode | ✅ | SBI firmware calls |
| Virtual instruction (WFI) | ✅ | |
| Timer interrupts | ✅ | Kernel-handled |
| External interrupts | ⚠️ Partial | Injection supported, routing limited |
| Software interrupts | ⚠️ Partial | Injection supported |
| Breakpoint | ✅ | |

### SBI Firmware

| Feature | Status | Notes |
|---------|--------|-------|
| DBCN (Debug Console) | ✅ | WRITE supported |
| TIME | ✅ | |
| Base | ✅ | |
| HSM | ❌ | |
| IPI | ❌ | |
| RFENCE | ❌ | |
| SRST | ❌ | |

### Device Emulation (U-SHV)

| Device | Status | Notes |
|--------|--------|-------|
| UART (NS16550A) | ✅ | |
| PLIC | ✅ | Interrupt controller |
| VirtIO devices | ❌ | Planned |

### Linux KVM Compatibility

| Feature | Status | Notes |
|---------|--------|-------|
| `/dev/kvm` device | ✅ | Linux ABI ioctl dispatch maps to SHV |
| VM/vCPU lifecycle ioctls | ✅ | `KVM_CREATE_VM`, `KVM_CREATE_VCPU`, `KVM_RUN` |
| User memory regions | ✅ | `KVM_SET_USER_MEMORY_REGION` |
| Register access | ✅ | RISC-V one-reg/core/timer/SBI registers |
| IRQ line / interrupt ioctls | ⚠️ Partial | Enough for current guests, not complete |

### Missing Features

| Feature | Priority | Notes |
|---------|----------|-------|
| SMP (multi-vCPU) | High | Currently single vCPU only |
| VirtIO block | Medium | For guest storage |
| VirtIO net | Medium | For guest networking |
| VirtIO console | Medium | |
| Device passthrough | Low | Requires IOMMU |
| IOMMU | Low | |
| Guest debug (GDB stub) | Low | |
| Snapshot/restore | Low | |
| Live migration | Low | |
| Nested virtualization | Very Low | |

## AArch64

**Status: Implemented, active bring-up**

The AArch64 hypervisor path is implemented under `kernel/src/arch/aarch64/hv/`.
It is newer than the RISC-V path and still has rough edges around broader
VGIC/device-model coverage, but it is no longer stub-only.

### Core Features

| Feature | Status | Notes |
|---------|--------|-------|
| VM creation (`sys_shv_vm_create`) | ✅ | |
| vCPU creation (`sys_shv_vcpu_create`) | ✅ | |
| vCPU run (`sys_shv_vcpu_run`) | ✅ | |
| Handle-based API | ✅ | Via `HandleControl` |
| Stage-2 page tables | ✅ | 4 KiB granule, 40-bit IPA configuration |
| Guest memory slots | ✅ | |
| VTTBR_EL2 / VTCR_EL2 setup | ✅ | Per-CPU VTCR setup and per-VM VTTBR root |
| Guest EL1 sysreg state | ✅ | `GuestSystemRegs` and KVM one-reg conversion |
| Guest entry/exit | ✅ | EL2/VHE world switch path |
| Host EL2 context save/restore | ✅ | |
| Linux KVM API compatibility | ✅ | `/dev/kvm` ioctl path, AArch64 one-reg/core/sysreg handling |

### Trap Handling

| Feature | Status | Notes |
|---------|--------|-------|
| Guest page faults | ✅ | Auto-maps RAM, exits on MMIO |
| MMIO read/write | ✅ | Decodes trapped data aborts |
| WFI/WFE | ✅ | |
| HVC/SMC firmware calls | ✅ | PSCI/SMCCC handling is wired through the KVM compatibility path |
| System register traps | ✅ | Timer and selected ID/cache/sysreg emulation |
| Breakpoint / illegal instruction | ✅ | Exits to userspace |
| Host interrupts while guest is running | ✅ | Exit path exists |

### Timer and Interrupts

| Feature | Status | Notes |
|---------|--------|-------|
| Virtual timer state | ✅ | CNTV registers and virtual counter offset |
| Guest timer PPI | ✅ | PPI 27 path |
| VGICv3 list registers | ✅ | Probed and saved/restored |
| GIC distributor/CPU interface MMIO | ⚠️ Partial | Emulation exists for the current guest path |
| External interrupt injection | ⚠️ Partial | SPI/PPI injection paths exist, routing is still limited |

### Linux KVM Compatibility

| Feature | Status | Notes |
|---------|--------|-------|
| `/dev/kvm` device | ✅ | Linux ABI ioctl dispatch maps to SHV |
| VM/vCPU lifecycle ioctls | ✅ | `KVM_CREATE_VM`, `KVM_CREATE_VCPU`, `KVM_RUN` |
| User memory regions | ✅ | `KVM_SET_USER_MEMORY_REGION` |
| AArch64 register API | ✅ | Core registers, one-reg sysregs, PSCI firmware registers |
| ARM vCPU init ioctls | ✅ | `KVM_ARM_PREFERRED_TARGET`, `KVM_ARM_VCPU_INIT`, finalize path |
| VGIC device ioctls | ⚠️ Partial | vGICv3/ITS attributes handled for current VMM needs |
| Firecracker-class VMM workloads | ✅ | Current compatibility target; full Linux KVM API parity is not claimed |

### Device Emulation (U-SHV)

| Device | Status | Notes |
|--------|--------|-------|
| PL011 UART | ✅ | |
| VirtIO devices | ❌ | Planned |

### Missing / Rough Areas

| Feature | Priority | Notes |
|---------|----------|-------|
| SMP (multi-vCPU) | High | Current path is effectively single-vCPU focused |
| Broader VGIC/GIC emulation | High | Enough for current bring-up, not complete |
| VirtIO block/net/console | Medium | For guest storage, networking, and console |
| Device passthrough | Low | Requires IOMMU |
| Guest debug (GDB stub) | Low | |
| Snapshot/restore | Low | |
| Live migration | Low | |
| Nested virtualization | Very Low | |

## API Stability

The hypervisor API is **experimental** and subject to change. Breaking changes may occur without notice until the API stabilizes.

## Testing

Guest test programs are available in `guest_tests/`:

| Test | Description |
|------|-------------|
| `hello` | Minimal bare-metal hello world |
| `timer_test` | Timer interrupt test |
| `uart_test` | UART MMIO test |
| `sbi_dbcn_test` | SBI debug console test |

## Contributing

When implementing new hypervisor features:

1. Update this status document
2. Add tests in `guest_tests/`
3. Document public APIs
4. Follow the existing code patterns in `kernel/src/arch/riscv64/hv/`
