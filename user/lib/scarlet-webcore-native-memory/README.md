# WebCore native shared memory

This static library owns Scarlet shared-memory handles for the WebKit Scarlet
port. The [C header](include/scarlet_webcore_memory.h) exposes creation,
duplication, mapping and release without transferring raw kernel handles or
libc descriptors to WebCore. Rust owns each opaque handle; callers release it
once with `scarlet_webcore_memory_close`. Mappings remain valid after their
handles are closed and must be released separately.

The implementation uses `scarlet-os` with its `std` feature and the matching
Scarlet Rust runtime. It does not emulate shared memory using files or private
allocations. Error returns do not establish a libc `errno` value.

Run from the Scarlet repository root with the matching Scarlet Rust toolchain
selected as `cargo` and `rustc`:

```sh
cargo build --manifest-path user/lib/scarlet-webcore-native-memory/Cargo.toml \
  --target aarch64-unknown-scarlet --release --locked
```

The output is
`user/lib/scarlet-webcore-native-memory/target/aarch64-unknown-scarlet/release/libscarlet_webcore_native_memory.a`.
Set `CARGO_TARGET_DIR` to keep build data elsewhere. Install that archive into
the Scarlet SDK's `lib` directory and `include/scarlet_webcore_memory.h` into
its `include` directory. Link with the same Scarlet libc, CRT and Rust runtime
as the rest of the application; do not mix target archives from host runtimes.

The Rust implementation was moved unchanged from the source recovery bundle's
native WebCore bridge. Guest integration must exercise real sharing through
duplicate writable and read-only mappings, handle closure before unmapping,
and invalid sizes; compiling the archive alone does not establish that behavior.
