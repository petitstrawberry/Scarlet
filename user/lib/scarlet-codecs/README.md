# scarlet-codecs

Userspace codec request builders for Scarlet stateless video decode.

This crate keeps bitstream parsing and request construction in userspace. Kernel
video backends receive prepared request parameters and lower them to hardware
command streams.

## Features

- `h264` is disabled by default.
- `vp9` is disabled by default while the AVD path is still experimental.
- `media-container-aac` is disabled by default. It adds reusable MP4/fMP4 demux,
  AAC-LC decode, and AVC framing/SPS inspection for native video decoders using
  the standard library. It does not depend on a software video decoder.
- `software-h264` explicitly enables the optional software AVC decoder.
  `software-media` remains a compatible alias for the combined implementation.
  AVC licensing opt-in applies to these media features too.

H.264/AVC may be patent-encumbered in some jurisdictions. The `h264` feature is
therefore an explicit opt-in.

## Software media core

`media::Mp4Parser` accepts byte chunks with `push_bytes` and yields `Track` and
`Sample` events through `next_event`. Drain events after each append; `BufferFull`
leaves the append unchanged. Call `finish` at EOF. Fragment decode and composition
timestamps are retained across separate MSE appends. `reset` discards incomplete
append state while retaining a completed initialization segment; create a new
parser for a new URL. The shared Symphonia ISO MP4 parser owns sample-table parsing.

`AvcConfiguration` converts complete AVCC samples into validated Annex-B access
units for a native decoder such as `/dev/video0`. It preserves SPS/PPS EBSP bytes,
updates same-ID in-band parameter sets, and exposes dimensions, colour and reorder
depth. The native configuration currently requires one SPS and one PPS; additional
parameter-set IDs are explicitly rejected to avoid ambiguous active-stream metadata.
SPS changes are validated before submission. For a bounded output queue, Main/High
streams must signal a reorder depth of at most two; Baseline without VUI has none.

With `software-h264`, `VideoDecoder` accepts AVCC samples and returns timestamped I420 planes, strides,
and SPS colour metadata. `AudioDecoder` returns timestamped interleaved signed
16-bit PCM. Decoder reset reinstalls configuration after a seek/discontinuity.
Clocks, network input, audio devices, rendering and output queue scheduling belong
to the caller, so no browser or video-player process is required by this crate.

Limits are explicit: 8 MiB parser storage, 1 MiB `moov` initialization, 4 MiB encoded
sample, 4096 samples per media segment, eight tracks, 3840×2160 coded AVC pictures,
eight reference pictures and two reordered pictures (at most three returned video
frames). In-band SPS changes are validated before decoder allocation. Fragment
absolute base offsets are rejected; `default-base-is-moof` fragments are supported.
Ordinary files must fit the buffer bound; non-faststart files need `finish`.
AVC parameter sets in `avcC` are required. AAC requires AudioSpecificConfig for LC
with one or two channels; VP9, AV1, Opus and HE-AAC are not advertised.

The `software_media` integration tests generate their own short fixtures with
FFmpeg (`FFMPEG` may override its path), and compare decoded I420 with an independent
FFmpeg decoder. Fixture directories are cleaned after use. Validation may set
`SCARLET_MEDIA_TEST_ROOT` to an automatically cleaned temporary directory.
