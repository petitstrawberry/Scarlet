# Darwin ABI Module — Implementation Plan

Scarlet OS kernel-native execution of Darwin (macOS) aarch64 binaries.

- **State**: Planning
- **Target architecture**: AArch64 (Apple Silicon)
- **Scope**: CLI tools only (no GUI / AppKit / Metal)
- **Reference branch**: `feature/windows-aarch64-abi-module` (design patterns)

---

## 1. macOS Software Stack — What We Need to Emulate

```
┌─────────────────────────────────────────────────────────────────────┐
│                        User Applications                           │
│  (ls, cat, curl, python, ruby, node, static C binaries, etc.)     │
├─────────────────────────────────────────────────────────────────────┤
│                    Frameworks & Libraries                           │
│  ┌──────────────┐ ┌──────────────┐ ┌───────────┐ ┌──────────────┐ │
│  │   AppKit     │ │  Foundation  │ │  CoreLib  │ │   libcurl    │ │
│  │  (GUI)  ✗    │ │  (ObjC)  △   │ │ (C)   ○   │ │  (C)     ○   │ │
│  └──────────────┘ └──────────────┘ └───────────┘ └──────────────┘ │
├─────────────────────────────────────────────────────────────────────┤
│                     Objective-C Runtime (objc4)         △          │
│  (Dynamic dispatch, ARC, classes, selectors, protocols)            │
├─────────────────────────────────────────────────────────────────────┤
│                   libSystem.dylib (= glibc equivalent)   ○         │
│  ┌────────────┐ ┌────────────┐ ┌───────────────┐ ┌─────────────┐ │
│  │  libc (✓)  │ │ libpthread │ │ libdispatch   │ │ libplatform │ │
│  │  BSD API   │ │   (△)      │ │  GCD (△)      │ │   (△)       │ │
│  └────────────┘ └────────────┘ └───────────────┘ └─────────────┘ │
├─────────────────────────────────────────────────────────────────────┤
│  libSystem_kernel.dylib                              ○             │
│  (Thin wrapper: SVC #0x80 / SVC #0x81 → kernel)                   │
├─────────────────────────────────────────────────────────────────────┤
│                          dyld (dynamic linker)          △          │
│  (Self-relocation, code signing, shared cache, lazy binding)      │
├═════════════════════════════════════════════════════════════════════┤
│                    ════ XNU Kernel ════                ✗ (target)   │
│  ┌─────────────────────────┐  ┌──────────────────────────────────┐ │
│  │     BSD Layer           │  │        Mach Layer                │ │
│  │  • POSIX syscalls       │  │  • mach_msg (IPC)                │ │
│  │  • VFS, fd table        │  │  • mach_port (rights mgmt)       │ │
│  │  • process (fork/exec)  │  │  • task / thread                 │ │
│  │  • signals              │  │  • vm_allocate / vm_map          │ │
│  │  • network (socket)     │  │  • exception ports               │ │
│  │  • mmap / mprotect      │  │  • clock, host info              │ │
│  └─────────┬───────────────┘  └─────────────┬────────────────────┘ │
│            │     System Call Interface       │                      │
│            │  SVC #0x80 (BSD class)          │  SVC #0x81 (Mach)   │
│            │  x16 = syscall# + 0x2000000     │  x16 = -(mach_trap) │
│            └─────────────────────────────────┴─────────────────────┘
└─────────────────────────────────────────────────────────────────────┘

Legend:  ○ = implement in Phase 1-3   △ = Phase 4+ / stub   ✗ = out of scope
```

### Key Observation

Darwin binaries issue two classes of system calls via distinct SVC instructions:

| Class | SVC | Encoding (x16) | Direction |
|-------|-----|----------------|-----------|
| **BSD** | `SVC #0x80` | `0x2000000 \| n` (positive) | POSIX: file I/O, process, network, signals |
| **Mach** | `SVC #0x81` | negative (sign-extended) | Low-level: IPC, vm, task/port management |

On aarch64 macOS, registers at SVC entry:
- `x16` = syscall number (not `x8` like Linux)
- `x0`–`x5` = arguments
- `x0` = return value

---

## 2. Scarlet Darwin ABI Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                   Darwin AArch64 Binary                             │
│              (Mach-O format, linked against libSystem)              │
└───────────┬─────────────────────────────────────────────────────────┘
            │ SVC #0x80 (BSD)            │ SVC #0x81 (Mach)
            │ x16 = BSD syscall#         │ x16 = Mach trap#
            ▼                            ▼
