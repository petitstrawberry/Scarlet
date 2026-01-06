# Task ID Namespace Design

## Overview

The task ID namespace system allows different ABI modules (Linux, xv6, Scarlet, etc.) to maintain separate task ID numbering schemes while sharing the same kernel task management infrastructure. This enables proper support for ABI-specific process/thread models without conflicts.

## Architecture

### Namespace Hierarchy

```
Root Namespace (id: 0)
├── Linux Namespace (created by LinuxRiscv64Abi)
│   ├── Task 1 (global_id: 5, namespace_id: 1)
│   └── Task 2 (global_id: 6, namespace_id: 2)
├── xv6 Namespace (created by Xv6Riscv64Abi)
│   ├── Task 3 (global_id: 7, namespace_id: 1)
│   └── Task 4 (global_id: 8, namespace_id: 2)
└── Direct tasks (ScarletAbi uses root namespace)
    ├── Task 5 (global_id: 9, namespace_id: 3)
    └── Task 6 (global_id: 10, namespace_id: 4)
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

The `AbiModule` trait provides a default implementation:

```rust
fn get_task_namespace(&self) -> Arc<TaskNamespace> {
    get_root_namespace().clone()
}
```

### Custom Namespaces

ABIs can create their own namespaces in their `Default` implementation:

```rust
impl Default for LinuxRiscv64Abi {
    fn default() -> Self {
        let linux_namespace = TaskNamespace::new_child(
            get_root_namespace().clone(),
            "linux".to_string(),
        );
        Self {
            namespace: linux_namespace,
            // ... other fields ...
        }
    }
}
```

## Benefits

1. **ABI Isolation**: Each ABI can have its own PID space without conflicts
2. **Compatibility**: Different ABIs can coexist with their own process models
3. **Transparency**: User space applications see familiar PID/TID values
4. **Flexibility**: New ABIs can easily define their own namespace strategy
5. **Backward Compatibility**: Global IDs ensure kernel internals continue to work

## Design Decisions

### Why Two IDs?

- **Global ID**: Required for scheduler, task lookup, and kernel-internal operations
- **Namespace ID**: Required for ABI compatibility and user space expectations

### Why Not Map Between Namespaces?

The current design keeps namespaces independent. Cross-namespace task lookup is intentionally not provided because:
1. Different ABIs have different process models
2. Cross-ABI process management is undefined
3. Simplifies implementation and reduces complexity

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
