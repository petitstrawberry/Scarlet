//! Native desktop preview support for ScarletUI.
//!
//! Preview is intentionally separate from normal `Application::run()`.
//! It is a development-only Rust dylib boundary used by the preview host.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::path::Path;
use std::time::Duration;

use crate::buffer::Buffer;
use crate::compositor::DamageRect;
use crate::element::{ComponentElement, Element, TextInputElementState};
use crate::error::Error;
use crate::event::{Event, WindowEvent};
use crate::geometry::{Alignment, Size};
use crate::pipeline::RenderingPipeline;
use crate::platform::{PlatformBackend, PlatformWindow, WindowCreateRequest};
use crate::view::View;
use crate::views::{AlignmentFrame, Window};

/// Preview entry symbol exported by preview dylibs.
pub const PREVIEW_ENTRY_SYMBOL: &[u8] = b"scarlet_ui_preview_entry";

/// Stable identifier for a preview entry inside a preview library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewId(String);

impl PreviewId {
    /// Create a preview ID.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow this ID as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for PreviewId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for PreviewId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// Metadata shown by the preview host before a session is created.
#[derive(Clone, Debug)]
pub struct PreviewDescriptor {
    /// Stable preview ID.
    pub id: PreviewId,
    /// Human-readable preview name.
    pub name: String,
}

impl PreviewDescriptor {
    /// Create preview metadata.
    pub fn new(id: impl Into<PreviewId>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
        }
    }
}

/// Initial context passed to a preview session.
#[derive(Clone, Copy, Debug)]
pub struct PreviewCreateContext {
    /// Initial logical window size.
    pub size: Size,
    /// Output scale in milli-units.
    pub scale_milli: u32,
}

impl Default for PreviewCreateContext {
    fn default() -> Self {
        Self {
            size: Size::new(800.0, 600.0),
            scale_milli: 1000,
        }
    }
}

/// Rendered preview frame.
pub struct PreviewFrame<'a> {
    /// BGRA frame buffer.
    pub buffer: &'a Buffer,
    /// Physical damage rectangles, or `None` for full present.
    pub damage: Option<&'a [DamageRect]>,
}

/// A running preview instance.
pub trait PreviewSession {
    /// Current preview title.
    fn title(&self) -> &str;

    /// Current logical size.
    fn size(&self) -> Size;

    /// Resize the preview.
    fn resize(&mut self, size: Size, scale_milli: u32);

    /// Dispatch an input event.
    fn handle_event(&mut self, event: &Event) -> bool;

    /// Take platform-neutral window events emitted by the view tree.
    fn take_emitted_events(&mut self) -> Vec<Event>;

    /// Return focused text input state, if any.
    fn focused_text_input_state(&self) -> Option<TextInputElementState>;

    /// Render the current frame.
    fn render(&mut self) -> Option<PreviewFrame<'_>>;
}

/// Preview library loaded from a Rust dylib.
pub trait PreviewLibrary {
    /// Return all previews exported by this dylib.
    fn previews(&self) -> Vec<PreviewDescriptor>;

    /// Create a preview session.
    fn create(
        &mut self,
        id: &PreviewId,
        context: PreviewCreateContext,
    ) -> Option<Box<dyn PreviewSession>>;
}

/// Statically registered preview factory.
pub struct PreviewRegistration {
    /// Stable preview ID.
    pub id: &'static str,
    /// Human-readable preview name.
    pub name: &'static str,
    /// Preferred logical preview size, or `Size::ZERO` for the host default.
    pub preferred_size: Size,
    /// Create a preview session.
    pub create: fn(PreviewCreateContext) -> Box<dyn PreviewSession>,
}

inventory::collect!(PreviewRegistration);

struct RegisteredPreviewLibrary;

impl PreviewLibrary for RegisteredPreviewLibrary {
    fn previews(&self) -> Vec<PreviewDescriptor> {
        inventory::iter::<PreviewRegistration>
            .into_iter()
            .map(|registration| PreviewDescriptor::new(registration.id, registration.name))
            .collect()
    }

