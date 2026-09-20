# Rust shared-object smoke fixture

This `no_std` Rust `cdylib` exports `rust_answer() -> i32` with the C ABI. It reads
an exported global pointer and its pointee using volatile loads, so the call
requires the loader to apply Rust-generated GOT/data relocations and return 42.

The build helper asks the selected Scarlet compiler for its current target JSON,
then enables `dynamic-linking` and `pic` in a generated target under the output
directory. Cargo builds genuine `core` and `compiler_builtins` sources for that
custom target using `-Zbuild-std`; no installed target, sysroot or compiler file
is changed. Use a Scarlet nightly toolchain that includes Rust library sources.

```sh
python3 tools/loader-smoke/build-rust-dso.py \
  --toolchain /path/to/scarlet-toolchain \
  --arch aarch64 --offline --output target/loader-smoke/rust-aarch64
```

Replace `aarch64` with `riscv64` to build RV64. The output is
`<output>/staging/system/lib/libsmoke-rust.so`, with a generated target JSON and
`build.json` recording compiler version, command and artifact checksum.
Copy the shared object into the guest's `/system/lib`, call
`dlopen("libsmoke-rust.so", RTLD_NOW | RTLD_GLOBAL)`, resolve `rust_answer` through
`dlsym`, and check that invoking it returns 42.

This proves loading code and data relocations from a Rust-produced shared
object. The interface deliberately uses the C ABI; this fixture makes no claim
about Rust's unstable dylib ABI, dynamically linked `std`, TLS, or running rustc.
The panic handler spins indefinitely; the tested function does not panic.
