# Scheduler Placement and Fairness

This document describes the current Scarlet scheduler policy and the intended
path toward priority, fairness, and preemption-aware scheduling.

## Goals

- Keep runnable tasks distributed across online CPUs.
- Prefer a CPU whose capacity can satisfy the task's measured or requested
  utilization.
- Use user and kernel hints as policy inputs, not as hard affinity.
- Avoid app-name based scheduler special cases.
- Keep normal work from being permanently starved by latency-sensitive work.

## Current Model

Scarlet currently uses per-CPU ready queues. A task is associated with one
scheduler CPU while it is queued or running. Sleeping tasks keep their last CPU
as placement history.

The scheduler tracks:

- CPU topology: core class, relative capacity, topology domain, and online CPU
  mask.
- Per-task utilization: an exponentially decayed `util_avg` in
  `SCHED_UTIL_SCALE` units.
- Per-task hints: `util_min` and core preference.
- Per-CPU load: current task, ready queue weight, utilization clamp, and
  runnable task count.
- Migration statistics: promotions, demotions, cooldown skips, and work steals.

The current implementation is not a fully preemptive fair scheduler. Placement
decisions happen at enqueue, wakeup, idle work stealing, and normal task switch
boundaries. This is intentional for the current stage: correctness and
observability are more important than aggressive balancing.

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

## Promotion and Demotion

When a task finishes a run and remains ready, the scheduler may migrate it:

- Promotion: if required capacity is above the current CPU capacity, move to a
  higher-capacity CPU that can satisfy it.
- Demotion: if current capacity is well above required capacity, move to a
  lower-capacity CPU only after low utilization is sustained for a minimum
  window.
- Cooldown: recently migrated tasks are not moved again immediately.

This prevents rapid P/E bouncing and avoids demoting a task after one quiet
sample.

## Priority and Fairness

Priority currently influences load weight, not time entitlement. A higher
priority task makes its CPU look more loaded, which biases future placement away
from stacking more work on that CPU. This is deliberately weaker than a hard
priority scheduler.

Minimum viable fairness before full preemption:

- Do not use `util_min` as a permanent monopoly on P cores.
- Keep work stealing available so idle CPUs can drain overloaded queues.
- Keep migration cooldowns to prevent thrash.
- Keep `util_min` bounded by `SCHED_UTIL_SCALE`.
- Keep latency-sensitive hints visible through diagnostics so bad hints can be
  found.

Known limitation: without preemption, a CPU-bound task can still run until it
reaches an existing scheduling boundary. Fair CPU-time distribution requires
timer-driven preemption.

## Preemption Integration

When timer preemption is added, the scheduler should keep the existing
placement policy and add a fair runtime policy on top:

1. Charge runtime on every preemption tick.
2. Maintain per-task virtual runtime or a comparable fairness metric.
3. Select the next task from the local CPU by fairness first, then placement
   constraints.
4. Use priority to weight CPU-time entitlement rather than only load score.
5. Treat `util_min` and core preference as placement hints, not as extra CPU-time
   entitlement.
6. Make migration decisions at controlled points after accounting is updated.

This keeps the current capacity-aware work useful while leaving room for a real
fair scheduler.

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
effective required capacity, core preference, and migration count.

These interfaces are not stable ABI yet. They exist to validate the scheduler
while the algorithm is still changing.
