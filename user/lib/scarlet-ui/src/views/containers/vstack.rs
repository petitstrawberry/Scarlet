//! VStack - Vertical stack layout container
//!
//! Arranges children in a vertical column with spacing.

use alloc::vec::Vec;
use core::any::Any;
use crate::view::View;
use crate::element::{Element, VStackElement};
use alloc::boxed::Box;
use super::ViewTuple;

/// VStack View - arranges children vertically
///
/// # Examples
///
/// ```ignore
/// let stack = VStack::new((
///     Text::new("Hello"),
///     Text::new("World"),
/// ))
/// .spacing(10.0)
/// .alignment(Alignment::Center);
/// ```
pub struct VStack<C: ViewTuple> {
    content: C,
    spacing: f32,
    alignment: crate::geometry::Alignment,
}

impl<C: ViewTuple> VStack<C> {
    /// Create a new VStack with the given content tuple
    pub fn new(content: C) -> Self {
        Self {
            content,
            spacing: 0.0,
            alignment: crate::geometry::Alignment::Center,
        }
    }

    /// Set spacing between children
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    /// Set alignment for children
    pub fn alignment(mut self, alignment: crate::geometry::Alignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Get spacing
    pub fn get_spacing(&self) -> f32 {
        self.spacing
    }

    /// Get alignment
    pub fn get_alignment(&self) -> crate::geometry::Alignment {
        self.alignment
    }
}

impl<C: ViewTuple + Clone> Clone for VStack<C> {
    fn clone(&self) -> Self {
        Self {
            content: self.content.clone(),
            spacing: self.spacing,
            alignment: self.alignment,
        }
    }
}

impl<C: ViewTuple + Clone + 'static> View for VStack<C> {
    fn create_element(&self) -> Box<dyn Element> {
        let children = self.content.create_elements();
        Box::new(VStackElement::new(children, self.spacing, self.alignment))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn crate::state::Listenable> {
        let mut listenables = alloc::vec::Vec::new();
        self.content.collect_listenables(&mut listenables);
        listenables
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

