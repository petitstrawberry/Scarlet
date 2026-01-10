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
use scarlet_ui::{Application, Window, VStack, Label, Button};

let mut app = Application::new().expect("Failed to connect to SWS");

let window = Window::new("UI Demo", 400, 300)
    .content(
        VStack::new()
            .child(Label::new("Hello"))
            .child(Button::new("Click", || {})),
    );

app.add_window(window).expect("Failed to create window");
app.run();
```

## Demo

See `ui_demo` at `user/bin/src/ui_demo.rs` for a working end-to-end example using:

- `Application` for connection/event loop
- `Window` for CSD and hit testing (close button)
- `Canvas` for simple drawing

## Event model (current)

- Application code does not manually poll events. `Application` owns the event loop and dispatches
    input through the root `Window` view.
- Input is delivered as a `scarlet-ui` `Event` and propagated through the view tree using a
    capture + bubble model.
- Redraw is demand-driven: views mark themselves dirty (via `needs_draw`) and the application
    commits only when a window needs drawing.
- Current limitation: window targeting is conservative (input is not yet fully routed per-window).
