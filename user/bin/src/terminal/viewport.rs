//! Reveal a PTY caret above the input panel without changing its rows or columns.
use alloc::{boxed::Box, vec, vec::Vec};
use core::any::Any;
use scarlet_ui::{
    Element, Point, Rect, Size, State, View,
    element::{
        ElementRenderObject, LayoutConstraints, RenderElement, ScrollOffsetUpdate, UpdateResult,
    },
};

#[derive(Clone)]
pub(super) struct TerminalViewport<V: View + Clone> {
    pub content: V,
    pub caret: Option<Rect>,
    pub pan: State<f32>,
}

impl<V: View + Clone + 'static> View for TerminalViewport<V> {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::with_view_children_and_updater(
            self.clone(),
            |view| ViewportRender {
                caret: view.caret,
                pan: view.pan.clone(),
                size: Size::ZERO,
            },
            |render, view| {
                render.caret = view.caret;
                render.pan = view.pan.clone();
                UpdateResult::Updated
            },
            |view| vec![view.content.clone_view()],
        ))
    }

    fn listenables(&self) -> Vec<&dyn scarlet_ui::Listenable> {
        self.content.listenables()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct ViewportRender {
    caret: Option<Rect>,
    pan: State<f32>,
    size: Size,
}

impl ElementRenderObject for ViewportRender {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        self.size = Size::new(constraints.max_width, constraints.max_height);
        self.size
    }

    fn layout_with_children(
        &mut self,
        constraints: LayoutConstraints,
        children: &mut [Box<dyn Element>],
    ) -> Size {
        if let Some(child) = children.first_mut() {
            self.size = child.layout(constraints);
            child.set_position(Point::new(0.0, -self.pan.get()));
        }
        self.size
    }

    fn avoid_input_occlusion(
        &mut self,
        occlusion: Option<Rect>,
        _caret: Option<Rect>,
        _reveal: bool,
    ) -> bool {
        // The terminal owns its TextInput context. Its PTY caret therefore
        // comes from the app, rather than an editable ScarletUI text element.
        let next = match (occlusion, self.caret) {
            (Some(area), Some(caret))
                if caret.left() < area.right()
                    && caret.right() > area.left()
                    && area.size.width > 0.0
                    && area.size.height > 0.0 =>
            {
                let bottom = area.top().max(caret.size.height).min(self.size.height);
                (caret.bottom() - bottom).max(0.0).min(caret.top().max(0.0))
            }
            (Some(_), None) => return false, // Scrollback has no live caret to reveal.
            _ => 0.0,
        };
        if self.pan.get() == next {
            return false;
        }
        self.pan.set(next);
        true
    }

    fn apply_scroll_offset(&mut self, children: &mut [Box<dyn Element>]) -> ScrollOffsetUpdate {
        if let Some(child) = children.first_mut() {
            child.set_position(Point::new(0.0, -self.pan.get()));
        }
        ScrollOffsetUpdate::NeedsComposite
    }

    fn clip_bounds(&self, origin: Point) -> Option<(Rect, f32)> {
        Some((Rect::new(origin, self.size), 0.0))
    }

    fn size(&self) -> Size {
        self.size
    }
    fn render(&mut self) {}
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