┌───────────────────────────────────────────────────────────────────┐
│              AArch64 Trap Handler (exception.rs)                  │
│   ExceptionClass::SvcAarch64 → syscall_dispatcher(trapframe)     │
│                                                                   │
│   syscall_dispatcher resolves ABI via PC → task.with_resolve_abi  │
└───────────────────────────┬───────────────────────────────────────┘
                            │
                            ▼
┌───────────────────────────────────────────────────────────────────┐
│           DarwinAarch64Abi :: handle_syscall(trapframe)           │
│                                                                   │
│   1. Read x16 (syscall number)                                   │
│   2. Decode SVC imm from ESR.ISS (0x80 vs 0x81)                  │
│   3. Route:                                                       │
│      ┌─── SVC #0x80 ──→ dispatch_bsd_syscall(num, tf)            │
│      └─── SVC #0x81 ──→ dispatch_mach_syscall(num, tf)           │
└───────────┬───────────────────────────────┬───────────────────────┘
            │                               │
            ▼                               ▼
┌─────────────────────┐       ┌─────────────────────────────────────┐
│  BSD Syscalls       │       │  Mach Traps                         │
│                     │       │                                     │
│  VFS ───────────────────→ Scarlet VfsManager                     │
│  (open, read,       │       │  mach_msg ────→ Scarlet EventSystem │
│   write, close,     │       │  + SharedMemory (large payloads)    │
│   stat, mmap,       │       │  + LocalSocket (handle transfer)    │
│   socket, ...)      │       │                                     │
│                     │       │  mach_port ──→ HandleTable          │
│  Process ───────────────→ Scarlet TaskManager                   │
│  (fork, execve,     │       │  + EventSubscription               │
│   exit, getpid,     │       │                                     │
│   wait4, ...)       │       │  vm_* ────────→ Scarlet VMM         │
│                     │       │  (delegates to mmap/munmap)         │
│  Network ──────────────→ Scarlet NetworkManager                  │
│  (socket, bind,     │       │                                     │
│   connect, listen,  │       │  task_for_pid → self only           │
│   accept, ...)      │       │  thread_create → stub              │
└─────────────────────┘       └─────────────────────────────────────┘
```

### Critical AArch64 Difference: SVC Immediate

On Darwin aarch64, `SVC #0x80` and `SVC #0x81` encode the syscall class in the **SVC immediate** field of the instruction itself. The ESR_EL1.ISS lower 16 bits contain this immediate.

**Current Scarlet behavior**: `exception.rs` treats all `SvcAarch64` as the same — it calls `syscall_dispatcher` which resolves ABI by PC address. For Darwin, we need to:

1. Extract the SVC immediate from `ESR_EL1.ISS[15:0]`
2. Use it to determine BSD vs Mach class
3. Pass this info to the Darwin ABI's `handle_syscall`

**Implementation note**: The ABI module is resolved by PC address. If the PC falls in a Darwin binary's address space, `DarwinAarch64Abi` is selected. The SVC immediate is available in `trapframe.esr_el1` — the ABI's `handle_syscall` can decode it.

---

## 3. Mach-O Binary Format and Loading

```
Mach-O File Layout (64-bit):
┌──────────────────────────────────┐
│  Mach-O Header (mach_header_64)  │
│  magic    = 0xFEEDFACF           │
│  cputype  = 0x0100000C (ARM64)   │
│  cpusubtype = 0x00000000 (ALL)   │
│  filetype = (see below)          │
│  ncmds    = N                    │
│  sizeofcmds = total              │
│  flags    = ...                  │
├──────────────────────────────────┤
│  Load Command 1 (LC_SEGMENT_64)  │
│  ┌──────────────────────────────┐│
│  │ segname, vmaddr, vmsize      ││
│  │ fileoff, filesize            ││
│  │ maxprot, initprot            ││
│  │ nsects, flags                ││
│  └──────────────────────────────┘│
│  Load Command 2 (LC_SEGMENT_64)  │
│  ...                             │
│  Load Command N                  │
│  (LC_DYLD_INFO, LC_SYMTAB,      │
│   LC_DYSYMTAB, LC_LOAD_DYLIB,   │
│   LC_MAIN / LC_UNIXTHREAD)      │
├──────────────────────────────────┤
│  __TEXT segment data             │
│  __DATA segment data             │
│  __LINKEDIT segment data         │
│  ...                             │
└──────────────────────────────────┘

filetype values:
  MH_EXECUTE    (0x02)  → executable
  MH_DYLIB      (0x06)  → dynamic library
  MH_BUNDLE     (0x08)  → loadable bundle
  MH_OBJECT     (0x01)  → relocatable object
```

