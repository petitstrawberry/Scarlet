# Slint Backend for Scarlet Window Server

## Overview

This document describes the architectural design and current implementation status of integrating the Slint UI library with the Scarlet Window Server (SWS).

The goal is to enable Slint applications to run on Scarlet OS by providing a Slint platform backend that:

- Renders Slint's software renderer output into SWS shared memory buffers.
- Translates SWS input events into Slint `WindowEvent`s.
- Optionally reserves a small client-side title bar region (CSD-like) and translates coordinates accordingly.

## Architecture

### High-Level Design

```
┌────────────────────────────────────────────────────────────────┐
│                    Slint Application Layer                      │
│  - User-defined .slint UI components                            │
│  - Application logic in Rust                                    │
└────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌────────────────────────────────────────────────────────────────┐
│                 Slint-Scarlet Backend Layer                     │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  ScarletPlatform (implements slint::platform::Platform) │  │
│  │  - Manages connection to SWS                             │  │
│  │  - Creates window adapters                               │  │
│  │  - Handles event loop integration                        │  │
│  └──────────────────────────────────────────────────────────┘  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  ScarletWindowAdapter (WindowAdapter trait)              │  │
│  │  - Owns SWS Surface for the window                       │  │
│  │  - Manages Slint software renderer                       │  │
│  │  - Handles window size and position                      │  │
│  └──────────────────────────────────────────────────────────┘  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  ScarletRenderer (implements slint::platform::Renderer) │  │
│  │  - Wraps Slint SoftwareRenderer                          │  │
│  │  - Manages pixel buffer for rendering                    │  │
│  │  - Handles buffer format conversion if needed            │  │
│  └──────────────────────────────────────────────────────────┘  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  EventMapper                                             │  │
│  │  - Translates SWS events to Slint events                 │  │
│  │  - Handles coordinate transformation                     │  │
│  │  - Routes events to appropriate windows                  │  │
│  └──────────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌────────────────────────────────────────────────────────────────┐
│               Scarlet Window Server (SWS) Layer                 │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  sws-client                                              │  │
│  │  - Connection to window server                           │  │
│  │  - Surface management                                    │  │
│  │  - Shared memory buffers                                 │  │
│  │  - Event delivery from server                            │  │
│  └──────────────────────────────────────────────────────────┘  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  sws_protocol                                            │  │
│  │  - IPC protocol definitions                              │  │
│  │  - Message serialization                                 │  │
│  └──────────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────┘
```

### Integration Points

#### 1. Platform Trait Implementation

The `slint::platform::Platform` trait must be implemented to provide:

- **`create_window_adapter()`**: Factory method for creating window adapters
- **`duration_since_start()`**: System time for animations
- Optional event loop integration

#### 2. WindowAdapter Trait Implementation

The `slint::platform::WindowAdapter` trait provides:

- **`window()`**: Reference to the Slint Window object
- **`size()`**: Physical window dimensions
- **`renderer()`**: Reference to the renderer implementation
- **`request_redraw()`**: Trigger redraw requests

#### 3. Renderer Implementation

Uses Slint's software renderer (`SoftwareRenderer`):

- Renders to a pixel buffer
- Supports various pixel formats (RGB565, RGBA8888, etc.)
- Can be configured for line-by-line rendering for embedded systems

### Window Decoration Strategy

Two possible approaches:

#### Option A: Pure Slint Windows

- Slint renders everything including decorations
- Simpler integration
- Less consistent with native Scarlet UI look

#### Option B: Hybrid Approach (Recommended)

- ScarletUI renders window decorations (title bar, borders, buttons)
- Slint renders into a content region within the window
- More complex but provides consistent look with other Scarlet apps

#### Option C: ScarletUI Decorations + Slint Content (Current)

- The `slint-scarlet` backend reuses ScarletUI's `Window` decoration logic to draw the title bar + border.
- Slint renders only into the content area below the title bar.
- Pointer input coordinates are translated ($y \leftarrow y - h_{titlebar}$) before dispatching to Slint.
- Title bar interactions are handled by ScarletUI (hover/pressed, buttons).
    - Drag requests compositor-level interactive move.
    - Minimize/maximize/close are forwarded to SWS window control requests.

```
┌─────────────────────────────────────────────────────┐
│  ScarletUI Title Bar        [_] [□] [×]  ← ScarletUI│
├─────────────────────────────────────────────────────┤
│                                                      │
│                                                      │
│          Slint Rendered Content Area                │
│                  ↑ Slint                             │
│                                                      │
│                                                      │
└─────────────────────────────────────────────────────┘
```

## Buffer Management

### Shared Memory Approach

1. **Buffer Allocation**:
   - Allocate shared memory region for pixel buffer
   - Size: `width * height * bytes_per_pixel`
   - Format: BGRA8888 (matches SWS default)

2. **Rendering Pipeline (current)**:
   ```
    Slint Render (SoftwareRenderer) → CPU pixel buffer → Copy/convert to SWS SHM → Commit to SWS
   ```

3. **Double Buffering (future)**:
    The current implementation commits directly to the SWS buffer. If tearing becomes an issue, introducing a front/back buffer scheme (or server-provided double buffering) is a likely next step.

### Buffer Format Considerations

SWS uses BGRA8888 format:
- Blue: byte 0
- Green: byte 1
- Red: byte 2
- Alpha: byte 3

Slint SoftwareRenderer supports multiple formats including BGRA8888, so no conversion needed.

## Event Handling

### Event Flow

