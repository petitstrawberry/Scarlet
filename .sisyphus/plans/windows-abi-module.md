# Windows ABI Module for Scarlet OS (AArch64)

## Overview

Implement a native Windows ABI module for Scarlet OS on AArch64, enabling direct execution of Windows ARM64 PE/COFF binaries. The approach uses real ntdll.dll from the user's Windows installation, with NT syscalls handled by Scarlet's kernel. Syscall numbers are auto-extracted from ntdll.dll via a build-time tool.

## Architecture

```
app.exe (PE/COFF ARM64)
  │
  │ PE Import Table resolution
  ▼
ntdll.dll (real, user-provided) + kernel32.dll + ucrtbase.dll
  │
  │ SVC #<imm>  (syscall number encoded in SVC immediate)
  ▼
Scarlet Kernel — WindowsAarch64Abi module
  │
  │ Maps NT syscalls → Scarlet kernel objects
  ▼
Scarlet VFS / VM / Task / TTY / IPC
```

## Key Design Decisions

1. **Real DLLs, not reimplementation** — ntdll.dll, kernel32.dll, ucrtbase.dll from user's Windows
2. **Syscall numbers from ntdll.dll** — Build-time tool extracts SVC immediates from Nt*/Zw* stubs
3. **No version pinning** — Tool works with any Windows ARM64 ntdll.dll
4. **NT syscall → Scarlet kernel object mapping** — NtCreateFile → VFS, NtAllocateVirtualMemory → VM, etc.
5. **PE/COFF loader in kernel** — Similar to existing ELF loader

## ARM64 Windows Syscall Convention

- **Instruction**: `SVC #<imm>` where `<imm>` is the syscall number
- **Arguments**: x0-x7 (AAPCS64)
- **Return**: x0
- **Kernel extracts syscall number**: From ESR_EL1 ISS field (bits [24:20] of SVC encoding)
- **SVC encoding**: `0xD4000001 | (imm << 5)`

Reference: Graceful Bits blog, hfiref0x/SyscallTables, metanit.com

## Syscall Table Sources

- Windows 10 22H2 (build 22631) ARM64: 486 syscalls
- Windows 11 23H2 (build 22631) ARM64: 486 syscalls  
- Windows 11 24H2 (build 26100) ARM64: 489 syscalls
- Full tables: https://github.com/hfiref0x/SyscallTables

---

## Implementation Phases

### Phase 0: ntsyscall_gen Tool (Build-time Syscall Extractor)

**Goal**: Rust CLI tool that parses ntdll.dll and generates Rust syscall table code.

**Location**: `tools/ntsyscall_gen/`

**What it does**:
1. Parse ntdll.dll PE Export Directory
2. Filter exports starting with "Nt" or "Zw"
3. For each export, scan function code for `SVC #<imm>` pattern
4. Extract syscall number from SVC encoding: `imm = (instruction >> 5) & 0xFFFF`
5. Generate `kernel/src/abi/windows/syscall_table.rs`

**Output format**:
```rust
// Generated from ntdll.dll (Windows 11 24H2 ARM64, Build 26100.3194)
// Regenerate: cargo run --release -p ntsyscall_gen -- /path/to/ntdll.dll

pub const NTDLL_VERSION: &str = "10.0.26100.3194";

#[derive(Debug, Clone, Copy)]
pub struct NtSyscallEntry {
    pub number: u16,
    pub name: &'static str,
}

pub const NT_SYSCALL_TABLE: &[NtSyscallEntry] = &[
    NtSyscallEntry { number: 0x00, name: "NtAcceptConnectPort" },
    NtSyscallEntry { number: 0x01, name: "NtAccessCheck" },
    NtSyscallEntry { number: 0x04, name: "NtAllocateVirtualMemory" },
    NtSyscallEntry { number: 0x06, name: "NtClose" },
    NtSyscallEntry { number: 0x18, name: "NtCreateFile" },
    // ... all 486+ entries
];

pub fn lookup_syscall_number(name: &str) -> Option<u16> { ... }
pub fn lookup_syscall_name(number: u16) -> Option<&'static str> { ... }
```

**CI integration**: Run tool against multiple ntdll.dll versions, diff output to detect changes.

