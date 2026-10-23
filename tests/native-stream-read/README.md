# Bounded native stream reads

`main.rs` writes a patterned 1 MiB file, offers a 4 MiB read buffer, requires
a short read of at most 64 KiB, and checks that `read_to_end` retrieves every
remaining byte exactly once. Run against the changed kernel, not on the host.

Build in the Scarlet development shell:

```sh
rustc --edition 2024 --target aarch64-unknown-scarlet -O \
  tests/native-stream-read/main.rs -o /tmp/bounded-stream-read
```

Stage the executable, `/init` and `/bin/scarlet-ld` in a disposable native
smoke root. Use `tests/native-desktop-smoke/run.py` with
`--program /bin/bounded-stream-read --expect-marker SCARLET_BOUNDED_STREAM_READ_OK`.
The full rust-analyzer acceptance additionally reads the actual native rustc
shared library that previously triggered a roughly 120 MiB kernel allocation.
