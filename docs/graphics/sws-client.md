# sws-client (SWS Client Library)

`sws-client` is the low-level client library for communicating with the Scarlet Window Server (SWS).

- Location: `user/lib/sws-client`
- Crate name: `sws_client`

This library is intentionally **not** a widget toolkit. It is the "Wayland client"-like layer that owns:

- The socket connection to SWS
- Non-blocking reads for event-driven input
- Window/surface creation and lifecycle
- Shared-memory handle reception and buffer mapping
- Converting protocol messages into typed events

## Design goals

- Event-driven control flow: `dispatch()` reads everything available (non-blocking), then the app consumes events.
- Zero-copy drawing: pixel buffers live in shared memory; clients write directly.
- Clear layering: protocol definition (`sws_protocol`) is separate from I/O/state (`sws-client`).
- Correct mixed traffic handling: synchronous replies are matched by
  `request_id`, while asynchronous server events are queued for later dispatch.

## Typical control flow

`sws-client` is a low-level building block. Most native applications should prefer
`scarlet-ui`, which owns the event loop and dispatches input into a view tree.

For low-level/advanced usage (or when implementing a toolkit), the typical flow is:

1. Connect once per process.
2. Create a surface (window).
3. Draw by writing to the surface buffer.
4. Notify damage via `commit()`.
5. In your main loop: `dispatch()` then consume events.

## Request routing

SWS uses a single socket for both synchronous replies and asynchronous server
events. `sws-client` therefore does not treat "the next server frame with the
right message type" as a reply.

For APIs that need a synchronous server answer, such as `create_surface`,
`resize_window`, `get_screen_size`, `get_output_scale`, `get_window_list`,
`create_text_input_context`, and IME registration/query calls, `sws-client`:

1. Allocates a non-zero per-connection `request_id`.
2. Sends the request with that `request_id`.
3. Reads frames until it receives a frame with `IS_RESPONSE` set and the same
   `request_id`.
4. Queues any asynchronous events encountered while waiting.

`dispatch()` ignores response frames and only converts asynchronous server
messages into client events. This keeps resize/configure/input broadcasts from
being misinterpreted as replies to synchronous calls.

Shared SGFX frame commits are the deliberate exception to synchronous request
routing. `commit_sgfx_frame` serializes a frame with `request_id = 0` and returns
without waiting for the compositor. The caller must retain the exact
`(window_id, buffer_id, generation, compositor_epoch, commit_serial)` token
until `SgfxBufferReleased` or `SgfxFrameRejected` is dispatched. Registration
and destruction remain ordinary non-zero-ID requests. This lets two shared
images pipeline rendering and presentation without allowing either image to be
overwritten while SWS can still sample it.

## Minimal example

```rust
use sws_client::Connection;

let conn = Connection::connect_default()?;
let surface_id = conn.create_surface(400, 300)?;

// draw
conn.with_surface_mut(surface_id, |surface| {
    surface.with_buffer(|buf, w, h| {
        let _ = (buf, w, h);
        // write pixels (BGRA)
    });
})
.ok_or(sws_client::Error::SurfaceNotFound)?;

conn.commit(surface_id)?;

loop {
    conn.dispatch()?;
    while let Some(ev) = conn.poll_event() {
        // handle events
        let _ = ev;
    }
}

```

## Window movement

The protocol provides two window movement messages with different intent:

- Interactive user-driven movement: `request_move_window(surface_id)` sends
    `REQUEST_MOVE_WINDOW`.
    - Recommended usage: send once when the user starts dragging a title bar.
    - The compositor should own the drag state and update the window position based
        on global pointer motion.

- Programmatic movement: `move_window(surface_id, x, y)` sends `MOVE_WINDOW`.
    - Recommended usage: one-shot reposition (e.g. centering a window).
    - Not recommended to stream continuously for interactive dragging.

Related:

- Parent/child relationship: `set_window_parent(surface_id, parent_surface_id)` sends
    `SET_WINDOW_PARENT`.
    - Recommended usage: mark dialogs/popups as transient so the compositor can keep them
        stacked above their parent and move them together during interactive drags.

## Maximize and fullscreen

These are separate presentation states:

- `maximize_window(surface_id)` uses the compositor workarea and leaves shell UI
  visible. `restore_window(surface_id)` leaves maximized/minimized state.
- `set_fullscreen(surface_id)` occupies the complete primary output and covers
  shell UI. `unset_fullscreen(surface_id)` leaves only fullscreen state.
- `Event::SurfaceStateChanged` reports compositor-confirmed flags from
  `sws_client::window_state`. `MAXIMIZED` and `FULLSCREEN` may both be set; in
  that case unsetting fullscreen returns to maximized state.
- `Event::SurfaceConfigure` supplies the authoritative physical buffer size for
  both state transitions. Clients should resize and redraw after receiving it.

The bundled ScarletUI `ui-demo` provides an interactive integration path: its
“Enter Fullscreen” button calls these APIs through `SWSPlatformWindow` and
removes client-side decorations until fullscreen is left.

## Notes / current limitations

- The client socket is configured to non-blocking mode once at connection time.
- Integration with a `poll`/`epoll`-style multiplexer is planned; currently `scarlet_std::socket::Socket` does not expose a stable POSIX-like fd API.
