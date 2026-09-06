# Userspace development map

Reviewed against the current manifests and startup code on 2026-09-06.
Scarlet has both normal Rust `std` applications and a retained legacy
`no_std` userland. The kernel's `no_std` requirement does not apply to every
userspace program.

## Runtime and library boundaries

| Component | Role |
| --- | --- |
| Scarlet Rust toolchain | Supplies Rust `std` for `riscv64gc-unknown-scarlet` and `aarch64-unknown-scarlet` |
| [scarlet-abi](../../user/lib/scarlet-abi/Cargo.toml) | Shared ABI numbers and records |
| [scarlet-sys](../../user/lib/scarlet-sys/Cargo.toml) | Raw syscall/native entry layer; unsafe inputs remain the caller's responsibility |
| [scarlet-os](../../user/lib/scarlet-os/Cargo.toml) | Typed native handles and OS operations for std and supported no_std consumers |
| [scarlet-rt](../../user/lib/scarlet-rt/Cargo.toml) | Scarlet runtime integration, including the explicit legacy-no-std configuration |
| [scarlet-std](../../user/lib/std/Cargo.toml) | Legacy no_std facade, often renamed to `std` by old programs; not the toolchain's Rust std |
| scarlet-sdk | Separate repository supplying `cargo-scarlet` and image plugins; not a native user library |
| ScarletUI / SGFX | Separate Git repositories for UI and graphics execution, consumed by the in-tree apps |

The [native API contract](../release/1.0-native-api-contract.md) records handle
ownership, fallible operations, raw unsafe boundaries, mappings, and GPU/SWS
lifetimes. Use typed wrappers where possible; syscall completion, GPU
completion, and window presentation are distinct events.

The legacy `scarlet_std::thread_local!` macro and `LocalKey` type have been
removed: their initialization, storage layout, and borrowing were unsound.
Normal Rust `std` applications use the toolchain's separate `std::thread_local!`
implementation. Legacy `no_std` programs should pass per-thread state into
their `thread::spawn` closures; there is no replacement typed TLS API in the
legacy facade. Runtime TLS allocation and thread-exit cleanup remain unchanged.

## In-tree builds

The root is not a single Cargo workspace. [.cargo/Cargo.toml](../../.cargo/Cargo.toml)
groups `user/bin`, `user/std-bin`, `user/video_player`, `user/tonic-demo`, and
`user/websocket-demo`. Their package manifests select this workspace explicitly.
The kernel and project BSPs are built separately.

Use the Nix development environment and the Scarlet Rust compiler. From the
repository root, for example:

```sh
cargo check --manifest-path .cargo/Cargo.toml -p scarlet-std-bin --bins \
  --target aarch64-unknown-scarlet
cargo check --manifest-path .cargo/Cargo.toml -p scarlet-std-bin --bins \
  --target riscv64gc-unknown-scarlet
```

These compile the std-based in-tree applications; they neither install the
binaries into an image nor launch the desktop. For the latter, use the chosen
project's `cargo scarlet image` / `run` workflow. During release staging,
validate the selected committed locks separately; do not substitute local
sibling-path overrides for a distributable dependency set.

[user/bin](../../user/bin/Cargo.toml) contains retained `no_std` programs such
as init. Build its package separately from std consumers to avoid unifying
incompatible runtime/panic-handler features in one Cargo invocation:

```sh
cargo check --manifest-path .cargo/Cargo.toml -p userprogram --bins \
  --target aarch64-unknown-scarlet
```

The JSON targets in `user/targets/` and `legacy-scarlet-std` graphics features
are compatibility paths. They are not the names of the normal std-capable
Scarlet targets. See [multi-architecture support](../architecture/multi-architecture.md).

## Adding a program to an image

Add or select a Cargo binary, then include it in the chosen project or bundle.
For example, in a bundle located directly under `bundles/<name>/`:

```toml
[[layers]]
kind = "cargo"
source = "../../user/std-bin"
package = "scarlet-std-bin"
bin = "hello"
to = "/system/scarlet/bin/hello"
```

The `source` path is relative to the declaring bundle, not the shell's current
directory. In `scarlet.toml`, use `[[images.<name>.layers]]` instead. Include
the bundle in the project; merely adding a `[[bin]]` does not install it.
External Cargo sources can select a Git repository and `subdir` without
requiring a manually created sibling checkout.

Features matter for optional applications such as video/websocket workloads;
retain the selected bundle's feature recipe. Do not remove an application
from the image to hide a dependency integration failure. See the
[build system](../build-system/README.md) for layer order and Cargo-layer replacement.

## Startup and services

The kernel loads `/system/scarlet/bin/init` from the initramfs and passes the
kernel command line. The default [init](../../user/bin/src/init.rs) honors
`root=` / `rootfstype=`, prepares the filesystem layout, and execs
`/system/scarlet/bin/stemd`. The microvm manifest instead installs its own
`microvm-init` at the same initial-program path.

[stemd](../services/stemd.md) owns service ordering, readiness, child reaping,
and the desktop application registry. The source configuration lives in
[base services](../../bundles/base/fs/system/scarlet/etc/stemd.d/services) and
[desktop services/apps](../../bundles/desktop/fs/system/scarlet/etc/stemd.d),
not the removed `mkfs/initramfs` tree. `logd` / `logctl` provide the current
volatile service log. The shell, SWS, SAS, input methods, and other services
are userspace processes, not kernel subsystems.

## Graphics, ABI compatibility, and verification

ScarletUI is developed in its own repository. The in-tree manifests select
`std,platform-sws` for Scarlet and a host platform where supported. Its
application/extension contract and implementation documentation are maintained
with that repository; the local [UI notes](../graphics/scarletui/design.md)
provide integration context, not a second independent copy of every trait.

SWS is the window server; [sws-client](../graphics/sws-client.md) is its low-level
client. The [SWS wire policy](../graphics/sws-ipc-protocol.md) requires a matched
server/client set. SGFX is the coordinated renderer/driver execution layer;
its Rust internals are rebuilt with consumers rather than independently
dynamically loaded. The intended future external graphics boundary is Vulkan
C ABI; a completed Vulkan frontend is not implied by today's native applications.

Linux binaries use the kernel's partial Linux ABI and their own interpreter/
rootfs artifacts. They are not built for Scarlet's Rust std targets. Follow
[Linux ABI status](../abi/linux/status.md) and
[userspace artifacts](../abi/linux/userspace-artifacts.md); ABI compatibility
does not emulate a different CPU architecture.

Host library tests, Scarlet-target compilation, image construction, and actual
application execution establish different things. Record the selected source
and lock set with results; a host test does not certify a native device path.
