//! Window adapter implementation for Scarlet OS

use slint::platform::{software_renderer as renderer, WindowAdapter};
use slint::PhysicalSize;
use std::rc::{Rc, Weak};
use std::cell::RefCell;
use sws_client::Connection;

/// Window adapter for Scarlet OS
pub struct ScarletWindowAdapter {
    window: slint::Window,
    surface_id: u32,
    size: RefCell<PhysicalSize>,
    // Store renderer to implement the Renderer trait
    renderer: RefCell<renderer::SoftwareRenderer>,
}

impl ScarletWindowAdapter {
    /// Create a new window adapter
    pub fn new(connection: &mut Connection) -> Result<Rc<Self>, slint::platform::PlatformError> {
        // Default window size
        let width = 800;
        let height = 600;
        
        // Create a surface (window) through SWS
        let surface_id = connection
            .create_surface(width, height)
            .map_err(|e| slint::platform::PlatformError::Other(
                std::format!("Failed to create surface: {:?}", e).into()
            ))?;
        
        let size = PhysicalSize::new(width, height);
        
        // Create software renderer
        let renderer = renderer::SoftwareRenderer::new();
        
        // Use Rc::new_cyclic to handle the circular reference between Window and WindowAdapter
        let adapter = Rc::new_cyclic(|weak: &Weak<Self>| {
            let window = slint::Window::new(weak.clone());
            
            Self {
                window,
                surface_id,
                size: RefCell::new(size),
                renderer: RefCell::new(renderer),
            }
        });

        // Inform Slint about the initial scale factor and size so hit-testing works.
        adapter.window.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
            scale_factor: 1.0,
        });
        adapter.window.dispatch_event(slint::platform::WindowEvent::Resized {
            size: slint::LogicalSize::new(width as f32, height as f32),
        });

        Ok(adapter)
    }

    /// Get the surface id for rendering
    pub fn surface_id(&self) -> u32 {
        self.surface_id
    }
    
    /// Get the renderer
    pub fn renderer_ref(&self) -> &RefCell<renderer::SoftwareRenderer> {
        &self.renderer
    }
}

impl WindowAdapter for ScarletWindowAdapter {
    fn window(&self) -> &slint::Window {
        &self.window
    }

    fn size(&self) -> PhysicalSize {
        *self.size.borrow()
    }

    fn renderer(&self) -> &dyn slint::platform::Renderer {
        // Return a reference to the software renderer
        // This is a bit tricky because we need to return a trait object
        // For now, we'll use a workaround
        unsafe {
            // SAFETY: We're converting the RefCell<SoftwareRenderer> to a raw pointer
            // and then to a reference. This is safe as long as we don't drop the RefCell
            // while the reference is in use.
            &*(&*self.renderer.as_ptr() as *const dyn slint::platform::Renderer)
        }
    }

    fn request_redraw(&self) {
        // Mark that we need to redraw
        // The actual rendering will happen in the event loop
    }
}
