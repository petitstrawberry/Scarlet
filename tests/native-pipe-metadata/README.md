# Native pipe metadata regression

`main.c` is an AArch64 native C guest test linked to the shared Scarlet SDK C
runtime. It calls both the real `fstat` adapter and raw `FileMetadata` syscall
302. It checks FIFO type, stable identity across both ends and duplicates,
different identities for different pipes, endpoint access permissions, queued
byte snapshots without consuming data, metadata after peer closure, and the
unchanged regular-file metadata path.

Invalid/closed handles, a high-word alias of a valid handle, and an invalid
output pointer must fail. Invalid-handle queries must not overwrite the supplied
metadata record. The syscall retains its legacy `usize::MAX` failure sentinel:
older `FileError::from_syscall_result` consumers recognize only that value.
Returning `-EBADF` from syscall 302 would incorrectly look successful to them.
The C adapter still rejects negative descriptors as `EBADF`; the test verifies
that behavior separately. A future errno-preserving metadata API should use a
separate status-aware syscall and migrate callers explicitly.

Anonymous pipes expose their existing shared `pipe_id`, endpoint read/write
capabilities, and the current queue length. They have no directory links or
tracked timestamps, so those fields are zero. They do not gain file seek or
truncate capability. Objects lacking file/pipe metadata remain unsupported.

Compile using the same raw AArch64 C flags and SDK runtime as other native C
fixtures, then run after the distribution init has established the Environment
and writable `/tmp`. Success requires exit 0 and `NATIVE_PIPE_METADATA_OK`.
The Git port's `compat/scarlet/http/guest-test.py` accepts
`--pipe-metadata-source /absolute/path/to/this/main.c` to compile and execute
this probe in the same guest before the HTTPS and Git checks.

Validated on 2026-11-13 with toolchain `71dd0425890d` and a release AArch64
kernel under TCG/OVMF. The probe exited 0 and printed its success marker; the
same guest subsequently completed Git smart HTTPS shallow clone, matched the
host HEAD/README, and passed `git fsck --full --strict`. Compact evidence is
retained by the Git port in `compat/scarlet/http/target/git-https-tcg-02`.
