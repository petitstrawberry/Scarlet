# Type-2 Hypervisor Architecture Design

## Overview

This document describes the design for refactoring Scarlet's hypervisor from the current "vcpu task" model to a **Type-2 architecture** where:
- **Scarlet (Kernel)**: Minimal privileged operations - VM-entry, VM-exit capture, timer handling
- **U-SHV (User VMM)**: Device emulation, guest management, I/O handling

The key insight is the separation of **ephemeral trap frames** (stack-based) from **persistent VCPU state** (struct-based), enabling proper context switching and userspace exit handling.

---

## 1. Control Flow

### 1.1 High-Level Flow

```
┌─────────────────────────────────────────────────────────────────┐
│                        USER SPACE (Violet)                       │
│  ┌─────────────────────────────────────────────────────────────┐│
│  │                      vcpu_loop()                             ││
│  │  loop {                                                      ││
│  │    exit = sys_shv_vcpu_run(vcpu_handle); // Blocks here          ││
│  │    match exit {                                              ││
│  │      Mmio(info) => handle_mmio(info),                       ││
│  │      Shutdown => break,                                      ││
│  │      ...                                                     ││
│  │    }                                                         ││
│  │  }                                                           ││
│  └─────────────────────────────────────────────────────────────┘│
└───────────────────────────┬─────────────────────────────────────┘
                            │ syscall
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│                      KERNEL SPACE (Scarlet)                      │
│  ┌─────────────────────────────────────────────────────────────┐│
│  │                 VcpuObject::run()                            ││
│  │  run_guest_loop(vcpu);                                       ││
│  │  loop {                                                      ││
│  │    // Guest exception → arch_exception_handler               ││
│  │    //   → arch_guest_trap_exit → back here                   ││
│  │    exit = arch_guest_trap_handler(trapframe, vm);            ││
│  │    if exit.is_some() { return exit; }                        ││
│  │    resume_guest_loop(trapframe);                             ││
│  │  }                                                           ││
│  └─────────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────────┘
```

### 1.2 Detailed Execution Phases

#### Phase 1: User Space Entry
```rust
// user/lib/std/src/hypervisor.rs (NEW)
pub fn vcpu_loop(vcpu_handle: u32) -> Result<(), HypervisorError> {
    loop {
        let mut exit: VcpuExit = VcpuExit::default();
        sys_shv_vcpu_run(vcpu_handle, &mut exit)?;
        
        match exit {
            VcpuExit::Mmio(info) => {
                handle_mmio(&info)?;
            }
            VcpuExit::Shutdown => break,
            VcpuExit::Hlt => break,
            VcpuExit::Unknown(code) => {
                log::warn!("Unknown VM exit: {:#x}", code);
            }
            VcpuExit::Io => {
                // Timer handled by kernel, just continue
            }
        }
    }
    Ok(())
}
```

#### Phase 2: Kernel Entry (syscall)
```rust
// kernel/src/hypervisor/syscall.rs
pub fn sys_shv_vcpu_run(trapframe: &mut Trapframe) -> usize {
    let vcpu_handle = trapframe.get_arg(0) as u32;
    let exit_ptr = trapframe.get_arg(1);
    
    let task = mytask().ok_or(usize::MAX)?;
    trapframe.increment_pc_next(task);
    
    // Get vCPU from handle table
    let vcpu = match task.handle_table.get(vcpu_handle) {
        Some(KernelObject::HypervisorVcpu(vcpu)) => vcpu,
        _ => return usize::MAX,
    };

    let vm_exit = match vcpu.run() {
        Ok(exit) => exit,
        Err(_) => return usize::MAX,
    };

    let exit = VcpuExit::from_vmexit(&vm_exit);
    unsafe {
        core::ptr::write(
            task.vm_manager.translate_vaddr(exit_ptr)? as *mut VcpuExit,
            exit,
        );
    }
    0
}
```

---

## 2. Data Structures

### 2.1 Shared User/Kernel Structures

These structures are `#[repr(C)]` and shared between kernel and userspace.

