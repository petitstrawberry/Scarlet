//! RenderElement - wraps RenderObjects for leaf elements
//!
//! RenderElement represents leaf nodes in the element tree that directly
//! render content (text, rectangles, images, etc.).

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::any::Any;

use crate::element::{Element, ElementId, LayoutConstraints, UpdateResult};
use crate::geometry::{Point, Rect, Size};
use crate::view::View;

/// RenderObject trait for leaf rendering nodes
///
/// RenderObjects are responsible for:
/// - Computing layout within constraints
/// - Rendering to a buffer
/// - Hit testing
pub trait RenderObject: Any {
    /// Layout this RenderObject and return its size
    fn layout(&mut self, constraints: LayoutConstraints) -> Size;

    /// Get the current size
    fn size(&self) -> Size;

    /// Render to buffer
    ///
    /// For leaf nodes, this renders content to the buffer.
    fn render(&mut self);

    /// Get the buffer (for compositing)
    ///
    /// Returns the buffer if this RenderObject has rendered content.
    /// Returns None for container nodes.
    fn get_buffer(&self) -> Option<&crate::buffer::Buffer> {
        None
    }

    /// Clear any cached buffers owned by this RenderObject.
    fn clear_buffer(&mut self) {}

    /// Hit test - check if a point is within this RenderObject
    fn hit_test(&self, point: Point) -> bool {
        let bounds = Rect {
            origin: Point::ZERO,
            size: self.size(),
        };
        bounds.contains(point)
    }

    /// Get as Any for downcasting
    fn as_any(&self) -> &dyn Any;

    /// Get as Any mut for downcasting
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Update this RenderObject from a new View
    ///
    /// This is called when the View has changed and the RenderObject
    /// should update its properties to match.
    ///
    /// Returns UpdateResult indicating whether the RenderObject was updated,
    /// needs replacement, or has no changes.
    ///
    /// Default implementation returns Replaced (requires full rebuild).
    /// Implementations should override this to provide efficient updates.
    fn update(&mut self, _new_view: &dyn View) -> UpdateResult {
        // Default: cannot update, need to replace
        UpdateResult::Replaced
    }

    /// Layout this RenderObject and its children
    ///
    /// Container render objects can override this to implement custom layout
    /// of their children (Flutter-style).
    fn layout_with_children(
        &mut self,
        constraints: LayoutConstraints,
        children: &mut [Box<dyn Element>],
    ) -> Size {
        let size = self.layout(constraints);

        for child in children {
            let child_constraints = if size.width.is_infinite() || size.height.is_infinite() {
                constraints
            } else {
                LayoutConstraints::tight(size.width, size.height)
            };
            child.layout(child_constraints);
            child.set_position(crate::geometry::Point::ZERO);
        }

        size
    }
}

/// Element that wraps a RenderObject
///
/// RenderElement holds both a View and its corresponding RenderObject,
/// enabling reconciliation during updates.
///
/// # Type Parameters
/// * `V` - The View type that created this element (must be Clone)
/// * `R` - The RenderObject type that handles rendering
pub struct RenderElement<V: View + Clone, R: RenderObject> {
    id: ElementId,
    view: V,
    render_object: R,
    children: Vec<Box<dyn Element>>,
    position: Point,
    last_constraints: Option<LayoutConstraints>,
}

impl<V: View + Clone, R: RenderObject> RenderElement<V, R> {
    /// Create a new RenderElement with a View and RenderObject
    pub fn new(view: V, render_object: R) -> Self {
        Self {
            id: ElementId::generate(),
            view,
            render_object,
            children: Vec::new(),
            position: Point::ZERO,
            last_constraints: None,
        }
    }

    /// Create a new RenderElement with a View, RenderObject, and children
    pub fn with_children(view: V, render_object: R, children: Vec<Box<dyn Element>>) -> Self {
        Self {
            id: ElementId::generate(),
            view,
            render_object,
            children,
            position: Point::ZERO,
            last_constraints: None,
        }
    }

    /// Get the View
    pub fn view(&self) -> &V {
        &self.view
    }

    /// Get mutable reference to the View
    pub fn view_mut(&mut self) -> &mut V {
        &mut self.view
    }

    /// Get the RenderObject
    pub fn render_object(&self) -> &R {
        &self.render_object
    }

    /// Get mutable reference to the RenderObject
    pub fn render_object_mut(&mut self) -> &mut R {
        &mut self.render_object
    }

    /// Add a child element
    pub fn add_child(&mut self, child: Box<dyn Element>) {
        self.children.push(child);
    }
}

