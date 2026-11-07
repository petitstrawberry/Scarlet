//! Keep the scroll viewport behind navigation chrome and fixed controls safe.
use scarlet_ui::{
    Element, ElementRenderObject, LayoutConstraints, Point, Rect, RenderElement, Size, View,
    element::UpdateResult, geometry::EdgeInsets, state::Listenable,
};
use std::any::Any;

#[derive(Clone)]
pub(super) struct BrowserPane<G, T, F> {
    pub grid: G,
    pub top: T,
    pub footer: F,
}

impl<G: View + Clone, T: View + Clone, F: View + Clone> View for BrowserPane<G, T, F> {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::with_view_children_and_updater(
            self.clone(),
            |_| BrowserPaneRender::default(),
            |_, _| UpdateResult::NoChange,
            |view| {
                vec![
                    view.grid.clone_view(),
                    view.top.clone_view(),
                    view.footer.clone_view(),
                ]
            },
        ))
    }

    fn listenables(&self) -> Vec<&dyn Listenable> {
        self.grid
            .listenables()
            .into_iter()
            .chain(self.top.listenables())
            .chain(self.footer.listenables())
            .collect()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Default)]
struct BrowserPaneRender {
    size: Size,
    insets: EdgeInsets,
}

impl ElementRenderObject for BrowserPaneRender {
    fn set_safe_area_insets(&mut self, insets: EdgeInsets) {
        self.insets = insets;
    }

    fn child_safe_area_insets(&self, _: EdgeInsets) -> EdgeInsets {
        // Each child gets the reservation appropriate to its own viewport below.
        EdgeInsets::ZERO
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        self.size = Size::new(
            if constraints.max_width.is_finite() {
                constraints.max_width
            } else {
                constraints.min_width
            },
            if constraints.max_height.is_finite() {
                constraints.max_height
            } else {
                constraints.min_height
            },
        );
        self.size
    }

    fn layout_with_children(
        &mut self,
        constraints: LayoutConstraints,
        children: &mut [Box<dyn Element>],
    ) -> Size {
        let size = self.layout(constraints);
        let left = self.insets.left.clamp(0., size.width);
        let top = self.insets.top.clamp(0., size.height);
        let safe = Rect::from_xywh(
            left,
            top,
            (size.width - left - self.insets.right).max(0.),
            (size.height - top - self.insets.bottom).max(0.),
        );
        let [grid, top, footer] = children else {
            return size;
        };
        let footer_size = footer.layout(LayoutConstraints::new(
            safe.size.width,
            safe.size.width,
            0.,
            safe.size.height,
        ));
        let footer_y = safe.bottom() - footer_size.height;
        footer.set_position(Point::new(safe.left(), footer_y));
        let top_size = top.layout(LayoutConstraints::new(
            safe.size.width,
            safe.size.width,
            0.,
            (footer_y - safe.top()).max(0.),
        ));
        top.set_position(safe.origin);

        // Horizontal bounds remain the navigation content column. Vertically,
        // the viewport reaches the canvas top so scrolled rows paint behind the
        // header. ScrollView consumes the top reservation as content clearance.
        grid.set_safe_area_insets(EdgeInsets::only_top(safe.top() + top_size.height));
        grid.layout(LayoutConstraints::tight(safe.size.width, footer_y.max(0.)));
        grid.set_position(Point::new(safe.left(), 0.));
        size
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
