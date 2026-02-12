# Scarlet Hypervisor Subsystem Design Document

## 1. Overview

Scarletカーネルに組み込まれるハイパーバイザサブシステムの設計文書である。
本サブシステムはLinux KVMのようなType-2ハイパーバイザをカーネル内に実装するものであり、
ABI moduleが `/dev/kvm` 相当のインターフェースを提供するための基盤として機能する。

### Design Principles

1. **ABI-agnostic API**: ハイパーバイザコアは特定のABI (Linux, Scarlet Native等) に依存しない。各ABI moduleがそれぞれの方式でハイパーバイザAPIを呼び出す。
2. **Architecture Abstraction**: 共通コード (`kernel/src/hypervisor/`) にアーキテクチャ固有の操作やCSR名を含めない。全てのarch固有処理は `kernel/src/arch/{riscv64,aarch64}/hv/` に隠蔽する。
3. **Common Signatures**: archモジュールが外部にexportするAPIは、riscv64/aarch64によらず同一のシグネチャを持つ。
4. **KernelObject Integration**: VMとvCPUはKernelObjectとして管理し、既存のハンドルテーブル・ControlOpsを活用する。
5. **Feature-gated**: 全てのハイパーバイザコードは `#[cfg(feature = "hypervisor")]` で条件コンパイルする。

---

## 2. High-Level Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    Userspace VMM                        │
│              (QEMU, kvmtool, custom, ...)               │
└────────────┬────────────────────────┬───────────────────┘
             │ Linux ABI              │ Scarlet Native ABI
             │ (ioctl on /dev/kvm)    │ (ControlOps on handles)
             ▼                        ▼
┌─────────────────────────────────────────────────────────┐
│                  ABI Translation Layer                   │
│  ┌───────────────────┐  ┌───────────────────────────┐   │
│  │ Linux ABI Module   │  │ Scarlet Native ABI Module │   │
│  │ KVM ioctl → API    │  │ syscall → ControlOps      │   │
│  └─────────┬─────────┘  └─────────────┬─────────────┘   │
└────────────┼───────────────────────────┼─────────────────┘
             │                           │
             ▼                           ▼
┌─────────────────────────────────────────────────────────┐
│             kernel/src/hypervisor/ (Common)              │
│                                                         │
│  ┌─────────┐  ┌──────────┐  ┌──────────────────────┐   │
│  │   Vm    │  │   Vcpu   │  │   MemorySlotManager  │   │
│  │ (per VM)│  │(per vCPU)│  │   (GPA→HPA mapping)  │   │
│  └────┬────┘  └────┬─────┘  └──────────┬───────────┘   │
│       │             │                   │               │
│       └─────────────┼───────────────────┘               │
│                     │                                   │
│                     ▼                                   │
│  ┌─────────────────────────────────────────────────┐    │
│  │         arch::hv (Arch-Specific Backend)        │    │
│  │  - ArchVm: G-stage page table management        │    │
│  │  - ArchVcpu: guest register context, run loop   │    │
│  │  - VmExit: VM exit reason abstraction           │    │
│  └─────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────┘
             │
             ▼
┌─────────────────────────────────────────────────────────┐
│                   Hardware                              │
│  RISC-V: H-extension (HS-mode → VS/VU-mode)            │
│  AArch64: EL2 virtualization (stub for v1)              │
└─────────────────────────────────────────────────────────┘
```

---

## 3. Module Structure

### 3.1 Common Module: `kernel/src/hypervisor/`

```
kernel/src/hypervisor/
├── mod.rs              # モジュールルート、pub use、初期化
├── vm.rs               # Vm 構造体 (VM管理)
├── vcpu.rs             # Vcpu 構造体 (vCPU管理)
├── memory.rs           # MemorySlot / MemorySlotManager (GPA→HPA)
├── exit.rs             # VmExit enum (arch-independent exit reasons)
└── error.rs            # HypervisorError 型
```

### 3.2 Architecture-Specific: `kernel/src/arch/{riscv64,aarch64}/hv/`

```
kernel/src/arch/riscv64/hv/
├── mod.rs              # ArchVm, ArchVcpu, init()
├── stage2.rs           # Sv48x4 G-stage page table
├── csr.rs              # H-extension CSR wrappers
├── switch.rs           # Guest entry/exit (assembly)
└── vmext.rs            # VM-exit reason mapping

