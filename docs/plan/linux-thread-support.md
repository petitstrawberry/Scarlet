# Linux Thread Support Plan (Linux ABI isolated)

## Goals
- Support Linux user binaries creating threads via `sys_clone` with Linux flags while keeping Scarlet core Linux-agnostic.
- Handle Linux-specific per-thread state (`parent_tid_ptr`, `child_tid_ptr`, `clear_child_tid`, robust list, TLS) strictly inside the Linux ABI.
- Provide scaffolding for futex (wait/wake) within the Linux ABI module; keep promotion to core as future work.

## Scope & Assumptions
- Target branch: `feature/#149-linux_riscv64_abi`.
- Scarlet core will not gain Linux-specific fields nor a generic `abi_task_state` bag. No additions to `Task` are planned for this work.
- We will reuse existing ABI trait hooks (`on_task_cloned`, `on_task_exit`) rather than extending the trait surface.
- Futex tables and robust-list interpretation live in the Linux ABI. If another ABI needs similar semantics, we may extract common primitives later.
- Use `scarlet-dev` container and `cargo make` for build/test.

## Architecture Overview
1. Per-task Linux state lives in `LinuxRiscv64Abi`
   - Define `LinuxThreadState` (parent/child/clear TID pointers, robust-list head/len, TLS pointer).
   - Store it as a field of `LinuxRiscv64Abi`; since the ABI instance is cloned per task, this provides per-task state without touching `Task`.
2. Use existing ABI hooks
   - `on_task_cloned(parent, child, flags)`: reset/initialize the child's `LinuxThreadState` as needed.
   - `on_task_exit(task)`: clear `clear_child_tid` address and perform robust-list based futex wakeups (TODO: implement wake path; safe to no-op initially).
3. Syscall wiring in Linux ABI
   - `sys_set_tid_address` and `sys_set_robust_list` update `LinuxThreadState` only (no `Task` writes).
   - `sys_clone` validates flags and populates `LinuxThreadState` (parent/child TID pointers, TLS) purely through the ABI state.
4. Futex (Linux ABI only for now)
   - Add `futex.rs` (or equivalent) with minimal wait/wake scaffolding and TODOs. Unimplemented ops return `-ENOSYS`.

## Implementation Plan
### A. Linux ABI state keeping
- Add `LinuxThreadState` struct under `kernel/src/abi/linux/riscv64` and a field `thread_state` to `LinuxRiscv64Abi`.
- Provide `thread_state()`/`thread_state_mut()` accessors.

### B. Syscall adjustments
- Update `proc.rs`:
  - Replace all `task.thread_metadata_mut()` usages with `abi.thread_state_mut()` updates.
  - Ensure `sys_set_tid_address`, `sys_set_robust_list`, and `sys_clone` only touch ABI state.

### C. Exit-time semantics
- Implement `on_task_exit` in Linux ABI:
  - If `clear_child_tid` is set, write 0 to that userspace address.
  - TODO: wake futex waiters for that address (both private and shared variants as needed).

### D. Futex scaffolding (ABI-local)
- Introduce `sys_futex` handler with minimal variants:
  - `FUTEX_WAIT{,_PRIVATE}` and `FUTEX_WAKE{,_PRIVATE}` planned; return `-ENOSYS` initially with TODO notes.
- Provide placeholders for wait-queues keyed by user addresses; integrate with Scarlet wakers later.

### E. Testing & Validation
- Build: `cargo make build-riscv64` in `scarlet-dev`.
- Run tests: `cargo make test-riscv64` (note: `no_std` tests use `#[test_case]`).
- Manual check: `sys_clone` path returns `-ENOSYS` only for unsupported thread creation flag combos; otherwise proceeds to process cloning as today.

## Acceptance Criteria
- No Linux-specific state resides in `Task`; all Linux thread state is owned by `LinuxRiscv64Abi`.
- `proc.rs` has no references to `thread_metadata_*`; syscalls update `LinuxThreadState` only.
- `on_task_exit` clears `clear_child_tid` (futex wake marked TODO).
- Futex syscall exists as a stub with clear TODOs and does not crash.
- All builds/tests pass via `cargo make`.

## Follow-ups
- Implement futex wait-queues and wake paths on top of Scarlet scheduler wakers.
- Consider signals for threads (`tgkill`/`tkill`) once basic pthreads workflows advance.
- Evaluate promotion of wait/wake primitives to core if a second ABI requires them.
