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
│  │    exit = sys_vcpu_run(vcpu_fd);  // Blocks here            ││
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
│  │                    sys_vcpu_run()                            ││
│  │  loop {                                                      ││
│  │    run_guest_loop(vcpu);  // Assembly -> VM-entry            ││
│  │    // ... VM-Exit occurs ...                                 ││
│  │    exit = handle_vm_exit(vcpu);                              ││
│  │    if exit.needs_userspace() {                               ││
│  │      return exit;  // Back to userspace                      ││
│  │    }                                                         ││
│  │    // Timer/interrupt: handle internally, continue           ││
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
        sys_vcpu_run(vcpu_handle, &mut exit)?;
        
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
pub fn sys_vcpu_run(trapframe: &mut Trapframe) -> usize {
    let vcpu_handle = trapframe.get_arg(0) as u32;
    let exit_ptr = trapframe.get_arg(1);
    
    let vcpu = get_vcpu_from_handle(vcpu_handle)?;
    let mut exit = VcpuExit::default();
    
    // Main run loop - kernel handles some exits internally
    loop {
        // Enter guest (assembly trampoline)
        unsafe { run_guest_loop(&vcpu) };
        
        // Parse exit reason from saved state
        let reason = vcpu.parse_exit_reason();
        
        match reason {
            VmExitReason::TimerInterrupt => {
                // Kernel handles internally
                handle_timer_interrupt();
                continue; // Re-enter guest
            }
            VmExitReason::MmioRead { .. } |
            VmExitReason::MmioWrite { .. } => {
                // Return to userspace
                exit = VcpuExit::from_vmexit(&reason);
                break;
            }
            VmExitReason::Shutdown | VmExitReason::Hlt => {
                exit = VcpuExit::Shutdown;
                break;
            }
            _ => {
                exit = VcpuExit::Unknown(reason.raw_code());
                break;
            }
        }
    }
    
    // Copy exit info to userspace
    copy_to_user(exit_ptr, &exit)?;
    trapframe.set_return_value(0);
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
    pub is_write: bool,
    pub _padding: [u8; 7],
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
sys_vcpu_run()
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
        sys_vcpu_run()       ← continues normally
```

This is the same pattern used by KVM and other Type-2 hypervisors.

---

## 3. World Switch

### 3.1 Design Principle: Reuse Existing Trampoline

The existing user trap trampoline (`_user_trap_entry` / `_user_trap_exit`) is reused for guest traps:
- No separate assembly for guest exit path
- Guest traps go through the same path as user traps
- Only difference: set `trapframe.epc = run_guest_loop_return` to return to kernel instead of guest

```
run_guest_loop()
    │ [prologue: ra, s0-s11 → stack]
    │
    └──► sret → [guest VS-mode]
                  │
                  ▼
              _user_trap_entry     ← Existing trampoline
                  │ [save regs to trapframe]
                  │
                  └──► arch_user_trap_handler()
                            │ if is_guest:
                            │   - guest_vcpu.store(trapframe)
                            │   - guest_vcpu.save_csrs()
                            │   - trapframe.epc = run_guest_loop_return
                            │
                            └──► arch_switch_to_user_or_guest()
                                      │
                                      └──► _user_trap_exit
                                            │ [restore regs]
                                            │
                                            └──► sret → run_guest_loop_return
                                                          │ [epilogue: stack → ra, s0-s11]
                                                          │
                                                          └──► ret → sys_vcpu_run()
```

### 3.2 Entry: `run_guest_loop` (New)

```rust
// kernel/src/arch/riscv64/hv/switch.rs (NEW)

use core::arch::naked_asm;
use super::GuestVcpu;

/// Offset constants - must match GuestVcpu struct layout
mod offset {
    pub const IREGS: usize = 0;
    pub const PC: usize = /* offsetof(GuestVcpu, pc) */;
    pub const GUEST_CSRS: usize = /* offsetof(GuestVcpu, guest_csrs) */;
}

#[naked]
pub unsafe extern "C" fn run_guest_loop(vcpu: *const GuestVcpu) {
    naked_asm!(
        // ===== PROLOGUE: Save host callee-saved to stack =====
        "addi sp, sp, -104",
        "sd ra, 0(sp)",
        "sd s0, 8(sp)",
        "sd s1, 16(sp)",
        "sd s2, 24(sp)",
        "sd s3, 32(sp)",
        "sd s4, 40(sp)",
        "sd s5, 48(sp)",
        "sd s6, 56(sp)",
        "sd s7, 64(sp)",
        "sd s8, 72(sp)",
        "sd s9, 80(sp)",
        "sd s10, 88(sp)",
        "sd s11, 96(sp)",
        
        // ===== Restore guest CSRs =====
        "li t0, {csrs_offset}",
        "add t0, a0, t0",
        "ld t1, 0(t0)",         // vsscratch
        "csrw vsscratch, t1",
        "ld t1, 8(t0)",         // vsepc
        "csrw vsepc, t1",
        "ld t1, 24(t0)",        // vsatp
        "csrw vsatp, t1",
        "ld t1, 32(t0)",        // vsstatus
        "csrw vsstatus, t1",
        "ld t1, 40(t0)",        // hstatus
        "csrw hstatus, t1",
        
        // ===== Load guest GPRs =====
        "ld x1, 8(a0)",
        "ld x2, 16(a0)",
        "ld x3, 24(a0)",
        "ld x4, 32(a0)",
        "ld x5, 40(a0)",
        "ld x6, 48(a0)",
        "ld x7, 56(a0)",
        "ld x8, 64(a0)",
        "ld x9, 72(a0)",
        "ld x10, 80(a0)",
        "ld x11, 88(a0)",
        "ld x12, 96(a0)",
        "ld x13, 104(a0)",
        "ld x14, 112(a0)",
        "ld x15, 120(a0)",
        "ld x16, 128(a0)",
        "ld x17, 136(a0)",
        "ld x18, 144(a0)",
        "ld x19, 152(a0)",
        "ld x20, 160(a0)",
        "ld x21, 168(a0)",
        "ld x22, 176(a0)",
        "ld x23, 184(a0)",
        "ld x24, 192(a0)",
        "ld x25, 200(a0)",
        "ld x26, 208(a0)",
        "ld x27, 216(a0)",
        "ld x28, 224(a0)",
        "ld x29, 232(a0)",
        "ld x30, 240(a0)",
        "ld x31, 248(a0)",
        
        // Load pc into sepc
        "li t0, {pc_offset}",
        "add t0, a0, t0",
        "ld t0, 0(t0)",
        "csrw sepc, t0",
        
        // Enter guest
        "li t0, 0x80000",       // HSTATUS_SPV
        "csrs hstatus, t0",
        "sret",
        
        // ===== EPILOGUE: Restore host and return =====
        // Label for sret to jump back to
        ".global run_guest_loop_return",
        "run_guest_loop_return:",
        "ld ra, 0(sp)",
        "ld s0, 8(sp)",
        "ld s1, 16(sp)",
        "ld s2, 24(sp)",
        "ld s3, 32(sp)",
        "ld s4, 40(sp)",
        "ld s5, 48(sp)",
        "ld s6, 56(sp)",
        "ld s7, 64(sp)",
        "ld s8, 72(sp)",
        "ld s9, 80(sp)",
        "ld s10, 88(sp)",
        "ld s11, 96(sp)",
        "addi sp, sp, 104",
        "ret",
        
        csrs_offset = const offset::GUEST_CSRS,
        pc_offset = const offset::PC,
    );
}

// Export for trap handler to use
pub const RUN_GUEST_LOOP_RETURN: usize = run_guest_loop_return as usize;
```

### 3.3 Exit: Integrated into Existing Trap Handler

Modify the existing `arch_user_trap_handler` to handle guest exits:

```rust
// kernel/src/arch/riscv64/trap/user.rs (MODIFIED)

#[unsafe(export_name = "arch_user_trap_handler")]
pub extern "C" fn arch_user_trap_handler(addr: usize) -> ! {
    let trapframe: &mut Trapframe = unsafe { transmute(addr) };
    set_trapvector(get_kernel_trapvector_paddr());

    let cause: usize;
    unsafe {
        asm!("csrr {0}, scause", out(reg) cause);
    }

    let interrupt = cause & 0x8000000000000000 != 0;
    let is_guest = is_guest_trap();  // Check hstatus.SPV

    if is_guest {
        // ===== Guest trap handling (INTEGRATED) =====
        let guest_vcpu = current_guest_vcpu();
        
        // 1. Persist guest state (existing Vcpu pattern)
        guest_vcpu.store(trapframe);
        
        // 2. Save guest CSRs (arch-specific, in GuestVcpu)
        guest_vcpu.save_csrs();
        
        // 3. Capture exit reason for sys_vcpu_run
        let exit_info = VmExitInfo::capture(trapframe.epc);
        set_last_vm_exit(exit_info);
        
        // 4. Redirect sret to return to kernel, not guest
        trapframe.epc = crate::arch::hv::switch::RUN_GUEST_LOOP_RETURN as u64;
        
        // 5. Clear hstatus.SPV so next trap isn't treated as guest
        csr::clear_hstatus_spv();
        
        // Fall through to arch_switch_to_user_or_guest
        // _user_trap_exit's sret will go to run_guest_loop_return
    } else if interrupt {
        arch_interrupt_handler(trapframe, cause & !0x8000000000000000);
    } else {
        arch_exception_handler(trapframe, cause);
    }
    
    // Same exit path for both guest and host
    arch_switch_to_user_or_guest(trapframe);
}

/// Check if current trap is from guest (VS-mode)
#[inline]
fn is_guest_trap() -> bool {
    use crate::arch::riscv64::hv::csr;
    (csr::read_hstatus() & csr::HSTATUS_SPV) != 0
}
```

### 3.4 About `save_csrs()` Method

The `save_csrs()` method is RISC-V specific, but that's acceptable because:
1. `GuestVcpu` lives in `kernel/src/arch/riscv64/hv/`
2. It's only called from RISC-V specific trap handler
3. The method name is clear about what it does

```rust
// kernel/src/arch/riscv64/hv/guest_vcpu.rs

impl GuestVcpu {
    /// Save guest CSRs from hardware to this struct
    /// Called immediately after VM-exit, before any CSR access
    pub fn save_csrs(&mut self) {
        self.guest_csrs.vsscratch = csr::read_vsscratch();
        self.guest_csrs.vsepc = csr::read_vsepc();
        self.guest_csrs.vsatp = csr::read_vsatp();
        self.guest_csrs.vsstatus = csr::read_vsstatus();
        self.guest_csrs.hstatus = csr::read_hstatus();
    }
    
    /// Restore guest CSRs from this struct to hardware
    /// Called immediately before VM-entry
    pub fn restore_csrs(&self) {
        csr::write_vsscratch(self.guest_csrs.vsscratch);
        csr::write_vsepc(self.guest_csrs.vsepc);
        csr::write_vsatp(self.guest_csrs.vsatp);
        csr::write_vsstatus(self.guest_csrs.vsstatus);
        csr::write_hstatus(self.guest_csrs.hstatus);
    }
}
```

### 3.5 What Gets Deleted

The separate `guest_trap_handler` in `kernel/src/arch/riscv64/hv/trap.rs` is no longer needed:
- Its logic moves into `arch_user_trap_handler`
- No more `task.exit()` for guest traps

---

## 4. Syscall Interface

### 4.1 New Syscalls

| Syscall | Number | Arguments | Returns |
|---------|--------|-----------|---------|
| `sys_vcpu_run` | 1102 | a0=vcpu_handle, a1=exit_ptr | 0 on success, usize::MAX on error |

### 4.2 Syscall Registration

```rust
// kernel/src/syscall/mod.rs (MODIFIED)

syscall_table! {
    // ... existing syscalls ...
    1100 => sys_hypervisor_vm_create,
    1101 => sys_hypervisor_vcpu_create,
    1102 => sys_vcpu_run,  // NEW
}
```

### 4.3 User-Side Wrapper

```rust
// user/lib/std/src/syscall.rs (MODIFIED)

pub enum Syscall {
    // ... existing ...
    HypervisorVmCreate = 1100,
    HypervisorVcpuCreate = 1101,
    VcpuRun = 1102,  // NEW
}

// user/lib/std/src/hypervisor.rs (NEW)
pub fn sys_vcpu_run(vcpu_handle: u32, exit: &mut VcpuExit) -> Result<(), HypervisorError> {
    let ret = syscall2(
        Syscall::VcpuRun as usize,
        vcpu_handle as usize,
        exit as *mut VcpuExit as usize,
    );
    if ret == usize::MAX {
        Err(HypervisorError::SyscallFailed)
    } else {
        Ok(())
    }
}
```

---

## 5. Files to Create/Modify

### 5.1 New Files

| File | Purpose |
|------|---------|
| `kernel/src/hypervisor/types.rs` | Shared VcpuExit, MmioInfo structs |
| `kernel/src/arch/riscv64/hv/guest_vcpu.rs` | GuestVcpu, GuestCsrState |
| `kernel/src/arch/riscv64/hv/switch.rs` | run_guest_loop naked function |
| `user/lib/std/src/hypervisor/mod.rs` | User-side hypervisor API |
| `user/lib/std/src/hypervisor/types.rs` | Mirror of kernel types |

### 5.2 Modified Files

| File | Changes |
|------|---------|
| `kernel/src/hypervisor/syscall.rs` | Add sys_vcpu_run |
| `kernel/src/hypervisor/vcpu.rs` | Integrate GuestVcpu, implement run() |
| `kernel/src/arch/riscv64/trap/user.rs` | Integrate guest trap handling |
| `kernel/src/arch/riscv64/hv/mod.rs` | Export GuestVcpu, switch module |
| `kernel/src/syscall/mod.rs` | Register syscall 1102 |
| `user/lib/std/src/syscall.rs` | Add VcpuRun syscall |

### 5.3 Deleted Files

| File | Reason |
|------|--------|
| `kernel/src/arch/riscv64/hv/trap.rs` | Logic moved into arch_user_trap_handler |

---

## 6. Implementation Phases

### Phase 1: Data Structures
1. Create `kernel/src/hypervisor/types.rs` with VcpuExit, MmioInfo
2. Create `kernel/src/arch/riscv64/hv/guest_vcpu.rs` with GuestVcpu (following existing Vcpu pattern)
3. Create mirror types in `user/lib/std/src/hypervisor/types.rs`

### Phase 2: World Switch
1. Create `kernel/src/arch/riscv64/hv/switch.rs` with run_guest_loop naked function
2. Add CSR save/restore methods (`save_csrs()`, `restore_csrs()`) to GuestVcpu

### Phase 3: Trap Handler Integration
1. Modify `arch_user_trap_handler` to detect guest traps via `hstatus.SPV`
2. Add guest state persistence using `GuestVcpu::store()` and `save_csrs()`
3. Set `trapframe.epc = RUN_GUEST_LOOP_RETURN` for kernel return
4. Delete `kernel/src/arch/riscv64/hv/trap.rs`

### Phase 4: Syscall
1. Add sys_vcpu_run to `kernel/src/hypervisor/syscall.rs`
2. Register syscall 1102
3. Add user-side wrapper

### Phase 5: User VMM
1. Create `user/bin/src/shv/` (basic VMM binary)
2. Implement vcpu_loop with MMIO handling
3. Add basic virtio device emulation

---

## 7. Key Invariants

1. **Stack Safety**: Guest registers are saved to stack trapframe first, then persisted to VcpuState before any kernel operation that might reschedule.

2. **CSR Atomicity**: Guest CSRs must be saved immediately after VM-exit, before any other CSR access.

3. **Host Context Isolation**: Host callee-saved registers are restored atomically when returning from guest - no intermediate kernel code runs with guest state.

4. **Single Entry Point**: All guest execution goes through `run_guest_loop`, all exits come through `arch_user_trap_handler`.

---

## 8. Testing Strategy

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

## 9. Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Assembly bugs cause subtle corruption | Extensive unit tests for context switch |
| CSR ordering issues | Document and audit all CSR access sequences |
| Stack overflow during world switch | Use dedicated per-vcpu stack |
| Race conditions with scheduler | GuestVcpu uses Mutex, world-switch is atomic |

---

## 10. References

- RISC-V H-extension specification
- KVM API design patterns
- Existing Scarlet hypervisor code: `kernel/src/hypervisor/`, `kernel/src/arch/riscv64/hv/`
