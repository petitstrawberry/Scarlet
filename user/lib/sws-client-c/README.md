# SWS C client

`libsws_client_c.so` exposes SWS windows, input and shared GPU-image registration
through `include/sws_client.h`. All consumers in one process must dynamically
link the same library; it owns one connection with independent input and GPU
lifecycle queues. Fullscreen waits for compositor confirmation and acknowledges
the configured backing extent. A successful GPU commit only means the message
was sent: reuse requires the exact buffer identity and commit serial in a release
event. Rejection or backend loss must be handled by the caller.

Build on an AArch64 musl system with Rust 1.88 or newer:

```sh
RUSTFLAGS='-C target-feature=-crt-static' cargo build --release
cc -O3 -Iinclude examples/display.c -Ltarget/release -lsws_client_c \
  -Wl,-rpath,/usr/lib -o sws-display
```

Install the library and its `libgcc_s.so.1` dependency in `/usr/lib` **inside the
Linux ABI filesystem view**, and `sws-display` in that view's `/bin`. In the
default Scarlet Environment the backing directory is
`/systems/linux-aarch64/usr/lib`. The view's musl interpreter and libc must be
installed as well. `/dev` and `/tmp` are shared by the default Environment.
`LD_PRELOAD` is not needed.

The library uses `scarlet-sys`' explicit native syscall transport for SWS object
handles. These handles are not Linux file descriptors; libc and Rust standard
library calls continue to use the process's Linux ABI. The kernel must include
the same native-call transport contract. Native Scarlet applications continue
to use their original syscall numbers.

Consumers maintained separately include the [SDL2 SWS video driver](https://github.com/petitstrawberry/scarlet-sdl2-sws),
the [vkQuake2 SWS adapter](https://github.com/petitstrawberry/vkquake2-scarlet/tree/master/scarlet),
and the [SuperTuxKart test recipe](https://github.com/petitstrawberry/stk-scarlet/tree/master/scarlet).
