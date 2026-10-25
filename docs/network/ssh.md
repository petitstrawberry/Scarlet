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
one bounded request buffer. Requests are processed in order; SSH backpressure
stops further reads instead of accumulating an entire transfer. Data is binary,
not terminal input. Closing the SSH connection drops its file and directory
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
