# zlib consumer acceptance test

This builds the **unmodified zlib 1.3.2 release** as a static library, then links
an ordinary C `main` directly with Scarlet's matching CRT and std-backed
`libscarlet_c.a`. All 15 upstream core and gzip source files are compiled. The
fixture exercises the actual upstream compression and file APIs.

Source provenance:

- [Official release](https://github.com/madler/zlib/releases/tag/v1.3.2), February 17, 2026.
- [Official source archive](https://zlib.net/fossils/zlib-1.3.2.tar.gz).
- SHA-256, also published on the [zlib home page](https://zlib.net/):
  `bb329a0a2cd0274d05519d61c667c062e06990d72e125ee2dfa8de64f0119d16`.
- The archive's license remains intact; the builder also copies it to
  `ZLIB-LICENSE` in the output directory. Upstream sources are downloaded into
  the build output, not vendored into Scarlet.

Build with matching release inputs:

```sh
python3 tools/native-rustc/consumer-zlib/build.py \
  --target aarch64-unknown-scarlet \
  --sysroot /path/to/scarlet-cross-sysroot \
  --libc /path/to/aarch64-unknown-scarlet/release/libscarlet_c.a \
  --clang /path/to/unwrapped/clang \
  --ar /path/to/llvm-ar \
  --linker /path/to/ld.lld \
  --output /tmp/scarlet-zlib-aarch64
```

`--source-archive /path/to/zlib-1.3.2.tar.gz` reuses a local download; its bytes
must match the same pinned SHA-256. Otherwise the builder downloads the pinned
URL. The output directory must not exist. RV64 uses the same command with
`--target riscv64gc-unknown-scarlet` and matching archive/sysroot.

The builder uses a bare LLVM target, `-ffreestanding`, `-nostdinc`, Scarlet's C
headers and Clang's builtin compiler headers. It clears environment-provided
include paths. There is no host `configure` detection, host C library, or
target-source patch. `Z_HAVE_UNISTD_H` selects Scarlet's descriptor interface;
the C11 target supplies standard integer widths and varargs. Recorded commands,
tool versions, hashes, archive-member architecture checks and a final ELF audit
are saved beside the output `zlib-probe`.

Run the executable **inside Scarlet** with a new writable directory:

```sh
/system/bin/zlib-probe /tmp/zlib-consumer
```

The caller must create the directory. Full acceptance requires exit status
**47** and the exact stdout line **`SCARLET_LIBC_ZLIB_OK`**. Building the ELF does
not establish guest success; the build report explicitly leaves guest execution
pending. The QEMU harness can stage and run this through `--zlib-probe`.
Use the matching Native TLS runtime/CRT and a kernel with the current native
descriptor operations; an older kernel cannot run this libc's descriptor adapter.

The fixture verifies:

- 131,113 bytes of deterministic mixed binary input through `compress2` and
  `uncompress`, plus rejection of a corrupted header.
- Incremental `deflate` and `inflate` with small, mismatched input/output chunks,
  progress checks, exact byte comparison and a trailing sentinel.
- Gzip writes, integer/string `gzprintf`, reads, EOF, forward/backward seek and
  file positions.
- `gzdopen` ownership of a duplicated descriptor, leaving the original open.
- Missing-file errno and a stored-CRC corruption reported by `gzerror`.

The generated files remain in the supplied directory for inspection, including
the deliberately corrupted gzip file. This is a real-library acceptance test,
not complete libc/POSIX conformance, exhaustive zlib testing, or proof of every
stdio format and filesystem edge case.
