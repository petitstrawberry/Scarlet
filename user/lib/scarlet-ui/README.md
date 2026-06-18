# ScarletUI

ScarletUI is a declarative UI framework for Scarlet applications. It is designed
to run the same application code on Scarlet OS through SWS and on native desktop
platforms through platform features such as winit.

The application API is platform-agnostic: application code declares scenes and
calls `app.run()`. Platform selection is a crate feature decision, not an app
runtime decision.

## Features

- **Declarative views**: compose UI with `View` values and container macros.
- **State management**: `State<T>` drives rebuild, layout, paint, and composite.
- **Scene model**: `Application::scenes()` declares top-level windows.
- **Multi-window runtime**: each opened window owns its own rendering pipeline.
- **Platform abstraction**: SWS and native desktop platforms share the same runner.
- **Text input**: keyboard, character input, IME preedit/commit, and focus sync.

## Platform Features

`scarlet-ui` currently has one selected platform feature per build.

| Feature | Target | Notes |
|---------|--------|-------|
| `platform-sws` | Scarlet OS / SWS | Default platform feature. Uses `sws-client` and `sws-protocol`. |
| `platform-winit` | native desktop | Uses `winit` + `softbuffer`; requires `std`. |

The default feature set is:

```toml
default = ["std", "platform-sws"]
```

For native desktop builds, depend on ScarletUI with `platform-winit`:

```toml
[target.'cfg(not(target_os = "scarlet"))'.dependencies]
scarlet-ui = { path = "../lib/scarlet-ui", default-features = false, features = ["std", "platform-winit"] }
```

For Scarlet OS builds, use `platform-sws`:

```toml
[target.'cfg(target_os = "scarlet")'.dependencies]
scarlet-ui = { path = "../lib/scarlet-ui", default-features = false, features = ["std", "platform-sws"] }
```

`platform-sws` and `platform-winit` are mutually exclusive. `std` and
`legacy-scarlet-std` are also mutually exclusive.

## Basic Application

```rust
use scarlet_ui::prelude::*;
use scarlet_ui::{hstack, vstack};
use scarlet_ui_macros::View;

#[derive(View, Clone, Default)]
struct CounterApp {
    count: State<i32>,
}

impl CounterApp {
    fn content(&self) -> impl View {
        vstack! {
            Text::new("Counter").font_size(24.0),
            Text::new(format!("Count: {}", self.count.get())),
            hstack! {
                Button::new("-").on_click({
                    let count = self.count.clone();
                    move || count.set(count.get() - 1)
                }),
                Button::new("+").on_click({
                    let count = self.count.clone();
                    move || count.set(count.get() + 1)
                }),
            },
        }
        .spacing(12.0)
        .padding(EdgeInsets::all(16.0))
    }
}

impl Application for CounterApp {
    fn scenes(&self) -> impl Scene {
        WindowGroup::new(
            "main",
            Window::new("Counter", self.content())
                .app_id("org.scarlet-os.counter")
                .size(Size::new(400.0, 300.0)),
        )
    }
}

fn main() -> scarlet_ui::Result<()> {
    let mut app = CounterApp::default();
    app.run()
}
```

## Application Model

`Application::scenes()` is the application UI entry point. `body()` is still used
by `#[derive(View)]` for reusable view components, but it is not the top-level
application entry point.

Each scene declaration produces a top-level `Window`. At runtime, ScarletUI
creates one `WindowSlot` per opened window:

```text
Application::scenes()
  -> Scene declarations
  -> ApplicationRunner<SelectedPlatform>
  -> WindowSlot { WindowId, PipelineId, RenderingPipeline, PlatformWindow }
```

Application code should not choose or name a platform implementation. It imports the
normal ScarletUI prelude and calls `app.run()`.

## Platform Integration

Platform code lives behind `PlatformBackend` and `PlatformWindow`. These are
internal runner boundaries for creating windows, polling events, presenting
buffers, routing text input, and handling window controls.

Most applications should not construct a `PlatformWindow` directly. Use
`Window`, `WindowGroup`, `open_window`, `dismiss_window`, and `app.run()`.

SWS-specific applications may use lifecycle hooks and downcast the
`dyn PlatformWindow` only when they intentionally depend on SWS-specific
capabilities.

## Common Views

- `Text`
- `Button`
- `TextField`
- `Toggle`
- `Slider`
- `Select`
- `CanvasView`
- `Image`
- `Rectangle`
- `Spacer`
- `Divider`
- `Window`
- `VStack`, `HStack`, `ZStack`
- `NavigationView`, `NavigationLink`

## Development Commands

Run ScarletUI tests:

```bash
cargo test --offline --lib --tests
```

Check the std smoke app for native desktop:

```bash
cd ../../std-bin
cargo check --offline --target aarch64-apple-darwin --bin ui_smoke
```

Check the same app for Scarlet targets:

```bash
cd ../../std-bin
cargo check --offline --target riscv64gc-unknown-scarlet --bin ui_smoke
cargo check --offline --target aarch64-unknown-scarlet --bin ui_smoke
```

## Design Documents

- `docs/graphics/scarletui/design.md`
- `docs/graphics/scarletui/api.md`