```
Hardware Input → Kernel → Window Server → SWS Client → EventMapper → Slint Window
```

### Event Types to Map

1. **Mouse Events**:
    - Absolute movement: `EV_ABS/ABS_X`, `EV_ABS/ABS_Y`, flushed on `EV_SYN`
    - Button press/release: `EV_KEY/BTN_LEFT` ($value \ne 0$ press, $value = 0$ release)
    - Coordinate transformation when the optional title bar is enabled

2. **Keyboard Events**:
   - Key press/release
   - Modifiers (Shift, Ctrl, Alt)
   - Text input

3. **Window Events**:
   - Resize
   - Focus change
   - Close request

### Coordinate Transformation

When using ScarletUI decorations, mouse coordinates must be transformed:

```rust
fn transform_coordinates(event_x: i32, event_y: i32, titlebar_height: u32) -> (i32, i32) {
    (event_x, event_y - titlebar_height as i32)
}
```

In the current implementation (ScarletUI decorations), events that are handled by ScarletUI in the title bar region are not forwarded to Slint; instead they trigger the corresponding SWS window control requests.

## Implementation Challenges

### 1. no_std Environment

Slint supports no_std but requires:
- Global allocator (✓ Scarlet has this)
- Cargo features: `unsafe-single-threaded`, `renderer-software`, `libm`, `compat-1-2`
- No threading support
- Custom Platform implementation

### 2. Event Loop Integration

Slint expects a platform-provided event loop. Options:

**Option A**: Use Slint's `run_event_loop()`:
- Slint drives the main loop
- Platform polls SWS for events
- Simple but less flexible

**Option B**: Manual event dispatch:
- Application controls main loop
- Explicitly dispatch events to Slint
- More control, better integration with ScarletUI apps

### 3. Build System Integration

Need to:
- Add slint-scarlet crate to workspace
- Update Makefile.toml for building with correct target
- Handle Slint build dependencies (slint-build crate)
- Embed Slint resources (fonts, images)

### 4. Resource Embedding

Slint resources (fonts, images) must be:
- Embedded at build time using `slint-build`
- Configured for software renderer
- Optimized for size in no_std environment

## Implementation Phases

### Phase 1: Basic Backend Structure ✓

- [x] Create slint-scarlet crate structure
- [x] Define Platform trait implementation
- [x] Define WindowAdapter trait implementation
- [x] Define Renderer wrapper

### Phase 2: Minimal Working Implementation

- [x] Implement rendering to SWS shared memory buffer
- [x] Handle window creation and destruction
- [x] Basic mouse event mapping (absolute pointer + left button)
- [x] Simple test application (`slint_demo`)

### Phase 3: Event Handling

- [ ] Complete event mapper implementation (wheel, multi-button, etc.)
- [x] Coordinate transformation for an optional title bar region
- [ ] Focus management
- [ ] Keyboard input handling

### Phase 4: Window Decorations

- [x] Integrate ScarletUI title bar + border (decorations)
- [x] Embed Slint content below a title bar region
- [x] Handle resize with title bar enabled (content size = surface size - title bar height)
- [ ] Window controls (close, minimize, maximize)

### Phase 5: Optimization

- [ ] Shared memory optimization
- [ ] Double buffering
- [ ] Dirty region tracking
- [ ] Minimize redraws

### Phase 6: Sample Applications

- [ ] Hello World example
- [ ] Widget showcase
- [ ] Interactive demo
- [ ] Performance test

## Usage Example

```rust
#![no_std]
#![no_main]

extern crate scarlet_std as std;

use slint_scarlet;

slint::slint! {
    export component MainWindow inherits Window {
        width: 400px;
        height: 300px;
        
        VerticalBox {
            Text {
                text: "Hello from Slint on Scarlet!";
                font-size: 24px;
            }
            
            Button {
                text: "Click Me";
                clicked => {
                    debug("Button clicked!");
                }
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    // Initialize Slint-Scarlet backend
    slint_scarlet::init().expect("Failed to initialize Slint backend");

    // Optional: enable/disable the built-in client-side title bar
    // slint_scarlet::set_use_csd_titlebar(true);
    
    // Create and show window
    let window = MainWindow::new().unwrap();
    window.run().unwrap();
    
    0
}
```

## Alternative: ScarletUI-Only Approach

Given the complexity of Slint integration, an alternative is to enhance ScarletUI itself:

### Advantages:
- Fully integrated with Scarlet OS
- Lighter weight
- No external dependencies
- Complete control over rendering

### Enhancement Ideas:
1. More widget types (ComboBox, ListView, TreeView)
2. Layout managers (Grid, Flow)
3. Theming system
4. Animation support
5. Custom drawing APIs

## Conclusion

Integrating Slint with Scarlet OS is technically feasible but requires:

1. Careful handling of no_std constraints
2. Custom Platform and WindowAdapter implementations  
3. Proper event loop integration
4. Buffer management for efficient rendering
5. Coordinate transformation for hybrid window decorations

The recommended approach is the hybrid model where ScarletUI provides window decorations and Slint renders content, providing both consistency with the native look and the power of Slint's UI framework.

For immediate needs, enhancing ScarletUI directly may be more practical and maintainable.

## References

- [Slint MCU Documentation](https://docs.slint.dev/latest/docs/rust/slint/docs/mcu/)
- [Slint Platform Trait](https://docs.slint.dev/latest/docs/rust/slint/platform/trait.Platform)
- [Scarlet Window Server Protocol](sws_ipc_protocol.md)
- [ScarletUI Framework Design](scarlet_ui_framework.md)
