# SSH and SFTP

Scarlet's `/bin/sshd` accepts the `sftp` SSH subsystem directly. It shares the
existing SSH authentication and host keys; it does not allocate a PTY, start a
shell, or need a separate `sftp-server` executable. The base bundle already
installs sshd and starts it after netcfgd.

Install an authorized public key in `/root/.ssh/authorized_keys`, as for an SSH
shell, then connect from the other computer:

```sh
sftp root@SCARLET_IP
# For the QEMU runner's configured SSH forwarding port:
sftp -P HOST_PORT root@127.0.0.1
```

The initial remote directory is `/root`. Absolute paths address the normal
Scarlet filesystem. Authentication remains root-only, matching sshd. Ed25519
and supported OpenSSH authorized-key formats follow sshd's existing behavior.
The listener uses port 22 by default (`SCARLET_SSH_PORT` overrides it).

The implementation speaks [SFTP version 3](https://datatracker.ietf.org/doc/html/draft-ietf-secsh-filexfer-02),
with OpenSSH's established symlink argument order and the
[`posix-rename@openssh.com` and `fsync@openssh.com` extensions](https://github.com/openssh/openssh-portable/blob/master/PROTOCOL).
Supported operations include listing and changing directories, stat/lstat,
upload/download and resume, exclusive creation, truncation, removal, mkdir/rmdir,
rename, symlinks/readlink, timestamps, and syncing uploaded files (`put -f`).
Ordinary protocol rename refuses an existing destination atomically; the POSIX
rename extension permits replacement. Open file handles retain their identity
through a rename. Transfers are limited to regular files.

Native permissions are coarse owner R/W/X capabilities. Creation uses that
native model, and metadata reports it without inventing POSIX group/world bits.
Changes to ownership, extended attributes, or unsupported permission bits return
`SSH_FX_OP_UNSUPPORTED`; they are never reported as successful. In particular,
`put -p` may report an unsupported permission-preservation request. Directory
permission changes are unsupported. Pathnames must be UTF-8, contain no NUL, and
fit the native 1024-byte pathname storage including its terminator.

Each connection retains at most 64 open SFTP handles, one pending response, and
one bounded request buffer. Requests execute in order. Incoming channel data is
still drained while a response waits for SSH send credit, so a later SSH window
adjustment cannot become stuck behind queued SFTP requests. Resource bounds
remain enforced instead of accumulating an entire transfer.

sshd advertises a 1 MiB receive window and 32 KiB maximum channel packets, with
64 KiB heap buffers for each SSH transport direction. Sunset's original small
constructor defaults remain available for embedded consumers. Bulk output
reserves space for control replies, and receive credit counts channel payload
bytes rather than packet headers. Data is binary, not terminal input. Closing the SSH connection drops its file and directory
handles. Existing sshd limits of one session channel per connection still apply.

## Validation

From a development shell with Rust and OpenSSH installed:

```sh
nix develop --command bash tests/sftp/test.sh
cargo build --manifest-path .cargo/Cargo.toml -p userprogram \
  --no-default-features --bin sshd --release --target aarch64-unknown-scarlet
```

The host harness compiles the production protocol engine and exercises real
filesystem requests. OpenSSH's direct-server mode checks binary transfer,
Japanese names, resume, rename, links, fsync, and cleanup. It does not establish
that Scarlet's syscalls or SSH transport work. The same smoke test can run over
an SSH connection to an owned test instance:

```sh
python3 tests/sftp/openssh_smoke.py --artifacts /ABSOLUTE/TEST_OUTPUT \
  --port HOST_PORT --identity /ABSOLUTE/TEST_IDENTITY
```

The native test uses root's normal SSH authentication and creates a uniquely
named temporary remote directory. It verifies SHA-256 after each download and
removes only its own remote files. Its fixture host identity and SSH options are
for the local test instance.

## Transfer performance checks

`tests/sftp/benchmark.py` times separate OpenSSH uploads and downloads, including
SSH connection setup, and compares SHA-256. Use `--payload /ABSOLUTE/FILE` to
reuse exactly the same bytes across revisions, or `--mib 32` to create a random
32 MiB fixture. A uniquely named remote file is removed after each run.

```sh
python3 tests/sftp/benchmark.py --artifacts /ABSOLUTE/RESULTS \
  --port HOST_PORT --identity /ABSOLUTE/TEST_IDENTITY --mib 32 --label current
```

`tests/sftp/latency_proxy.py` is an optional, bounded loopback relay for controlled
propagation delay. It does not establish real device/network throughput.

The 2026-10-26 QEMU/HVF check reproduced a stalled 8 MiB download with the old
bridge. The corrected bridge completed repeated 32 MiB transfers with matching
hashes: approximately 14–42 MB/s upload and 16–42 MB/s download in that host's
runs. The final executable measured 17.2 MB/s upload and 16.0 MB/s download. These are QEMU measurements, not a Chromebook hardware result. Earlier
1 MiB interoperability checks did not exercise exhaustion of SSH send credit;
keep the larger transfer regression when changing the bridge.
