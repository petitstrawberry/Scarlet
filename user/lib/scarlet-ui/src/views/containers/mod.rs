//! Container Views - Layout containers
//!
//! This module provides layout containers for arranging child views.

use alloc::vec::Vec;
use alloc::vec;
use alloc::boxed::Box;
use crate::element::Element;
use crate::view::View;
use crate::state::Listenable;

mod vstack;
mod hstack;
mod zstack;

#[cfg(test)]
mod tests;

pub use vstack::VStack;
pub use hstack::HStack;
pub use zstack::{ZStack, ZStackRenderObject};

/// Trait for tuples of Views
///
/// This trait allows containers to accept a variable number of child views
/// through tuples, providing static typing while maintaining flexibility.
///
/// # Examples
///
/// ```ignore
/// let stack = VStack::new((
///     Text::new("Hello"),
///     Text::new("World"),
/// ));
/// ```
pub trait ViewTuple {
    /// Create Elements for all views in this tuple
    fn create_elements(&self) -> Vec<Box<dyn Element>>;

    /// Collect all Listenable dependencies from views in this tuple
    fn collect_listenables<'a>(&'a self, collector: &mut Vec<&'a dyn Listenable>);
}

// Implement ViewTuple for unit type (empty tuple)
impl ViewTuple for () {
    fn create_elements(&self) -> Vec<Box<dyn Element>> {
        Vec::new()
    }

    fn collect_listenables<'a>(&'a self, _collector: &mut Vec<&'a dyn Listenable>) {
        // Empty tuple has no listenables
    }
}

// Implement ViewTuple for 1-tuple
impl<V1: View> ViewTuple for (V1,) {
    fn create_elements(&self) -> Vec<Box<dyn Element>> {
        vec![self.0.create_element()]
    }

    fn collect_listenables<'a>(&'a self, collector: &mut Vec<&'a dyn Listenable>) {
        collector.extend(self.0.listenables());
    }
}

// Implement ViewTuple for 2-tuple
impl<V1: View, V2: View> ViewTuple for (V1, V2) {
    fn create_elements(&self) -> Vec<Box<dyn Element>> {
        vec![
            self.0.create_element(),
            self.1.create_element(),
        ]
    }

    fn collect_listenables<'a>(&'a self, collector: &mut Vec<&'a dyn Listenable>) {
        collector.extend(self.0.listenables());
        collector.extend(self.1.listenables());
    }
}

// Implement ViewTuple for 3-tuple
impl<V1: View, V2: View, V3: View> ViewTuple for (V1, V2, V3) {
    fn create_elements(&self) -> Vec<Box<dyn Element>> {
        vec![
            self.0.create_element(),
            self.1.create_element(),
            self.2.create_element(),
        ]
    }

    fn collect_listenables<'a>(&'a self, collector: &mut Vec<&'a dyn Listenable>) {
        collector.extend(self.0.listenables());
        collector.extend(self.1.listenables());
        collector.extend(self.2.listenables());
    }
}

// Implement ViewTuple for 4-tuple
impl<V1: View, V2: View, V3: View, V4: View> ViewTuple for (V1, V2, V3, V4) {
    fn create_elements(&self) -> Vec<Box<dyn Element>> {
        vec![
            self.0.create_element(),
            self.1.create_element(),
            self.2.create_element(),
            self.3.create_element(),
        ]
    }

    fn collect_listenables<'a>(&'a self, collector: &mut Vec<&'a dyn Listenable>) {
        collector.extend(self.0.listenables());
        collector.extend(self.1.listenables());
        collector.extend(self.2.listenables());
        collector.extend(self.3.listenables());
    }
}

// Implement ViewTuple for 5-tuple
impl<V1: View, V2: View, V3: View, V4: View, V5: View> ViewTuple for (V1, V2, V3, V4, V5) {
    fn create_elements(&self) -> Vec<Box<dyn Element>> {
        vec![
            self.0.create_element(),
            self.1.create_element(),
            self.2.create_element(),
            self.3.create_element(),
            self.4.create_element(),
        ]
    }

    fn collect_listenables<'a>(&'a self, collector: &mut Vec<&'a dyn Listenable>) {
        collector.extend(self.0.listenables());
        collector.extend(self.1.listenables());
        collector.extend(self.2.listenables());
        collector.extend(self.3.listenables());
        collector.extend(self.4.listenables());
    }
}

// Implement ViewTuple for 6-tuple
impl<V1: View, V2: View, V3: View, V4: View, V5: View, V6: View> ViewTuple for (V1, V2, V3, V4, V5, V6) {
    fn create_elements(&self) -> Vec<Box<dyn Element>> {
        vec![
            self.0.create_element(),
            self.1.create_element(),
            self.2.create_element(),
            self.3.create_element(),
            self.4.create_element(),
            self.5.create_element(),
        ]
    }

    fn collect_listenables<'a>(&'a self, collector: &mut Vec<&'a dyn Listenable>) {
        collector.extend(self.0.listenables());
        collector.extend(self.1.listenables());
        collector.extend(self.2.listenables());
        collector.extend(self.3.listenables());
        collector.extend(self.4.listenables());
        collector.extend(self.5.listenables());
    }
}
