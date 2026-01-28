//! Event modifier views
//!
//! Provides on_click, on_hover, on_exit modifiers for any view.

use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject};
use crate::geometry::Size;
use alloc::boxed::Box;
use alloc::vec;

/// Click event modifier - adds click handler to any view
#[derive(Clone)]
pub struct OnClick<V: View, F: Clone + 'static> {
    inner: V,
    callback: F,
}

impl<V: View, F: Fn() + Clone + 'static> OnClick<V, F> {
    /// Create a new OnClick modifier
    pub fn new(inner: V, callback: F) -> Self {
        Self { inner, callback }
    }

    /// Get the inner view
    pub fn inner(&self) -> &V {
        &self.inner
    }

    /// Get the callback
    pub fn callback(&self) -> &F {
        &self.callback
    }

    /// Invoke the click callback
    pub fn invoke_on_click(&self) {
        (self.callback)();
    }
}

impl<V: View + Clone, F: Fn() + Clone + 'static> View for OnClick<V, F> {
    fn create_element(&self) -> Box<dyn Element> {
        let mut render_object = OnClickRenderObject::new();
        // Store callback in render object
        // We need to clone the callback since F: Clone
        render_object.set_callback(Box::new(self.callback.clone()));

        Box::new(RenderElement::with_children(
            self.clone(),
            render_object,
            vec![self.inner.create_element()],
        ))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn crate::state::Listenable> {
        self.inner.listenables()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Click RenderObject
pub struct OnClickRenderObject {
    is_hovered: bool,
    callback: Option<Box<dyn Fn()>>,
    size: Size,
}

impl OnClickRenderObject {
    pub fn new() -> Self {
        Self { is_hovered: false, callback: None, size: Size::ZERO }
    }

    pub fn set_callback(&mut self, callback: Box<dyn Fn()>) {
        self.callback = Some(callback);
    }

    pub fn invoke_on_click(&self) {
        if let Some(ref cb) = self.callback {
            cb();
        }
    }
}

impl ElementRenderObject for OnClickRenderObject {
    fn layout(&mut self, _constraints: crate::element::LayoutConstraints) -> Size {
        Size::ZERO
    }

    fn layout_with_children(
        &mut self,
        constraints: crate::element::LayoutConstraints,
        children: &mut [Box<dyn Element>],
    ) -> Size {
        if let Some(child) = children.first_mut() {
            let size = child.layout(constraints);
            self.size = size;
            if crate::debug::is_enabled() {
                scarlet_std::println!("[OnClickRenderObject::layout_with_children] size={}x{}", size.width, size.height);
            }
            size
        } else {
            self.size = Size::ZERO;
            Size::ZERO
        }
    }

    fn size(&self) -> Size {
        self.size
    }

    fn hit_test(&self, point: crate::geometry::Point) -> bool {
        let result = point.x >= 0.0 && point.x < self.size.width && point.y >= 0.0 && point.y < self.size.height;
        if crate::debug::is_enabled() {
            scarlet_std::println!("[OnClickRenderObject::hit_test] point=({:?}), size={:?}, result={}", point, self.size, result);
        }
        result
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn render(&mut self) {
        // Modifier doesn't directly render
    }
}

/// Hover event modifier - adds hover enter handler to any view
#[derive(Clone)]
pub struct OnHover<V: View, F: Clone + 'static> {
    inner: V,
    callback: F,
}

impl<V: View, F: Fn() + Clone + 'static> OnHover<V, F> {
    /// Create a new OnHover modifier
    pub fn new(inner: V, callback: F) -> Self {
        Self { inner, callback }
    }

    /// Get the inner view
    pub fn inner(&self) -> &V {
        &self.inner
    }

    /// Get the callback
    pub fn callback(&self) -> &F {
        &self.callback
    }
}

impl<V: View + Clone, F: Fn() + Clone + 'static> View for OnHover<V, F> {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::with_children(
            self.clone(),
            OnHoverRenderObject::new(),
            vec![self.inner.create_element()],
        ))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn crate::state::Listenable> {
        self.inner.listenables()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Hover RenderObject
pub struct OnHoverRenderObject {
    is_hovered: bool,
}

impl OnHoverRenderObject {
    pub fn new() -> Self {
        Self { is_hovered: false }
    }
}

impl ElementRenderObject for OnHoverRenderObject {
    fn layout(&mut self, _constraints: crate::element::LayoutConstraints) -> Size {
        Size::ZERO
    }

    fn layout_with_children(
        &mut self,
        constraints: crate::element::LayoutConstraints,
        children: &mut [Box<dyn Element>],
    ) -> Size {
        if let Some(child) = children.first_mut() {
            child.layout(constraints)
        } else {
            Size::ZERO
        }
    }

    fn size(&self) -> Size {
        Size::ZERO
    }

    fn hit_test(&self, _point: crate::geometry::Point) -> bool {
        true
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn render(&mut self) {
        // Modifier doesn't directly render
    }
}

/// Exit event modifier - adds hover exit handler to any view
#[derive(Clone)]
pub struct OnExit<V: View, F: Clone + 'static> {
    inner: V,
    callback: F,
}

impl<V: View, F: Fn() + Clone + 'static> OnExit<V, F> {
    /// Create a new OnExit modifier
    pub fn new(inner: V, callback: F) -> Self {
        Self { inner, callback }
    }

    /// Get the inner view
    pub fn inner(&self) -> &V {
        &self.inner
    }

    /// Get the callback
    pub fn callback(&self) -> &F {
        &self.callback
    }
}

impl<V: View + Clone, F: Fn() + Clone + 'static> View for OnExit<V, F> {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::with_children(
            self.clone(),
            OnExitRenderObject::new(),
            vec![self.inner.create_element()],
        ))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn crate::state::Listenable> {
        self.inner.listenables()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Exit RenderObject
pub struct OnExitRenderObject {
    is_hovered: bool,
}

impl OnExitRenderObject {
    pub fn new() -> Self {
        Self { is_hovered: false }
    }
}

impl ElementRenderObject for OnExitRenderObject {
    fn layout(&mut self, _constraints: crate::element::LayoutConstraints) -> Size {
        Size::ZERO
    }

    fn layout_with_children(
        &mut self,
        constraints: crate::element::LayoutConstraints,
        children: &mut [Box<dyn Element>],
    ) -> Size {
        if let Some(child) = children.first_mut() {
            child.layout(constraints)
        } else {
            Size::ZERO
        }
    }

    fn size(&self) -> Size {
        Size::ZERO
    }

    fn hit_test(&self, _point: crate::geometry::Point) -> bool {
        true
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn render(&mut self) {
        // Modifier doesn't directly render
    }
}
