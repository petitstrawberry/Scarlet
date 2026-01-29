//! NavigationView implementation
//!
//! Simplified implementation - content rebuilding happens via full element rebuild.

use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::string::ToString;
use core::any::Any;
use crate::view::View;
use crate::state::Listenable;
use crate::element::{Element, RenderElement};
use crate::views::navigation::render::NavigationViewRenderObject;
use crate::views::navigation::view::NavigationView;
use crate::views::navigation::tuple::NavigationLinkTuple;
use crate::views::Spacer;

// View implementation for NavigationView
impl<T> View for NavigationView<T>
where
    T: NavigationLinkTuple + Clone + 'static,
{
    fn create_element(&self) -> Box<dyn Element> {
        let selected = self.selected_index_state().get();
        let content_view = self.links().build_content(selected);

        let sidebar_placeholder = Spacer::new();
        let mut children = Vec::new();
        children.push(sidebar_placeholder.create_element());
        children.push(content_view.create_element());

        // Collect labels and icons
        let mut labels = Vec::new();
        let mut icons = Vec::new();
        for i in 0..self.links().count() {
            labels.push(self.links().get_label(i).to_string());
            icons.push(*self.links().get_icon(i));
        }

        let render_object = NavigationViewRenderObject::new(
            labels,
            icons,
            self.selected_index_state().clone(),
            self.get_sidebar_width(),
        );

        Box::new(RenderElement::with_children(
            self.clone(),
            render_object,
            children,
        ))
    }

    fn listenables(&self) -> Vec<&dyn Listenable> {
        let mut v = Vec::new();
        v.push(self.selected_index_state() as &dyn Listenable);
        v
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
