# Apple AVD Hardware Decode Bring-up

This document tracks the Scarlet-side bring-up plan for Apple Video Decoder
(AVD) on Apple Silicon.

## Current Goal

Decode userspace-prepared stateless H.264 and VP9 requests with Apple AVD and
return NV12 frames through the Scarlet video decode device contract at
`/dev/video0`.

The first working path is:

```text
video-player
  -> scarlet-video-client
  -> /dev/video0
  -> Apple AVD video backend
  -> AVD low-level MMIO + DART driver
  -> Rust Cortex-M3 firmware
  -> NV12 frame
  -> video-player NV12-to-BGRA conversion
```

SWS remains BGRA-only for the initial implementation. Native NV12 surfaces,
GPU shader conversion, HEVC, AV1, HDR, and full error recovery are still out of
scope for this bring-up branch. The kernel contract uses a Scarlet stateless
request split: userspace owns codec parsing, compressed-header processing,
probability/context updates, and reference management; the AVD driver validates
the accepted subset and lowers stateless parameters to hardware commands.

## Firmware ABI

Scarlet loads a small `thumbv7m-none-eabi` firmware into the AVD Cortex-M3 SRAM.
The firmware is not a codec parser. It applies version/tier tunables, enables
NVIC, arms decode IRQ forwarding for the H.264 mailbox path, forwards DONE/ERROR
IRQs, and reports panic/debug state to the kernel. VP9 bring-up currently uses
the direct decode-engine MMIO path documented in
[`apple-avd-vp9.md`](apple-avd-vp9.md), not a firmware mailbox decode command.

Firmware-to-kernel messages:

| Message | Value | Meaning |
| --- | ---: | --- |
| `MSG_READY` | `0x0000_0001` | Firmware initialized and waiting for work |
| `MSG_PANIC` | `0x0000_0002` | Firmware panic/hardfault |
| `MSG_VP_DONE` | `0x0000_0100` | Video pipe decode completed |
| `MSG_VP_ERROR` | `0x0000_0200` | Video pipe decode error |
| `MSG_PP_DONE` | `0x0000_1000` | Post-process pipe completed |
| `MSG_UNKNOWN_IRQ` | `0x0001_0000` | Unexpected IRQ vector |

## Bring-up Sequence

1. Locate `/arm-io/avd`.
2. Locate `/arm-io/dart-avd`.
3. Enable PMGR power for both nodes.
4. Initialize DART and attach stream 0.
5. Map AVD MMIO.
6. Clear code/SRAM windows.
7. Load the selected CM3 firmware blob.
8. Start CM3.
9. Wait for `MSG_READY`.

## Current Branch State

The branch currently provides:

- `firmware/apple-avd-fw-rs`: a Rust `thumbv7m-none-eabi` CM3 firmware image
  with version/tier feature flags, tunable MMIO replay, mailbox command polling,
  IRQ forwarding, and panic reporting.
- `drivers/video/apple-avd`: an Apple platform driver that probes AVD MMIO,
  resolves the DART-backed DMA context, embeds and starts the Rust CM3 firmware,
  initializes the decode engine MMIO defaults, DART-maps input/output/workspace
  buffers, keeps decoded reference frames in AVD workspace slots, lowers
  stateless H.264 and VP9 requests to AVD instruction streams, records
  firmware/mailbox trace events, registers a Scarlet video backend, and exposes
  a `/dev/videoN` hardware frontend through the common Scarlet video device
  layer.
- `/dev/avd0`: a text debug device for the first registered AVD instance. Write
  `info`, `fw-ping`, `dart-test`, `decode-one`, `poll-decode`, `trace`, or
  `clear-trace`, then read the device to fetch the report.
- `kernel::device::video`: shared `/dev/video*` ABI definitions plus a decode
  backend registry used by AVD and future non-VirtIO backends.
- `user/lib/scarlet-codecs`: userspace codec request builders. The current H.264
  module owns Annex B scanning, SPS/PPS/slice parsing, POC, DPB, and reference
  list construction for `scarlet-video-client`.
- `user/lib/scarlet-video-client`: the application-facing stateful/stateless
  decoder abstraction, including session/mmap ownership and NV12 frame
  validation. `video-player` no longer carries the raw video device ABI.

The `/dev/video0` AVD frontend implements the shared mmap/ioctl entrypoints and
accepts `SCARLET_VIDEO_SUBMIT_H264_STATELESS` and
`SCARLET_VIDEO_SUBMIT_VP9_STATELESS` requests. For H.264, userspace supplies
SPS/PPS/slice metadata, POC, DPB entries, and reference lists. For VP9,
userspace supplies uncompressed-header fields, the compressed-header-derived
probability state, tile byte ranges, and last/golden/alternate reference
timestamps. The backend validates the AVD-supported subset, lowers generic
stateless parameters to AVD instruction streams, and expects one pending decode
at a time. Reference pictures are retained in AVD workspace slots and copied
back into the current single-buffer NV12 userspace layout on completion.
VP9 hardware bring-up should use the trace-first workflow in
[`apple-avd-vp9-re.md`](apple-avd-vp9-re.md) before changing AVD-private DMA,
workspace, or instruction-lowering rules.

Useful build checks:

```bash
cargo make build-apple-avd-firmware
cargo make build-apple-avd-firmware-all
cargo check --manifest-path drivers/video/apple-avd/Cargo.toml
```

## Milestones

1. Build Rust CM3 firmware binaries for known AVD version/tier combinations.
2. Bring up MMIO, PMGR, DART, SRAM load, CM3 boot, and mailbox receive.
3. Port AVD engine initialization, instruction FIFO, and H.264 submit path.
4. Add VP9 stateless request lowering using userspace-supplied probability and
   tile state.
5. Decode one known-good H.264 frame into NV12 and verify a checksum.
6. Decode one known-good VP9 frame into NV12 and verify a checksum.
7. Expose AVD through `/dev/videoN` behind the current mapped session API.
8. Add reference tracking, frame ordering, teardown cleanup, IRQ completion,
   cache maintenance audit, and recovery paths.

## Debug Hooks

The driver should grow these commands or equivalent kernel debug hooks:

The `/dev/avd0` debug device currently provides the equivalent commands:

| Write command | Purpose |
| --- | --- |
| `info` | Print ADT/MMIO/IRQ, firmware state, current status registers, and backend capabilities. |
| `fw-ping` | Poll the firmware mailbox and report before/after status snapshots. |
| `dart-test` | Allocate a 16 KiB-aligned buffer, map it through the AVD DART context, clean/invalidate cache, and report the IOVA. |
| `decode-one` | Submit the built-in 16x16 H.264 IDR sample with fixed stateless parameters through the same AVD backend used by `/dev/videoN`. |
| `poll-decode` | Poll completion for a previously submitted `decode-one` request. |
| `trace` | Dump retained MMIO/mailbox/decode trace events, including VP9 `InstructionWord` entries for instruction FIFO comparison. |
| `clear-trace` | Clear retained trace events. |

The first smoke media target is `/root/media/bad_apple.h264`, after a smaller
one-frame test asset passes.

## Full Plan

The original detailed task breakdown is kept in `avd-plan.md` at the repository
root while this bring-up branch is in progress.
