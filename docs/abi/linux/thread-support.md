# Linux Thread Support

## Overview

The Scarlet Linux ABI supports multi-threaded Linux binaries through `sys_clone`, per-thread state management, and futex-based synchronization — all implemented within the Linux ABI module without polluting the core kernel.

## Architecture

Thread support is fully contained within the Linux ABI layer. The kernel's core `Task` struct has no Linux-specific fields. Instead, `LinuxAbi` is cloned per task and carries its own `LinuxThreadState`.

```text
sys_clone (Linux ABI syscall)
    │
    ▼
LinuxRiscv64Abi::on_task_cloned()
    │
    ├── Clone LinuxAbi (including LinuxThreadState)
    ├── Handle CLONE_FILES (share or unshare fd table)
    ├── Set TGID (thread group ID) for threads
    └── Initialize child thread state
```

## LinuxThreadState

Per-thread Linux-specific state stored inside `LinuxAbi`:

| Field | Purpose |
|-------|---------|
| `parent_tid_ptr` | `CLONE_PARENT_SETTID` — write child TID to parent |
| `child_tid_ptr` | `CLONE_CHILD_SETTID` — write child TID to child |
| `clear_child_tid_ptr` | `set_tid_address` — clear on exit, futex wake |
| `robust_list_head` | `set_robust_list` — robust mutex list head |
| `robust_list_len` | Robust list length |
| `tls_pointer` | Thread-local storage pointer |
| `tgid` | Thread group ID (PID for threads, 0 for processes) |
| `pending_clone_is_thread` | Whether next clone creates a thread |
| `sigaltstack_sp/size/flags` | Alternate signal stack |

## Syscalls

| Syscall | Location | Description |
|---------|----------|-------------|
| `clone` (220) | `generic/proc.rs` (dispatch), `riscv64/mod.rs` (`on_task_cloned`) | Thread/process creation |
| `set_tid_address` (96) | `generic/proc.rs` | Register clear-on-exit address |
| `set_robust_list` (99) | `generic/proc.rs` | Register robust futex list |

## Futex

Minimal futex implementation in `kernel/src/abi/linux/generic/futex.rs`:

| Operation | Support |
|-----------|---------|
| `FUTEX_WAIT` | Implemented — block on address |
| `FUTEX_WAKE` | Implemented — wake waiters |
| `FUTEX_WAIT_BITSET` | Implemented |
| `FUTEX_WAKE_BITSET` | Implemented |

Wait queues are global, keyed by user address. Wake integrates with the Scarlet scheduler.

## Thread Lifecycle

1. **Creation**: `sys_clone` → `on_task_cloned()` initializes child's `LinuxThreadState`.
2. **TLS**: Thread-local storage pointer set during clone.
3. **Running**: Thread operates with its own `LinuxAbi` instance (fd table shared or cloned).
4. **Exit**: `on_task_exit()` clears `clear_child_tid_ptr` and wakes futex waiters.

## Design Constraints

- **Linux-agnostic core**: No Linux-specific fields in kernel `Task`.
- **ABI isolation**: All thread state lives in `LinuxAbi`, cloned per task.
- **Hook-based**: Uses `AbiModule` hooks (`on_task_cloned`, `on_task_exit`) instead of extending the trait surface.
