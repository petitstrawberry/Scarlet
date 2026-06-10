# Video Decode Device API

## Overview

Scarlet exposes video decode hardware as a character device such as
`/dev/vvideo0`. The API described here is the Scarlet video decode device
contract used by userspace. It is not inherently tied to VirtIO, although the
current implementation in this PR is backed by the VirtIO video driver and a
vhost-user-video host process.

The current API is intentionally small:

- `write()` / `read()` provide a compatibility path for one H.264 Annex B access
  unit at a time.
- `mmap()` plus `control()` commands provide the preferred zero-copy-ish path
  used by `video_player`.
- Session-aware commands allow several independent task groups to own separate
  streams on the same device.

This interface is not a stable userspace ABI yet. The structures below document
the current PR state so that callers and future driver changes have a shared
reference.

## Device Model

Video decode devices are exposed as `vvideoN`, starting at `vvideo0`. Userspace
talks to the device through normal file operations, `mmap()`, and `control()`.
A future non-VirtIO implementation should be able to expose the same device
contract.

The current backend is a PCI VirtIO video device. That driver negotiates
`VIRTIO_F_VERSION_1` and the VirtIO video `RESOURCE_GUEST_PAGES` feature. The
host backend added with this work is `tools/vhost-video-videotoolbox`,
which decodes through Apple's VideoToolbox when the QEMU vhost-user-video path
is enabled.

The kernel side accepts these coded stream formats:

| Name | Value |
| --- | ---: |
| H.264 | `4098` |
| AV1 | `4103` |

The convenience `write()` path is H.264-only. The mapped control path carries the
coded format in each submit request.

## Frame Format

Decoded frames are returned as an `SVF1` frame:

| Offset | Size | Field |
| ---: | ---: | --- |
| 0 | 4 | ASCII magic `SVF1` |
| 4 | 4 | width, little-endian `u32` |
| 8 | 4 | height, little-endian `u32` |
| 12 | 4 | pixel format, little-endian `u32` |
| 16 | 4 | payload length, little-endian `u32` |
| 20 | N | frame payload |

The current player expects NV12 video-range payloads. That pixel format is
reported as `0x3432_3076` (`v024`).

## `write()` / `read()` Path

The stream path is useful as a simple smoke test:

1. Open `/dev/vvideo0`.
2. Write one H.264 Annex B access unit.
3. Read 20 bytes of `SVF1` header.
4. Read `payload_len` bytes of NV12 payload.

Only one pending decode is allowed per stream. If the backend has not produced a
frame yet, reads may return status text or zero bytes rather than a complete
frame. New users should prefer the mapped control path.

## Mapped Buffer Layout

Each video session owns one shared mapping:

| Region | Offset | Size |
| --- | ---: | ---: |
| input bitstream | `0` | `8 * 1024 * 1024` |
| output frame | `8 * 1024 * 1024` | aligned `16 * 1024 * 1024 + 20` |

The output payload starts at `output_offset + 20` after a successful dequeue.
The mapping offset for stream `stream_id` is:

```text
(stream_id - 1) * mapped_buffer_len
```

Both `mmap` offset and length must be page-aligned. The driver currently
supports at most four sessions.

## Control Commands

All commands use raw `#[repr(C)]` structures copied between Scarlet userspace
and the kernel. The command names currently use the `VVIDEO_` prefix because
they were introduced with the `vvideoN` device. They should be read as video
decode device commands, not as a VirtIO-only userspace ABI. All current Scarlet
targets are little-endian; do not treat these layouts as a portable cross-OS
ABI.

