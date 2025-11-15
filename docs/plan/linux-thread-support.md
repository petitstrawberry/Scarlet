# Linux Thread Support Plan

## Goals
- Enable Linux user binaries to create POSIX threads via `sys_clone` with `CLONE_THREAD` and related flags.
- Provide Linux ABI compliant handling of `parent_tid_ptr`, `child_tid_ptr`, TLS setup, and `set_tid_address` semantics without leaking Linux-specific concepts into Scarlet core.
- Establish groundwork for futex-based synchronization within the Linux ABI layer, including TODOs for future core-level abstraction.

## Scope & Assumptions
- All changes target the existing `feature/#149-linux_riscv64_abi` branch.
- Kernel core remains Linux-agnostic; new hooks/fields must use generic terminology (`thread_handle`, `abi_exit_hook`, etc.).
- Futex wake/wait tables will initially live inside the Linux ABI module. A future task may extract them into scarlet core if additional ABIs need them.
- `scarlet-dev` container and `cargo make` remain the canonical build/test environment.

## Architecture Overview
1. **Task Extensions** (`kernel/src/task/mod.rs`):
   - Add optional `thread_handle` metadata shared among tasks belonging to one process address space.
   - Introduce `abi_exit_hook` references to let ABI layers run cleanup when a task exits (e.g., futex wakeups, parent/child TID clears).
   - Ensure cloning preserves relevant handles and registers ABI-specific exit metadata for the child.
2. **ABI Trait Update** (`kernel/src/abi/mod.rs`):
   - Extend the trait with `on_task_exit(&self, task: &Task)` and `prepare_thread_clone(&self, parent: &Task, child: &mut Task, clone_args: &CloneArgs)` (names tentative but generic).
   - Wire trait invocations into the scheduler/task lifecycle (e.g., after `Task::exit` tears down core state).
3. **Linux ABI Implementation** (`kernel/src/abi/linux/riscv64/proc.rs`):
   - Implement the trait hooks to handle `parent_tid_ptr`, `child_tid_ptr`, TLS register initialization, and deferred futex wakeups.
   - Expand `sys_clone` to support all combinations used by musl pthreads, including stack/tls args validation and clone flag masking.
   - Track TID addresses per task for `set_tid_address` / exit-time clearing.
4. **Futex Table (Linux ABI only for now)** (`kernel/src/abi/linux/.../futex.rs` new file or util):
   - Maintain wait queues keyed by user-space addresses.
   - Provide helper APIs `wait_futex`, `wake_futex`, `requeue_futex` used by syscall implementations.
   - Document TODO for promoting these helpers into Scarlet core if another ABI needs them.
5. **Syscall Surface**:
   - Flesh out `sys_set_tid_address`, `sys_futex`, and ensure `sys_clone` populates necessary ABI data.
   - Add stubs/tests for unimplemented futex operations with clear error codes (`ENOSYS`) plus TODO comments referencing this plan.
6. **Testing & Validation**:
   - Use musl-based pthread smoke tests (e.g., sample binary) to ensure `sys_clone` path progresses beyond "Thread creation not yet supported".
   - Run `cargo make test` after each major milestone.

## Detailed Task Breakdown
### 1. Task/ABI Groundwork
- Update `Task` struct:
  - `thread_handle: Option<Arc<ThreadGroup>>` to share VM-level data between pthread siblings.
  - `abi_exit: Option<AbiExitContext>` storing pointers for tid cleanup and futex interactions.
- Update constructors and `clone_task` to accept clone metadata from ABI.
- Modify `Task::exit` to call into ABI hook after scheduler bookkeeping but before final Task drop.

### 2. ABI Trait & Hooks
- Modify `Abi` trait signature with:
  - `fn prepare_clone(&self, parent: &Task, child: &mut Task, clone_args: &CloneArgs) -> Result<(), AbiErr>`.
  - `fn on_task_exit(&self, task: &Task);`
- Update all ABI implementors (Linux, xv6, etc.) to satisfy new trait methods (no-op where unsupported).
- Propagate hook invocations inside `Task::clone_task` and `Task::exit`.

### 3. Linux `sys_clone` Implementation
- Parse musl flags (`CLONE_VM`, `CLONE_FS`, `CLONE_FILES`, `CLONE_SIGHAND`, `CLONE_THREAD`, `CLONE_SETTLS`, `CLONE_PARENT_SETTID`, `CLONE_CHILD_CLEARTID`, `CLONE_CHILD_SETTID`).
- Validate unsupported combinations early (e.g., `CLONE_NEW*` returning `EINVAL`).
- Allocate child stack pointer if provided; verify alignment.
- Initialize child TLS register (`tp` on RISC-V) when `CLONE_SETTLS` set.
- Record `parent_tid_ptr` / `child_tid_ptr` in the child's `AbiExitContext` for exit-time updates.
- Create sibling thread within shared task group when `CLONE_THREAD` is set; otherwise fall back to process fork semantics.

### 4. `set_tid_address` & Exit Hooks
- Track per-task clear-tid pointer (defaults to zero) updated via `sys_set_tid_address`.
- On task exit, Linux ABI hook clears memory at `clear_child_tid` and wakes futex waiters, mirroring Linux semantics.

### 5. Futex Infrastructure (Linux ABI Local)
- Implement wait queues keyed by `UserAddress` + optional private/shared scope.
- Provide blocking wait integrated with Scarlet scheduler wakers.
- Implement at least `FUTEX_WAIT`, `FUTEX_WAKE`, `FUTEX_WAIT_PRIVATE`, `FUTEX_WAKE_PRIVATE`.
- Add TODO note indicating intention to move to shared core infrastructure when another ABI needs futex functionality.

### 6. Documentation & TODOs
- Reference this document from future issues/PR descriptions.
- Add inline TODOs where futex abstraction is deferred; include links or note "See docs/plan/linux-thread-support.md".

## Acceptance Criteria
- `sys_clone` no longer returns "Thread creation not yet supported" for musl pthreads using supported flags.
- Child threads correctly inherit VM/context and run user TLS setups.
- `set_tid_address` and `CLONE_CHILD_CLEARTID` semantics clear memory and wake futex waiters on exit.
- Futex wait/wake helpers exist inside Linux ABI with clear TODO for promotion.
- `cargo make test` passes (or failure reasons documented if tests unavailable).

## Open Questions / Follow-Ups
- Do we need Linux-compatible `tgkill`/`tkill` syscalls immediately? (Not required for initial pthread bring-up but may unblock signal handling.)
- Should `ThreadGroup` encapsulate signal handling / thread ID allocation now or later? For MVP we can map Scarlet task IDs to Linux TIDs with a per-thread counter maintained by the ABI.
- Additional clone flag support (e.g., `CLONE_SETTLS` for TLS descriptor objects) will be validated against musl's expectations; future ABIs may extend the trait accordingly.
