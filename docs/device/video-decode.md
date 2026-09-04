# Video Decode Device API

## Overview

Scarlet exposes video decode hardware as a character device such as
`/dev/video0`. The API described here is the Scarlet video decode device
contract used by userspace. It is not tied to a particular transport: VirtIO
video and Apple AVD backends both plug into the same frontend.

The current API is intentionally small:

- `write()` / `read()` provide a compatibility path for stateful backends that
  accept one coded access unit at a time.
- `mmap()` plus `control()` commands provide the preferred zero-copy-ish path
  used by `scarlet-video-client`; applications such as `video-player` use the
  client abstraction instead of issuing controls directly.
- Session-aware commands allow several independent task groups to own separate
  streams on the same device.
- Stateless H.264 and VP9 submit use a Scarlet request split: userspace parses
  codec headers and manages codec reference state, while the kernel validates
  copied parameters and lowers them to the selected backend.
  `user/lib/scarlet-codecs` contains the request builders owned by
  `user/lib/scarlet-video-client`; future stateless codecs should follow the
  same split.

New applications should use `scarlet-video-client`. Its public flow is
`open → configure → submit → dequeue`, and it returns validated NV12 frames
without exposing whether the selected backend is stateful or stateless. The raw
structures below remain the kernel/client transport contract and are documented
for backend development.

This interface is not a stable userspace ABI yet. The structures below document
the current state so that callers and future driver changes have a shared
reference.

## Device Model

Video decode devices are exposed as `videoN`, starting at `video0`. Userspace
talks to the device through normal file operations, `mmap()`, and `control()`.

The PCI VirtIO video backend negotiates `VIRTIO_F_VERSION_1` and the VirtIO
video `RESOURCE_GUEST_PAGES` feature. The host backend added with this work is
`tools/vhost-video-videotoolbox`, which decodes through Apple's VideoToolbox
when the QEMU vhost-user-video path is enabled. Apple AVD uses the same Scarlet
frontend with an Apple platform-device backend.

The kernel side accepts these coded stream formats:

| Name | Value |
| --- | ---: |
| H.264 | `4098` |
| HEVC/H.265 | `4099` |
| VP9 | `4102` |
| AV1 | `4103` |

The convenience `write()` path is backend-specific compatibility behavior. The
mapped control path carries the coded format in each stateful submit request.
Apple AVD advertises stateless H.264 and VP9 rather than stateful codec
parsing, so callers must use `SCARLET_VIDEO_SUBMIT_H264_STATELESS` or
`SCARLET_VIDEO_SUBMIT_VP9_STATELESS` there.

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

The stream path is useful as a simple smoke test on stateful backends:

1. Open `/dev/video0`.
2. Write one H.264 Annex B access unit.
3. Read 20 bytes of `SVF1` header.
4. Read `payload_len` bytes of NV12 payload.

Only one pending decode is allowed per stream. If the backend has not produced a
frame yet, reads may return status text or zero bytes rather than a complete
frame. New users should prefer the mapped control path. Stateless backends such
as Apple AVD do not parse the Annex B stream in the kernel.

## Mapped Buffer Layout

Each video session owns one shared mapping:

| Region | Offset | Size |
| --- | ---: | ---: |
| input bitstream | `input_offset` | `input_len` |
| output frames | `output_offset` | `output_len` |

After a successful dequeue, `ScarletVideoDequeuedFrame.payload_offset` and
`payload_len` identify the decoded payload inside the full mapping. Backends may
use `output_len` as a pool of frame slots, so callers must not assume the payload
always starts at `output_offset + 20`. Both `mmap` offset and length must be
page-aligned. The reported lengths come from the selected backend capabilities.

## Control Commands

All commands use raw `#[repr(C)]` structures copied between Scarlet userspace
and the kernel. All current Scarlet targets are little-endian; do not treat
these layouts as a portable cross-OS ABI.

