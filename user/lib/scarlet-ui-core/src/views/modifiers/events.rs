//! Event modifier views
//!
//! Provides event modifiers for any view.

use crate::element::{Element, ElementRenderObject, RenderElement};
use crate::event::{FocusEvent, KeyEvent};
use crate::geometry::Size;
use crate::state::{Listenable, State};
use crate::view::View;
use alloc::boxed::Box;
use alloc::vec;
use core::any::Any;

/// Click event modifier - adds click handler to any view
#[derive(Clone)]
pub struct OnClick<V: View, F: Clone + 'static> {
    inner: V,
    callback: F,
}

/// Focusable modifier - makes any view accept keyboard focus.
#[derive(Clone)]
pub struct Focusable<V: View> {
    inner: V,
    focused: State<bool>,
}

impl<V: View> Focusable<V> {
    /// Create a new Focusable modifier.
    ///
    /// # Arguments
    ///
    /// * `inner` - Wrapped view.
    /// * `focused` - State that tracks focus.
    ///
    /// # Returns
    ///
    /// A focusable wrapper for the view.
    pub fn new(inner: V, focused: State<bool>) -> Self {
        Self { inner, focused }
    }

    /// Return the focus state.
    pub fn focused_state(&self) -> &State<bool> {
        &self.focused
    }
}

impl<V: View + Clone> View for Focusable<V> {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::with_children(
            self.clone(),
            FocusableRenderObject::new(self.focused.clone()),
            vec![self.inner.create_element()],
        ))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn Listenable> {
        let mut listenables = self.inner.listenables();
        listenables.push(&self.focused);
        listenables
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Render object for [`Focusable`].
pub struct FocusableRenderObject {
    focused: State<bool>,
    size: Size,
}

impl FocusableRenderObject {
    /// Create a new focusable render object.
    pub fn new(focused: State<bool>) -> Self {
        Self {
            focused,
            size: Size::ZERO,
        }
    }

    /// Return whether this object is currently focused.
    pub fn is_focused(&self) -> bool {
        self.focused.get()
    }

    /// Apply a focus event.
    pub fn handle_focus(&self, event: FocusEvent) -> bool {
        match event {
            FocusEvent::Gained => self.focused.set(true),
            FocusEvent::Lost => self.focused.set(false),
        }
        true
    }
}

impl ElementRenderObject for FocusableRenderObject {
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
        point.x >= 0.0 && point.y >= 0.0 && point.x < self.size.width && point.y < self.size.height
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn render(&mut self) {}
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
        Self {
            is_hovered: false,
            callback: None,
            size: Size::ZERO,
        }
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
                crate::logln!(
                    "[OnClickRenderObject::layout_with_children] size={}x{}",
                    size.width,
                    size.height
                );
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
        let result = point.x >= 0.0
            && point.x < self.size.width
            && point.y >= 0.0
            && point.y < self.size.height;
        if crate::debug::is_enabled() {
            crate::logln!(
                "[OnClickRenderObject::hit_test] point=({:?}), size={:?}, result={}",
                point,
                self.size,
                result
            );
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

/// Keyboard event modifier - adds a key handler to any view.
#[derive(Clone)]
pub struct OnKey<V: View, F: Clone + 'static> {
    inner: V,
    callback: F,
}

impl<V: View, F: Fn(KeyEvent) -> bool + Clone + 'static> OnKey<V, F> {
    /// Create a new OnKey modifier.
    ///
    /// # Arguments
    ///
    /// * `inner` - Wrapped view.
    /// * `callback` - Function called for keyboard events.
    ///
    /// # Returns
    ///
    /// A new [`OnKey`] modifier.
    pub fn new(inner: V, callback: F) -> Self {
        Self { inner, callback }
    }

    /// Get the inner view.
    pub fn inner(&self) -> &V {
        &self.inner
    }

    /// Get the callback.
    pub fn callback(&self) -> &F {
        &self.callback
    }
}

impl<V: View + Clone, F: Fn(KeyEvent) -> bool + Clone + 'static> View for OnKey<V, F> {
    fn create_element(&self) -> Box<dyn Element> {
        let mut render_object = OnKeyRenderObject::new();
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

/// Render object for [`OnKey`].
pub struct OnKeyRenderObject {
    callback: Option<Box<dyn Fn(KeyEvent) -> bool>>,
    size: Size,
}

impl OnKeyRenderObject {
    /// Create an empty key modifier render object.
    pub fn new() -> Self {
        Self {
            callback: None,
            size: Size::ZERO,
        }
    }

    /// Set the key callback.
    ///
    /// # Arguments
    ///
    /// * `callback` - Function called for keyboard events.
    pub fn set_callback(&mut self, callback: Box<dyn Fn(KeyEvent) -> bool>) {
        self.callback = Some(callback);
    }

    /// Invoke the key callback.
    ///
    /// # Arguments
    ///
    /// * `event` - Keyboard event.
    ///
    /// # Returns
    ///
    /// `true` when the event was consumed.
    pub fn invoke_on_key(&self, event: KeyEvent) -> bool {
        if crate::debug::is_enabled() {
            crate::logln!(
                "[OnKeyRenderObject] invoke: event={:?} has_callback={}",
                event,
                self.callback.is_some()
            );
        }
        let handled = self
            .callback
            .as_ref()
            .map(|callback| callback(event))
            .unwrap_or(false);
        if crate::debug::is_enabled() {
            crate::logln!("[OnKeyRenderObject] handled={}", handled);
        }
        handled
    }
}

impl ElementRenderObject for OnKeyRenderObject {
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
        point.x >= 0.0 && point.x < self.size.width && point.y >= 0.0 && point.y < self.size.height
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn render(&mut self) {
        // Modifier doesn't directly render.
    }
}