### Loading Strategy

```
                    Mach-O Binary
                         │
            ┌────────────┴────────────┐
            │                         │
    Static binary?            Dynamic binary?
    (no LC_LOAD_DYLIB)       (has LC_LOAD_DYLIB)
            │                         │
            ▼                         ▼
    Direct segment map          Phase 1: map segments only
    + jump to LC_MAIN           (skip dylib resolution)
                                Phase 4+: minimal dyld stub
                                    or require static binaries
```

**Phase 1 approach**: Support only statically-linked Mach-O binaries. Map segments using Scarlet's VMM, set up stack with argv/envp, jump to entry point from `LC_MAIN` or `LC_UNIXTHREAD`.

**Phase 4+ approach**: Minimal dyld replacement that:
- Maps segments from LC_SEGMENT_64 commands
- Processes LC_LOAD_DYLIB by mapping those libraries from the Darwin VFS namespace
- Performs symbol binding (non-lazy first, lazy on fault)
- Handles relocations from LC_DYLD_INFO

---

## 4. Darwin ↔ Scarlet Component Mapping

```
┌─────────────────────────────────────────────────────────────────┐
│                     Darwin (macOS)                              │
│                                                                 │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌───────────────┐  │
│  │ BSD VFS  │  │ BSD Proc │  │ BSD Sock │  │ Mach IPC      │  │
│  │ open     │  │ fork     │  │ socket   │  │ mach_msg      │  │
│  │ read     │  │ execve   │  │ bind     │  │ mach_port_*   │  │
│  │ write    │  │ exit     │  │ listen   │  │ task_for_pid  │  │
│  │ mmap     │  │ getpid   │  │ accept   │  │ vm_allocate   │  │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └───────┬───────┘  │
│       │              │              │                │          │
═══════╪══════════════╪══════════════╪════════════════╪══════════
       │   Darwin ABI Module (translation layer)      │
═══════╪══════════════╪══════════════╪════════════════╪══════════
       │              │              │                │          │
│  ┌────▼─────┐  ┌────▼─────┐  ┌────▼─────┐  ┌───────▼───────┐  │
│  │  Scarlet  │  │  Scarlet  │  │  Scarlet  │  │   Scarlet     │  │
│  │VfsManager │  │   Task    │  │  Network  │  │   IPC         │  │
│  │           │  │ Manager   │  │ Manager   │  │               │  │
│  │ ext2      │  │ fork()    │  │ TCP/IP    │  │ EventSystem   │  │
│  │ FAT32     │  │ exec()    │  │ UDP       │  │ SharedMemory  │  │
│  │ TmpFS     │  │ exit()    │  │ Local     │  │ Pipe          │  │
│  │ OverlayFS │  │ clone()   │  │ ARP/ICMP  │  │ LocalSocket   │  │
│  └──────────┘  └──────────┘  └──────────┘  │ HandleTable    │  │
│                                              │ (SCM_RIGHTS)   │  │
│                                              └───────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

### Detailed Mapping Table

| Darwin Component | Scarlet Equivalent | Mapping Complexity |
|:---|:---|:---:|
| `open(2)` / `openat(2)` | `VfsManager::open()` | Low |
| `read(2)` / `write(2)` | `FileObject::read/write` via handle | Low |
| `close(2)` | `HandleTable::remove()` | Low |
| `mmap(2)` / `mprotect(2)` | `VmManager::mmap/mprotect` | Low |
| `munmap(2)` | `VmManager::munmap` | Low |
| `socket(2)` | `NetworkManager::create_socket` | Low |
| `bind/connect/listen/accept` | SocketObject methods | Low |
| `sendto/recvfrom` | SocketObject methods | Low |
| `fork(2)` | `Task::clone_task()` | Medium |
| `execve(2)` | Mach-O loader + `Task::exec` | Medium |
| `wait4(2)` | Task event wait (ChildExit) | Medium |
| `sigaction/sigreturn` | Event system → signal frame | Medium |
| `mach_msg` | `EventManager::send_event` + `SharedMemory` | High |
| `mach_port_allocate` | `HandleTable::insert` + `EventManager` | Medium |
| `mach_port_deallocate` | `HandleTable::remove` | Low |
| `vm_allocate` | `mmap` (anonymous) | Low |
| `vm_deallocate` | `munmap` | Low |
| `task_for_pid` | Self-only stub | Low |
| `thread_create` | Stub (return error) | Low |

---

## 5. Mach Port → Scarlet IPC Mapping

```
┌─────────────────────────────────────────────────────────────────────┐
│                        Darwin Mach IPC                              │
│                                                                     │
│  Task A                           Task B                            │
│  ┌─────────────┐                  ┌─────────────┐                  │
│  │ Send Right  │──mach_msg()───→  │ Recv Right  │                  │
│  │ (port name) │                  │ (port name) │                  │
│  └─────────────┘                  └──────┬──────┘                  │
│       │                                  │                         │
│       │  Small msg (<64KB)               │  Large msg (≥64KB)      │
│       │  ┌─────────────────┐             │  ┌─────────────────┐   │
│       │  │ header + body   │             │  │ header (small)  │   │
│       │  │ (inline data)   │             │  │ + OOL data ptr  │   │
│       │  │ + port desc(s)  │             │  │ + port desc(s)  │   │
│       │  └────────┬────────┘             │  └────────┬────────┘   │
═══════╪═══════════╪════════════════════════╪═════════╪═════════════
        │           │   Scarlet IPC          │         │
