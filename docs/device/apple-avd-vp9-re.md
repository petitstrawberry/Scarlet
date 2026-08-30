# Apple AVD VP9 Trace Workflow

This document is the VP9 reverse-engineering workflow for Scarlet's Apple AVD
backend. The intent is to stop changing DMA/MMIO guesses directly in the driver
and first build a reproducible comparison against macOS traces and the
`eiln/avd` firmware emulator.

## Reference Tools

Local paths used during bring-up:

- `eiln/avd`: `/tmp/eiln-avd`
- EILN firmware emulator: `/tmp/eiln-avd/avd_emu.py`
- EILN VP9 tests: `/tmp/eiln-avd/tools/test.py`
- Asahi m1n1 AVD tracer:
  `/Users/petitstrawberry/Development/asahi/m1n1/proxyclient/hv/trace_avd.py`

The EILN emulator does not consume Scarlet stateless VP9 parameters directly.
It consumes Apple's macOS `frame_params` blob plus the AVD CM3 firmware, then
emulates the firmware path that writes the AVD instruction FIFO.

## Capture macOS Baseline

Boot macOS under m1n1 hypervisor tracing and load:

```text
/Users/petitstrawberry/Development/asahi/m1n1/proxyclient/hv/trace_avd.py
```

The tracer is intentionally inactive at boot. In the hypervisor console, set an
output name before running the target decode:

```python
tracer.outdir = "eve_2160p_vp9"
tracer.save_firmware("data/fw.bin")
```

Then trigger VideoToolbox decode from macOS, for example with an IVF copy of the
same VP9 elementary stream:

```bash
ffmpeg -hwaccel videotoolbox -i eve_2160p_vp9.ivf -f null -
```

For VP9, `trace_avd.py` saves:

- `data/vp9/<name>/frame.<timestamp>.<frame_params_iova>.bin`
- `data/vp9/<name>/probs.<timestamp>.<frame_params_iova>.<probs_iova>.bin`

The `frame.*.bin` files are macOS's source AVD `frame_params` structures. The
`probs.*.bin` files are the probability workspace snapshots read from DART
stream 0. Copy the captured directory into the EILN tree:

```bash
mkdir -p /tmp/eiln-avd/data/vp9
cp -R data/vp9/eve_2160p_vp9 /tmp/eiln-avd/data/vp9/
cp data/fw.bin /tmp/eiln-avd/data/fw.bin
```

## Run EILN Emulator and Tests

Generate the instruction FIFO stream from Apple's firmware and one traced frame:

```bash
cd /tmp/eiln-avd
python3 avd_emu.py -f data/fw.bin -i data/vp9/eve_2160p_vp9/frame.<...>.bin -u
```

Run all frames in a trace directory:

```bash
cd /tmp/eiln-avd
python3 avd_emu.py -f data/fw.bin -d vp9/eve_2160p_vp9 -a -u
```

Compare emulator output, EILN's Python VP9 lowering, and probability packing:

```bash
cd /tmp/eiln-avd
python3 tools/test.py -m vp09 -d vp9/eve_2160p_vp9 -f data/fw.bin -e -q -a
```

Use `-j` as well when an input IVF is available in the EILN data tree and the
generated Python `frame_params` should be compared with the trace.

## Scarlet Dumps

Scarlet can dump the userspace VP9 stateless request before it submits to
`/dev/video0`:

```bash
video-player --hwdc --dump-vp9-stateless root/vp9-dump root/eve-hanaarashi-mv-2160p-vp9-opus.webm
```

For each VP9 frame, this writes:

- `scarlet-vp9.<timestamp>.input.bin`: coded WebM VP9 access unit.
- `scarlet-vp9.<timestamp>.frame-params.bin`: Scarlet
  `ScarletVideoVp9FrameParams` ABI bytes.
- `scarlet-vp9.<timestamp>.probs.bin`: Scarlet's 0x774-byte packed VP9
  probability state.
- `scarlet-vp9.<timestamp>.tiles.bin`: Scarlet `ScarletVideoVp9Tiles` ABI bytes.
- `scarlet-vp9.<timestamp>.manifest.txt`: human-readable dimensions, flags,
  tile counts, and header sizes.

These files are not macOS `frame_params` and cannot be passed directly to
`avd_emu.py`. They are intended for field-by-field comparison before the Apple
AVD driver lowers the request to private frame-parameter and instruction data.

## Comparison Order

Work in this order:

1. Match VP9 parser facts:
   - key/show frame flags
   - coded and render size
   - uncompressed and compressed header byte sizes
   - tile count and tile byte ranges
   - refresh flags and reference timestamps
2. Match probability packing:
   - compare Scarlet `*.probs.bin` with EILN `probs.*.bin` after accounting for
     EILN's 0x4000-byte DART snapshot containing the packed probability region.
3. Match Apple frame-parameter fields:
   - use EILN `tools/test.py -j` field names as the checklist.
   - especially check VP9 fields around dimensions, flags, probability address
     low bits, PPS/SPS tile addresses, output addresses, and reference
     dimensions.
4. Match instruction stream:
   - after one Scarlet VP9 submit, read `/dev/avd0` command `trace` and compare
     `InstructionWord <index> <word>` events with `avd_emu.py -u` output.
   - address-bearing words must be compared by meaning and alignment, not by
     raw IOVA value.
5. Only after the above matches, test real hardware completion and output
   checksums.

## Known Boundary

As of this note, the local `/tmp/eiln-avd` tree has emulator and VP9 tooling but
does not include captured `data/vp9/*` traces or firmware. A real macOS trace is
required before the emulator can produce a useful VP9 baseline for the Scarlet
sample clip.