kernel/src/arch/aarch64/hv/
├── mod.rs              # ArchVm, ArchVcpu (stubs)
└── stage2.rs           # Stage-2 page table (stub)
```

### 3.3 Re-export Pattern (arch/mod.rs)

既存のパターンに倣い、`arch/mod.rs` でcfg-gatedにre-exportする:

```rust
#[cfg(feature = "hypervisor")]
pub mod hv {
    #[cfg(target_arch = "riscv64")]
    pub use crate::arch::riscv64::hv::*;

    #[cfg(target_arch = "aarch64")]
    pub use crate::arch::aarch64::hv::*;
}
```

---

## 4. Data Structures

### 4.1 Common Types

```rust
/// VM instance
pub struct Vm {
    id: VmId,
    arch: ArchVm,                          // arch-specific state
    memory_slots: MemorySlotManager,       // GPA→HPA mappings
    vcpus: Vec<Arc<Mutex<Vcpu>>>,          // owned vCPUs
    max_vcpus: usize,
}

/// Virtual CPU
pub struct Vcpu {
    id: VcpuId,
    vm: Weak<Mutex<Vm>>,                   // back-reference to parent VM
    arch: ArchVcpu,                        // arch-specific registers & state
}

/// Guest Physical Address → Host Physical Address mapping
pub struct MemorySlot {
    slot_id: u32,
    guest_phys_addr: u64,                  // GPA start
    memory_size: u64,                      // region size
    host_phys_addr: u64,                   // HPA backing
    flags: MemorySlotFlags,
}

/// VM exit reason (architecture-independent)
pub enum VmExit {
    IoRead { port: u64, size: u8 },
    IoWrite { port: u64, size: u8, data: u64 },
    MmioRead { addr: u64, size: u8 },
    MmioWrite { addr: u64, size: u8, data: u64 },
    Hlt,
    Shutdown,
    SystemEvent,
    Unknown(u64),
    FailEntry { hardware_entry_failure_reason: u64 },
    InternalError,
}

/// Hypervisor error types
pub enum HypervisorError {
    NotSupported,
    InvalidVmId,
    InvalidVcpuId,
    MaxVcpusReached,
    MemorySlotOverlap,
    MemorySlotNotFound,
    InvalidMemoryRegion,
    ArchError(&'static str),
}
```

### 4.2 Architecture-Specific Types (共通シグネチャ)

各archモジュールが以下の型と関数を**同一シグネチャ**でexportする:

```rust
/// arch/{riscv64,aarch64}/hv/mod.rs

/// Architecture-specific VM state
pub struct ArchVm { ... }

impl ArchVm {
    /// Create a new architecture-specific VM context
    pub fn new() -> Result<Self, &'static str>;

    /// Map a guest physical address region to host physical address
    pub fn map_memory(
        &mut self,
        guest_phys_addr: u64,
        host_phys_addr: u64,
        size: u64,
        flags: MemorySlotFlags,
    ) -> Result<(), &'static str>;

    /// Unmap a guest physical address region
    pub fn unmap_memory(
        &mut self,
        guest_phys_addr: u64,
        size: u64,
    ) -> Result<(), &'static str>;
}

/// Architecture-specific vCPU state (guest registers, etc.)
pub struct ArchVcpu { ... }

impl ArchVcpu {
    /// Create a new architecture-specific vCPU context
    pub fn new() -> Result<Self, &'static str>;

