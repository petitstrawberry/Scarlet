# SHV Implementation Status

This document tracks the implementation status of the Scarlet Hypervisor (SHV).

## Architecture Support

| Feature | RISC-V 64-bit | AArch64 |
|---------|---------------|---------|
| Basic VM creation | ✅ Implemented | ❌ Not Implemented |
| vCPU creation | ✅ Implemented | ❌ Not Implemented |
| Stage-2 MMU | ✅ Implemented | ❌ Not Implemented |
| Guest entry/exit | ✅ Implemented | ❌ Not Implemented |
| Trap handling | ✅ Implemented | ❌ Not Implemented |
| CSR/_sysreg management | ✅ Implemented | ❌ Not Implemented |

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

**Status: Not Implemented**

The AArch64 hypervisor support exists only as stub code. The following are placeholder modules:

- `kernel/src/arch/aarch64/hv/mod.rs` - Module structure only
- `kernel/src/arch/aarch64/hv/vm.rs` - Stub `VmObject` implementation
- `kernel/src/arch/aarch64/hv/guest_vcpu.rs` - Stub structures
- `kernel/src/arch/aarch64/hv/mmu.rs` - Empty
- `kernel/src/arch/aarch64/hv/switch.rs` - Empty
- `kernel/src/arch/aarch64/hv/trap.rs` - Empty

### Required for AArch64 Support

1. Stage-2 translation page tables
2. VTTBR_EL2 management
3. VCPU state (ELR_EL2, SPSR_EL2, etc.)
4. Exception handling from EL1 to EL2
5. Timer virtualization
6. GIC virtualization

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