**Deliverables**:
- [ ] `tools/ntsyscall_gen/Cargo.toml`
- [ ] `tools/ntsyscall_gen/src/main.rs` — PE parser + SVC scanner
- [ ] `tools/ntsyscall_gen/src/pe.rs` — Minimal PE/COFF parser (export directory)
- [ ] `tools/ntsyscall_gen/src/scanner.rs` — ARM64 SVC pattern scanner
- [ ] `tools/ntsyscall_gen/src/codegen.rs` — Rust code generation
- [ ] `tools/ntsyscall_gen/README.md` — Usage, DLL source instructions
- [ ] Generated `kernel/src/abi/windows/syscall_table.rs` (committed)
- [ ] Test: verify against known syscall tables from hfiref0x/SyscallTables

---

### Phase 1: PE/COFF Loader

**Goal**: Kernel module to load PE executables and DLLs into task memory.

**Location**: `kernel/src/task/pe_loader/`

**PE structures needed** (all no_std compatible):
```
IMAGE_DOS_HEADER          — MZ header, e_lfanew offset
IMAGE_FILE_HEADER         — Machine (0xAA64), NumberOfSections
IMAGE_OPTIONAL_HEADER64   — Magic (0x20b), ImageBase, AddressOfEntryPoint
IMAGE_SECTION_HEADER      — VirtualAddress, PointerToRawData, Characteristics
IMAGE_DATA_DIRECTORY      — Import, Export, BaseRelocation, TLS
IMAGE_IMPORT_DESCRIPTOR   — OriginalFirstThunk, Name, FirstThunk
IMAGE_EXPORT_DIRECTORY    — AddressOfFunctions, AddressOfNames, AddressOfOrdinals
IMAGE_BASE_RELOCATION     — PageRVA, BlockSize, entries
IMAGE_TLS_DIRECTORY64     — StartAddressOfRawData, AddressOfCallBacks
```

**Loading algorithm**:
1. Validate: MZ header → PE signature → Machine 0xAA64 → PE32+ magic 0x20b
2. Map sections (VirtualAddress → task VM, copy from PointerToRawData)
3. Apply base relocations (delta = actual_base - preferred_base)
4. Resolve imports (walk Import Directory → load DLLs → build IAT)
5. Initialize TLS (allocate TLS slot, copy template, call callbacks)
6. Call DllMain for DLL_PROCESS_ATTACH

**ARM64 relocation types** (18 types from LLVM COFF.h):
- `IMAGE_REL_ARM64_ABSOLUTE` (0x0000) — skip
- `IMAGE_REL_ARM64_ADDR64` (0x000E) — 64-bit VA
- `IMAGE_REL_ARM64_BRANCH26` (0x0003) — B/BL (26-bit offset)
- `IMAGE_REL_ARM64_PAGEBASE_REL21` (0x0004) — ADRP instruction
- `IMAGE_REL_ARM64_PAGEOFFSET_12A` (0x0006) — ADD immediate (imm12)
- `IMAGE_REL_ARM64_PAGEOFFSET_12L` (0x0007) — LDR immediate (imm12)
- `IMAGE_REL_ARM64_REL32` (0x0011) — 32-bit PC-relative
- ... (full list in generated code)

**Deliverables**:
- [ ] `kernel/src/task/pe_loader/mod.rs` — Public interface
- [ ] `kernel/src/task/pe_loader/headers.rs` — PE structure definitions
- [ ] `kernel/src/task/pe_loader/loader.rs` — Section mapping, relocations
- [ ] `kernel/src/task/pe_loader/import.rs` — Import resolution
- [ ] `kernel/src/task/pe_loader/export.rs` — Export lookup (for DLLs)
- [ ] `kernel/src/task/pe_loader/reloc.rs` — ARM64 relocation processing
- [ ] `kernel/src/task/pe_loader/tls.rs` — TLS initialization
- [ ] `kernel/src/task/pe_loader/tests/` — Test with simple PE binaries
- [ ] Integration: hook into AbiModule::execute_binary()

---

### Phase 2: Windows ABI Module (Skeleton)

**Goal**: Implement AbiModule trait for Windows AArch64, wire up PE loader and syscall dispatch.

**Location**: `kernel/src/abi/windows/`

**Module structure**:
```
kernel/src/abi/windows/
  mod.rs              — WindowsAarch64Abi struct, AbiModule impl
  aarch64/
    mod.rs            — AArch64-specific trapframe handling
    syscall.rs        — syscall_table! macro invocation + dispatch
  syscall_table.rs    — Generated by ntsyscall_gen (Phase 0)
  pe_detect.rs        — can_execute_binary() PE detection
  object/
    mod.rs            — NT Object Manager (handle → kernel object mapping)
    file.rs           — NT File objects → Scarlet VFS handles
    process.rs        — NT Process objects → Scarlet Task
    thread.rs         — NT Thread objects
    event.rs          — NT Event/Mutant/Timer objects
  peb.rs              — PEB/TEB initialization
  heap.rs             — NT Heap (RtlAllocateHeap stub)
  error.rs            — NTSTATUS codes
```