```rust
// kernel/src/hypervisor/types.rs (NEW)
// user/lib/std/src/hypervisor/types.rs (mirror)

/// VM exit reason codes
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub enum VcpuExitReason: u32 {
    #[default]
    Unknown = 0,
    Io = 1,           // Kernel handled (timer, etc.)
    MmioRead = 2,
    MmioWrite = 3,
    Hlt = 4,
    Shutdown = 5,
    FailEntry = 6,
    InternalError = 7,
}

/// MMIO access information
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MmioInfo {
    pub address: u64,
    pub data: u64,
    pub size: u8,
    pub reg: u8,
    pub is_write: bool,
    pub _padding: [u8; 5],
}

/// VM exit information returned to userspace
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VcpuExit {
    pub reason: VcpuExitReason,
    pub _padding: u32,
    pub mmio: MmioInfo,
    pub fail_code: u64,
}
```

### 2.2 Kernel-Internal VCPU State

Following the existing `Vcpu` struct pattern in `kernel/src/arch/riscv64/vcpu/mod.rs`:

```rust
// kernel/src/arch/riscv64/hv/guest_vcpu.rs (NEW)

use crate::arch::riscv64::IntRegisters;
use crate::arch::riscv64::fpu::{FpuContext, VectorContext};
use crate::arch::Mode;
use crate::arch::Trapframe;
use alloc::boxed::Box;

/// Guest CSR state (VS-mode CSRs that must be saved/restored)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GuestCsrState {
    pub vsscratch: u64,
    pub vsepc: u64,
    pub vscause: u64,
    pub vstval: u64,
    pub vsatp: u64,
    pub vsstatus: u64,
    // H-extension state
    pub hstatus: u64,
}

/// Guest VCPU state - follows existing Vcpu struct pattern
///
/// This mirrors the existing `Vcpu` structure but adds guest-specific
/// CSR state for hypervisor support.
#[derive(Debug, Clone)]
pub struct GuestVcpu {
    // ===== Following existing Vcpu pattern =====
    /// General-purpose registers
    pub iregs: IntRegisters,
    /// Floating-point register context
    pub fpu: FpuContext,
    pub fpu_used: bool,
    /// Vector register context
    pub vector: Option<Box<VectorContext>>,
    pub vector_used: bool,
    /// Program counter
    pc: u64,
    /// Address space ID for guest
    asid: usize,
    /// Execution mode (GuestUser or GuestKernel)
    mode: Mode,
    
    // ===== Guest-specific additions =====
    /// Guest CSR state (VS-mode CSRs)
    pub guest_csrs: GuestCsrState,
    /// VM ID this VCPU belongs to
    pub vm_id: u32,
    /// VCPU ID within the VM
    pub vcpu_id: u32,
}

impl GuestVcpu {
    pub fn new(vm_id: u32, vcpu_id: u32) -> Self {
        Self {
            iregs: IntRegisters::new(),
            fpu: FpuContext::new(),
            fpu_used: false,
            vector: None,
            vector_used: false,
            pc: 0,
            asid: 0,
            mode: Mode::GuestUser,
            guest_csrs: GuestCsrState::default(),
            vm_id,
            vcpu_id,
        }
    }
    
    /// Store guest state from trapframe (following existing Vcpu pattern)
    pub fn store(&mut self, trapframe: &Trapframe) {
        self.iregs = trapframe.regs;
        self.pc = trapframe.epc;
    }
    
    /// Switch to guest state into trapframe (following existing Vcpu pattern)
    pub fn switch(&mut self, trapframe: &mut Trapframe) {
        trapframe.regs = self.iregs;
        trapframe.epc = self.pc;
    }
    
    pub fn set_pc(&mut self, pc: u64) {
        self.pc = pc;
    }
    
    pub fn get_pc(&self) -> u64 {
        self.pc
    }
    
    pub fn get_mode(&self) -> Mode {
        self.mode
    }
    
    /// Save guest CSRs from hardware
    pub fn save_csrs(&mut self) {
        self.guest_csrs.vsscratch = csr::read_vsscratch();
        self.guest_csrs.vsepc = csr::read_vsepc();
        self.guest_csrs.vsatp = csr::read_vsatp();
        self.guest_csrs.vsstatus = csr::read_vsstatus();
        self.guest_csrs.hstatus = csr::read_hstatus();
    }
    
    /// Restore guest CSRs to hardware
    pub fn restore_csrs(&self) {
        csr::write_vsscratch(self.guest_csrs.vsscratch);
        csr::write_vsepc(self.guest_csrs.vsepc);
        csr::write_vsatp(self.guest_csrs.vsatp);
        csr::write_vsstatus(self.guest_csrs.vsstatus);
        csr::write_hstatus(self.guest_csrs.hstatus);
    }
}
```

