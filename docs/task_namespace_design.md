# Task ID Namespace Design

## Overview

The task ID namespace system provides the infrastructure to optionally separate task ID numbering schemes for different contexts (e.g., containers, cgroups). By default, all tasks share the root namespace, enabling cross-ABI task visibility and collaboration - a key characteristic of Scarlet.

## Default Behavior: Shared Namespace

**By default, all ABI modules (Linux, xv6, Scarlet) use the root namespace.** This means:
- Tasks from different ABIs can see and interact with each other
- PIDs/TIDs are shared across the entire system
- Task collaboration between different OS environments is seamless

This design reflects Scarlet's philosophy: different task types should appear as "companions" rather than being isolated.

## Architecture

### Namespace Hierarchy (Default Configuration)

```
Root Namespace (id: 0) - Used by all ABIs by default
├── Linux Task 1 (global_id: 1, namespace_id: 1)
├── xv6 Task 2 (global_id: 2, namespace_id: 2)
├── Scarlet Task 3 (global_id: 3, namespace_id: 3)
├── Linux Task 4 (global_id: 4, namespace_id: 4)
└── xv6 Task 5 (global_id: 5, namespace_id: 5)
```

### When to Use Separate Namespaces

Separate namespaces should only be created explicitly when needed:
- Container isolation
- cgroups or similar resource management
- Security boundaries requiring PID isolation
- Testing different task hierarchies

### Namespace Hierarchy (With Explicit Separation)

```
Root Namespace (id: 0)
├── Container 1 Namespace (explicitly created)
│   ├── Task 1 (global_id: 5, namespace_id: 1)
│   └── Task 2 (global_id: 6, namespace_id: 2)
└── Container 2 Namespace (explicitly created)
    ├── Task 3 (global_id: 7, namespace_id: 1)
    └── Task 4 (global_id: 8, namespace_id: 2)
```

### Key Concepts

1. **Global Task ID**: Unique across the entire kernel, used internally for task management and scheduling. Never exposed to user space.

2. **Namespace-Local ID**: Unique within a namespace, exposed to user space as PID/TID. This is what applications see.

3. **Namespace**: A container for task IDs that maintains its own counter for allocating namespace-local IDs.

4. **Namespace Inheritance**: Child tasks inherit their parent's namespace, ensuring consistent ID visibility within a process hierarchy.

## Implementation Details

### Task Structure

Each `Task` contains:
- `id`: Global unique identifier (used by scheduler, internal kernel code)
- `namespace_id`: Local ID within the task's namespace (exposed to user space)
- `namespace`: Arc reference to the TaskNamespace

### TaskNamespace Structure

```rust
pub struct TaskNamespace {
    id: usize,                    // Unique namespace ID
    next_task_id: Mutex<usize>,   // Next namespace-local ID to allocate
    parent: Option<Arc<TaskNamespace>>, // Parent namespace
    name: String,                 // Name for debugging
}
```

### Task Creation Flow

1. ABI module creates or uses a namespace via `get_task_namespace()`
2. `Task::new_with_namespace()` is called with the namespace
3. Global ID is allocated from global counter
4. Namespace-local ID is allocated from namespace counter
5. Task stores both IDs and a reference to the namespace

### Syscall Handling

Syscalls that return task IDs (fork, getpid, set_tid_address, etc.) use `task.get_namespace_id()` to return the namespace-local ID visible to user space.

Example from xv6 fork:
```rust
pub fn sys_fork(...) -> usize {
    let parent_task = mytask().unwrap();
    match parent_task.clone_task(CloneFlags::default()) {
        Ok(child_task) => {
            // Return namespace-local ID to user space
            let child_namespace_id = child_task.get_namespace_id();
            // ... schedule child ...
            child_namespace_id
        }
        Err(_) => usize::MAX
    }
}
```

## ABI Integration

### Default Implementation

All ABIs use the root namespace by default, enabling cross-ABI task visibility:

```rust
// All ABIs share this default behavior
fn get_task_namespace(&self) -> Arc<TaskNamespace> {
    get_root_namespace().clone()
}
```

### Example: Default Shared Namespace

```rust
impl Default for LinuxRiscv64Abi {
    fn default() -> Self {
        // Use root namespace for cross-ABI task visibility
        let namespace = get_root_namespace().clone();
        Self {
            namespace,
            // ... other fields ...
        }
    }
}
```

### Creating Separate Namespaces (When Needed)

For containers, cgroups, or explicit isolation:

```rust
// Create a separate namespace explicitly
let container_namespace = TaskNamespace::new_child(
    get_root_namespace().clone(),
    "container_1".to_string(),
);

// Create task in isolated namespace
let isolated_task = Task::new_with_namespace(
    "isolated_task".to_string(),
    0,
    TaskType::User,
    container_namespace,
);
```

## Benefits

1. **Cross-ABI Collaboration**: Tasks from different ABIs can see and interact with each other by default - Scarlet's key characteristic
2. **Unified PID Space**: All tasks share the same PID space, making inter-process communication natural
3. **Optional Isolation**: Namespaces can be created explicitly when isolation is needed (containers, cgroups)
4. **Flexibility**: Infrastructure supports both shared and isolated scenarios
5. **Backward Compatibility**: Global IDs ensure kernel internals continue to work

## Design Decisions

### Why Shared Namespace by Default?

Scarlet's design philosophy is **collaboration between different OS environments**. By sharing the root namespace:
1. Linux, xv6, and Scarlet tasks appear as "companions"
2. Task visibility across ABIs is seamless
3. Inter-ABI IPC and coordination is straightforward
4. The system behaves more like a unified OS rather than isolated containers

Separation is only applied when explicitly needed (e.g., containers, security boundaries).

### Why Two IDs?

- **Global ID**: Required for scheduler, task lookup, and kernel-internal operations
- **Namespace ID**: Required for ABI compatibility and user space expectations

### Why Not Always Separate Namespaces?

With shared namespaces by default:
1. Cross-ABI task visibility enables collaboration
2. Task management is simpler (no namespace translation needed)
3. IPC between different ABI tasks is straightforward
4. Reflects Scarlet's design goal of unified multi-OS execution

When isolation is needed (containers, security), explicit namespace creation provides that capability.

### Why Arc for Namespace?

Using `Arc<TaskNamespace>` allows:
1. Safe sharing between tasks
2. Easy cloning during task creation
3. Automatic cleanup when no tasks reference the namespace
4. Thread-safe reference counting

## Future Enhancements

### Potential Improvements

1. **Namespace-scoped task lookup**: Add methods to find tasks within a namespace by their namespace-local ID
2. **Namespace visibility**: Implement parent namespace visibility into child namespaces (similar to Linux PID namespaces)
3. **Namespace limits**: Add per-namespace task limits and resource tracking
4. **Dynamic namespace creation**: Allow user space to create namespaces via syscalls

### Considerations

When implementing enhancements, consider:
- Performance impact on scheduler hot paths
- Memory overhead of namespace structures
- Complexity of cross-namespace operations
- Security implications of namespace visibility

## Testing

Tests are provided in `kernel/src/task/mod.rs` and `kernel/src/task/namespace.rs`:

1. Namespace creation and hierarchy
2. Task ID allocation within namespaces
3. Namespace inheritance during task cloning
4. Separation between global and namespace-local IDs
5. Multiple tasks in the same namespace

## References

- Linux PID namespaces: https://man7.org/linux/man-pages/man7/pid_namespaces.7.html
- xv6 process model: https://pdos.csail.mit.edu/6.828/2020/xv6/book-riscv-rev1.pdf