**AbiModule implementation**:
```rust
pub struct WindowsAarch64Abi {
    namespace: Arc<TaskNamespace>,
    handle_table: Vec<Option<NtObject>>,  // NT handle → object mapping
    peb: Arc<Mutex<PebData>>,
    tls_index: u32,
    // ...
}

impl AbiModule for WindowsAarch64Abi {
    fn name() -> &'static str { "windows-aarch64" }
    
    fn can_execute_binary(&self, file, path, _) -> Option<u8> {
        // Check MZ header (0x5A4D) + PE signature + Machine 0xAA64
    }
    
    fn execute_binary(&self, file, argv, envp, task, trapframe) -> Result<()> {
        // 1. Initialize PEB/TEB
        // 2. Load ntdll.dll, kernel32.dll, ucrtbase.dll
        // 3. Resolve app imports
        // 4. Set up stack (Windows ABI layout: RTL_USER_PROCESS_PARAMETERS, etc.)
        // 5. Set entry point (BaseThreadInitThunk or app entry)
    }
    
    fn handle_syscall(&self, trapframe) -> Result<usize, &'static str> {
        // Extract syscall number from trapframe (ESR_EL1 ISS field)
        // Dispatch to NT handler
    }
}
```

**AArch64 trap handling**:
- SVC from EL0 → exception at offset 0x400 in vector table
- ESR_EL1 EC field: 0x15 = SVC64
- ESR_EL1 ISS field bits [24:20] = SVC immediate = syscall number
- Scarlet's existing AArch64 trap handler needs modification to pass SVC immediate

**Deliverables**:
- [ ] `kernel/src/abi/windows/mod.rs` — Module root, AbiModule impl
- [ ] `kernel/src/abi/windows/aarch64/mod.rs` — AArch64 trapframe + dispatch
- [ ] `kernel/src/abi/windows/aarch64/syscall.rs` — syscall_table! invocation
- [ ] `kernel/src/abi/windows/pe_detect.rs` — PE binary detection
- [ ] `kernel/src/abi/windows/peb.rs` — PEB/TEB structures + initialization
- [ ] `kernel/src/abi/windows/error.rs` — NTSTATUS constants
- [ ] `kernel/src/abi/windows/object/mod.rs` — NT Object Manager skeleton
- [ ] AArch64 trap handler modification to extract SVC immediate

---

### Phase 3: Minimal NT Syscall Handlers (Console "Hello World")

**Goal**: Implement enough NT syscalls to run a simple console app that prints text.

**Required NT syscalls** (~20 functions):

**Memory Management**:
- `NtAllocateVirtualMemory` → Scarlet VM alloc
- `NtFreeVirtualMemory` → Scarlet VM free
- `NtQueryVirtualMemory` → Scarlet VM query
- `NtProtectVirtualMemory` → Scarlet VM protect

**File I/O**:
- `NtCreateFile` → Scarlet VFS open
- `NtReadFile` → Scarlet VFS read
- `NtWriteFile` → Scarlet VFS write / TTY write
- `NtClose` → Scarlet handle close
- `NtQueryInformationFile` → Scarlet VFS stat
- `NtSetInformationFile` → Scarlet VFS (partial)

**Process/Thread**:
- `NtCreateProcess` → Scarlet Task create (or stub)
- `NtCreateThread` → Scarlet Task clone
- `NtTerminateProcess` → Scarlet Task exit
- `NtGetCurrentProcessId` → task.get_id()
- `NtQueryInformationProcess` → stub

**Object**:
- `NtWaitForSingleObject` → Scarlet wait
- `NtSetEvent` → Scarlet event signal

**Console** (for stdout/stderr):
- Console handles (STD_OUTPUT_HANDLE=0x7, STD_INPUT_HANDLE=0x3, STD_ERROR_HANDLE=0xB)
- Map to Scarlet TTY devices

**Heap**:
- `NtHeapCreate` / `RtlAllocateHeap` / `RtlFreeHeap` → Simple bump allocator

**Deliverables**:
- [ ] NT memory syscalls → `kernel/src/abi/windows/object/memory.rs`
- [ ] NT file syscalls → `kernel/src/abi/windows/object/file.rs`
- [ ] NT process/thread syscalls → `kernel/src/abi/windows/object/process.rs`
- [ ] NT object syscalls → `kernel/src/abi/windows/object/event.rs`
- [ ] Console handle initialization → `kernel/src/abi/windows/console.rs`
- [ ] Heap implementation → `kernel/src/abi/windows/heap.rs`
- [ ] **Milestone**: ARM64 `hello.exe` prints to Scarlet TTY

