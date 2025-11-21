# Scarlet Graphics Core Abstraction

This document describes Scarlet's OS-independent graphics abstraction layer.

## Architecture Overview

Scarlet's graphics subsystem provides a minimal, OS-independent interface for framebuffer operations while supporting advanced features through capability-based extensions.

```
┌─────────────────────────────────────────┐
│      OS ABI Layers (Linux, etc.)        │
└──────────────────┬──────────────────────┘
                   │ Trait method calls
                   ▼
┌─────────────────────────────────────────┐
│  kernel/src/device/graphics             │
│  ┌─────────────────────────────────┐   │
│  │ GraphicsDevice (core trait)     │   │
│  └─────────────────────────────────┘   │
│  ┌─────────────────────────────────┐   │
│  │ GraphicsBuffer (memory trait)   │   │
│  └─────────────────────────────────┘   │
│  ┌─────────────────────────────────┐   │
│  │ PageFlipCapable (capability)    │   │
│  └─────────────────────────────────┘   │
│  ┌─────────────────────────────────┐   │
│  │ GraphicsManager                 │   │
│  └─────────────────────────────────┘   │
└──────────────────┬──────────────────────┘
                   │ Device-specific impl
                   ▼
┌─────────────────────────────────────────┐
│  kernel/src/drivers/graphics            │
│  • VirtIO GPU                           │
│  • (future: Intel, AMD, NVIDIA, etc.)   │
└─────────────────────────────────────────┘
```

## Core Design Principles

### 1. OS-Independent Core

The core graphics abstractions (`GraphicsDevice`, `GraphicsBuffer`, `PageFlipCapable`, etc.) are completely OS-independent. They:

- Do not reference Linux, Windows, or any specific OS concepts
- Use generic terminology (framebuffer, flush, page flip)
- Can be implemented by any graphics driver
- Can be used by any OS ABI layer

### 2. Graphics Buffers as First-Class Objects

Graphics memory is managed via the `GraphicsBuffer` trait, which allows buffers to be:
- Treated as first-class kernel objects
- Mapped into user memory (via `MemoryMappingOps`)
- Controlled via ioctls (via `ControlOps`)
- Shared between processes

### 3. Dynamic Framebuffer Addresses

Modern GPUs manage framebuffer memory dynamically. The design acknowledges this by:

- Making `get_framebuffer_address()` a query method that returns the **current** address
- Not caching framebuffer addresses at the manager level
- Allowing drivers to change the framebuffer address (e.g., during page flips)
- Treating VRAM vs system RAM as an implementation detail

From the CPU's perspective, framebuffer access is always just memory access. Whether it's VRAM or system RAM is handled by the driver and memory subsystem.

### 3. Capability-Based Extension

Additional features beyond basic framebuffer operations are exposed through capability traits:

- **PageFlipCapable**: For hardware-accelerated page flipping
- **RenderDevice** (future): For 3D rendering and command submission
- **MultiDisplay** (future): For multi-monitor support

This allows:
- Devices to implement only the features they support
- OS code to detect and use advanced features when available
- Graceful fallback when features are unavailable

## GraphicsDevice Trait

The `GraphicsDevice` trait provides the minimal fbdev-equivalent interface:

```rust
pub trait GraphicsDevice: Device {
    fn get_display_name(&self) -> &'static str;
    fn get_framebuffer_config(&self) -> Result<FramebufferConfig, &'static str>;
    fn get_framebuffer_address(&self) -> Result<usize, &'static str>;
    fn flush_framebuffer(&self, x: u32, y: u32, width: u32, height: u32) -> Result<(), &'static str>;
    fn init_graphics(&self) -> Result<(), &'static str>;
}
```

### Key Methods

- **get_framebuffer_config()**: Returns resolution, format, and stride information
- **get_framebuffer_address()**: Returns the current framebuffer physical address (may change)
- **flush_framebuffer()**: Ensures writes are visible on display
- **init_graphics()**: Idempotent initialization

### Framebuffer Configuration

```rust
pub struct FramebufferConfig {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub stride: u32,
}
```

The configuration describes the display resolution and pixel format. The stride (bytes per line) may be larger than `width * bytes_per_pixel` for alignment requirements.