═══════╪═══════════╪════════════════════════╪═════════╪═════════════
        │           ▼                        │         ▼             │
│       │  ┌─────────────────┐              │  ┌─────────────────┐  │
│       │  │  Event System   │              │  │  SharedMemory   │  │
│       │  │  Event::direct  │              │  │  + Event        │  │
│       │  │  EventPayload:: │              │  │  (notification  │  │
│       │  │  Bytes(body)    │              │  │   of shmem id)  │  │
│       │  └─────────────────┘              │  └─────────────────┘  │
│       │           │                        │         │             │
│       │           ▼                        │         ▼             │
│       │  ┌─────────────────────────────────────────────────────┐  │
│       │  │              Scarlet Kernel                         │  │
│       │  │                                                     │  │
│       │  │  Port Right → HandleTable entry                     │  │
│       │  │  Send right = handle with EventSender capability    │  │
│       │  │  Recv right = handle with EventReceiver capability  │  │
│       │  │  Send-once = handle with CloneOps (one-shot)       │  │
│       │  │  Port set  = multiple subscriptions + Selectable    │  │
│       │  │                                                     │  │
│       │  │  Handle transfer: LocalSocket.send_handle()         │  │
│       │  │  (clone_for_dup ensures correct refcounting)        │  │
│       │  └─────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────┘
```

### Mach Port Rights → Scarlet HandleTable

| Mach Right | Scarlet Representation | Clone Behavior |
|:---|:---|:---|
| `MACH_PORT_RIGHT_SEND` | `HandleTable` entry pointing to `EventChannel` or `EventSubscription` | `clone_for_dup` — increments refcount |
| `MACH_PORT_RIGHT_RECEIVE` | `HandleTable` entry pointing to `EventSubscription` with filters | `clone_for_dup` — only one receiver allowed (enforce in ABI) |
| `MACH_PORT_RIGHT_SEND_ONCE` | Handle with `CloneOps::custom_clone` that invalidates after first use | One-shot: `clone_for_dup` returns error |
| `MACH_PORT_RIGHT_PORT_SET` | Collection of `EventSubscription` handles + `Selectable` for poll | N/A — managed by ABI module |
| `MACH_PORT_RIGHT_DEAD_NAME` | Invalid handle / closed handle | N/A |

### mach_msg Flow in Scarlet

```
mach_msg(options, send, recv) 
         │
         ├─── MACH_SEND_MSG ──┐
         │                    │
         │         ┌──────────┴──────────┐
         │         │  Payload size?       │
         │         └──┬───────────────┬───┘
         │            │ < 64KB        │ ≥ 64KB
         │            ▼               ▼
         │    EventPayload::Bytes   SharedMemory::new()
         │    EventManager::         + Event notification
         │      send_event()        + LocalSocket::send_handle()
         │                          (to transfer shmem to receiver)
         │
         └─── MACH_RCV_MSG ──┐
                              │
                    ┌─────────┴──────────┐
                    │  Pending message?   │
                    └──┬──────────────┬───┘
                       │ Yes          │ No
                       ▼              ▼
                 Copy to user       Block (wait)
                 buffer from        via EventSubscription
                 EventQueue         or return MACH_RCV_TIMED_OUT