    /// Enter guest execution and return on VM exit
    pub fn run(&mut self) -> Result<VmExit, &'static str>;

    /// Get general-purpose registers
    pub fn get_regs(&self) -> GuestRegisters;

    /// Set general-purpose registers
    pub fn set_regs(&mut self, regs: &GuestRegisters);

    /// Get the instruction pointer / program counter
    pub fn get_pc(&self) -> u64;

    /// Set the instruction pointer / program counter
    pub fn set_pc(&mut self, pc: u64);
}

/// Guest general-purpose registers (arch-independent representation)
pub struct GuestRegisters {
    pub regs: [u64; 32],   // x0-x31 (RISC-V) / x0-x30 (AArch64)
}
```

---

## 5. KernelObject Integration

### 5.1 New KernelObject Variants

```rust
pub enum KernelObject {
    // ... existing variants ...

    #[cfg(feature = "hypervisor")]
    HypervisorVm(Arc<Mutex<hypervisor::Vm>>),

    #[cfg(feature = "hypervisor")]
    HypervisorVcpu(Arc<Mutex<hypervisor::Vcpu>>),
}
```

### 5.2 ControlOps for VM Operations

VMとvCPUに対するControlOpsを実装し、ハンドル経由でのioctl-like操作を可能にする:

| Command | Object | Description |
|---------|--------|-------------|
| `HV_CREATE_VCPU` | Vm | vCPUを作成し、そのハンドルを返す |
| `HV_SET_MEMORY_REGION` | Vm | GPA→HPAメモリスロットを設定 |
| `HV_DESTROY` | Vm | VM全体を破棄 |
| `HV_RUN` | Vcpu | ゲスト実行を開始し、VM exitで返る |
| `HV_GET_REGS` | Vcpu | 汎用レジスタを取得 |
| `HV_SET_REGS` | Vcpu | 汎用レジスタを設定 |
| `HV_GET_SREGS` | Vcpu | 特権レジスタを取得 |
| `HV_SET_SREGS` | Vcpu | 特権レジスタを設定 |

### 5.3 Multi-ABI Consumption

**Linux ABI Module**:
- `/dev/kvm` デバイスファイルをDevFSに登録
- `ioctl(fd, KVM_CREATE_VM, ...)` → `hypervisor::Vm::new()` → ハンドルをfdとして返す
- `ioctl(vm_fd, KVM_CREATE_VCPU, id)` → `vm.create_vcpu(id)` → ハンドルをfdとして返す
- `ioctl(vcpu_fd, KVM_RUN, ...)` → `vcpu.run()` → VmExitを `kvm_run` shared page に書き込む

**Scarlet Native ABI Module**:
- `sys_open("/dev/kvm")` でハイパーバイザデバイスハンドルを取得
- `sys_control(handle, HV_CREATE_VM, ...)` で新規VMハンドルを取得
- `sys_control(vm_handle, HV_CREATE_VCPU, ...)` でvCPUハンドルを取得
- `sys_control(vcpu_handle, HV_RUN, ...)` でゲスト実行

---

## 6. RISC-V H-Extension Implementation

### 6.1 Privilege Mode Model

```
┌──────────────┐
│ M-mode (SBI) │  ← OpenSBI / firmware
├──────────────┤
│ HS-mode      │  ← Scarlet kernel (hypervisor host)
├──────────────┤
│ VS-mode      │  ← Guest kernel
├──────────────┤
│ VU-mode      │  ← Guest userspace
└──────────────┘
```

Scarletカーネルは **HS-mode** で動作する。H-extension CSRに直接アクセスし、
`hgatp`, `hstatus`, `henvcfg` 等を使用してゲストのVS/VU-modeへの遷移を制御する。

### 6.2 G-Stage Page Table (Sv48x4)

RISC-V H-extensionでは、Guest Physical Address (GPA) → Supervisor Physical Address (SPA/HPA) の
変換に**G-stage**ページテーブルを使用する。Sv48x4はSv48と同じ4レベル構造だが、
ルートテーブルが16KiB (4ページ) であり、GPAの有効ビットが2ビット多い (50ビット)。

- ルートテーブル: 2048エントリ (16KiB, 4ページ連続)
- 既存のSv48 (`sv48.rs`) とはルートサイズが異なるため再利用不可
- `arch/riscv64/hv/stage2.rs` に新規実装

### 6.3 Guest Entry/Exit Flow

```
[HS-mode: Scarlet Kernel]
    │
    ├─ Save host state (callee-saved regs, sstatus, etc.)
    ├─ Load guest hstatus, henvcfg
    ├─ Write hgatp (G-stage page table)
    ├─ hfence.gvma (flush G-stage TLB)
    ├─ Restore guest VS-mode registers (vsstatus, vsepc, etc.)
    ├─ Restore guest GPRs from ArchVcpu context
    ├─ sret → [VS-mode: Guest]
    │
    ... guest execution ...
    │
    ├─ VM exit (trap from VS-mode to HS-mode)
    ├─ Save guest GPRs to ArchVcpu context
    ├─ Save guest VS-mode registers
    ├─ Clear hgatp
    ├─ Restore host state
    ├─ Decode scause/stval → VmExit enum
    └─ Return to hypervisor run loop
```

### 6.4 VM Exit Reasons (RISC-V → Common)

| scause | Description | Maps to VmExit |
|--------|-------------|----------------|
| Guest instruction page fault | ゲストのIPFが発生 | `MmioRead` or `FailEntry` |
| Guest load page fault | ゲストのロードPF | `MmioRead` |
| Guest store page fault | ゲストのストアPF | `MmioWrite` |
| Virtual supervisor ecall | VS-modeからのecall | `SystemEvent` |
| Guest timer interrupt | vstimecompにより発生 | (re-inject or handle) |

---

## 7. AArch64 Stub (v1)

v1ではAArch64ハイパーバイザは未実装とし、全APIが `Err("Hypervisor not supported on this architecture")` を返す。
ただし、型定義とモジュール構造は完全に揃え、ビルドが通る状態を維持する。

---

## 8. Implementation Phases

### Phase 1: Infrastructure
- `Cargo.toml` に `hypervisor` feature追加
- `kernel/src/hypervisor/` 共通モジュールスケルトン
- `kernel/src/arch/{riscv64,aarch64}/hv/` モジュール作成
- `KernelObject` に `HypervisorVm` / `HypervisorVcpu` variant追加
- `main.rs` にモジュール宣言追加

### Phase 2: Core RISC-V
- Sv48x4 G-stageページテーブル実装
- H-extension CSRラッパー
- ゲストvCPUコンテキスト (レジスタ保存/復帰)
- ゲストentry/exit アセンブリ
- VM-exitハンドラ (scause → VmExit変換)

### Phase 3: Common API
- `Vm::new()`, `Vm::create_vcpu()`, `Vm::set_memory_region()`
- `Vcpu::run()`, `Vcpu::get_regs()`, `Vcpu::set_regs()`
- ControlOps実装

### Phase 4: Integration
- ハンドルテーブルへの登録
- `/dev/kvm` デバイスファイル登録 (Linux ABI向け)
- Linux ABI ioctl変換層 (将来のタスク)

---

## 9. Testing Strategy

- **Unit tests**: Sv48x4ページテーブルのmap/unmap、VmExit変換
- **Integration tests**: VM作成 → vCPU作成 → メモリ設定 → レジスタ設定 の一連のフロー
- **Architecture tests**: `cargo make test-riscv64` / `cargo make test-aarch64` の両方でビルド・テスト通過

注意: ゲスト実行の完全なテストにはQEMU上でのH-extensionサポートが必要。
v1では構造のテスト (ページテーブル操作、レジスタ設定等) を中心にテストを行う。
