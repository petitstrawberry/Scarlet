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
// user/lib/std/src/hypervisor/mod.rs
pub fn vcpu_loop(vcpu: &Vcpu) -> Result<(), ()> {
    loop {
        let exit = vcpu.run()?;
        
        match exit.reason {
            VcpuExitReason::MmioRead | VcpuExitReason::MmioWrite => {
                handle_mmio(&exit)?;
            }
            VcpuExitReason::Shutdown => break,
            VcpuExitReason::Hlt => break,
            VcpuExitReason::Unknown => {
                log::warn!("Unknown VM exit: {:#x}", exit.fail_code);
            }
            VcpuExitReason::Io => {
                // Timer handled by kernel, just continue
            }
            VcpuExitReason::FirmwareCall => {
                handle_firmware_call(&exit)?;
            }
            VcpuExitReason::VirtualInstruction => {
                handle_virtual_instruction(&exit)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn handle_mmio(exit: &VcpuExit) -> Result<(), ()> {
    if exit.mmio.is_write {
        // Handle MMIO write
        let addr = exit.mmio.address;
        let data = exit.mmio.data;
        let size = exit.mmio.size;
        // ... device emulation ...
    } else {
        // Handle MMIO read - write result to guest register
        let addr = exit.mmio.address;
        let size = exit.mmio.size;
        let value = emulate_mmio_read(addr, size)?;
        vcpu.set_reg(exit.mmio.reg, value)?;
    }
    // Advance PC past the MMIO instruction
    Ok(())
}
```

#### Phase 2: Kernel Entry (syscall)
```rust
// kernel/src/hypervisor/syscall.rs
pub fn sys_shv_vcpu_run(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let vcpu_handle = trapframe.get_arg(0) as u32;
    let exit_ptr = trapframe.get_arg(1);
    
    trapframe.increment_pc_next(task);
    
    // Validate exit_ptr bounds
    let exit_size = core::mem::size_of::<VcpuExit>();
    let exit_end = match exit_ptr.checked_add(exit_size - 1) {
        Some(end) => end,
        None => return usize::MAX,
    };
    let exit_map = match task.vm_manager.search_memory_map(exit_ptr) {
        Some(map) => map,
        None => return usize::MAX,
    };
    if exit_end > exit_map.vmarea.end {
        return usize::MAX;
    }

    // Translate the exit pointer to kernel address
    let exit_kaddr = match task.vm_manager.translate_vaddr(exit_ptr) {
        Some(addr) => addr,
        None => return usize::MAX,
    };
    
    // Get vCPU from handle table
    let vcpu = match task.handle_table.get(vcpu_handle) {
        Some(KernelObject::HypervisorVcpu(vcpu)) => vcpu,
        _ => return usize::MAX,
    };

    // Run the vCPU
    let vm_exit = match vcpu.run() {
        Ok(exit) => exit,
        Err(_) => return usize::MAX,
    };

    // Convert VmExit to VcpuExit and write to user space
    let exit = VcpuExit::from_vmexit(&vm_exit);
    unsafe {
        core::ptr::write(exit_kaddr as *mut VcpuExit, exit);
    }
    0
}
```

---

## 2. Data Structures

### 2.1 Shared User/Kernel Structures

These structures are `#[repr(C)]` and shared between kernel and userspace.

```rust
// kernel/src/hypervisor/types.rs
// user/lib/std/src/hypervisor/types.rs (mirror)

/// VM exit reason codes
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VcpuExitReason {
    #[default]
    Unknown = 0,
    Io = 1,              // Kernel handled (timer, etc.)
    MmioRead = 2,
    MmioWrite = 3,
    Hlt = 4,
    Shutdown = 5,
    FailEntry = 6,
    InternalError = 7,
    FirmwareCall = 8,    // SBI/BIOS firmware call (e.g., ECALL from VS-mode)
    VirtualInstruction = 9,  // Virtual instruction trap (WFI, etc.)
    IllegalInstruction = 10, // Illegal instruction in guest
    Breakpoint = 11,     // Breakpoint exception
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

/// Instruction information for virtual/illegal instruction exits
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct InstructionInfo {
    pub inst: u32,
    pub inst_len: u8,
    pub has_inst: bool,
    pub _padding: [u8; 6],
}

/// VM exit information returned to userspace
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VcpuExit {
    pub reason: VcpuExitReason,
    pub epc: u64,           // Guest program counter at exit
    pub mmio: MmioInfo,
    pub inst: InstructionInfo,  // Instruction info for instruction exits
    pub fail_code: u64,
}
```

### 2.2 Kernel-Internal VCPU State

Following the existing `Vcpu` struct pattern in `kernel/src/arch/riscv64/vcpu/mod.rs`:

```rust
// kernel/src/arch/riscv64/hv/csr.rs

/// Guest CSR state (VS-mode CSRs that must be saved/restored)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GuestCsrState {
    pub sscratch: u64,
    pub sepc: u64,
    pub scause: u64,
    pub stval: u64,
    pub stvec: u64,
    pub satp: u64,
    pub sstatus: u64,
    pub sie: u64,
    pub sip: u64,
}

impl GuestCsrState {
    /// Save guest CSRs from VS-mode hardware registers
    pub fn save() -> Self {
        Self {
            sscratch: read_vsscratch(),
            sepc: read_vsepc(),
            scause: read_vscause(),
            stval: read_vstval(),
            stvec: read_vstvec(),
            satp: read_vsatp(),
            sstatus: read_vsstatus(),
            sie: read_vsie(),
            sip: read_vsip(),
        }
    }

    /// Restore guest CSRs to VS-mode hardware registers
    pub fn restore(&self) {
        write_vsscratch(self.sscratch);
        write_vsepc(self.sepc);
        write_vscause(self.scause);
        write_vstval(self.stval);
        write_vstvec(self.stvec);
        write_vsatp(self.satp);
        write_vsstatus(self.sstatus);
        write_vsie(self.sie);
        write_vsip(self.sip);
    }
}

/// Hypervisor CSR state (HS-mode CSRs for context switching between VMs)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct HypervisorCsrState {
    pub hgatp: u64,
    pub htimedelta: u64,
    pub hvip: u64,
}
```

```rust
// kernel/src/arch/riscv64/hv/guest_vcpu.rs

use crate::arch::riscv64::IntRegisters;
use crate::arch::riscv64::fpu::{FpuContext, VectorContext};
use crate::arch::riscv64::{Mode, Trapframe};
use alloc::boxed::Box;

/// Guest VCPU state - follows existing Vcpu struct pattern
#[repr(C)]
#[derive(Debug, Clone)]
pub struct GuestVcpu {
    iregs: IntRegisters,
    csrs: GuestCsrState,
    pc: u64,
    fpu: FpuContext,
    fpu_used: bool,
    vector: Option<Box<VectorContext>>,
    vector_used: bool,
    asid: usize,
    mode: Mode,
    vm_id: u32,
    vcpu_id: u32,
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
            mode: Mode::GuestKernel,
            csrs: GuestCsrState::default(),
            vm_id,
            vcpu_id,
        }
    }
    
    /// Save guest state from trapframe and CSRs
    pub fn save(&mut self, trapframe: &Trapframe) {
        self.iregs = trapframe.regs;
        self.pc = trapframe.epc;
        self.csrs = GuestCsrState::save();
    }
    
    /// Switch to guest state into trapframe
    pub fn switch(&mut self, trapframe: &mut Trapframe) {
        trapframe.regs = self.iregs;
        trapframe.epc = self.pc;
    }
    
    /// Initialize CSRs for guest entry
    pub fn init_csrs(&self) {
        self.csrs.restore();
    }
    
    // ... register access methods ...
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

`GuestVcpu` follows this pattern for the common fields. The key differences are:
- `csrs: GuestCsrState` - VS-mode CSR persistence (field name is `csrs`, not `guest_csrs`)
- `vm_id` / `vcpu_id` - Metadata for VM association
- `mode` defaults to `Mode::GuestKernel` (not `GuestUser`)

### 2.4 Kernel-Internal VmExit Type

The kernel uses an internal `VmExit` enum to represent exit reasons before converting to the shared `VcpuExit` structure:

```rust
// kernel/src/hypervisor/types.rs

#[derive(Debug, Clone, Copy)]
pub enum VmExit {
    MmioRead {
        epc: u64,
        addr: u64,
        size: u8,
        reg: u8,
    },
    MmioWrite {
        epc: u64,
        addr: u64,
        size: u8,
        reg: u8,
        data: u64,
    },
    FirmwareCall {
        epc: u64,
    },
    VirtualInstruction {
        epc: u64,
        inst: Option<u32>,
        inst_len: Option<u8>,
    },
    IllegalInstruction {
        epc: u64,
        inst: Option<u32>,
        inst_len: Option<u8>,
    },
    Breakpoint {
        epc: u64,
    },
    Hlt,
    Shutdown,
    FailEntry {
        hardware_entry_failure_reason: u64,
    },
    InternalError,
    Unknown(u64),
}
```

The `VcpuExit::from_vmexit()` function converts `VmExit` to the userspace-visible `VcpuExit` structure.

### 2.5 Host Context Handling

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

// user/lib/std/src/hypervisor/mod.rs
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

### 4.5 Control Operations via HandleControl

In addition to dedicated syscalls, VM and VCPU control operations use the unified `HandleControl` syscall (similar to ioctl):

```rust
// VM control commands
pub mod vm_ctl {
    pub const SET_MEMORY_REGION: u32 = 0x01;
    pub const GET_VCPU_COUNT: u32 = 0x02;
    pub const SET_FAST_PATH: u32 = 0x03;
}

// VCPU control commands
pub mod vcpu_ctl {
    pub const RUN: u32 = 0x01;
    pub const GET_ONE_REG: u32 = 0x02;
    pub const SET_ONE_REG: u32 = 0x03;
    pub const INJECT_INTERRUPT: u32 = 0x04;
    pub const CLEAR_INTERRUPT: u32 = 0x05;
}

// Fast path flags for kernel-internal handling
pub mod fast_path {
    pub const TIMER: u32 = 0x01;
}

#[repr(C)]
pub struct VmMemoryRegion {
    pub slot_id: u32,
    pub flags: u32,
    pub guest_phys_addr: u64,
    pub memory_size: u64,
    pub host_phys_addr: u64,
}

#[repr(C)]
pub struct VcpuOneReg {
    pub index: u32,
    pub _padding: u32,
    pub value: u64,
}

pub fn vm_control(vm_handle: u32, command: u32, arg: usize) -> Result<i32, ()>;
pub fn vcpu_control(vcpu_handle: u32, command: u32, arg: usize) -> Result<i32, ()>;
```

### 4.6 High-Level API Wrappers

The userspace library provides convenient `Vm` and `Vcpu` structs:

```rust
pub struct Vm {
    handle: u32,
}

impl Vm {
    pub fn create() -> Result<Self, ()>;
    pub fn create_vcpu(&self, vcpu_id: u32) -> Result<Vcpu, ()>;
    pub fn add_memory_region(&self, slot_id: u32, guest_phys_addr: u64, size: u64, host_addr: u64) -> Result<(), ()>;
    pub fn set_fast_path(&self, flags: u32) -> Result<(), ()>;
}

pub struct Vcpu {
    handle: u32,
    vm_handle: u32,
}

impl Vcpu {
    pub fn run(&self) -> Result<VcpuExit, ()>;
    pub fn get_reg(&self, index: u32) -> Result<u64, ()>;
    pub fn set_reg(&self, index: u32, value: u64) -> Result<(), ()>;
    pub fn inject_interrupt(&self, irq_type: usize) -> Result<(), ()>;
    pub fn clear_interrupt(&self, irq_type: usize) -> Result<(), ()>;
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
- Scarlet hypervisor implementation:
  - `kernel/src/hypervisor/` - Core hypervisor subsystem
  - `kernel/src/arch/riscv64/hv/` - RISC-V H-extension support
  - `kernel/src/arch/aarch64/hv/` - AArch64 virtualization support (experimental)
  - `user/lib/std/src/hypervisor/` - Userspace hypervisor library
  - `user/bin/src/ushv/` - U-SHV userspace VMM implementation
- Guest test programs: `guest_tests/`
