# Focused desktop application checks

This workspace compiles the production application files directly, excluding
unrelated networking/media dependencies. It is a test harness, not an alternative
OS packaging path. Applications are packaged by `bundles/desktop/bundle.toml`.

Use a Scarlet Rust toolchain with native standard library commit
`26d00a8a3a8489d73f5ae3575880e9f915dfd0f2` or later. From the repository root:

```sh
cargo test --manifest-path tests/desktop-apps/Cargo.toml
cargo check --manifest-path tests/desktop-apps/Cargo.toml \
  --features native-desktop --target aarch64-unknown-scarlet --bins
cargo check --manifest-path tests/settings-app/Cargo.toml \
  --target aarch64-unknown-scarlet --bins
cargo test --manifest-path tests/clipboard-protocol/Cargo.toml
cargo check --manifest-path tests/clipboard-protocol/Cargo.toml \
  --target aarch64-unknown-scarlet --features server --bin sws
```

Host tests exercise models and mounted UI events. `files_data` uses temporary
fixtures; no personal files are touched. The Files GUI and Settings use native
services and are cross-checked separately. Host applications use Winit and SGFX;
unsupported native services report their absence. Explicit Screenshot, Logs and
Applications fixtures are never presented as live native data.

For joint UI development, use Cargo's `[patch."https://github.com/petitstrawberry/scarlet-ui"]`
configuration to point its crates at an adjacent checkout. The published pins
are the reproducible default; a local patch is not required after integration.

See `FILES_VALIDATION.md`, `../../docs/desktop/utilities-validation.md`, and the
ScarletUI performance/typography documents for workload definitions and runtime
limits. Cross-compilation cannot verify SWS IPC, capture ownership, input,
filesystem syscalls or real system metrics; those require a booted Scarlet OS.
