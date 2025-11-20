# Graphics Abstraction Design

This document describes the design of Scarlet's graphics abstraction layer and its Linux DRM compatibility.

## Architecture Overview

Scarlet's graphics subsystem is designed with OS-independence at its core, while providing compatibility layers for specific OS ABIs (like Linux DRM).

```
┌─────────────────────────────────────────┐
│      Linux Applications (DRM API)       │
└──────────────────┬──────────────────────┘
                   │ DRM ioctls
                   ▼
┌─────────────────────────────────────────┐
│  kernel/src/abi/linux/drm               │
│  (Linux DRM Compatibility Layer)        │
└──────────────────┬──────────────────────┘
                   │ Trait method calls
                   ▼
┌─────────────────────────────────────────┐
│  kernel/src/device/graphics             │
│  ┌─────────────────────────────────┐   │
│  │ GraphicsDevice (core trait)     │   │
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

The core graphics abstractions (`GraphicsDevice`, `PageFlipCapable`, etc.) are completely OS-independent. They:

- Do not reference Linux, Windows, or any specific OS concepts
- Use generic terminology (framebuffer, flush, page flip)
- Can be implemented by any graphics driver
- Can be used by any OS ABI layer

### 2. Dynamic Framebuffer Addresses

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

- **get_framebuffer_config()**: Returns resolution, format, and stride
- **get_framebuffer_address()**: Returns the current framebuffer physical address
- **flush_framebuffer()**: Ensures writes are visible on display
- **init_graphics()**: Idempotent initialization

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

### Fallback Strategy

When a device doesn't implement `PageFlipCapable`, the OS can provide a fallback:

1. Allocate a back buffer in system RAM
2. Let the application render to the back buffer
3. When flip is requested:
   - Copy back buffer to framebuffer (`memcpy`)
   - Flush framebuffer (`flush_framebuffer`)

This is slower than hardware page flipping but provides compatibility.

## Linux DRM Compatibility Layer

The DRM compatibility layer is isolated in `kernel/src/abi/linux/drm/` and provides:

### DRM ioctls

- **DRM_IOCTL_VERSION**: Reports driver version info
- **DRM_IOCTL_MODE_GETRESOURCES**: Lists CRTCs, connectors, encoders
- **DRM_IOCTL_MODE_GETCRTC**: Gets CRTC configuration
- **DRM_IOCTL_MODE_SETCRTC**: Sets CRTC configuration
- **DRM_IOCTL_MODE_CREATE_DUMB**: Creates dumb buffer
- **DRM_IOCTL_MODE_MAP_DUMB**: Maps dumb buffer for CPU access
- **DRM_IOCTL_MODE_DESTROY_DUMB**: Destroys dumb buffer
- **DRM_IOCTL_MODE_PAGE_FLIP**: Performs page flip

### MVP Implementation

The MVP implementation provides basic functionality:

- **Single display**: One CRTC, one connector, one encoder
- **Dumb buffers**: Simple CPU-accessible buffers
- **Page flip**: Implemented via memcpy + flush
- **No 3D**: Only 2D framebuffer operations

### DRM to GraphicsDevice Mapping

| DRM Concept | Scarlet Concept |
|-------------|-----------------|
| CRTC | Display output (1:1 with device) |
| Connector | Physical display connection |
| Encoder | Signal conversion (abstracted) |
| Dumb buffer | Allocated memory region |
| Page flip | Buffer swap (memcpy in MVP) |
| Framebuffer | FramebufferConfig + address |

### Future Extensions

The DRM layer can be extended to support:

- Hardware-accelerated page flipping (via `PageFlipCapable`)
- Multiple displays (multiple CRTCs/connectors)
- 3D rendering (via future `RenderDevice` trait)
- Synchronization (vblank events, fences)
- Advanced pixel formats
- Direct rendering to display buffers

## GraphicsManager

The `GraphicsManager` coordinates graphics devices and creates `/dev/fbX` character devices.

### Key Responsibilities

- Discover graphics devices from `DeviceManager`
- Extract framebuffer resources
- Create character devices for user-space access
- Maintain logical names (fb0, fb1, etc.)

### Dynamic Address Support

The manager doesn't cache framebuffer addresses. Instead:

1. `FramebufferResource` stores a reference to the source device
2. When address is needed, query the device's `get_framebuffer_address()`
3. This ensures we always get the current active address

## Implementation Status

### Completed (MVP)

- ✅ Enhanced `GraphicsDevice` trait documentation
- ✅ `PageFlipCapable` trait definition
- ✅ DRM module structure (`kernel/src/abi/linux/drm/`)
- ✅ DRM type definitions (structures, constants)
- ✅ Basic DRM ioctl implementations:
  - VERSION, GETRESOURCES, GETCRTC
  - CREATE_DUMB, MAP_DUMB, DESTROY_DUMB
  - PAGE_FLIP (memcpy + flush)

### Planned (Future Work)

- ⏳ Update `FramebufferResource` to query device for address
- ⏳ Implement `PageFlipCapable` in VirtIO GPU driver
- ⏳ Add GETCONNECTOR, GETENCODER, SETCRTC ioctls
- ⏳ Implement proper mmap support for dumb buffers
- ⏳ Add vblank event support
- ⏳ Multi-display support
- ⏳ 3D rendering capabilities (`RenderDevice` trait)

## Usage Example

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

### Page Flipping (with fallback)

```rust
// Try hardware page flip
if let Some(page_flip_device) = device.as_page_flip_capable() {
    // Create back buffer
    let buffer_id = page_flip_device.create_flip_buffer(
        config.width, 
        config.height, 
        config.format
    )?;
    
    // Get buffer address and render to it
    let buffer_addr = page_flip_device.get_flip_buffer_address(buffer_id)?;
    // ... render ...
    
    // Hardware flip
    page_flip_device.page_flip(buffer_id)?;
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

## Testing Strategy

### Unit Tests

- Test `GraphicsDevice` trait implementations
- Test DRM structure serialization/deserialization
- Test buffer allocation and management

### Integration Tests

- Test DRM ioctl handling with mock devices
- Test page flip fallback mechanism
- Test character device operations

### System Tests

- Run Linux DRM applications
- Verify display output
- Performance benchmarking

## References

- [Linux DRM Documentation](https://www.kernel.org/doc/html/latest/gpu/drm-uapi.html)
- [VirtIO GPU Specification](https://docs.oasis-open.org/virtio/virtio/v1.1/virtio-v1.1.html#x1-3430008)
- [Mesa DRI Implementation](https://www.mesa3d.org/)