## GraphicsBuffer Trait

The `GraphicsBuffer` trait represents a contiguous region of graphics memory (VRAM or GTT) that can be mapped and controlled:

```rust
pub trait GraphicsBuffer: Send + Sync + MemoryMappingOps + ControlOps {
    fn size(&self) -> usize;
    fn physical_address(&self) -> usize;
    fn as_any(&self) -> &dyn Any;
}
```

This trait inherits from:
- **MemoryMappingOps**: Allows the buffer to be mapped into user address space (mmap).
- **ControlOps**: Allows the buffer to accept ioctl commands (future DMA-BUF support).

## PageFlipCapable Trait

The `PageFlipCapable` trait extends `GraphicsDevice` with page flipping support:

```rust
pub trait PageFlipCapable: GraphicsDevice {
    fn page_flip(&self, buffer_id: u32) -> Result<(), &'static str>;
    fn create_flip_buffer(&self, width: u32, height: u32, format: PixelFormat) -> Result<u32, &'static str>;
    fn destroy_flip_buffer(&self, buffer_id: u32) -> Result<(), &'static str>;
    fn get_flip_buffer_address(&self, buffer_id: u32) -> Result<usize, &'static str>;
}
```

### Hardware Page Flipping

Hardware page flipping allows:
- Double/triple buffering without CPU copies
- Tear-free display updates
- Lower latency
- Better performance

### Fallback Strategy

When a device doesn't implement `PageFlipCapable`, the OS ABI layer can provide a fallback:

1. Allocate a back buffer in system RAM
2. Let the application render to the back buffer
3. When flip is requested:
   - Copy back buffer to framebuffer (`memcpy`)
   - Flush framebuffer (`flush_framebuffer`)

This is slower than hardware page flipping but provides compatibility.

## GraphicsManager

The `GraphicsManager` acts as Scarlet's OS-independent graphics subsystem (analogous to Linux DRM).

### Key Responsibilities

- Discover graphics devices from `DeviceManager`
- Extract and manage framebuffer resources
- Create character devices for user-space access
- Maintain logical names (fb0, fb1, etc.)
- Provide OS-independent API for ABI layers

### Methods for ABI Layers

ABI layers (like Linux DRM) should use GraphicsManager methods instead of directly accessing devices:

- `create_dumb_buffer(width, height, bpp)` - Create a new dumb buffer (returns `Arc<dyn GraphicsBuffer>`)
- `get_framebuffer_config_by_device(device_id)` - Query device configuration
- `get_framebuffer_address_by_device(device_id)` - Query current framebuffer address
- `flush_framebuffer_by_device(device_id, x, y, w, h)` - Flush framebuffer region
- `get_device_id_by_framebuffer(fb_name)` - Lookup device by framebuffer name

### Dynamic Address Support

The manager doesn't cache framebuffer addresses. Instead:

1. `FramebufferResource` stores a reference to the source device
2. When address is needed, query the device's `get_framebuffer_address()`
3. This ensures we always get the current active address

## Usage Examples

### Basic Framebuffer Access

```rust
// Get graphics device
let device = get_graphics_device()?;
let graphics_device = device.as_graphics_device()?;

// Get current configuration and address
let config = graphics_device.get_framebuffer_config()?;
let fb_addr = graphics_device.get_framebuffer_address()?;

// Draw something
unsafe {
    let fb = fb_addr as *mut u32;
    *fb = 0xFF0000FF; // Red pixel
}

// Flush to display
graphics_device.flush_framebuffer(0, 0, config.width, config.height)?;
```

### Using GraphicsManager (for ABI layers)

```rust
use crate::device::graphics::manager::GraphicsManager;

// Get configuration through manager
let graphics_manager = GraphicsManager::get_manager();
let config = graphics_manager.get_framebuffer_config_by_device(device_id)?;
let fb_addr = graphics_manager.get_framebuffer_address_by_device(device_id)?;

// Render operations...

// Flush through manager
graphics_manager.flush_framebuffer_by_device(device_id, 0, 0, config.width, config.height)?;
```

### Page Flipping with Fallback

