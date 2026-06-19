//! Scene declarations for ScarletUI applications.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::view::View;
use crate::views::Window;

/// Stable declaration key for a top-level window scene.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SceneWindowKey(String);

impl SceneWindowKey {
    /// The default primary scene key.
    pub fn main() -> Self {
        Self(String::from("main"))
    }

    /// Borrow this key as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for SceneWindowKey {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for SceneWindowKey {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// Runtime identity for an opened application window.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct WindowId(u64);

impl WindowId {
    /// Generate a new runtime window ID.
    pub fn generate() -> Self {
        static NEXT_WINDOW_ID: AtomicU64 = AtomicU64::new(1);
        Self(NEXT_WINDOW_ID.fetch_add(1, Ordering::SeqCst))
    }

    /// Return the raw numeric value.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Context passed to application window lifecycle hooks.
#[derive(Clone, Debug)]
pub struct WindowContext {
    pub window_id: WindowId,
    pub scene_key: SceneWindowKey,
    pub pipeline_id: crate::pipeline::PipelineId,
    pub platform_window_id: u64,
    pub is_primary: bool,
}

/// One top-level window declaration produced by a Scene.
pub struct WindowDeclaration {
    pub key: SceneWindowKey,
    pub view: Box<dyn View>,
    pub opens_at_launch: bool,
}

/// Builder used by scenes to declare top-level windows.
pub struct SceneBuilder {
    declarations: Vec<WindowDeclaration>,
}

impl SceneBuilder {
    /// Create an empty scene builder.
    pub fn new() -> Self {
        Self {
            declarations: Vec::new(),
        }
    }

    /// Add a top-level window declaration.
    pub fn window<V>(&mut self, key: impl Into<SceneWindowKey>, view: V)
    where
        V: View + 'static,
    {
        self.window_with_launch_policy(key, view, true);
    }

    /// Add a top-level window declaration with an explicit launch policy.
    pub fn window_with_launch_policy<V>(
        &mut self,
        key: impl Into<SceneWindowKey>,
        view: V,
        opens_at_launch: bool,
    ) where
        V: View + 'static,
    {
        self.declarations.push(WindowDeclaration {
            key: key.into(),
            view: Box::new(view),
            opens_at_launch,
        });
    }

    /// Consume and return all declarations.
    pub fn into_declarations(self) -> Vec<WindowDeclaration> {
        self.declarations
    }
}

impl Default for SceneBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Top-level application scene declaration.
pub trait Scene {
    /// Build this scene into top-level window declarations.
    fn build(self, builder: &mut SceneBuilder);
}

impl Scene for () {
    fn build(self, _builder: &mut SceneBuilder) {}
}

/// SwiftUI-like window group declaration.
pub struct WindowGroup<V: View + Clone + 'static> {
    key: SceneWindowKey,
    window: Window<V>,
}

impl<V: View + Clone + 'static> WindowGroup<V> {
    /// Create a new window group declaration.
    pub fn new(key: impl Into<SceneWindowKey>, window: Window<V>) -> Self {
        Self {
            key: key.into(),
            window,
        }
    }
}

impl<V: View + Clone + 'static> Scene for WindowGroup<V> {
    fn build(self, builder: &mut SceneBuilder) {
        builder.window_with_launch_policy(self.key, self.window, true);
    }
}

impl<V: View + Clone + 'static> Scene for Window<V> {
    fn build(self, builder: &mut SceneBuilder) {
        let key = self
            .scene_key_value()
            .map(SceneWindowKey::from)
            .unwrap_or_else(SceneWindowKey::main);
        let opens_at_launch = self.opens_at_launch_value();
        builder.window_with_launch_policy(key, self, opens_at_launch);
    }
}

impl<A: Scene, B: Scene> Scene for (A, B) {
    fn build(self, builder: &mut SceneBuilder) {
        self.0.build(builder);
        self.1.build(builder);
    }
}

impl<A: Scene, B: Scene, C: Scene> Scene for (A, B, C) {
    fn build(self, builder: &mut SceneBuilder) {
        self.0.build(builder);
        self.1.build(builder);
        self.2.build(builder);
    }
}

impl<A: Scene, B: Scene, C: Scene, D: Scene> Scene for (A, B, C, D) {
    fn build(self, builder: &mut SceneBuilder) {
        self.0.build(builder);
        self.1.build(builder);
        self.2.build(builder);
        self.3.build(builder);
    }
}
