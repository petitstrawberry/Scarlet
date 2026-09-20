# Userspace CPU capabilities

`kernel/src/arch/cpu_features.rs` owns the architecture-independent contract
between CPU discovery and userspace. Each architecture translates one CPU's
registers into the capability words it can safely expose, then reports those
words to `CpuFeatureRegistry`. The registry stores per-CPU reports, intersects
them once before the first ELF is loaded, and never reduces the published
result. A CPU that reports late may enter the scheduler only if it supports
every capability already promised to userspace.

The architecture backend owns register decoding and the meaning of its ELF
capability bits. The boot protocol owns starting and waiting for secondary CPU
probes. The ELF loader reads only the frozen result; both the initial process
and later execs receive the same contract. A new architecture can use this
registry without adopting AArch64 ID register rules.

On AArch64, `kernel/src/arch/aarch64/cpu_features.rs` maps supported ID
register fields to Linux-compatible `AT_HWCAP` and `AT_HWCAP2`. It advertises
FP/SIMD only when Scarlet preserves userspace FP/SIMD context, and withholds
extensions whose userspace context handling is not implemented. Linux/PSCI and
Limine boot paths probe their APs before the first ELF; APs still pass the
existing scheduler release gate later. A probe timeout never weakens the
published capabilities: a delayed AP must pass the same superset check.

For outline atomics, the Rust target has an ARMv8-A baseline and emits helpers
that choose LSE or LL/SC at runtime. Scarlet std reads `AT_HWCAP` before
running `.init_array`; the normal LSE constructor then enables LSE helpers on
capable systems. The same target std runs on Cortex-A57 and LSE-capable CPUs.