    fn create(
        &mut self,
        id: &PreviewId,
        mut context: PreviewCreateContext,
    ) -> Option<Box<dyn PreviewSession>> {
        inventory::iter::<PreviewRegistration>
            .into_iter()
            .find(|registration| registration.id == id.as_str())
            .map(|registration| {
                if !registration.preferred_size.is_zero() {
                    context.size = registration.preferred_size;
                }
                (registration.create)(context)
            })
    }
}

/// Create a preview library from all registered preview entries.
pub fn registered_preview_library() -> Box<dyn PreviewLibrary> {
    Box::new(RegisteredPreviewLibrary)
}

struct SinglePreviewLibrary<F, V>
where
    F: Fn() -> V + 'static,
    V: View + Clone + 'static,
{
    descriptor: PreviewDescriptor,
    factory: F,
}

impl<F, V> PreviewLibrary for SinglePreviewLibrary<F, V>
where
    F: Fn() -> V + 'static,
    V: View + Clone + 'static,
{
    fn previews(&self) -> Vec<PreviewDescriptor> {
        alloc::vec![self.descriptor.clone()]
    }

    fn create(
        &mut self,
        id: &PreviewId,
        context: PreviewCreateContext,
    ) -> Option<Box<dyn PreviewSession>> {
        if id != &self.descriptor.id {
            return None;
        }
        Some(Box::new(ViewPreviewSession::new(
            self.descriptor.name.clone(),
            (self.factory)(),
            context,
        )))
    }
}

