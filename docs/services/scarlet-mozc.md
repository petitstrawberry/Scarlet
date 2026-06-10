# Scarlet Mozc

`scarlet_mozc` is intended to be a Scarlet-native SWS input-method client that talks to a Linux `mozc_server` process running under Scarlet's Linux ABI.

The conversion engine must stay outside SWS. SWS continues to broker text-input state, key arbitration, preedit, commit, deletion, and IME-owned popup placement. The Mozc-specific client is responsible for converting SWS IME events into Mozc `Command` requests and translating Mozc responses back into SWS preedit, commit, status, and candidate UI updates.

## Architecture

- `mozc_server` runs as a Linux ABI process.
- `mozc_server` creates an abstract Unix-domain stream socket on Linux.
- A Scarlet-native IME client connects to that socket through Scarlet native `Socket::connect_abstract`.
- The native client sends serialized `mozc.commands.Command` protobuf messages to the server and then shuts down the write side of the socket.
- The server reads until EOF, processes the command, writes a serialized `Command` response, and closes the connection.

## Native Client

`scarlet_mozc` is a Scarlet-native SWS input-method service. It is intentionally separate from SWS and from `scarlet_skk`.

Current implemented path:

- registers as the `scarlet-mozc` SWS input method
- loads Mozc's IPC key file from `/scarlet/system/scarlet/root/.config/mozc/.session.ipc`
- reconstructs the Linux abstract socket name as `tmp/.mozc.<key>.mozc_server`
- sends minimal Mozc `Command` protobuf requests:
  - `CREATE_SESSION`
  - `SEND_KEY`
- applies Mozc `Output` fields back to SWS:
  - `result.value` as committed text
  - `preedit.segment.value` as preedit text
  - `status.mode` as IME status
  - `candidate_window` / `all_candidate_words` as an IME-owned popup window
  - preedit segment annotations as styled preedit spans

To try it, start a Linux ABI `mozc_server` first, then run:

```sh
mozc_server &
scarlet_mozc
```

The `mozc_server` command is a Scarlet-native launcher. On AArch64 it expects a
musl-linked Linux server at
`/scarlet/system/linux-aarch64/usr/lib/mozc/mozc_server`. Build it with the same
Buildroot toolchain used by the Linux rootfs, then deploy the staged root
overlay:

The launcher creates `/scarlet/system/scarlet/root/.config/mozc` and runs the
Linux server with `HOME=/scarlet/system/scarlet/root`. Linux ABI processes see
the native Scarlet tree at `/scarlet`, so Mozc writes its profile files,
including `.session.ipc`, into the same native profile directory that
`scarlet_mozc` reads.

```sh
ARCH=aarch64 \
BUILDROOT_DIR=/opt/buildroot-aarch64 \
PREBUILT_DIR=/opt/prebuilt \
WORKDIR="$PWD/.scarlet/cache" \
bash tools/linux/build_mozc_server.sh

ARCH=aarch64 \
PREBUILT_DIR=/opt/prebuilt \
bash tools/linux/deploy_rootfs.sh
```

When using the existing Scarlet devcontainer, use `BUILDROOT_DIR=/opt/buildroot-aarch64`
and `PREBUILT_DIR=/opt/prebuilt`. If Buildroot was built into a repository-local
cache instead, use the same `PREBUILT_DIR` for both `build_buildroot.sh` and
`build_mozc_server.sh` so `deploy_rootfs.sh` can find `rootfs.tar`.

For an already deployed local rootfs, the staged binary can be copied into
`bundles/linux/rootfs/linux-aarch64/usr/lib/mozc/mozc_server`.

`build_mozc_server.sh` must be run on Linux. It intentionally does not import a
distro `mozc_server`; the output is linked against the Buildroot musl sysroot so
the C++ runtime ABI matches Scarlet's Linux rootfs. It builds Mozc's
`//server:mozc_server` target directly. If a future Mozc revision needs
`build_tools/update_deps.py`, pass its arguments through `MOZC_UPDATE_DEPS_ARGS`.
If a newer host toolchain needs Mozc's SFrame workaround, pass
`--config=no_sframe` through `MOZC_BAZEL_EXTRA_ARGS`; it is not enabled by
default because the current Buildroot binutils rejects that assembler option.

Mozc uses `fts.h`, which is not provided by musl. The build script stages
`musl-fts` into the Buildroot sysroot and links `mozc_server` with `-lfts`.
Bazel also runs target-architecture helper binaries while generating Mozc data,
so on an AArch64 Linux host the script prepares `/lib/ld-musl-aarch64.so.1`
links to the Buildroot musl runtime when needed.

Upstream Mozc denies Linux `mozc_server` startup when `uid` or `euid` is `0`.
Scarlet currently launches Linux ABI services as root, so the build script
applies a Scarlet-local source patch that treats root server startup as a normal
run level. Set `MOZC_ALLOW_ROOT_SERVER=0` to disable that patch for non-Scarlet
builds.

## ABI Requirements

Mozc's Linux IPC path currently needs:

- `socket(PF_UNIX, SOCK_STREAM, 0)`
- abstract `sockaddr_un` support, where `sun_path[0] == 0`
- `bind`, `listen`, `accept`, `connect`
- stream `read` / `write` / `send` / `recv`
- `shutdown(SHUT_WR)` to terminate a request body
- `getsockopt(SOL_SOCKET, SO_PEERCRED)` for peer validation
- `select` / `pselect` style readiness waits for IPC timeouts

Scarlet's local socket registry now preserves abstract socket names separately from filesystem paths by storing them internally as `NUL + name`. Abstract sockets do not create VFS socket files.

## Next Steps

1. Exercise `scarlet_mozc` against the real server and fill ABI gaps surfaced by `mozc_server`.
2. Refine Mozc candidate-popup behavior and placement across focused clients and window movement.
3. Add session commands for explicit composition-mode switching and clean session deletion.
4. Add stronger diagnostics around Mozc IPC/profile discovery failures.
