# scarlet-video-client

Userspace hardware video decoder abstraction for Scarlet.

The crate owns the `/dev/video0` session, mapped buffers, submit/dequeue ABI,
and the userspace request contexts needed by stateless decoders. Callers submit
complete coded access units and receive validated NV12 frames without needing
to distinguish stateful VirtIO Video from stateless Apple AVD.

## Basic flow

```rust,ignore
use scarlet_video_client::{ScarletVideoDecoder, VideoFormat};

let mut decoder = ScarletVideoDecoder::open()?;
decoder.configure(VideoFormat::H264)?;
decoder.submit(annex_b_access_unit, presentation_time_us)?;
if let Some(frame) = decoder.dequeue()? {
    render_nv12(frame.width(), frame.height(), frame.payload());
}
```

Only one decode may be pending for a decoder session. `dequeue()` waits for the
submitted request to complete. The borrowed frame remains valid until it is
dropped; use `DecodedFrame::try_into_owned()` when a frame must outlive the
decoder borrow or cross a thread boundary.

## Features

- `std` is enabled by default and uses Rust's Scarlet `std` runtime together
  with the runtime-neutral `scarlet-os` handle API.
- `legacy-scarlet-std` preserves existing `no_std` applications. Select it
  with default features disabled; it is mutually exclusive with `std`.
- `h264-stateful-hw` and `av1-stateful-hw` are enabled by default.
- `hevc-stateful-hw` and `vp9-stateful-hw` enable the corresponding stateful
  paths.
- `h264-stateless-hw` and `vp9-stateless-hw` enable userspace request building
  through `scarlet-codecs`.

H.264/AVC may be patent-encumbered in some jurisdictions, so the stateless
H.264 parser remains an explicit opt-in.
