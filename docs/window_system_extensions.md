# Window System Extensions

This document describes the extended features of the Scarlet Window System (SWS).

## Overview

The window system has been extended with the following features:
1. **Window Minimize/Maximize**: Hide windows or expand them to fullscreen
2. **Window Types**: Control Z-order with special window types (Desktop, Normal, Taskbar, AlwaysOnTop)
3. **Alpha Blending**: Support for transparent and semi-transparent windows
4. **Dirty Rectangle Optimization**: Efficient partial screen updates (already implemented)

## Window Minimize/Maximize

Windows can be minimized (hidden but kept in the window list) or maximized (expanded to fill the screen).

### Protocol Messages

- **MINIMIZE_WINDOW** (ID: 17)
  - Payload: `window_id: u32` (4 bytes)
  - Hides the window from display but keeps it in the window list
  
- **MAXIMIZE_WINDOW** (ID: 18)
  - Payload: `window_id: u32` (4 bytes)
  - Expands the window to fill the entire screen
  - Saves the current position and size for restoration
  
- **RESTORE_WINDOW** (ID: 19)
  - Payload: `window_id: u32` (4 bytes)
  - Restores a minimized window (makes it visible) or a maximized window (restores saved geometry)

### State Management

Windows have three state flags:
- `visible`: bool - Whether the window is displayed
- `minimized`: bool - Whether the window is minimized
- `maximized`: bool - Whether the window is maximized
- `saved_geometry`: Option<(x, y, width, height)> - Saved position/size before maximize

### Usage Example

```rust
use sws_protocol::{client_msg, payload_minimize_window, payload_maximize_window, payload_restore_window};

// Minimize a window
let payload = payload_minimize_window(window_id);
send_message(client_msg::MINIMIZE_WINDOW, &payload);

// Maximize a window
let payload = payload_maximize_window(window_id);
send_message(client_msg::MAXIMIZE_WINDOW, &payload);

// Restore a window
let payload = payload_restore_window(window_id);
send_message(client_msg::RESTORE_WINDOW, &payload);
```

## Window Types

Windows can be assigned different types that control their Z-order behavior.

### Window Types

```rust
pub enum WindowType {
    Normal,      // Standard application windows (ID: 0)
    AlwaysOnTop, // Always stays above normal windows (ID: 1)
    Taskbar,     // Taskbar/panel windows (ID: 2)
    Desktop,     // Desktop background windows (ID: 3)
}
```

### Z-Order Hierarchy

Windows are stacked in the following order (bottom to top):
1. **Desktop** - Background layer (wallpaper, icons)
2. **Normal** - Standard application windows
3. **Taskbar** - Persistent UI elements (taskbar, panels)
4. **AlwaysOnTop** - Windows that stay above everything else

Within each type, windows maintain their relative order based on focus and raise operations.

### Protocol Message

- **SET_WINDOW_TYPE** (ID: 20)
  - Payload: `window_id: u32`, `window_type: u32` (8 bytes)
  - Sets the window type (0=Normal, 1=AlwaysOnTop, 2=Taskbar, 3=Desktop)

### Usage Example

```rust
use sws_protocol::{client_msg, payload_set_window_type};

// Set window to always stay on top
let payload = payload_set_window_type(window_id, 1); // 1 = AlwaysOnTop
send_message(client_msg::SET_WINDOW_TYPE, &payload);

// Set window as desktop background
let payload = payload_set_window_type(window_id, 3); // 3 = Desktop
send_message(client_msg::SET_WINDOW_TYPE, &payload);
```

## Alpha Blending

Windows support transparency through per-window opacity control and per-pixel alpha blending.

### Opacity Control

Each window has an `opacity` field (0.0 = fully transparent, 1.0 = fully opaque).

### Protocol Message

- **SET_WINDOW_OPACITY** (ID: 21)
  - Payload: `window_id: u32`, `opacity: u8` (5 bytes)
  - Sets the window opacity (0-255, where 255 is fully opaque)

### Blending Formula

For windows with opacity < 1.0, the compositor applies alpha blending:

```
effective_alpha = pixel_alpha * window_opacity
output = (src * effective_alpha + dst * (255 - effective_alpha)) / 255
```

