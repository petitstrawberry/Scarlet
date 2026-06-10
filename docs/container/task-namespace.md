# Task ID Namespace Design

## Overview

The task ID namespace system provides the infrastructure to optionally separate task ID numbering schemes for different contexts (e.g., containers). By default, all tasks share the root namespace, enabling cross-ABI task visibility and collaboration — a key characteristic of Scarlet.

## Core Concept

Each task has two IDs:

- **Global ID**: Unique across the entire kernel. Used internally by the scheduler, task lookup, and all kernel-internal operations. Never exposed to user space.
- **Namespace-Local ID**: Unique within a namespace. Exposed to user space as PID/TID via syscalls (`getpid`, `fork` return values, etc.).

## Architecture

### Default: Shared Root Namespace

All ABI modules use the root namespace by default. This means tasks from Linux, xv6, and Scarlet share a single PID space and can see each other.

```text
Root Namespace (id: 0)
├── Linux Task (global: 1, local: 1)
├── xv6 Task   (global: 2, local: 2)
└── Scarlet Task (global: 3, local: 3)
```

### Explicit Isolation (Containers)

Separate namespaces are created explicitly via `CreateNamespace` syscall:

```text
Root Namespace (id: 0)
├── Container A (id: 1) — explicitly created
│   ├── Task (global: 5, local: 1)
│   └── Task (global: 6, local: 2)
└── Container B (id: 2) — explicitly created
    └── Task (global: 7, local: 1)
```

## TaskNamespace Structure

```rust
pub struct TaskNamespace {
    id: usize,                                    // Unique namespace ID
    next_task_id: Mutex<usize>,                   // Counter for local ID allocation
    local_to_global: Mutex<BTreeMap<usize, usize>>, // Local → Global ID mapping
    global_to_local: Mutex<BTreeMap<usize, usize>>, // Global → Local ID mapping
    parent: Option<Arc<TaskNamespace>>,           // Parent namespace (None for root)
    name: String,                                 // Debug name
}
```

The bidirectional `local_to_global` / `global_to_local` maps enable syscall boundary translation (PID namespace semantics) while keeping all kernel internals globally-addressed.

### Key Methods

| Method | Description |
|--------|-------------|
| `new_root(name)` | Create the root namespace (id: 0) |
| `new_child(parent, name)` | Create a child namespace with auto-incremented ID |
| `allocate_task_id()` | Allocate a bare local ID |
| `allocate_for(global_id)` | Allocate a local ID and register bidirectional mapping |
| `register_task(local_id, global_id)` | Register an existing mapping |
| `unregister_task(global_id)` | Remove mapping for a global ID |
| `local_to_global(local_id)` | Translate local → global |
| `global_to_local(global_id)` | Translate global → local |

## Syscall: CreateNamespace (92)

```c
int create_namespace(unsigned long flags, const char *name);
```

| Flag | Value | Effect |
|------|-------|--------|
| `NS_CREATE_TASK` | 0x01 | Create separate task namespace |
| `NS_CREATE_VFS` | 0x02 | Create separate VFS namespace |
| `NS_CREATE_NET` | 0x04 | Network namespace (stub) |
| `NS_CREATE_IPC` | 0x08 | IPC namespace (stub) |

Flags can be OR'd together. The `name` parameter is optional (NULL for auto-generated).

## ABI Integration

All ABIs share the root namespace by default:

```rust
// LinuxRiscv64Abi default
let namespace = get_root_namespace().clone();
```

Syscalls that return task IDs use `task.get_namespace_id()` to return the namespace-local ID:

```rust
pub fn sys_fork(...) -> usize {
    let child_task = parent_task.clone_task(CloneFlags::default())?;
    child_task.get_namespace_id() // Returns local PID visible to user space
}
```

## Namespace Hierarchy

Child namespaces inherit from their parent. The bidirectional mapping allows:

- Parent namespace can translate child-local PIDs to global IDs
- Child namespace only sees its own local IDs
- Kernel internals always operate on global IDs

## Testing

Tests are in `kernel/src/task/namespace.rs`:

- Namespace creation and hierarchy
- Task ID allocation with bidirectional mapping
- Local-to-global and global-to-local translation
- Namespace inheritance during task cloning

## References

- [Container System](namespace-isolation.md)
- Linux PID namespaces: <https://man7.org/linux/man-pages/man7/pid_namespaces.7.html>