| Command | Value | Argument | Return |
| --- | ---: | --- | --- |
| `SCARLET_VIDEO_GET_BUFFER` | `0x5600` | `*mut ScarletVideoBufferInfo` | `0` |
| `SCARLET_VIDEO_SUBMIT` | `0x5601` | `*const ScarletVideoSubmit` | `0` |
| `SCARLET_VIDEO_DEQUEUE` | `0x5602` | `*mut ScarletVideoDequeuedFrame` | `1` if ready, `0` if empty |
| `SCARLET_VIDEO_CREATE_SESSION` | `0x5603` | `*mut ScarletVideoSessionInfo` | `0` |
| `SCARLET_VIDEO_SUBMIT_SESSION` | `0x5604` | `*const ScarletVideoSessionSubmit` | `0` |
| `SCARLET_VIDEO_DEQUEUE_SESSION` | `0x5605` | `*mut ScarletVideoSessionDequeuedFrame` | `1` if ready, `0` if empty |
| `SCARLET_VIDEO_DESTROY_SESSION` | `0x5606` | `*const ScarletVideoSessionInfo` | `0` |
| `SCARLET_VIDEO_GET_CAPS` | `0x5607` | `*mut ScarletVideoCapabilities` | `0` |
| `SCARLET_VIDEO_SUBMIT_H264_STATELESS` | `0x5608` | `*const ScarletVideoH264StatelessSubmit` | `0` |
| `SCARLET_VIDEO_SUBMIT_VP9_STATELESS` | `0x5609` | `*const ScarletVideoVp9StatelessSubmit` | `0` |

`SCARLET_VIDEO_GET_BUFFER` uses the default stream, stream id `1`. New code
should use `SCARLET_VIDEO_CREATE_SESSION` first; passing `stream_id = 0`
allocates an available session, while passing a nonzero `stream_id` claims or
queries that session.

For `SCARLET_VIDEO_CREATE_SESSION`, `ScarletVideoSessionInfo.padding` is treated
as an optional input coded format. `0` preserves the historical default of
H.264. Callers creating a VP9 stateless session pass `SCARLET_VIDEO_FORMAT_VP9`
there; the kernel clears the field before copying the structure back.

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

## Stateless VP9

`SCARLET_VIDEO_SUBMIT_VP9_STATELESS` takes mapped input bytes plus userspace
pointers to:

- `ScarletVideoVp9FrameParams`: uncompressed-header derived frame syntax,
  render size, tile log2 dimensions, refresh flags, and last/golden/alternate
  reference timestamps.
- `ScarletVideoVp9Probabilities`: the 0x774-byte packed probability state after
  userspace parses the VP9 compressed header.
- `ScarletVideoVp9Tiles`: byte ranges for every tile payload inside the mapped
  input frame.

The kernel does not parse VP9 bitstream headers or maintain the VP9 frame
context as codec state. Userspace demuxes frames, parses the uncompressed and
compressed headers, updates probability state, constructs the tile table, and
tracks reference timestamps. The backend validates the AVD-supported subset and
lowers the request to hardware commands. Apple AVD's VP9-specific direct submit
sequence is tracked in [`apple-avd-vp9.md`](apple-avd-vp9.md).

For Apple AVD VP9 bring-up, `video-player` can dump the userspace stateless
request before ioctl submission:

```bash
video-player --hwdc --dump-vp9-stateless root/vp9-dump root/example.webm
```

The dump contains Scarlet's generic VP9 ABI structures, not Apple/macOS
`frame_params`. Use the trace workflow in
[`apple-avd-vp9-re.md`](apple-avd-vp9-re.md) to compare those structures against
m1n1 captures and `eiln/avd`.

## Typical Mapped Decode Sequence

`scarlet-video-client` performs this sequence internally:

1. Open `/dev/video0`.
2. Call `SCARLET_VIDEO_CREATE_SESSION` with `stream_id = 0`.
3. Map `buffer.mmap_len` bytes at `buffer.mmap_offset` with read/write shared
   permissions.
4. Copy one coded access unit into `input_offset`.
5. Call `SCARLET_VIDEO_SUBMIT_SESSION` with the returned stream id, input length, coded
   format, and optional timestamp.
6. Poll `SCARLET_VIDEO_DEQUEUE_SESSION` until it returns `1`.
7. Read metadata from `ScarletVideoDequeuedFrame`.
8. Use `payload_offset` and `payload_len` to read the decoded NV12 payload from
   the mapping.
9. On teardown, unmap the buffer and call `SCARLET_VIDEO_DESTROY_SESSION`.

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

- The raw ABI remains experimental and is mirrored privately by
  `scarlet-video-client`; applications no longer duplicate it.
- The mapped path has fixed buffer sizes and a fixed session limit.
- The output path currently assumes a single-buffer NV12 frame.
- Error reporting is mostly string-based through kernel `Result<&'static str>`
  and status reads.
- Backend support depends on the registered kernel backend. VirtIO depends on
  the host vhost-user-video process and VideoToolbox capabilities; Apple AVD
  depends on the platform DTB exposing AVD and DART resources.
