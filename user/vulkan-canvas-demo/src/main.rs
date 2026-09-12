#[cfg(target_os = "scarlet")]
mod vulkan_cube;

#[cfg(not(target_os = "scarlet"))]
fn main() {
    eprintln!("vulkan-canvas-demo runs on Scarlet with SWS and the SGFX compositor");
}

#[cfg(target_os = "scarlet")]
fn main() {
    if let Err(error) = scarlet_app::run() {
        eprintln!("vulkan-canvas-demo: {error}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "scarlet")]
mod scarlet_app {
    use std::any::Any;
    use std::boxed::Box;
    use std::cell::RefCell;
    use std::error::Error;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::Instant;

    use scarlet_ui::element::Element;
    use scarlet_ui::prelude::*;
    use scarlet_ui::{
        ApplicationRunExt, ExternalGpuSurface, PlatformWindow, RendererBackendKind, SgfxTexture,
        shared_bgra8_texture_from_raw, vstack,
    };

    use crate::vulkan_cube::{IMAGE_HEIGHT, IMAGE_WIDTH, VulkanCube};

    const ID_TEXTURE: u32 = 7_001;
    type AppResult<T> = std::result::Result<T, Box<dyn Error>>;

    #[derive(Clone)]
    struct DemoApp {
        texture: State<Arc<SgfxTexture>>,
        renderer: Rc<RefCell<VulkanCube>>,
        started: Instant,
        rendering_enabled: bool,
        frame_pending: bool,
    }

    impl DemoApp {
        fn new() -> AppResult<Self> {
            let (mut renderer, raw_handle) = VulkanCube::new()?;
            let texture = unsafe {
                shared_bgra8_texture_from_raw(raw_handle, IMAGE_WIDTH, IMAGE_HEIGHT)
                    .map_err(|error| format!("cannot adopt Vulkan shared image: {error}"))?
            };
            renderer.render(0.58)?;
            Ok(Self {
                texture: State::new(StateId::new(ID_TEXTURE), texture),
                renderer: Rc::new(RefCell::new(renderer)),
                started: Instant::now(),
                rendering_enabled: true,
                frame_pending: true,
            })
        }

        fn content(&self) -> impl View + Clone {
            vstack! {
                Text::new("Vulkan-SGFX shared image").font_size(22.0),
                Text::new("Indexed Vulkan cube embedded between ordinary ScarletUI views")
                    .font_size(14.0),
                ExternalGpuSurface::from_state(
                    f32::INFINITY,
                    f32::INFINITY,
                    self.texture.clone(),
                ),
                Text::new("Vulkan core API  →  SGFX IR  →  VirGL  →  direct shared-image composite")
                    .font_size(13.0),
            }
            .spacing(8.0)
            .padding(14.0)
        }
    }

    impl View for DemoApp {
        fn create_element(&self) -> Box<dyn Element> {
            self.content().create_element()
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    impl Application for DemoApp {
        fn scenes(&self) -> impl Scene {
            Window::new("Vulkan-SGFX Cube", self.content())
                .app_id("org.scarlet-os.vulkan-canvas-demo")
                .size(Size::new(620.0, 700.0))
                .min_size(Size::new(360.0, 440.0))
                .resizable(true)
                .background_color(Color::rgb(7u8, 11u8, 22u8))
        }

        fn on_window_created(&mut self, _context: &WindowContext, window: &mut dyn PlatformWindow) {
            if window.renderer_backend() != RendererBackendKind::Sgfx {
                self.rendering_enabled = false;
                eprintln!(
                    "vulkan-canvas-demo requires the ScarletUI SGFX renderer and an SGFX SWS compositor"
                );
            }
        }

        fn on_idle(&mut self) {
            // The SGFX/SWS path completes composition before reporting the
            // preceding frame. Keep one surface update pending so this single
            // shared image is never rewritten ahead of its consumer.
            if !self.rendering_enabled || self.frame_pending {
                return;
            }
            let seconds = self.started.elapsed().as_secs_f32();
            if let Err(error) = self.renderer.borrow_mut().render(0.58 + seconds * 0.72) {
                self.rendering_enabled = false;
                eprintln!("Vulkan frame failed: {error}");
                return;
            }
            let texture = self.texture.get();
            self.texture.set(texture);
            self.frame_pending = true;
        }

        fn on_frame_presented(&mut self, _context: &WindowContext) {
            self.frame_pending = false;
        }

        fn debug_logging(&self) -> bool {
            false
        }
    }

    pub fn run() -> AppResult<()> {
        let mut app = DemoApp::new()?;
        app.run()
            .map_err(|error| format!("ScarletUI runner failed: {error}"))?;
        Ok(())
    }
}