impl<V: View + Clone, R: RenderObject> Element for RenderElement<V, R> {
    fn id(&self) -> ElementId {
        self.id
    }

    fn type_name(&self) -> &str {
        "RenderElement"
    }

    fn type_name_debug(&self) -> alloc::string::String {
        alloc::format!("RenderElement<{}, {}>",
            core::any::type_name::<V>(),
            core::any::type_name::<R>()
        )
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn children(&self) -> &[Box<dyn Element>] {
        &self.children
    }

    fn children_mut(&mut self) -> &mut [Box<dyn Element>] {
        &mut self.children
    }

    fn update(&mut self, new_view: &dyn View) -> UpdateResult {
        // Try to downcast the new_view to the same type as our stored view
        if let Some(typed_view) = new_view.as_any().downcast_ref::<V>() {
            // Update the stored view (clone from the reference)
            self.view = typed_view.clone();
            // Delegate to the RenderObject's update method
            self.render_object.update(new_view)
        } else {
            // Type mismatch - need to replace
            UpdateResult::Replaced
        }
    }

    fn rebuild(&mut self) -> UpdateResult {
        // RenderElement doesn't need to rebuild since properties are
        // updated directly through the update() method.
        // The stored view remains the same, and changes happen through
        // State updates triggering update() calls.
        UpdateResult::NoChange
    }

    fn mount(&mut self) {
        for child in &mut self.children {
            child.mount();
        }
    }

    fn unmount(&mut self) {
        for child in &mut self.children {
            child.unmount();
        }
    }

    fn flex_factor(&self) -> u32 {
        // Check if this is a Spacer (which should expand to fill available space)
        let type_name = core::any::type_name_of_val(&self.render_object);
        if type_name.contains("SpacerRenderObject") {
            1
        } else {
            0
        }
    }

    fn fill_width(&self) -> bool {
        if let Some(frame) = self
            .render_object
            .as_any()
            .downcast_ref::<crate::views::FrameRenderObject>()
        {
            return matches!(frame.width_value(), Some(w) if !w.is_finite());
        }

        if self.children.len() == 1 {
            return self.children[0].fill_width();
        }

        false
    }

    fn fill_height(&self) -> bool {
        if let Some(frame) = self
            .render_object
            .as_any()
            .downcast_ref::<crate::views::FrameRenderObject>()
        {
            return matches!(frame.height_value(), Some(h) if !h.is_finite());
        }

        if self.children.len() == 1 {
            return self.children[0].fill_height();
        }

        false
    }

    fn wants_keyboard_focus(&self) -> bool {
        if self
            .view
            .as_any()
            .downcast_ref::<crate::views::TextField>()
            .is_some_and(|field| field.focused_state().get())
        {
            return true;
        }
        self.render_object
            .as_any()
            .downcast_ref::<crate::views::modifiers::FocusableRenderObject>()
            .is_some_and(|focusable| focusable.is_focused())
    }

    fn accepts_keyboard_focus(&self) -> bool {
        self.view
            .as_any()
            .downcast_ref::<crate::views::TextField>()
            .is_some()
            || self
                .render_object
                .as_any()
                .downcast_ref::<crate::views::modifiers::FocusableRenderObject>()
                .is_some()
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        self.last_constraints = Some(constraints);
        // Delegate layout to the RenderObject (which may layout children)
        let type_name = core::any::type_name_of_val(&self.render_object);
        if crate::debug::is_enabled() {
            scarlet_std::println!("[RenderElement::layout] START: type_name={}, constraints=({:?}, {:?}) -> ({:?}, {:?})",
                type_name, constraints.min_width, constraints.min_height, constraints.max_width, constraints.max_height);
        }
        let size = self
            .render_object
            .layout_with_children(constraints, &mut self.children);
        if crate::debug::is_enabled() {
            scarlet_std::println!("[RenderElement::layout] render_object returned size={}x{}", size.width, size.height);
        }

        size
    }

    fn last_layout_constraints(&self) -> Option<LayoutConstraints> {
        self.last_constraints
    }

    fn set_last_layout_constraints(&mut self, constraints: LayoutConstraints) {
        self.last_constraints = Some(constraints);
    }

    fn position(&self) -> Point {
        self.position
    }

    fn set_position(&mut self, position: Point) {
        self.position = position;
    }

    fn bounds(&self) -> Rect {
        Rect {
            origin: self.position,
            size: self.render_object.size(),
        }
    }

    fn hit_test(&self, point: Point) -> bool {
        // Translate point to local coordinates
        let local_point = Point {
            x: point.x - self.position.x,
            y: point.y - self.position.y,
        };
        self.render_object.hit_test(local_point)
    }

    fn render(&mut self) {
        // Render this element first
        self.render_object.render();

        // Render children
        for child in &mut self.children {
            child.render();
        }

        // TODO: Composite child buffers into parent buffer if parent has a buffer
        // This is currently handled by specialized Elements like WindowRenderElement
    }

    fn get_buffer(&self) -> Option<&crate::buffer::Buffer> {
        self.render_object.get_buffer()
    }

    fn clear_buffers(&mut self) {
        self.render_object.clear_buffer();
        for child in self.children.iter_mut() {
            child.clear_buffers();
        }
    }

    fn render_object(&self) -> Option<&dyn RenderObject> {
        Some(&self.render_object)
    }

    fn render_object_mut(&mut self) -> Option<&mut dyn RenderObject> {
        Some(&mut self.render_object)
    }

    fn handle_event(&mut self, _event: &crate::event::Event, _phase: crate::event::Phase) -> bool {
        use crate::event::{Event, MouseButton, MouseEvent, Phase};

        if let Event::Keyboard(key_event) = _event {
            if _phase == Phase::Target
                && let Some(text_field) = self.view.as_any().downcast_ref::<crate::views::TextField>()
            {
                return crate::views::text_field::handle_text_field_keyboard(text_field, *key_event);
            }
            if (_phase == Phase::Target || _phase == Phase::Bubble)
                && let Some(render_object) = self
                    .render_object
                    .as_any_mut()
                    .downcast_mut::<crate::views::modifiers::OnKeyRenderObject>()
            {
                return render_object.invoke_on_key(*key_event);
            }
            for child in self.children.iter_mut() {
                if child.handle_event(_event, _phase) {
                    return true;
                }
            }
            return false;
        }

        if let Event::Focus(focus_event) = _event {
            if _phase == Phase::Target
                && let Some(text_field) = self.view.as_any().downcast_ref::<crate::views::TextField>()
            {
                return crate::views::text_field::handle_text_field_focus(text_field, *focus_event);
            }
            if _phase == Phase::Target
                && let Some(render_object) = self
                    .render_object
                    .as_any_mut()
                    .downcast_mut::<crate::views::modifiers::FocusableRenderObject>()
            {
                return render_object.handle_focus(*focus_event);
            }
            return false;
        }

        let Event::Mouse(mouse_event) = _event else {
            return false;
        };

        let is_target_phase = _phase == Phase::Target;
        let is_bubble_phase = _phase == Phase::Bubble;

        if (is_target_phase || is_bubble_phase)
            && self
                .render_object
                .as_any_mut()
                .downcast_mut::<crate::views::modifiers::OnClickRenderObject>()
                .is_some()
        {
            if let MouseEvent::ButtonReleased { button: MouseButton::Left, .. } = mouse_event {
                if let Some(render_object) = self
                    .render_object
                    .as_any_mut()
                    .downcast_mut::<crate::views::modifiers::OnClickRenderObject>()
                {
                    render_object.invoke_on_click();
                    crate::pipeline::mark_element_needs_paint(self.id);
                    return true;
                }
            }
        }

        if !is_target_phase {
            return false;
        }

        if let Some(button) = self.view.as_any().downcast_ref::<crate::views::Button>() {
            if let Some(render_object) = self
                .render_object
                .as_any_mut()
                .downcast_mut::<crate::views::ButtonRenderObject>()
            {
                if crate::debug::is_enabled() {
                    scarlet_std::println!(
                        "[RenderElement] Button event id={:?}: {:?}",
                        self.id,
                        mouse_event
                    );
                }
                match mouse_event {
                    MouseEvent::Entered { .. } => {
                        render_object.set_hovered(true);
                        crate::pipeline::mark_element_needs_paint(self.id);
                        return true;
                    }
                    MouseEvent::Exited { .. } => {
                        render_object.set_hovered(false);
                        render_object.set_pressed(false);
                        crate::pipeline::mark_element_needs_paint(self.id);
                        return true;
                    }
                    MouseEvent::ButtonPressed { button: MouseButton::Left, .. } => {
                        render_object.set_pressed(true);
                        crate::pipeline::mark_element_needs_paint(self.id);
                        return true;
                    }
                    MouseEvent::ButtonReleased { button: MouseButton::Left, .. } => {
                        if render_object.is_pressed() {
                            button.invoke_on_click();
                        }
                        render_object.set_pressed(false);
                        crate::pipeline::mark_element_needs_paint(self.id);
                        return true;
                    }
                    _ => {}
                }
            }
        }

        if let Some(menu_item) = self.view.as_any().downcast_ref::<crate::views::MenuItem>() {
            if let Some(render_object) = self
                .render_object
                .as_any_mut()
                .downcast_mut::<crate::views::menu::MenuItemRenderObject>()
            {
                match mouse_event {
                    MouseEvent::Entered { .. } => {
                        render_object.set_hovered(true);
                        menu_item.invoke_on_hover();
                        crate::pipeline::mark_element_needs_paint(self.id);
                        return true;
                    }
                    MouseEvent::Moved { .. } => {
                        if !render_object.is_hovered() {
                            render_object.set_hovered(true);
                        }
                        menu_item.invoke_on_hover();
                        crate::pipeline::mark_element_needs_paint(self.id);
                        return true;
                    }
                    MouseEvent::Exited { .. } => {
                        render_object.set_hovered(false);
                        render_object.set_pressed(false);
                        crate::pipeline::mark_element_needs_paint(self.id);
                        return true;
                    }
                    MouseEvent::ButtonPressed { button: MouseButton::Left, .. } => {
                        render_object.set_pressed(true);
                        crate::pipeline::mark_element_needs_paint(self.id);
                        return true;
                    }
                    MouseEvent::ButtonReleased { button: MouseButton::Left, .. } => {
                        if render_object.is_pressed() {
                            menu_item.invoke_on_click();
                        }
                        render_object.set_pressed(false);
                        crate::pipeline::mark_element_needs_paint(self.id);
                        return true;
                    }
                    _ => {}
                }
            }
        }

        if let Some(_menu) = self.view.as_any().downcast_ref::<crate::views::Menu>() {
            if let Some(render_object) = self
                .render_object
                .as_any_mut()
                .downcast_mut::<crate::views::menu::MenuRenderObject>()
            {
                match mouse_event {
                    MouseEvent::Entered { x, y } | MouseEvent::Moved { x, y } => {
                        let local_x = *x as f32;
                        let local_y = *y as f32;
                        let hovered = render_object.hit_test(local_x, local_y);
                        if hovered != render_object.hovered() {
                            render_object.set_hovered(hovered);
                            crate::pipeline::mark_element_needs_paint(self.id);
                        }
                        return true;
                    }
                    MouseEvent::Exited { .. } => {
                        if render_object.hovered().is_some() {
                            render_object.set_hovered(None);
                            crate::pipeline::mark_element_needs_paint(self.id);
                        }
                        return true;
                    }
                    MouseEvent::ButtonReleased { button: MouseButton::Left, x, y } => {
                        let local_x = *x as f32;
                        let local_y = *y as f32;
                        if let Some(index) = render_object.hit_test(local_x, local_y) {
                            render_object.invoke_item(index);
                            return true;
                        }
                    }
                    _ => {}
                }
            }
        }

        if let Some(toggle) = self.view.as_any().downcast_ref::<crate::views::Toggle>() {
            if let MouseEvent::ButtonReleased { button: MouseButton::Left, .. } = mouse_event {
                if crate::debug::is_enabled() {
                    scarlet_std::println!(
                        "[RenderElement] Toggle click id={:?}",
                        self.id
                    );
                }
                let state = toggle.get_is_on().clone();
                state.update(|value| *value = !*value);
                return true;
            }
        }

        if let Some(slider) = self.view.as_any().downcast_ref::<crate::views::Slider>() {
            if let Some(render_object) = self
                .render_object
                .as_any_mut()
                .downcast_mut::<crate::views::SliderRenderObject>()
            {
                let dragging_state = slider.get_dragging().clone();
                if crate::debug::is_enabled() {
                    scarlet_std::println!(
                        "[RenderElement] Slider event id={:?}: {:?} dragging={} size=({:.1},{:.1}) pos=({:.1},{:.1})",
                        self.id,
                        mouse_event,
                        render_object.is_dragging(),
                        render_object.size().width,
                        render_object.size().height,
                        self.position.x,
                        self.position.y
                    );
                }
                fn update_slider_value(
                    render_object: &mut crate::views::SliderRenderObject,
                    slider: &crate::views::Slider,
                    id: ElementId,
                    x: i32,
                    commit: bool,
                ) -> bool {
                    let local_x = x as f32;
                    let new_value = render_object.value_from_local_x(local_x);
                    let state_value = slider.get_value().get();
                    if crate::debug::is_enabled() {
                        scarlet_std::println!(
                            "[RenderElement] Slider update: local_x={:.1} new_value={:.3} state={:.3}",
                            local_x,
                            new_value,
                            state_value
                        );
                    }
                    let mut changed = false;
                    if (render_object.get_value() - new_value).abs() > 0.001 {
                        render_object.set_value(new_value);
                        crate::pipeline::mark_element_needs_paint(id);
                        changed = true;
                    }
                    if commit && (state_value - new_value).abs() > 0.001 {
                        slider.get_value().set(new_value);
                        changed = true;
                    }
                    changed
                }

                match mouse_event {
                    MouseEvent::ButtonPressed { button: MouseButton::Left, x, .. } => {
                        render_object.set_dragging(true);
                        if !dragging_state.get() {
                            dragging_state.set(true);
                        }
                        update_slider_value(render_object, slider, self.id, *x, true);
                        return true;
                    }
                    MouseEvent::Moved { x, .. } => {
                        if render_object.is_dragging() {
                            update_slider_value(render_object, slider, self.id, *x, true);
                            return true;
                        }
                    }
                    MouseEvent::ButtonReleased { button: MouseButton::Left, x, .. } => {
                        if render_object.is_dragging() {
                            update_slider_value(render_object, slider, self.id, *x, true);
                            render_object.set_dragging(false);
                            if dragging_state.get() {
                                dragging_state.set(false);
                            }
                            return true;
                        }
                    }
                    MouseEvent::Exited { .. } => {}
                    _ => {}
                }
            }
        }

        // Handle CanvasView events
        if let Some(_canvas) = self.view.as_any().downcast_ref::<crate::views::CanvasView>() {
            if let Some(render_object) = self
                .render_object
                .as_any()
                .downcast_ref::<crate::views::CanvasRenderObject>()
            {
                if let Some(handler) = render_object.event_handler() {
                    if let Ok(mut handler) = handler.try_borrow_mut() {
                        let handled = handler(_event);
                        if handled {
                            crate::pipeline::mark_element_needs_paint(self.id);
                        }
                        return handled;
                    }
                }
            }
        }

        // Handle NavigationView events - check render object directly
        if let Some(render_object) = self
            .render_object
            .as_any_mut()
            .downcast_mut::<crate::views::navigation::NavigationViewRenderObject>()
        {
            match mouse_event {
                MouseEvent::Entered { x, y } | MouseEvent::Moved { x, y } => {
                    // Coordinates are already localized by EventDispatcher
                    let local_x = *x as f32;
                    let local_y = *y as f32;

                    // Check if point is within sidebar
                    if local_x >= 0.0 && local_x <= render_object.sidebar_width() {
                        // Calculate which item is being hovered
                        if let Some(index) = render_object.index_at_y(local_y) {
                            if index < render_object.link_count() && render_object.hovered_index() != Some(index) {
                                render_object.set_hovered_index(Some(index));
                                crate::pipeline::mark_element_needs_paint(self.id);
                            }
                        } else {
                            if render_object.hovered_index().is_some() {
                                render_object.set_hovered_index(None);
                                crate::pipeline::mark_element_needs_paint(self.id);
                            }
                        }
                    } else {
                        if render_object.hovered_index().is_some() {
                            render_object.set_hovered_index(None);
                            crate::pipeline::mark_element_needs_paint(self.id);
                        }
                    }
                    return true;
                }
                MouseEvent::Exited { .. } => {
                    if render_object.hovered_index().is_some() {
                        render_object.set_hovered_index(None);
                        crate::pipeline::mark_element_needs_paint(self.id);
                    }
                    return true;
                }
                MouseEvent::ButtonReleased { button: MouseButton::Left, x, y } => {
                    // Coordinates are already localized by EventDispatcher
                    let local_x = *x as f32;
                    let local_y = *y as f32;

                    // Check if click is within sidebar
                    if local_x >= 0.0 && local_x <= render_object.sidebar_width() {
                        if let Some(index) = render_object.index_at_y(local_y) {
                            if index < render_object.link_count() {
                                // Update selected index
                                let selected_state = render_object.selected_index();
                                let current = selected_state.get();

                                if current != index {
                                    selected_state.set(index);
                                    crate::pipeline::mark_element_dirty(self.id);
                                }
                            }
                        }
                    }
                    return true;
                }
                _ => {}
            }
        }

        false
    }
}
