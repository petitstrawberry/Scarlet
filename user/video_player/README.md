# video_player

Scarlet-native media player.

This crate is built separately from `user/bin` so codec support can be selected
per build or per filesystem bundle.

## Default Features

The crate default is:

```toml
default = ["av1-stateful-hw", "h264-stateful-hw", "mp4-aac"]
```

That enables MP4/AAC playback, stateful AV1 hardware decode, and the stateful
H.264 hardware decoder path. For H.264 decode, the default path is stateful
hardware decode only.

At runtime, `video_player` also defaults to hardware decode. Pass
`--software` (or `--swdec`) to explicitly select the software path.

## Opt-In Codec Paths

- `h264-stateless-hw` enables userspace H.264 request building through
  `scarlet-codecs/h264`.
- `h264-sw` enables the software H.264 decoder dependency.
- `vp9-stateless-hw` enables userspace VP9 request building through
  `scarlet-codecs/vp9`.
- `vp9-stateful-hw` and `hevc-stateful-hw` reserve stateful hardware decode
  paths.

H.264/AVC may be patent-encumbered in some jurisdictions. Stateless and
software H.264 support are therefore explicit opt-ins.

Enabling a codec feature does not grant or provide any codec patent licenses.
Distributors and users are responsible for supplying any licenses or permissions
required in their jurisdiction.