### 2.3 Relationship with Existing Vcpu

The existing `Vcpu` struct in `kernel/src/arch/riscv64/vcpu/mod.rs`:

```rust
// Existing pattern (for reference)
pub struct Vcpu {
    pub iregs: IntRegisters,
    pub fpu: FpuContext,
    pub fpu_used: bool,
    pub vector: Option<Box<VectorContext>>,
    pub vector_used: bool,
    pc: u64,
    asid: usize,
    mode: Mode,
}
```

`GuestVcpu` follows this pattern exactly for the common fields, making it familiar and consistent with the codebase. The only addition is:
- `guest_csrs: GuestCsrState` - VS-mode CSR persistence
- `vm_id` / `vcpu_id` - Metadata for handle lookup

### 2.4 Host Context Handling

**No separate `HostContext` struct is needed.** The host's callee-saved registers (ra, s0-s11) are naturally saved on the stack by the standard function call convention:

```
sys_shv_vcpu_run()
    └── run_guest_loop()    ← prologue pushes ra, s0-s11 to stack
            │
            ▼
        [guest execution via sret]
            │
            ▼
        trap_handler()       ← saves guest state, then ret
            │
            ▼
        run_guest_loop()     ← epilogue pops ra, s0-s11, ret
            │
            ▼
        sys_shv_vcpu_run()       ← continues normally
```

This is the same pattern used by KVM and other Type-2 hypervisors.

---

## 3. World Switch

### 3.1 Design Principle: Guest Exit Returns to VcpuObject::run

Guest exceptions (ECALL from VS-mode, guest page faults) are handled by the normal exception path:

```
arch_exception_handler
    └──► arch_guest_trap_exit
              └──► return to VcpuObject::run() after run_guest_loop
```

The VM-exit path does not rely on a dedicated run_guest_loop_return label. The arch-specific
`arch_guest_trap_exit` returns to the caller of `run_guest_loop`, which is `VcpuObject::run`.

### 3.2 Entry: `run_guest_loop`

```rust
// kernel/src/arch/riscv64/hv/switch.rs
// Pseudo-layout (details omitted):
// 1) Save host callee-saved registers.
// 2) Restore guest CSRs + GPRs from GuestVcpu state.
// 3) Set sepc and enter guest with sret.
// 4) VM-exit returns via arch_guest_trap_exit, restoring host state and
//    returning to the caller (VcpuObject::run).
```

### 3.3 Exit Handling via `arch_guest_trap_handler`

`VcpuObject::run` calls `arch_guest_trap_handler` after VM-exit. If it returns `Some(VmExit)`
the syscall returns to userspace; otherwise the kernel re-enters the guest with
`resume_guest_loop(trapframe)`.

### 3.4 Guest State Save Timing

Guest state is saved after `arch_guest_trap_handler` returns (inside `VcpuObject::run`),
so the saved state reflects any architectural changes made while handling the exit.

### 3.5 Interrupt Routing

Normal interrupts are handled by the host. Only guest exceptions (ECALL from VS-mode,
guest page faults) are routed to the guest handling path.

---

## 4. Syscall Interface

### 4.1 Handle-Based Syscall Model

All hypervisor syscalls use the kernel's handle table for resource management. This provides:
- Automatic cleanup when handles are closed
- Unified resource tracking across kernel objects
- Type-safe access via KernelObject enum

### 4.2 Syscalls

