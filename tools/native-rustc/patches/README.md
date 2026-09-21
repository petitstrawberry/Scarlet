# Experimental Rust host patches

These patches are inputs to a separate Rust-fork experiment, not changes to the
installed cross-toolchain. They are **not a complete native rustc port**.

`rust-39c689-native-host.patch` applies to
`petitstrawberry/rust@39c689a4859b9d8ee1828720135defd125c03d31`. It enables PIC and
dynamic linking for the two 64-bit Scarlet targets, requests
`/system/bin/scarlet-ld`, sets experimental host metadata, and supplies Scarlet's
`std::env::consts` values. It also fixes the Scarlet omission in the std
Unix-path `is_absolute` cfg list, so paths such as `/init` are recognized as
absolute without declaring Scarlet to be Unix. It preserves panic=abort, OS-level TLS, and the empty
target family. Bootstrap already links std statically into rustc_driver; no
system-wide shared std is introduced.

Apply only in an isolated Rust checkout:

```sh
git -C "$RUST_EXPERIMENT" apply --check "$SCARLET/tools/native-rustc/patches/rust-39c689-native-host.patch"
git -C "$RUST_EXPERIMENT" apply "$SCARLET/tools/native-rustc/patches/rust-39c689-native-host.patch"
```

The locked compiler uses **both** libloading 0.8.9 (rustc_metadata) and 0.9.0
(rustc_codegen_llvm). Copy each unpacked crate into its own experiment directory;
do not edit Cargo registry caches or a shared vendor directory. For 0.8.9:

```sh
git -C "$LIBLOADING_08_EXPERIMENT" apply "$SCARLET/tools/native-rustc/patches/libloading-0.8.9-scarlet.patch"
cp "$SCARLET/tools/native-rustc/libloading_scarlet.rs" "$LIBLOADING_08_EXPERIMENT/src/scarlet.rs"
```

For 0.9.0, also adapt its changed error API (keep the default `std` feature):

```sh
git -C "$LIBLOADING_09_EXPERIMENT" apply "$SCARLET/tools/native-rustc/patches/libloading-0.9.0-scarlet.patch"
cp "$SCARLET/tools/native-rustc/libloading_scarlet.rs" "$LIBLOADING_09_EXPERIMENT/src/scarlet.rs"
git -C "$LIBLOADING_09_EXPERIMENT" apply "$SCARLET/tools/native-rustc/patches/libloading-0.9.0-adapter.patch"
```

In the experiment's root Cargo.toml, add two aliases to the **existing**
`[patch.crates-io]` table, with absolute paths to those copies:

```toml
libloading_08 = { package = "libloading", path = "/absolute/experiment/libloading-0.8.9" }
libloading_09 = { package = "libloading", path = "/absolute/experiment/libloading-0.9.0" }
```

Update only that experiment's Cargo.lock to account for the path patches.
Vendoring requires regenerating that experiment's vendor metadata; editing
vendored files in place invalidates Cargo checksums.

The adapter exposes the subset used by rustc's metadata loader:
`Library::new`, `get`, `close`, `Symbol::into_raw`, dereferencing, and the raw
pointer accessor used by LLVM's optional Enzyme code. It explicitly opens with
`RTLD_NOW | RTLD_GLOBAL` (`0x102`). It does not provide Unix APIs or the full 0.9
`AsFilename`/`AsSymbolName` surface. It intentionally provides no `Send`/`Sync`
guarantees. Its mutex serializes calls within one adapter instance; that is not
a substitute for a process-wide loader concurrency implementation. Concurrent or
recursive adapter calls fail instead of waiting. The adapter conservatively
rejects null-valued symbols even if `dlsym` reports no error; this is an adapter
limitation, not a Scarlet loader restriction.

Do not create a fake libdl, mark Scarlet `unix`, or ignore all undefined symbols
to make compiler linking appear successful. The interpreter supplies `dlopen`,
`dlsym`, `dlclose`, and `dlerror`, but the native linker integration must preserve
those unresolved **dynamic** symbols explicitly. `--unresolved-symbols=ignore-all`
can instead resolve an executable reference to zero and is not a validated host
link recipe. The provided adapter was compiled to rlib and used from compiler-like
metadata probes on RV64/AArch64; it has not been executed inside a native rustc.
