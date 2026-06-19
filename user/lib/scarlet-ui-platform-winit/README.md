# scarlet-ui-platform-winit

Native desktop backend for ScarletUI preview and host development.

```text
scarlet-ui-platform-winit
  |-- scarlet-ui-core
  |-- winit
  `-- softbuffer
```

This crate implements `PlatformBackend` and `PlatformWindow` from
`scarlet-ui-core` for desktop windows. Applications normally enable it through
the `scarlet-ui` facade:

```toml
scarlet-ui = { path = "../lib/scarlet-ui", default-features = false, features = ["std", "platform-winit"] }
```

Useful check:

```bash
cargo check
```
