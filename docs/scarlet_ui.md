# scarlet-ui (High-level UI Toolkit)

`scarlet-ui` is the high-level UI toolkit for Scarlet native applications.

- Location: `user/lib/scarlet-ui`
- Depends on: `sws-client` (crate `sws_client`) for low-level communication

This crate is responsible for:

- UI-facing API (application, windows)
- Client-side decorations (CSD) drawing
- Widgets / layout primitives (work-in-progress)

## Layering

- `sws_protocol`: message formats + parsing/serialization
- `sws-client`: connection + surfaces + event dispatch
- `scarlet-ui`: window/UI abstraction + drawing helpers

## Minimal example

```rust
use scarlet_ui::{Application, Color, Rect};

let mut app = Application::new()?;
let mut window = app.create_window("UI Demo", 400, 300)?;
let surface_id = window.surface_id();

{
    let mut canvas = window.canvas();
    canvas.fill_rect(Rect::new(0, 0, 400, 300), Color::WHITE);
}

app.commit(surface_id)?;

loop {
    while let Some((win_id, event)) = app.poll_event() {
        let _ = (win_id, event);
    }
    let _ = app.commit(surface_id);
}
```

## Demo

See `ui_demo` at `user/bin/src/ui_demo.rs` for a working end-to-end example using:

- `Application` for connection/event loop
- `Window` for CSD and hit testing (close button)
- `Canvas` for simple drawing

## Event model (current)

- `Application::poll_event()` returns `(surface_id, Event)`.
- Mouse absolute coordinates are delivered as separate X/Y updates (matching evdev semantics).
- Window targeting is currently conservative and may be refined as multi-window routing matures.