```

---

## 6. Directory Structure

```
kernel/src/abi/
├── mod.rs                          # ABI registry (add: pub mod darwin)
├── linux/                          # Existing
├── scarlet/                        # Existing
├── xv6/                            # Existing
└── darwin/                         # NEW
    ├── mod.rs                      # Module root, DarwinAbiVersion enum,
    │                               # registration (register_abi!)
    ├── aarch64/
    │   ├── mod.rs                  # DarwinAarch64Abi struct,
    │   │                           #   impl AbiModule trait
    │   │                           #   handle_syscall → SVC imm dispatch
    │   ├── syscall_table.rs        # Auto-generated (darwin_syscall_gen)
    │   │                           #   BSD_SYSCALL_TABLE + MACH_TRAP_TABLE
    │   ├── bsd_syscalls.rs         # BSD syscall implementations:
    │   │                           #   fs: open/read/write/close/stat/mmap
    │   │                           #   proc: fork/execve/exit/getpid/wait4
    │   │                           #   net: socket/bind/connect/listen/accept
    │   │                           #   signal: sigaction/kill/sigreturn
    │   ├── mach_syscalls.rs        # Mach trap implementations:
    │   │                           #   mach_msg (Event System)
    │   │                           #   mach_port_allocate/deallocate
    │   │                           #   task_for_pid, vm_allocate
    │   └── macho_loader.rs         # Mach-O binary loader
    ├── error.rs                    # Darwin errno → Scarlet error mapping
    │                               #   (KERN_SUCCESS, KERN_FAILURE, etc.)
    └── path.rs                     # Darwin path ↔ Scarlet VFS path
                                    #   /Users/foo → /home/foo
                                    #   /usr/lib → /.scarlet/darwin/usr/lib

tools/
└── darwin_syscall_gen/             # NEW: syscall table generator
    ├── Cargo.toml
    └── src/
        └── main.rs                 # Input: syscalls.master or dylib
                                    # Output: syscall_table.rs
