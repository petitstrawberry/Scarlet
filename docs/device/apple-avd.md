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

## Milestones

1. Add docs, ABI constants, firmware crate skeleton, and kernel driver skeleton.
2. Build Rust CM3 firmware binaries for known AVD version/tier combinations.
3. Bring up MMIO, PMGR, DART, SRAM load, CM3 boot, and mailbox receive.
4. Port AVD engine initialization, instruction FIFO, and H.264 submit path.
5. Decode one known-good H.264 frame into NV12 and verify a checksum.
6. Expose AVD through `/dev/vvideo0` behind the current mapped session API.
7. Add reference tracking, frame ordering, teardown cleanup, IRQ completion,
   cache maintenance audit, and recovery paths.

## Debug Hooks

The driver should grow these commands or equivalent kernel debug hooks:

```text
avd-info
avd-fw-ping
avd-dart-test
avd-decode-one
avd-trace
```

The first smoke media target is `/root/media/bad_apple.h264`, after a smaller
one-frame test asset passes.

## Full Plan

The original detailed task breakdown is kept in `avd-plan.md` at the repository
root while this bring-up branch is in progress.