Where:
- `src` = source pixel color (BGRA format)
- `dst` = destination (background) pixel color
- `pixel_alpha` = alpha channel from window buffer
- `window_opacity` = per-window opacity setting

### Performance Optimization

- Windows with `opacity = 1.0` skip blending and use direct copy for performance
- Blending uses integer arithmetic to avoid floating-point operations

### Usage Example

```rust
use sws_protocol::{client_msg, payload_set_window_opacity};

// Make window 50% transparent
let payload = payload_set_window_opacity(window_id, 128); // 128/255 ≈ 0.5
send_message(client_msg::SET_WINDOW_OPACITY, &payload);

// Make window fully opaque
let payload = payload_set_window_opacity(window_id, 255);
send_message(client_msg::SET_WINDOW_OPACITY, &payload);
```

## Dirty Rectangle Optimization

The compositor uses dirty rectangle tracking to minimize redraw operations.

### How It Works

1. **Damage Tracking**: Each window operation marks affected screen regions as "dirty"
2. **Incremental Redraw**: Only dirty regions are redrawn instead of the entire screen
3. **Damage Coalescing**: Adjacent dirty regions are merged to reduce overhead

### Configuration

The feature is controlled by the `ENABLE_DIRTY_RECT` constant in `compositor.rs`:

```rust
const ENABLE_DIRTY_RECT: bool = true;
```

Set to `false` to disable and force full-screen redraws (useful for debugging).

### Affected Operations

All window operations properly track damage:
- Window creation/destruction
- Window movement/resize
- Minimize/maximize/restore
- Opacity changes
- Buffer updates from clients
- Cursor movement

## Implementation Details

### Window Structure

```rust
pub struct Window {
    pub id: WindowId,
    // ... other fields ...
    
    // New fields for extensions
    pub window_type: WindowType,      // Z-order control
    pub minimized: bool,               // Hidden state
    pub maximized: bool,               // Fullscreen state
    pub saved_geometry: Option<(i32, i32, u32, u32)>, // For restore
    pub opacity: f32,                  // 0.0 - 1.0
}
```

### Window Manager Methods

```rust
impl WindowManager {
    pub fn minimize_window(&mut self, id: WindowId) -> bool;
    pub fn maximize_window(&mut self, id: WindowId, screen_width: u32, screen_height: u32) -> bool;
    pub fn restore_window(&mut self, id: WindowId) -> bool;
    pub fn set_window_type(&mut self, id: WindowId, window_type: WindowType) -> bool;
    pub fn set_window_opacity(&mut self, id: WindowId, opacity: f32) -> bool;
    pub fn raise_to_top_with_type(&mut self, id: WindowId);
}
```

### Compositor Integration

The compositor handles all window operations through IPC events:

```rust
match event {
    IpcEvent::MinimizeWindow { window_id } => { /* ... */ }
    IpcEvent::MaximizeWindow { window_id } => { /* ... */ }
    IpcEvent::RestoreWindow { window_id } => { /* ... */ }
    IpcEvent::SetWindowType { window_id, window_type } => { /* ... */ }
    IpcEvent::SetWindowOpacity { window_id, opacity } => { /* ... */ }
}
```

## Testing

All features have been tested with:
- **RISC-V64**: 477 tests passed
- **AArch64**: 432 tests passed

No regressions were introduced by these changes.

## Future Enhancements

Potential improvements for future development:

1. **UI Integration**:
   - Add minimize/maximize buttons to window decorations
   - Window manager menu for operations
   - Keyboard shortcuts (e.g., Alt+F9 for minimize)

2. **Visual Effects**:
   - Smooth animations for minimize/maximize
   - Window previews for minimized windows
   - Shadow effects for depth perception

3. **Advanced Opacity**:
   - Blur effects for transparent windows
   - Per-region opacity masks
   - Dynamic opacity based on focus

4. **Performance**:
   - Hardware-accelerated alpha blending
   - Cached transparency layers
   - Smarter dirty region tracking

## References

- Window System Protocol: `user/lib/sws_protocol/src/lib.rs`
- Window Manager: `user/bin/src/sws/window.rs`
- Compositor: `user/bin/src/sws/compositor.rs`
- IPC Server: `user/bin/src/sws/ipc.rs`
