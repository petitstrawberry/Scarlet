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

## Typical control flow

`sws-client` is a low-level building block. Most native applications should prefer
`scarlet-ui`, which owns the event loop and dispatches input into a view tree.

For low-level/advanced usage (or when implementing a toolkit), the typical flow is:

1. Connect once per process.
2. Create a surface (window).
3. Draw by writing to the surface buffer.
4. Notify damage via `commit()`.
5. In your main loop: `dispatch()` then consume events.

## Minimal example

```rust
use sws_client::Connection;

let mut conn = Connection::connect_default()?;
let surface_id = conn.create_surface(400, 300)?;

// draw
{
    let surface = conn.surface_mut(surface_id).unwrap();
    surface.with_buffer(|buf, w, h| {
        let _ = (buf, w, h);
        // write pixels (BGRA)
    });
}

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

## Notes / current limitations

- The client socket is configured to non-blocking mode once at connection time.
- Integration with a `poll`/`epoll`-style multiplexer is planned; currently `scarlet_std::socket::Socket` does not expose a stable POSIX-like fd API.