/// Create a preview library with a single preview entry.
pub fn single_preview_library<F, V>(name: &'static str, factory: F) -> Box<dyn PreviewLibrary>
where
    F: Fn() -> V + 'static,
    V: View + Clone + 'static,
{
    Box::new(SinglePreviewLibrary {
        descriptor: PreviewDescriptor::new(name, name),
        factory,
    })
}

/// Create a preview session from a `View`.
pub fn preview_session_from_view<V>(
    fallback_title: &'static str,
    view: V,
    context: PreviewCreateContext,
) -> Box<dyn PreviewSession>
where
    V: View + Clone + 'static,
{
    Box::new(ViewPreviewSession::new(
        fallback_title.to_string(),
        view,
        context,
    ))
}

struct ViewPreviewSession {
    title: String,
    pipeline: RenderingPipeline,
}

#[derive(Clone)]
struct PreviewShell<V: View + Clone> {
    title: String,
    size: Size,
    content: V,
}

impl<V: View + Clone> PreviewShell<V> {
    fn new(title: String, size: Size, content: V) -> Self {
        Self {
            title,
            size,
            content,
        }
    }
}

impl<V> View for PreviewShell<V>
where
    V: View + Clone + 'static,
{
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(ComponentElement::new_with_builder(
            self.clone(),
            Self::create_shell_element,
        ))
    }

    fn listenables(&self) -> Vec<&dyn crate::state::Listenable> {
        self.content.listenables()
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

impl<V> PreviewShell<V>
where
    V: View + Clone + 'static,
{
    fn create_shell_element(shell: &Self) -> Box<dyn Element> {
        Window::new(
            shell.title.clone(),
            AlignmentFrame::new(shell.content.clone(), Alignment::Center),
        )
        .size(shell.size)
        .create_element()
    }
}

impl ViewPreviewSession {
    fn new<V>(fallback_title: String, view: V, context: PreviewCreateContext) -> Self
    where
        V: View + Clone + 'static,
    {
        let mut pipeline = RenderingPipeline::new();
        pipeline.set_scale_milli(context.scale_milli);
        let requested_size = if context.size.width > 0.0 && context.size.height > 0.0 {
            context.size
        } else {
            PreviewCreateContext::default().size
        };
        pipeline.set_root(
            PreviewShell::new(fallback_title.clone(), requested_size, view).create_element(),
        );
        let window_info = pipeline.layout_initial();
        let mut title = window_info.title;
        if title == "ScarletUI Application" {
            title = fallback_title;
        }
        pipeline.resize(requested_size);
        Self { title, pipeline }
    }
}

impl PreviewSession for ViewPreviewSession {
    fn title(&self) -> &str {
        &self.title
    }

    fn size(&self) -> Size {
        self.pipeline.window_size()
    }

    fn resize(&mut self, size: Size, scale_milli: u32) {
        self.pipeline.set_scale_milli(scale_milli);
        self.pipeline.resize(size);
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        self.pipeline.handle_event(event)
    }

    fn take_emitted_events(&mut self) -> Vec<Event> {
        self.pipeline.take_emitted_events()
    }

    fn focused_text_input_state(&self) -> Option<TextInputElementState> {
        self.pipeline.focused_text_input_state()
    }

    fn render(&mut self) -> Option<PreviewFrame<'_>> {
        let (buffer, damage) = self.pipeline.render_with_damage()?;
        Some(PreviewFrame { buffer, damage })
    }
}

/// Loaded preview dylib.
pub struct LoadedPreviewLibrary {
    api: Box<dyn PreviewLibrary>,
    _library: libloading::Library,
}

impl LoadedPreviewLibrary {
    /// Load a preview dylib.
    ///
    /// # Safety
    ///
    /// The dylib must be built by the same Rust toolchain and against the same
    /// ScarletUI preview API as the host. All preview objects created from this
    /// library must be dropped before this value is dropped.
    pub unsafe fn load(path: impl AsRef<Path>) -> core::result::Result<Self, String> {
        // SAFETY: The caller guarantees `path` points to a compatible preview
        // dylib whose lifetime is managed by `Self`.
        let library = unsafe { libloading::Library::new(path.as_ref()) }
            .map_err(|error| error.to_string())?;
        // SAFETY: The preview dylib contract requires this symbol to exist with
        // the exact `unsafe fn() -> Box<dyn PreviewLibrary>` signature.
        let entry = unsafe {
            library
                .get::<unsafe fn() -> Box<dyn PreviewLibrary>>(PREVIEW_ENTRY_SYMBOL)
                .map_err(|error| error.to_string())?
        };
        // SAFETY: The symbol ABI is validated by the preview contract above,
        // and the returned trait object cannot outlive `library`.
        let api = unsafe { entry() };
        drop(entry);
        Ok(Self {
            api,
            _library: library,
        })
    }

    /// Return all previews exported by this library.
    pub fn previews(&self) -> Vec<PreviewDescriptor> {
        self.api.previews()
    }

    /// Create a preview session.
    pub fn create(
        &mut self,
        id: &PreviewId,
        context: PreviewCreateContext,
    ) -> Option<Box<dyn PreviewSession>> {
        self.api.create(id, context)
    }
}

/// Preview host backed by a supplied platform backend.
pub struct PreviewHost {
    session: Option<Box<dyn PreviewSession>>,
    loaded: Option<LoadedPreviewLibrary>,
    window: Box<dyn PlatformWindow>,
    preview_id: PreviewId,
    sync_after_reload: bool,
    full_present_frames: u8,
}

impl PreviewHost {
    /// Create a host from a selected preview exported by a dylib and a backend.
    pub fn new_with_backend(
        mut loaded: LoadedPreviewLibrary,
        preview: Option<&str>,
        mut backend: Box<dyn PlatformBackend>,
    ) -> core::result::Result<Self, String> {
        let descriptor = select_preview(loaded.previews(), preview)?;
        let scale_milli = backend.output_scale_milli();
        let context = PreviewCreateContext {
            scale_milli,
            ..PreviewCreateContext::default()
        };
        let session = loaded
            .create(&descriptor.id, context)
            .ok_or_else(|| String::from("failed to create preview session"))?;
        let size = session.size();
        let mut window = backend
            .create_window(WindowCreateRequest {
                app_id: String::from("org.scarlet-os.scarletui.preview"),
                title: session.title().to_string(),
                size,
                window_type: 0,
                menu_titles: String::new(),
                focus_on_create: true,
                active_on_focus: true,
                opaque: true,
            })
            .map_err(|error| error.to_string())?;
        window.set_title(session.title());
        Ok(Self {
            session: Some(session),
            loaded: Some(loaded),
            window,
            preview_id: descriptor.id,
            sync_after_reload: true,
            full_present_frames: 2,
        })
    }

    /// Replace the loaded preview library after a successful rebuild.
    pub fn reload(&mut self, mut loaded: LoadedPreviewLibrary) -> core::result::Result<(), String> {
        let size = self.window.size();
        let scale_milli = self.window.output_scale_milli();
        let session = loaded
            .create(&self.preview_id, PreviewCreateContext { size, scale_milli })
            .or_else(|| {
                let descriptor = loaded.previews().into_iter().next()?;
                self.preview_id = descriptor.id.clone();
                loaded.create(&descriptor.id, PreviewCreateContext { size, scale_milli })
            })
            .ok_or_else(|| String::from("failed to create reloaded preview session"))?;

        self.session = None;
        self.loaded = None;
        self.window.set_title(session.title());
        self.loaded = Some(loaded);
        self.session = Some(session);
        self.sync_after_reload = true;
        self.full_present_frames = 2;
        Ok(())
    }

    /// Run one event/render tick. Returns `false` when the preview should exit.
    pub fn tick(&mut self, timeout: Duration) -> core::result::Result<bool, String> {
        let Some(session) = self.session.as_mut() else {
            return Err(String::from("preview host has no active session"));
        };

        let mut had_event = false;
        while let Some(event) = self.window.poll_event() {
            had_event = true;
            if !Self::handle_event(self.window.as_mut(), session.as_mut(), event)? {
                return Ok(false);
            }
        }

        self.window
            .sync_text_input(session.focused_text_input_state().as_ref());

        if self.sync_after_reload {
            let size = self.window.size();
            let scale_milli = self.window.output_scale_milli();
            session.resize(size, scale_milli);
            self.sync_after_reload = false;
        }

        if let Some(frame) = session.render() {
            let damage = if self.full_present_frames > 0 {
                self.full_present_frames -= 1;
                None
            } else {
                frame.damage
            };
            self.window.present_with_damage(frame.buffer, damage);
        }

        if !had_event {
            self.window.wait_for_event(timeout);
        }

        Ok(true)
    }

    fn handle_event(
        window: &mut dyn PlatformWindow,
        session: &mut dyn PreviewSession,
        event: Event,
    ) -> core::result::Result<bool, String> {
        match event {
            Event::Quit => return Ok(false),
            Event::Resize { width, height } => {
                let size = Size::new(width as f32, height as f32);
                session.resize(size, window.output_scale_milli());
            }
            event => {
                let _ = session.handle_event(&event);
            }
        }

        for emitted in session.take_emitted_events() {
            match emitted {
                Event::Window(WindowEvent::CloseRequested) => return Ok(false),
                Event::Window(WindowEvent::MinimizeRequested) => {
                    window.minimize().map_err(error_to_string)?;
                }
                Event::Window(WindowEvent::MaximizeRequested) => {
                    window.maximize().map_err(error_to_string)?;
                }
                Event::Window(WindowEvent::RestoreRequested) => {
                    window.restore().map_err(error_to_string)?;
                }
                Event::Window(WindowEvent::MoveRequested) => {
                    window.request_move().map_err(error_to_string)?;
                }
                _ => {}
            }
        }

        Ok(true)
    }
}

fn select_preview(
    previews: Vec<PreviewDescriptor>,
    preview: Option<&str>,
) -> core::result::Result<PreviewDescriptor, String> {
    let Some(preview) = preview else {
        return previews
            .into_iter()
            .next()
            .ok_or_else(|| String::from("preview library exports no previews"));
    };
    previews
        .iter()
        .find(|descriptor| descriptor.id.as_str() == preview || descriptor.name == preview)
        .cloned()
        .ok_or_else(|| format!("preview not found: {preview}"))
}

impl Drop for PreviewHost {
    fn drop(&mut self) {
        self.session = None;
        self.loaded = None;
    }
}

fn error_to_string(error: Error) -> String {
    error.to_string()
}
