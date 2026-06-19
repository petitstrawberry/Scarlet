# scarlet-ui-core

`scarlet-ui-core` contains the ScarletUI implementation shared by all platforms.
It owns views, elements, scenes, rendering pipelines, state, preview core, and
the platform traits.

This crate intentionally does not depend on SWS or desktop window libraries.
Backends implement these public traits instead:

```text
PlatformBackend
  -> create_window(WindowCreateRequest)
  -> Box<dyn PlatformWindow>

PlatformWindow
  -> poll_event()
  -> present_with_damage()
  -> sync_text_input()
  -> window controls
```

Normal applications should depend on `scarlet-ui`, not this crate directly.
`scarlet-ui` re-exports the app-facing API and wires the selected backend.

Useful checks:

```bash
cargo check
cargo check --no-default-features --features legacy-scarlet-std
cargo test
```