```

---

## 7. AArch64 Syscall Calling Convention

### Register Usage at SVC

```
BSD syscalls (SVC #0x80):
┌────────┬──────────────────────────────────┐
│ Register │ Purpose                        │
├────────┼──────────────────────────────────┤
│  x16    │ Syscall number │ SYSCALL_CLASS_UNIX │
│  x0     │ arg0 / return value             │
│  x1     │ arg1                            │
│  x2     │ arg2                            │
│  x3     │ arg3                            │
│  x4     │ arg4                            │
│  x5     │ arg5                            │
│  x8     │ (clobbered by kernel)           │
└────────┴──────────────────────────────────┘

Mach traps (SVC #0x81):
┌────────┬──────────────────────────────────┐
│ Register │ Purpose                        │
├────────┼──────────────────────────────────┤
│  x16    │ Mach trap number (negative)     │
│  x0     │ arg0 / return value             │
│  x1     │ arg1                            │
│  x2     │ arg2                            │
│  x3     │ arg3                            │
│  x4     │ arg4                            │
│  x5     │ arg5                            │
│  x6     │ arg6                            │
│  x7     │ (clobbered by kernel)           │
└────────┴──────────────────────────────────┘
```

### SVC Immediate Extraction

```rust
// In DarwinAarch64Abi::handle_syscall:
fn handle_syscall(&mut self, trapframe: &mut Trapframe) -> Result<usize, &'static str> {
    let esr = trapframe.esr_el1;
    let svc_imm = (esr & 0xFFFF) as u16;    // ISS[15:0] = SVC immediate
    let syscall_num = trapframe.regs.reg[16] as u32; // x16

    match svc_imm {
        0x80 => {
            // BSD syscall: x16 has class offset
            let bsd_num = (syscall_num & 0xFFFFFF) as u32;
            self.dispatch_bsd_syscall(bsd_num, trapframe)
        }
        0x81 => {
            // Mach trap: x16 has negative trap number
            let mach_num = syscall_num as i32;
            self.dispatch_mach_syscall(mach_num, trapframe)
        }
        _ => Err("Unknown SVC immediate for Darwin ABI"),
    }
}
```

### Darwin Error Encoding

Darwin uses **carry flag** for error indication (unlike Linux which uses negative return):

- `C=0`: success, `x0` = return value
- `C=1`: error, `x0` = errno (positive)

We need to set/clear carry in SPSR on return:

```rust
fn set_darwin_return(trapframe: &mut Trapframe, result: Result<usize, DarwinErrno>) {
    match result {
        Ok(val) => {
            trapframe.regs.reg[0] = val;
            // Clear carry flag in SPSR (C=0 → success)
            trapframe.spsr &= !(1 << 29);
        }
        Err(errno) => {
            trapframe.regs.reg[0] = errno as usize;
            // Set carry flag in SPSR (C=1 → error)
            trapframe.spsr |= 1 << 29;
        }
    }
}
```

---

## 8. Darwin Path Translation

The Overlay VFS mounts `/scarlet/system/darwin-aarch64/` as the task root, so system paths like `/usr/lib`, `/System/Library`, and `/bin` are resolved automatically. No manual prefix mapping is needed — same design as the Linux ABI.

The only manual translation required is for Darwin-specific user paths:

```
Darwin Path                    Scarlet VFS Path
─────────────                  ────────────────
/Users/alice/file.txt    →    /home/alice/file.txt
/usr/lib/libSystem.dylib →    /usr/lib/libSystem.dylib    (overlay FS resolves)
/usr/bin/ls              →    /usr/bin/ls                 (overlay FS resolves)
/System/Library/...      →    /System/Library/...         (overlay FS resolves)
/dev/null                →    /dev/null                   (bind mount from Scarlet)
/tmp/file                →    /tmp/file                   (bind mount from Scarlet)
relative/path            →    <cwd>/relative/path         (unchanged)
```

### VFS Namespace Isolation

Uses the same `setup_overlay_environment` pattern as the Linux ABI:

```
Darwin ABI Task VFS (overlay):
┌──────────────────────────────────────────────────────┐
│  OverlayFS                                           │
│  ┌────────────────────────────────────────────────┐  │
│  │ Upper: /data/config/darwin-aarch64/ (writable)│  │
│  │ Lower: /scarlet/system/darwin-aarch64/ (ro)   │  │
│  └────────────────────────────────────────────────┘  │
│                                                      │
│  Bind mounts from base VFS:                         │
│  /dev  ← /dev  (Scarlet devices)                    │
│  /home ← /home (shared user data)                   │
│  /tmp  ← /tmp  (shared temp)                        │
│  /scarlet ← / (Scarlet gateway, host OS access)     │
└──────────────────────────────────────────────────────┘
```

Only `/Users/alice/...` is translated to `/home/alice/...`. All other paths are resolved as-is by the overlay FS, so `path.rs` remains minimal.

---

## 9. Darwin Error Codes

| Darwin errno | Value | Scarlet Error |
|:---|:---:|:---|
| `EPERM` | 1 | Permission denied |
| `ENOENT` | 2 | File not found |
| `ESRCH` | 3 | Process not found |
| `EINTR` | 4 | Interrupted |
| `EIO` | 5 | I/O error |
| `ENXIO` | 6 | Device not found |
| `ENOMEM` | 12 | Out of memory |
| `EACCES` | 13 | Access denied |
| `EFAULT` | 14 | Bad address |
| `EINVAL` | 22 | Invalid argument |
| `EMFILE` | 24 | Too many open files |
| `ENOSYS` | 38 | Function not implemented |
| `ENOTSOCK` | 88 | Not a socket |
| `ECONNREFUSED` | 61 | Connection refused |

Mach return codes (different namespace):
| KERNReturn | Value | Meaning |
|:---|:---:|:---|
| `KERN_SUCCESS` | 0 | Success |
| `KERN_INVALID_ADDRESS` | 1 | Bad address |
| `KERN_PROTECTION_FAILURE` | 2 | Permission denied |
| `KERN_NO_ACCESS` | 8 | No access |
| `KERN_FAILURE` | 5 | Generic failure |
| `KERN_RESOURCE_SHORTAGE` | 6 | Out of resources |
| `KERN_NOT_RECEIVER` | 8 | Not port owner |
| `KERN_NO_SPACE` | 3 | No VM space |

---

## 10. `darwin_syscall_gen` Tool

### Purpose

Extract syscall numbers from macOS system artifacts and generate `syscall_table.rs`.

### Input Sources

1. **Primary**: XNU source `bsd/kern/syscalls.master` (authoritative BSD syscall list)
2. **Secondary**: `libSystem_kernel.dylib` (extract actual numbers used by a specific macOS build)
3. **Mach traps**: XNU source `osfmk/kern/syscall_sw.c` (mach_trap_table)

### Output

```rust
// Auto-generated by darwin_syscall_gen
// macOS Sequoia 15.0 (build 24A335)
// DO NOT EDIT MANUALLY

pub const SYSCALL_TABLE_VERSION: &str = "15.0.0 (24A335)";
pub const SYSCALL_CLASS_UNIX: u32 = 0x2000000;

/// BSD syscall table entry
pub struct BsdSyscallEntry {
    pub number: u32,     // Raw number (without class offset)
    pub name: &'static str,
    pub nargs: u8,
    pub implemented: bool,  // Whether Scarlet implements this
}

pub const BSD_SYSCALL_TABLE: &[BsdSyscallEntry] = &[
    BsdSyscallEntry { number: 1, name: "exit", nargs: 1, implemented: true },
    BsdSyscallEntry { number: 2, name: "fork", nargs: 0, implemented: true },
    BsdSyscallEntry { number: 3, name: "read", nargs: 3, implemented: true },
    // ...
];

/// Mach trap table entry
pub struct MachTrapEntry {
    pub number: i32,     // Negative value
    pub name: &'static str,
    pub nargs: u8,
    pub implemented: bool,
}

pub const MACH_TRAP_TABLE: &[MachTrapEntry] = &[
    MachTrapEntry { number: -26, name: "mach_msg", nargs: 7, implemented: true },
    MachTrapEntry { number: -28, name: "mach_port_allocate", nargs: 3, implemented: true },
    // ...
];
```

---

## 11. Implementation Phases

### Phase 1: Foundation (Week 1-2)

```
┌─────────────────────────────────────────┐
│  kernel/src/abi/darwin/                 │
│                                         │
│  ✓ mod.rs         (module registration) │
│  ✓ aarch64/mod.rs (DarwinAarch64Abi)    │
│  ✓ error.rs       (errno conversion)    │
│  ✓ path.rs        (path translation)    │
│                                         │
│  tools/darwin_syscall_gen/              │
│                                         │
│  ✓ Cargo.toml                           │
│  ✓ src/main.rs   (syscalls.master → .rs)│
│                                         │
│  Generated:                             │
│  ✓ aarch64/syscall_table.rs             │
│                                         │
│  Deliverables:                          │
│  • Module compiles and registers        │
│  • Trap handler detects SVC #0x80/0x81  │
│  • Mach-O loader maps static binaries   │
│  • can_execute_binary detects Mach-O    │
└─────────────────────────────────────────┘
```

### Phase 2: BSD Syscalls (Week 3-4)

```
┌─────────────────────────────────────────┐
│  kernel/src/abi/darwin/aarch64/         │
│                                         │
│  ✓ bsd_syscalls.rs                      │
│    • File I/O: open/read/write/close    │
│    • Memory: mmap/munmap/mprotect       │
│    • Process: fork/execve/exit/getpid   │
│    • Network: socket/bind/connect/      │
│              listen/accept/sendto/      │
│              recvfrom/shutdown          │
│    • FS metadata: stat/lstat/fstat      │
│    • Signals: sigaction/kill/sigreturn  │
│                                         │
│  Testing:                               │
│  • Static hello-world Mach-O binary     │
│  • Static ls/cat/echo equivalents       │
└─────────────────────────────────────────┘
```

### Phase 3: Mach Traps — Minimum Stubs (Week 5)

```
┌─────────────────────────────────────────┐
│  kernel/src/abi/darwin/aarch64/         │
│                                         │
│  ✓ mach_syscalls.rs                     │
│    • mach_msg      → EventManager       │
│    • mach_port_allocate → HandleTable   │
│    • mach_port_deallocate               │
│    • task_for_pid  → self-only          │
│    • vm_allocate   → mmap delegate      │
│    • vm_deallocate → munmap delegate    │
│    • mach_task_self → constant          │
│    • semaphore_*  → stub                │
│                                         │
│  Key integration:                       │
│  • Mach port names ↔ HandleTable       │
│  • Small messages → EventPayload::Bytes│
│  • Handle transfer → LocalSocket       │
└─────────────────────────────────────────┘
```

### Phase 4: Dynamic Linking (Week 6-8)

```
┌─────────────────────────────────────────┐
│  Minimal dyld replacement               │
│                                         │
│  • LC_LOAD_DYLIB resolution             │
│  • Non-lazy symbol binding              │
│  • Lazy symbol binding (on fault)       │
│  • LC_DYLD_INFO processing              │
│  • OR: require static binaries only     │
│                                         │
│  Testing:                               │
│  • Dynamically-linked C CLI tools       │
│  • Simple dylib loading                 │
└─────────────────────────────────────────┘
```

### Phase 5: Real-World Testing (Week 9-10)

```
┌─────────────────────────────────────────┐
│  Target applications (demand-driven)    │
│                                         │
│  Priority targets:                      │
│  1. Static C CLI tools (ls, cat, etc.)  │
│  2. curl (networking validation)        │
│  3. Python/Ruby CLI scripts             │
│  4. Custom-compiled static binaries     │
│                                         │
│  Out of scope:                          │
│  ✗ AppKit / GUI applications           │
│  ✗ Metal / GPU                         │
│  ✗ Objective-C runtime (full)          │
│  ✗ IOKit driver framework              │
└─────────────────────────────────────────┘
```

---

## 12. Known Risks and Mitigations

| Risk | Impact | Probability | Mitigation |
|:---|:---:|:---:|:---|
| **dyld self-relocation** | High | Certain | Phase 1: static-only. Phase 4: minimal dyld replacement |
| **dyld shared cache** (no individual dylibs on macOS 12+) | High | High | Extract dylibs from cache, or target older macOS binaries |
| **Code signature enforcement** | Medium | Certain | Skip verification (development kernel) |
| **Objective-C runtime** required by most binaries | High | High | Start with pure C static binaries; ObjC stub for later |
| **Syscall number instability** across macOS versions | Medium | High | Per-version tables; `darwin_syscall_gen` for regeneration |
| **Mach port complexity** (port sets, notify, bootstrap) | Medium | Medium | Minimum viable subset; expand as needed |
| **Thread-local storage (TPIDR_EL0)** differences | Low | Medium | Map Darwin TLS layout to Scarlet's TPIDR handling |

---

## 13. Relationship to Existing ABI Modules

```
┌─────────────────────────────────────────────────────────────────┐
│                      AbiModule Trait                            │
│  handle_syscall() / can_execute_binary() / execute_binary()    │
│  on_task_cloned() / on_task_exit() / handle_event()            │
│  get_task_namespace() / setup_overlay_environment()             │
├──────────┬──────────────┬──────────────┬───────────────────────┤
│ Scarlet  │   Linux      │    xv6       │     Darwin (NEW)      │
│ Native   │  riscv64     │   riscv64    │     aarch64           │
├──────────┼──────────────┼──────────────┼───────────────────────┤
│ELF       │ ELF          │ ELF          │ Mach-O                │
│(OSABI=83)│(OSABI=0)     │(xv6 magic)  │(magic 0xFEEDFACF)     │
├──────────┼──────────────┼──────────────┼───────────────────────┤
│Scarlet   │ fd→handle    │ fd→handle    │ fd→handle             │
│syscalls  │ map in ABI   │ map in ABI   │ map in ABI            │
│          │              │              │ + carry-flag errno    │
├──────────┼──────────────┼──────────────┼───────────────────────┤
│Direct    │ sigaction    │ minimal      │ sigaction +           │
│events    │ via events   │ signals      │ Mach exception ports  │
│          │              │              │ (via events)           │
├──────────┼──────────────┼──────────────┼───────────────────────┤
│kernel    │ kernel       │ kernel       │ kernel                │
│syscall   │ syscall     │ syscall      │ syscall table          │
│table     │ table        │ table        │ (auto-generated)      │
└──────────┴──────────────┴──────────────┴───────────────────────┘
```

### Shared Patterns from Linux ABI

The Darwin ABI will reuse these patterns established by the Linux ABI:

1. **FD → Handle mapping**: Same `fd_to_handle: Vec<Option<u32>>` pattern
2. **Handle operations**: `allocate_fd()`, `get_handle()`, `remove_fd()`
3. **Event → Signal conversion**: `handle_event()` maps events to Darwin signals
4. **Task namespace**: Per-ABI namespace for PID management
5. **Clone/copy semantics**: `on_task_cloned()` for fork support
6. **Overlay VFS**: Same overlay mount pattern for filesystem isolation

### Key Differences from Linux ABI

| Aspect | Linux ABI | Darwin ABI |
|:---|:---|:---|
| Binary format | ELF | Mach-O |
| Syscall number register | x8 (aarch64) | x16 (aarch64) |
| Error reporting | Negative return | Carry flag + positive errno |
| SVC class | Single SVC | SVC #0x80 (BSD) + SVC #0x81 (Mach) |
| IPC | SysV/POSIX IPC | Mach ports (message passing) |
| TLS | `set_tls_pointer` via `prctl` | `TPIDR_EL0` + `thread_set_tsd_base` |
| Path separator | `/` | `/` (same) |
| Dynamic linker | ld-linux.so | dyld (more complex) |
| Signal delivery | `rt_sigaction` | Different signal numbers, Mach exceptions |

---

*Document version: 1.0 — 2026-04-21*
*Scarlet OS Darwin ABI Module Implementation Plan*
