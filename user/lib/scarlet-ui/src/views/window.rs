//! Window View - Top-level window container
//!
//! Window is a View that wraps a child and provides window-level properties.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;
use core::any::Any;

use crate::view::View;
use crate::element::{Element, ComponentElement};
use crate::geometry::Size;

/// Window View - top-level window container
///
/// Window provides window-level properties like title, size, and decorations.
/// It wraps a child View and delegates element creation to it.
pub struct Window<V: View> {
    app_id: String,
    title: String,
    size: Size,
    child: V,
    resizable: bool,
    decorated: bool,
}

impl<V: View> Window<V> {
    /// Create a new Window with a title and child
    pub fn new(title: impl Into<String>, child: V) -> Self {
        let title_str = title.into();
        Self {
            app_id: String::from("com.example.scarletui"),
            title: title_str,
            size: Size::new(800.0, 600.0),
            child,
            resizable: true,
            decorated: true,
        }
    }

    /// Set the application ID
    pub fn app_id(mut self, app_id: impl Into<String>) -> Self {
        self.app_id = app_id.into();
        self
    }

    /// Set the window size
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    /// Set whether the window is resizable
    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    /// Set whether the window has decorations (title bar, borders)
    pub fn decorated(mut self, decorated: bool) -> Self {
        self.decorated = decorated;
        self
    }

    /// Get the application ID
    pub fn get_app_id(&self) -> &str {
        &self.app_id
    }

    /// Get the window title
    pub fn get_title(&self) -> &str {
        &self.title
    }

    /// Get the window size
    pub fn get_window_size(&self) -> Size {
        self.size
    }

    /// Check if the window is resizable
    pub fn is_resizable(&self) -> bool {
        self.resizable
    }

    /// Check if the window is decorated
    pub fn is_decorated(&self) -> bool {
        self.decorated
    }

    /// Get the child View
    pub fn child(&self) -> &V {
        &self.child
    }

    /// Get mutable reference to the child View
    pub fn child_mut(&mut self) -> &mut V {
        &mut self.child
    }
}

impl<V: View + Clone> Clone for Window<V> {
    fn clone(&self) -> Self {
        Self {
            app_id: self.app_id.clone(),
            title: self.title.clone(),
            size: self.size,
            child: self.child.clone(),
            resizable: self.resizable,
            decorated: self.decorated,
        }
    }
}

impl<V: View + Clone> View for Window<V> {
    fn create_element(&self) -> Box<dyn Element> {
        // Create a ComponentElement that wraps the child element
        Box::new(ComponentElement::new(self.clone()))
    }

    fn listenables(&self) -> Vec<&dyn crate::state::Listenable> {
        self.child.listenables()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
