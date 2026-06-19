# scarlet-ui-platform-sws

SWS backend for ScarletUI.

```text
scarlet-ui-platform-sws
  |-- scarlet-ui-core
  |-- sws-client
  `-- sws-protocol
```

This crate implements `PlatformBackend` and `PlatformWindow` from
`scarlet-ui-core` for Scarlet Window Server. Applications normally enable it
through the `scarlet-ui` facade:

```toml
scarlet-ui = { path = "../lib/scarlet-ui", default-features = false, features = ["legacy-scarlet-std", "platform-sws"] }
```

Useful check:

```bash
cargo check --no-default-features --features legacy-scarlet-std
```