```rust
// Try hardware page flip using as_any() downcasting
use core::any::Any;

if let Some(any_device) = device.as_any().downcast_ref::<SomeGpuDriver>() {
    // Check if it implements PageFlipCapable
    // Create back buffer
    let buffer_id = any_device.create_flip_buffer(
        config.width, 
        config.height, 
        config.format
    )?;
    
    // Get buffer address and render to it
    let buffer_addr = any_device.get_flip_buffer_address(buffer_id)?;
    // ... render ...
    
    // Hardware flip
    any_device.page_flip(buffer_id)?;
} else {
    // Fallback: allocate back buffer in system RAM
    let back_buffer = allocate_buffer(config.size());
    
    // Render to back buffer
    // ...
    
    // Copy to framebuffer
    let fb_addr = graphics_device.get_framebuffer_address()?;
    copy_nonoverlapping(back_buffer, fb_addr, config.size());
    
    // Flush
    graphics_device.flush_framebuffer(0, 0, config.width, config.height)?;
}
```

## Implementing Graphics Drivers

### Minimal Implementation

A basic graphics driver must implement `GraphicsDevice`:

```rust
impl GraphicsDevice for MyDriver {
    fn get_display_name(&self) -> &'static str {
        "MyDriver Display"
    }

    fn get_framebuffer_config(&self) -> Result<FramebufferConfig, &'static str> {
        // Return current configuration
        Ok(self.config)
    }

    fn get_framebuffer_address(&self) -> Result<usize, &'static str> {
        // Return current framebuffer physical address
        Ok(self.framebuffer_phys_addr)
    }

    fn flush_framebuffer(&self, x: u32, y: u32, width: u32, height: u32) 
        -> Result<(), &'static str> {
        // Ensure CPU writes are visible
        // May involve cache flushes, DMA operations, etc.
        Ok(())
    }

    fn init_graphics(&self) -> Result<(), &'static str> {
        // Initialize hardware (idempotent)
        Ok(())
    }
}
```

### Adding Page Flip Support

For hardware page flipping, also implement `PageFlipCapable`:

```rust
impl PageFlipCapable for MyDriver {
    fn page_flip(&self, buffer_id: u32) -> Result<(), &'static str> {
        // Switch display to show the specified buffer
        self.set_scanout_buffer(buffer_id)
    }

    fn create_flip_buffer(&self, width: u32, height: u32, format: PixelFormat) 
        -> Result<u32, &'static str> {
        // Allocate a buffer for double buffering
        let buffer_id = self.allocate_buffer(width, height, format)?;
        Ok(buffer_id)
    }

    fn destroy_flip_buffer(&self, buffer_id: u32) -> Result<(), &'static str> {
        // Free the buffer
        self.free_buffer(buffer_id)
    }

    fn get_flip_buffer_address(&self, buffer_id: u32) -> Result<usize, &'static str> {
        // Return buffer's physical address
        self.get_buffer_address(buffer_id)
    }
}
```

## Testing

### Unit Tests

- Test `GraphicsDevice` trait implementations
- Test buffer allocation and management
- Test configuration queries

### Integration Tests

- Test GraphicsManager device discovery
- Test character device creation
- Test address query mechanisms

## Future Extensions

### Multi-Display Support

```rust
pub trait MultiDisplay: GraphicsDevice {
    fn get_display_count(&self) -> usize;
    fn get_display_config(&self, display_id: u32) -> Result<FramebufferConfig, &'static str>;
    fn set_display_mode(&self, display_id: u32, config: &FramebufferConfig) -> Result<(), &'static str>;
}
```

### 3D Rendering

```rust
pub trait RenderDevice: GraphicsDevice {
    fn submit_command_buffer(&self, commands: &[u8]) -> Result<(), &'static str>;
    fn create_render_target(&self, width: u32, height: u32) -> Result<u32, &'static str>;
    fn wait_idle(&self) -> Result<(), &'static str>;
}
```

## References

- `kernel/src/device/graphics/mod.rs` - Core trait definitions
- `kernel/src/device/graphics/manager.rs` - GraphicsManager implementation
- `kernel/src/drivers/virtio/gpu.rs` - Example driver implementation
