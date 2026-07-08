# scarlet-codecs

Userspace codec request builders for Scarlet stateless video decode.

This crate keeps bitstream parsing and request construction in userspace. Kernel
video backends receive prepared request parameters and lower them to hardware
command streams.

## Features

- `h264` is disabled by default.
- `vp9` is disabled by default while the AVD path is still experimental.

H.264/AVC may be patent-encumbered in some jurisdictions. The `h264` feature is
therefore an explicit opt-in.
