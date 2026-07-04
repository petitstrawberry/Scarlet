# Apple AVD Hardware Decode Bring-up

This document tracks the Scarlet-side bring-up plan for Apple Video Decoder
(AVD) on Apple Silicon.

## Initial Goal

Decode H.264 Annex B access units with Apple AVD and return NV12 frames through
the existing Scarlet video decode device contract at `/dev/vvideo0`.

The first working path is:

```text
video_player
  -> /dev/vvideo0
  -> Apple AVD video backend
  -> AVD low-level MMIO + DART driver
  -> Rust Cortex-M3 firmware
  -> NV12 frame
  -> video_player NV12-to-BGRA conversion
```

SWS remains BGRA-only for the initial implementation. Native NV12 surfaces,
GPU shader conversion, V4L2 Request API, HEVC, VP9, AV1, HDR, and full error
recovery are explicitly out of scope until H.264 playback works through the
existing Scarlet video API.

## Firmware ABI

Scarlet loads a small `thumbv7m-none-eabi` firmware into the AVD Cortex-M3 SRAM.
The firmware is not a codec parser. It applies version/tier tunables, enables
NVIC, forwards DONE/ERROR IRQs, and reports panic/debug state to the kernel.

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
  initializes the H.264 engine MMIO defaults, DART-maps input/output/workspace
  buffers, generates a first-pass AVD H.264 instruction stream, records
  firmware/mailbox trace events, registers a Scarlet video backend, and exposes
  a `/dev/vvideo0` hardware frontend.
- `/dev/avd0`: a text debug device for the first registered AVD instance. Write
  `info`, `fw-ping`, `dart-test`, `decode-one`, `poll-decode`, `trace`, or
  `clear-trace`, then read the device to fetch the report.
- `kernel::device::video`: shared `/dev/vvideo*` ABI definitions plus a decode
  backend registry used by AVD and future non-VirtIO backends.

The `/dev/vvideo0` AVD frontend implements the existing mmap/ioctl entrypoints
and submits H.264 Annex B access units through the Apple AVD backend. The first
frontend parses SPS/slice metadata, supports progressive 8-bit 4:2:0 H.264, and
expects one pending decode at a time while reference tracking is brought up.

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
4. Decode one known-good H.264 frame into NV12 and verify a checksum.
5. Expose AVD through `/dev/vvideo0` behind the current mapped session API.
6. Add reference tracking, frame ordering, teardown cleanup, IRQ completion,
   cache maintenance audit, and recovery paths.

## Debug Hooks

The driver should grow these commands or equivalent kernel debug hooks:

The `/dev/avd0` debug device currently provides the equivalent commands:

| Write command | Purpose |
| --- | --- |
| `info` | Print ADT/MMIO/IRQ, firmware state, current status registers, and backend capabilities. |
| `fw-ping` | Poll the firmware mailbox and report before/after status snapshots. |
| `dart-test` | Allocate a 16 KiB-aligned buffer, map it through the AVD DART context, clean/invalidate cache, and report the IOVA. |
| `decode-one` | Submit the built-in 16x16 H.264 Annex B IDR access unit through the same AVD backend used by `/dev/vvideo0`. |
| `poll-decode` | Poll completion for a previously submitted `decode-one` request. |
| `trace` | Dump retained MMIO/mailbox/decode trace events. |
| `clear-trace` | Clear retained trace events. |

The first smoke media target is `/root/media/bad_apple.h264`, after a smaller
one-frame test asset passes.

## Full Plan

The original detailed task breakdown is kept in `avd-plan.md` at the repository
root while this bring-up branch is in progress.