---

### Phase 4: DLL Loading & Advanced Syscalls

**Goal**: Support loading dependent DLLs and more complex applications.

**DLL loading**:
- DLL search order: app directory → system directory → Windows directory
- Circular dependency detection
- Reference counting for DLL lifetime
- DLL_PROCESS_ATTACH / DLL_THREAD_ATTACH / DLL_THREAD_DETACH / DLL_PROCESS_DETACH
- LoadLibrary / GetProcAddress from within loaded code

**Additional syscalls**:
- `NtCreateSection` → Shared memory / memory-mapped files
- `NtMapViewOfSection` → Map section into process
- `NtUnmapViewOfSection`
- `NtQuerySystemInformation` → Version info, etc.
- `NtSetTimer` / `NtCancelTimer` → Scarlet timers
- `NtDelayExecution` → Scarlet sleep
- `NtQueryPerformanceCounter` → Timer read

**Unicode support**:
- NT syscalls use UTF-16 (wide strings)
- Convert to/from UTF-8 for Scarlet VFS paths

**Deliverables**:
- [ ] DLL loader with dependency resolution
- [ ] LoadLibrary / GetProcAddress support
- [ ] Shared memory (sections)
- [ ] Timer syscalls
- [ ] Unicode path conversion
- [ ] **Milestone**: ARM64 console app with multiple DLLs runs

---

### Phase 5: Build Integration & Testing

**Goal**: Integrate into Scarlet build system, add tests and documentation.

**Build system** (`Makefile.toml`):
- `cargo make build-ntsyscall-gen` — Build the extractor tool
- `cargo make generate-syscall-table` — Run tool against ntdll.dll
- Add `windows-aarch64` ABI to kernel features
- AArch64 build targets include Windows ABI module

**Testing**:
- Unit tests for PE loader (test with crafted PE binaries)
- Unit tests for relocation processing
- Integration tests with simple ARM64 PE executables
- Syscall handler tests

**Documentation**:
- `docs/abi/windows/design.md` — Architecture overview
- `docs/abi/windows/status.md` — Implementation status
- `docs/abi/windows/setup.md` — How to set up DLLs and run apps

**Deliverables**:
- [ ] Makefile.toml integration
- [ ] Test suite
- [ ] Documentation
- [ ] CI pipeline for syscall table generation

---

## File Tree (Final State)

```
tools/
  ntsyscall_gen/
    Cargo.toml
    README.md
    src/
      main.rs
      pe.rs
      scanner.rs
      codegen.rs

kernel/src/
  abi/
    windows/
      mod.rs                  — WindowsAarch64Abi
      aarch64/
        mod.rs                — AArch64 trapframe handling
        syscall.rs            — syscall_table! dispatch
      syscall_table.rs        — Generated by ntsyscall_gen
      pe_detect.rs            — PE binary detection
      peb.rs                  — PEB/TEB structures
      error.rs                — NTSTATUS codes
      console.rs              — Console handle mapping
      heap.rs                 — NT heap
      object/
        mod.rs                — NT Object Manager
        file.rs               — NT File → VFS
        process.rs            — NT Process → Task
        thread.rs             — NT Thread → Task
        event.rs              — NT Event/Mutant/Timer
        memory.rs             — NT Virtual Memory → VM
  task/
    pe_loader/
      mod.rs                  — Public interface
      headers.rs              — PE structure definitions
      loader.rs               — Section mapping
      import.rs               — Import resolution
      export.rs               — Export lookup
      reloc.rs                — ARM64 relocations
      tls.rs                  — TLS initialization
      tests/

docs/
  abi/
    windows/
      design.md
      status.md
      setup.md
```

## Risk & Open Questions

1. **DLL availability**: Users must provide ARM64 Windows DLLs. No x86_64 DLL support.
2. **ntdll.dll internal calls**: ntdll may call into the kernel via mechanisms other than SVC (e.g., Ldr* functions, direct struct access to PEB). Need to audit.
3. **SEH (Structured Exception Handling)**: Windows uses SEH extensively. ARM64 uses unwinding tables. May need SEH emulation.
4. **COM initialization**: Many Windows apps require COM. Long-term challenge.
5. **AArch64 Scarlet maturity**: The AArch64 port is less mature than RISC-V. May need fixes there first.
6. **ntdll.dll self-relocation**: ntdll uses relocations. Our PE loader must handle this before ntdll can be used for anything.
