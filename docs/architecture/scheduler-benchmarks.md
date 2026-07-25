# Scheduler Benchmark Scenarios

This document defines repeatable scheduler workloads for validating placement,
utilization tracking, EEVDF fairness, and responsiveness on both homogeneous
and heterogeneous systems.

The benchmark driver is `sched_bench` from `user/std-bin`.

## Common Commands

Build user binaries for AArch64:

```bash
cargo make build-userbin-std-debug-aarch64
```

Run a mixed workload:

```sh
sched_bench --scenario mixed --threads 8 --seconds 30
```

Request full-capacity placement for worker threads:

```sh
sched_bench --scenario cpu --threads 2 --seconds 30 --performance
```

Observe task placement in another terminal:

```sh
watch -n 1 top --sort cpu
```

Observe CPU topology, runnable counts, cpufreq state, and migration counters:

```sh
watch -n 1 cat /dev/cpuinfo
```

## Scenarios

`cpu`

: CPU-bound workers. Use this to check that sustained high utilization reaches
  high-capacity CPUs when they exist.

`bursty`

: Repeated medium compute bursts with short sleeps. Use this to check that
  short wakeups do not cause excessive migration churn.

`sleepy`

: Mostly sleeping workers with small compute bursts. Use this to check that
  low-utilization work can remain on lower-capacity or less-loaded CPUs.

`mixed`

: A repeating mix of CPU-bound, bursty, and sleepy workers. Use this as the
  default regression scenario because it exercises load spreading, wake
  placement, and idle work stealing together.

## QEMU Homogeneous Baseline

QEMU usually reports balanced cores with equal capacity. Expected behavior:

- `core class` is `balanced` for all CPUs.
- `cpu capacity` is equal for all online CPUs.
- `sched_bench --scenario mixed --threads 8 --seconds 30` spreads runnable work
  across idle CPUs.
- `scheduler work steals` may increase when queues become uneven.
- `scheduler promotions` and `scheduler demotions` should normally stay at 0
  because all capacities are equal.

Regression signal:

- Runnable workers pile up on one CPU while other CPUs stay idle.
- `scheduler work steals` increases continuously but run queues remain badly
  imbalanced.
- `top` shows CPU-bound workers stuck on one CPU for the whole run.

## EEVDF Fairness Baseline

Run more CPU-bound workers than available CPUs:

```sh
sched_bench --scenario cpu --threads 16 --seconds 60
```

Expected behavior:

- Every worker continues to accumulate CPU time; no worker remains runnable
  without making progress.
- Equal-nice workers on the same CPU converge toward equal CPU time over a long
  run, allowing for migration and sampling noise.
- Per-task virtual runtime remains close among continuously runnable workers,
  while virtual deadlines advance as requests are consumed.
- The local scheduler uses one-shot slice deadlines rather than a periodic tick.

The in-kernel fair-queue conformance tests additionally validate eligible
minimum-deadline selection, weighted average virtual runtime, new-task and
migration placement, request renewal, proportional nice weights, monotonic
minimum virtual runtime, and starvation freedom under repeated selection.

## Apple Silicon Heterogeneous Baseline

Apple Silicon exposes efficiency and performance classes when topology probing
is available. Expected behavior:

- E cores have lower `cpu capacity` than P cores.
- `sched_bench --scenario cpu --threads 2 --seconds 30 --performance` gives
  workers `MIN=1024`, `REQ=1024`, and should place them on P cores when P cores
  are online and available.
- `scheduler promotions` can increase when a high-util worker starts on an E
  core and moves to a P core.
- `sched_bench --scenario sleepy --threads 4 --seconds 30` should not force P
  cores to stay busy.

Regression signal:

- `--performance` workers remain on E cores while idle P cores exist.
- `REQ` in `top` is high but `cpu capacity` of the worker CPU is lower.
- Repeated promotions/demotions happen for the same workers without workload
  changes, indicating migration thrash.

## Responsiveness Check

Run the desktop, then start a mixed load:

```sh
sched_bench --scenario mixed --threads 8 --seconds 60
```

While it runs:

- Move the pointer and type in a terminal.
- Keep `top --sort cpu` visible.
- Check whether SWS, input services, and terminal tasks keep making progress.

Expected behavior:

- Interactive tasks should still appear runnable and should not be buried behind
  all CPU-bound workers.
- On Apple Silicon, sustained heavy workers may move to P cores, but small
  services should not require manual app-name special cases.

The desktop interaction check remains manual. Queue ordering, weighted slices,
and starvation freedom are covered by deterministic in-kernel tests.