| Command | Value | Argument | Return |
| --- | ---: | --- | --- |
| `VVIDEO_GET_BUFFER` | `0x5600` | `*mut ScarletVideoBufferInfo` | `0` |
| `VVIDEO_SUBMIT` | `0x5601` | `*const ScarletVideoSubmit` | `0` |
| `VVIDEO_DEQUEUE` | `0x5602` | `*mut ScarletVideoDequeuedFrame` | `1` if ready, `0` if empty |
| `VVIDEO_CREATE_SESSION` | `0x5603` | `*mut ScarletVideoSessionInfo` | `0` |
| `VVIDEO_SUBMIT_SESSION` | `0x5604` | `*const ScarletVideoSessionSubmit` | `0` |
| `VVIDEO_DEQUEUE_SESSION` | `0x5605` | `*mut ScarletVideoSessionDequeuedFrame` | `1` if ready, `0` if empty |
| `VVIDEO_DESTROY_SESSION` | `0x5606` | `*const ScarletVideoSessionInfo` | `0` |

`VVIDEO_GET_BUFFER` uses the default stream, stream id `1`. New code should use
`VVIDEO_CREATE_SESSION` first; passing `stream_id = 0` allocates an available
session, while passing a nonzero `stream_id` claims or queries that session.

## ABI Structures

```rust
#[repr(C)]
struct ScarletVideoBufferInfo {
    mmap_offset: u64,
    mmap_len: u64,
    input_offset: u64,
    input_len: u32,
    output_offset: u64,
    output_len: u32,
}

#[repr(C)]
struct ScarletVideoSubmit {
    input_len: u32,
    coded_format: u32,
    timestamp: u64,
}

#[repr(C)]
struct ScarletVideoDequeuedFrame {
    width: u32,
    height: u32,
    pixel_format: u32,
    payload_offset: u64,
    payload_len: u32,
    flags: u32,
    timestamp: u64,
}

#[repr(C)]
struct ScarletVideoSessionInfo {
    stream_id: u32,
    padding: u32,
    buffer: ScarletVideoBufferInfo,
}

#[repr(C)]
struct ScarletVideoSessionSubmit {
    stream_id: u32,
    input_len: u32,
    coded_format: u32,
    padding: u32,
    timestamp: u64,
}

#[repr(C)]
struct ScarletVideoSessionDequeuedFrame {
    stream_id: u32,
    padding: u32,
    frame: ScarletVideoDequeuedFrame,
}
```

If `timestamp` is zero on submit, the driver assigns a monotonically increasing
per-session timestamp. `flags` is currently always zero.

## Typical Mapped Decode Sequence

1. Open `/dev/vvideo0`.
2. Call `VVIDEO_CREATE_SESSION` with `stream_id = 0`.
3. Map `buffer.mmap_len` bytes at `buffer.mmap_offset` with read/write shared
   permissions.
4. Copy one coded access unit into `input_offset`.
5. Call `VVIDEO_SUBMIT_SESSION` with the returned stream id, input length, coded
   format, and optional timestamp.
6. Poll `VVIDEO_DEQUEUE_SESSION` until it returns `1`.
7. Read metadata from `ScarletVideoDequeuedFrame`.
8. Use `payload_offset` and `payload_len` to read the decoded NV12 payload from
   the mapping.
9. On teardown, unmap the buffer and call `VVIDEO_DESTROY_SESSION`.

The default-session commands follow the same model but omit the explicit
`stream_id` fields.

## Ownership and Lifetime

Sessions are owned by the current task's thread-group id. A task may only
dequeue, mmap, or destroy sessions it owns. Closing the device releases sessions
owned by that task group, and the driver also cleans up sessions whose owner task
has exited.

The driver allows only one pending decode per session. A second submit before the
previous frame is completed returns an error. `select`/`poll` readiness is still
coarse in this prototype, so callers should treat a dequeue return value of `0`
as "not ready yet" and retry with their own timeout.

## Current Limitations

- The ABI is duplicated in the kernel driver and `video_player`; it should move
  to a shared userspace-visible header or crate before being treated as stable.
- The mapped path has fixed buffer sizes and a fixed session limit.
- The output path currently assumes a single-buffer NV12 frame.
- Error reporting is mostly string-based through kernel `Result<&'static str>`
  and status reads.
- The only implementation today is the VirtIO/vhost-user-video backend, so
  backend support depends on the host vhost-user-video process and VideoToolbox
  capabilities.
