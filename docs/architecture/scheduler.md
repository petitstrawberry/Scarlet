# Scheduler Placement and Fairness

This document describes Scarlet's capacity-aware placement policy and its
tickless EEVDF fair runtime scheduler.

## Goals

- Keep runnable tasks distributed across online CPUs.
- Prefer a CPU whose capacity can satisfy the task's measured or requested
  utilization.
- Use user and kernel hints as policy inputs, not as hard affinity.
- Avoid app-name based scheduler special cases.
- Keep normal work from being permanently starved by latency-sensitive work.

## Current Model

Scarlet uses one EEVDF fair run queue per CPU. A task is associated with one
scheduler CPU while it is queued or running. Sleeping tasks keep their last CPU
as placement history and are placed at the destination queue's current virtual
time when they wake.

The scheduler tracks:

- CPU topology: core class, relative capacity, topology domain, and online CPU
  mask.
- Per-task utilization: an exponentially decayed `util_avg` in
  `SCHED_UTIL_SCALE` units.
- Per-task hints: `util_min` and core preference.
- Per-task fair state: nice-derived load weight, virtual runtime, virtual
  deadline, and current slice.
- Per-CPU load: current task, ready queue weight, utilization clamp, and
  runnable task count.
- Per-CPU fair state: weighted average virtual runtime, total runnable weight,
  runnable count, and a monotonic minimum-virtual-runtime floor.
- Migration statistics: promotions, demotions, cooldown skips, and work steals.

Placement decisions happen at enqueue, wakeup, idle work stealing, and normal
task switch boundaries. Runtime preemption uses a per-task one-shot timer; no
periodic scheduler tick is required.

## Placement Capacity

For placement, each task has an effective required capacity:

```text
effective_util = max(measured util_avg, requested util_min)
required_capacity = max(preference_floor, effective_util)
```

The preference floor is:

- `Performance`: full default capacity.
- `Efficiency` or `Any`: minimum scheduler capacity.

`util_min` is validated in the kernel and must be in `0..=SCHED_UTIL_SCALE`.
User space can set it for the current task through `std::task::set_sched_util_min`.
New threads can temporarily override the parent hint through
`std::thread::Builder::util_min` or `Builder::performance`; the child inherits
the requested value through clone, then the parent hint is restored.

Hints are best effort. They raise the capacity floor used by placement, but they
do not pin the task and do not guarantee exclusive CPU time.

## CPU Selection

When a task is enqueued or woken:

1. Pinned tasks stay on their pinned CPU.
2. The scheduler scans online CPUs.
3. CPUs below the task's required capacity are skipped when possible.
4. Among suitable CPUs, the scheduler picks the lowest load score.
5. Ties use the task's core preference.
6. If no suitable CPU exists, the least-loaded fallback CPU is used.

The load score is based on runnable task weight divided by CPU capacity. This
keeps homogeneous systems balanced while still allowing higher-capacity cores to
absorb more work on heterogeneous systems.

Idle CPUs may steal ready work from busier queues. Work stealing observes the
same capacity requirement and migration cooldown rules as direct migration.
Within a topology domain, the scheduler may also move ready work laterally from
an overloaded CPU to a less loaded peer CPU with the same effective capacity.
This keeps P-core and E-core groups from piling runnable work onto one core
while sibling cores are idle or lightly loaded.

## Promotion and Demotion

When a task finishes a run and remains ready, the scheduler may migrate it:

- Promotion: if required capacity is above the current CPU capacity, move to a
  higher-capacity CPU that can satisfy it.
- Demotion: if current capacity is well above required capacity, move to a
  lower-capacity CPU only after low utilization is sustained for a minimum
  window.
- Lateral balance: if peer CPUs in the same topology domain have meaningfully
  lower load, move ready work sideways without changing capacity class.
- Cooldown: recently migrated tasks are not moved again immediately.

This prevents rapid P/E bouncing, avoids demoting a task after one quiet sample,
and keeps load balancing local to the current performance domain unless a
capacity change is actually needed.

## Priority and Fairness

Legacy task priority influences placement load. Fair CPU-time entitlement is
controlled separately by the task's nice value. Nice values from -20 through
+19 map to the Linux scheduler weight table, with nice 0 using
`NICE_0_LOAD = 1024`. A task with a larger weight receives a proportionally
larger wall-time slice while its virtual runtime advances more slowly:

```text
vruntime += delta_exec * NICE_0_LOAD / weight
slice = max(0.75 ms, period * weight / total_weight)
virtual_deadline = vruntime + slice * NICE_0_LOAD / weight
```

The target period is 5 ms while the runnable set fits within that latency. It
grows by the 0.75 ms minimum granularity when more tasks are runnable. `util_min`
and core preference affect CPU placement only; they do not grant extra runtime.

Among eligible entities (`vruntime <= avg_vruntime`), the scheduler selects the
smallest virtual deadline. Deadline ties use virtual runtime and then task ID,
making selection deterministic. If no entity is eligible, selection advances to
the entity at the smallest virtual runtime so every runnable task continues to
make progress.

## Preemption Integration

The scheduler charges EEVDF runtime on every switch-out and one-shot slice
boundary. A partially consumed request retains its virtual deadline; reaching
the deadline renews the request from the updated virtual runtime. The selected
task's fair slice arms the local exact timer, falling back to the legacy task
slice only for tasks without initialized fair state.

Runnable tasks remain in a per-CPU `BTreeMap` ordered by virtual deadline,
virtual runtime, and task ID. Work stealing removes an entity from the donor
queue, normalizes its virtual runtime to the destination's monotonic floor, and
then claims it on the idle CPU. Normal post-switch migration uses the same
normalization before insertion.

## Queue Invariants

- A task is running on at most one CPU.
- A ready task is owned by at most one CPU queue.
- Idle tasks are never enqueued as normal ready work.
- Cross-CPU movement must claim or release `running_cpu` before the task can be
  selected elsewhere.
- Scheduler-driven migration updates the task migration timestamp and count.
- Diagnostic fields must be derived from scheduler state and must not depend on
  one-off logging.

## Diagnostics

`/dev/cpuinfo` is a temporary diagnostic device. It exposes per-CPU topology,
capacity, runnable count, utilization clamp, scheduler migration counters, and
cpufreq state when available.

`GetTaskInfoList` exposes per-task placement state. User tools such as `top`
can show the current CPU, measured utilization, requested minimum utilization,
effective required capacity, core preference, migration count, nice value, fair
weight, virtual runtime, and virtual deadline.

These interfaces are not stable ABI yet. They exist to validate the scheduler
while the algorithm is still changing.
