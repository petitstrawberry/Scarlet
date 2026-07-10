# Apple AVD VP9 Bring-Up Notes

This document records the VP9-specific Apple AVD path Scarlet should follow.
It is intentionally narrower than the general video decode ABI docs: the goal
is to keep the AVD backend aligned with observed Apple hardware behavior and to
avoid mixing VP9 direct-submit rules with the existing H.264 mailbox path.

## Source Material

The current model is derived from these local primary sources:

- Asahi m1n1:
  `/Users/petitstrawberry/Development/asahi/m1n1/proxyclient/m1n1/fw/avd/decoder.py`
- Asahi m1n1 AVD init:
  `/Users/petitstrawberry/Development/asahi/m1n1/proxyclient/m1n1/fw/avd/__init__.py`
- `eiln/avd` VP9 HAL:
  `/tmp/eiln-avd/avid/vp9/halv3.py`
- `eiln/avd` VP9 context/allocation model:
  `/tmp/eiln-avd/avid/vp9/decoder.py`
- `eiln/avd` VP9 tile parser:
  `/tmp/eiln-avd/avid/vp9/parser.py`
- Trace and emulator workflow:
  [`apple-avd-vp9-re.md`](apple-avd-vp9-re.md)

These sources agree on the important split:

- Userspace parses VP9 headers, compressed-header probabilities, reference
  state, and tile ranges.
- The AVD driver lowers that state into AVD-private instruction words and DMA
  addresses.
- VP9 decode start is a direct decode-engine MMIO submit, not a CM3 firmware
  mailbox decode command.

## Register Path

The VP9 path uses the same instruction FIFO register block as H.264, but a
different video-pipe selector/control register.

| Purpose | Register | VP9 value |
| --- | ---: | ---: |
| VP9 instruction words | `0x1104010` | write every instruction word |
| Decode submit | `0x1104014` | `0x2bfff107`, then `0x2bfff007` for extra tiles |
| VP9 pipe control | `0x110404c` | write `0` before submit |
| Decode status/clear | `0x1104060` | status polling and stage clears |
| Decode status mask | `0x1104064` | initialized to `0x3` |
| FIFO base | `0x1104068 + fifo * 4` | instruction FIFO IOVA >> 8 |
| FIFO size | `0x1104084 + fifo * 4` | `0x100000` |
| FIFO read pointer | `0x11040a0 + fifo * 4` | `0` |
| FIFO write pointer | `0x11040bc + fifo * 4` | `0` |
| Decode pipe mask | `0x110405c` | OR with `0x38000` for VP9 |

H.264 uses the pipe at `0x1104048` and mask `0x1c00`. VP9 must not reuse that
pipe slot.

## Decode Sequence

The reference order for one VP9 frame is:

1. Copy the complete VP9 frame payload to the mapped input IOVA.
2. Copy the packed 0x774-byte VP9 probability table to the mapped probability
   workspace.
3. Configure instruction FIFO 0:
   - FIFO base: `instruction_fifo_iova >> 8`
   - FIFO size: `0x100000`
   - FIFO read/write pointers: `0`
4. Configure VP9 pipe:
   - Write `0` to `0x110404c`
   - OR `0x38000` into `0x110405c`
   - Keep postprocess mask `0x500000` armed from init
5. Write the generated VP9 instruction words to `0x1104010`.
6. Start decode:
   - Write `0x2bfff107` to `0x1104014`
   - For every extra tile, write `0x2bfff007` to `0x1104014`
7. Wait for video stage:
   - Poll until `(status & 0x00c00000) == 0x00c00000`
   - Clear with `0x00020000`
8. Submit postprocess:
   - Write `0x2b000107` to `0x1104014`
9. Wait for postprocess stage:
   - Poll until `(status & 0x00003000) == 0x00002000`
   - Clear with `0x00400000`
10. Only then complete the queued frame.

Do not synthesize completion from `0x02842108` while still waiting for the
video stage. In the reference flow, that value is the final state after the
postprocess clear, not proof that a newly-submitted frame decoded.

## Instruction Stream Checklist

For the 3840x2160 8-tile test clip, the first keyframe should produce 98 VP9
instruction words. Important fixed points:

- First instruction word: `0x2bfff100`
- Header command: `0x2db032e0` for a keyframe
- Coded size word: `((height - 1) << 16) | (width - 1)`
- Transform word: chroma/colorspace bits plus VP9 tx mode
- Current RVRA addresses are pushed as IOVA >> 7
- Probability, PPS, SPS, Y, and UV addresses are pushed as IOVA >> 8
- Tile payload addresses are full low 32-bit IOVAs
- Intermediate tile terminator: `0x2bfff000`
- Final tile terminator: `0x2b000400`

The tile parser must compute ranges after both the uncompressed and compressed
headers. All non-final tile sizes are big-endian 32-bit values.

## Workspace Checklist

The VP9 RVRA layout follows the `eiln/avd` calculation:

```text
width32  = align(width, 32)
height32 = align(height, 32)
size0    = width32 * height32 + width32 * height32 / 4
size1    = max(next_power_of_two(width) * next_power_of_two(height) / 32, 0x100)
size2    = size0 / 2 for 4:2:0
total    = align(size0 + size1 + size2, 0x4000) + width_dependent_padding
offsets  = [size0, 0, size0 + size1 + size2, size0 + size1]
```

For the current Profile 0 / 8-bit / yuv420p target, `size2 = size0 / 2`.
The SPS tile scratch area has seven 0x8000-byte slots because the VP9 HAL uses
slot indices 0, 1, 3, 4, 5, and 6.

## Scarlet Implementation Rules

- Keep the H.264 firmware mailbox path unchanged.
- Do not send a VP9 decode command through the CM3 mailbox unless a real
  firmware ABI for that command has been proven.
- Do not treat firmware mailbox VP/PP messages as VP9 completion. VP9 direct
  completion is status-bit driven.
- Do not run the broad H.264 pre-submit decode-status latch clear before a VP9
  direct submit. The reference VP9 path only clears the stage bits after the
  corresponding wait conditions are observed.
- On VP9 timeout, report failure and recover the decode engine. Do not return a
  black/stale frame and do not mark the request completed by a fallback status.
- Keep VP9 failure recovery isolated so a subsequent H.264 stream can still use
  the H.264 mailbox path.

## Current Failure Interpretation

The observed Scarlet log:

```text
vp9 decode start ok tiles=8
decode poll ... phase=WaitingVideo status=0x2842108
```

means the VP9 direct submit is reaching the decode block, but Scarlet never sees
the video-stage done mask `0x00c00000`. The value `0x02842108` is not enough to
advance. The next implementation check is to stop applying H.264-style
pre-submit status clearing and mailbox completion rules to VP9.

Before changing the hardware path further, capture a macOS trace with m1n1 and
compare Scarlet's VP9 parser/probability/tile output against the EILN toolchain
as described in [`apple-avd-vp9-re.md`](apple-avd-vp9-re.md).