| Syscall | Number | Arguments | Returns |
|---------|--------|-----------|---------|
| `sys_shv_vm_create` | 1100 | (none) | vm_handle on success, usize::MAX on error |
| `sys_shv_vcpu_create` | 1101 | a0=vm_handle, a1=vcpu_id | vcpu_handle on success, usize::MAX on error |
| `sys_shv_vcpu_run` | 1102 | a0=vcpu_handle, a1=exit_ptr | 0 on success, usize::MAX on error |

**Note:** The old model used global VM manager with vm_id/vcpu_id lookups. The new model uses handle-based access:
- `sys_shv_vm_create()`: Creates VM, inserts into handle table as `KernelObject::HypervisorVm`
- `sys_shv_vcpu_create()`: Gets VM from handle, creates vCPU, inserts as `KernelObject::HypervisorVcpu`
- `sys_shv_vcpu_run()`: Gets vCPU from handle, runs until exit

### 4.3 Syscall Registration

```rust
// kernel/src/syscall/mod.rs (MODIFIED)

syscall_table! {
    // ... existing syscalls ...
    1100 => sys_shv_vm_create,
    1101 => sys_shv_vcpu_create,
    1102 => sys_shv_vcpu_run,
}
```

### 4.4 User-Side Wrapper

```rust
// user/lib/std/src/syscall.rs (MODIFIED)

pub enum Syscall {
    // ... existing ...
    ShvVmCreate = 1100,
    ShvVcpuCreate = 1101,
    ShvVcpuRun = 1102,
}

// user/lib/std/src/hypervisor.rs
pub fn vm_create() -> Result<u32, ()> {
    let ret = syscall2(Syscall::ShvVmCreate, 0, 0);
    if ret == usize::MAX {
        Err(())
    } else {
        Ok(ret as u32)
    }
}

pub fn vcpu_create(vm_handle: u32, vcpu_id: u32) -> Result<u32, ()> {
    let ret = syscall2(
        Syscall::ShvVcpuCreate,
        vm_handle as usize,
        vcpu_id as usize,
    );
    if ret == usize::MAX {
        Err(())
    } else {
        Ok(ret as u32)
    }
}

pub fn vcpu_run(vcpu_handle: u32, exit: &mut VcpuExit) -> Result<(), ()> {
    let ret = syscall2(
        Syscall::ShvVcpuRun,
        vcpu_handle as usize,
        exit as *mut VcpuExit as usize,
    );
    if ret == usize::MAX { Err(()) } else { Ok(()) }
}
```

---

## 5. Key Invariants

1. **Stack Safety**: Guest registers are saved to stack trapframe first, then persisted to VcpuState before any kernel operation that might reschedule.

2. **CSR Atomicity**: Guest CSRs must be saved immediately after VM-exit, before any other CSR access.

3. **Host Context Isolation**: Host callee-saved registers are restored atomically when returning from guest - no intermediate kernel code runs with guest state.

4. **Single Entry Point**: All guest execution goes through `run_guest_loop`, and guest exits return via `arch_guest_trap_exit` to `VcpuObject::run`.

---

## 6. Testing Strategy

1. **Unit Tests** (kernel):
   - GuestVcpu save/restore correctness
   - Exit reason parsing
   - CSR save/restore

2. **Integration Tests**:
   - Simple guest that executes `wfi` and exits
   - MMIO read/write round-trip to userspace
   - Timer interrupt handling (kernel internal)

3. **End-to-End**:
   - Boot Linux guest with virtio console
   - Run xv6 under SHV

---

## 7. Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Assembly bugs cause subtle corruption | Extensive unit tests for context switch |
| CSR ordering issues | Document and audit all CSR access sequences |
| Stack overflow during world switch | Use dedicated per-vcpu stack |
| Race conditions with scheduler | GuestVcpu uses Mutex, world-switch is atomic |

---

## 8. References

- RISC-V H-extension specification
- KVM API design patterns
- Existing Scarlet hypervisor code: `kernel/src/hypervisor/`, `kernel/src/arch/riscv64/hv/`
