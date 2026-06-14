//! NavigationLinkTuple trait and implementations
//!
//! This module defines the trait for tuples of NavigationLinks and provides
//! implementations for tuples of size 0-16.

use crate::view::View;
use crate::views::navigation::link::NavigationLink;
use alloc::boxed::Box;

/// Trait for tuples of NavigationLinks
///
/// This trait allows NavigationView to accept tuples of NavigationLinks
/// with different types, providing static typing while maintaining flexibility.
///
/// # Examples
///
/// ```ignore
/// // Tuples of different sizes all implement NavigationLinkTuple
/// let link1 = NavigationLink::new("Home", Icon::Home, || Text::new("Home"));
/// let link2 = NavigationLink::new("Settings", Icon::Settings, || Text::new("Settings"));
///
/// // Works with 2 links
/// let nav2 = NavigationView::new((link1, link2));
///
/// // Works with 3 links
/// let link3 = NavigationLink::new("Info", Icon::Info, || Text::new("Info"));
/// let nav3 = NavigationView::new((link1, link2, link3));
/// ```
pub trait NavigationLinkTuple {
    /// Get the number of links in this tuple
    fn count(&self) -> usize;

    /// Get the label for the link at the given index
    ///
    /// # Panics
    ///
    /// Panics if index is out of bounds (>= count())
    fn get_label(&self, index: usize) -> &str;

    /// Get the icon for the link at the given index
    ///
    /// # Panics
    ///
    /// Panics if index is out of bounds (>= count())
    fn get_icon(&self, index: usize) -> &Icon;

    /// Build the content view for the link at the given index
    ///
    /// # Panics
    ///
    /// Panics if index is out of bounds (>= count())
    fn build_content(&self, index: usize) -> Box<dyn View>;
}

// Import Icon type for use in trait methods
use crate::views::navigation::link::Icon;

// Implement NavigationLinkTuple for unit type (empty tuple)
impl NavigationLinkTuple for () {
    fn count(&self) -> usize {
        0
    }

    fn get_label(&self, _index: usize) -> &str {
        panic!("Cannot access label in empty NavigationLink tuple")
    }

    fn get_icon(&self, _index: usize) -> &Icon {
        panic!("Cannot access icon in empty NavigationLink tuple")
    }

    fn build_content(&self, _index: usize) -> Box<dyn View> {
        panic!("Cannot access content in empty NavigationLink tuple")
    }
}

// Implement NavigationLinkTuple for 1-tuple
impl NavigationLinkTuple for (NavigationLink,) {
    fn count(&self) -> usize {
        1
    }

    fn get_label(&self, index: usize) -> &str {
        match index {
            0 => self.0.label(),
            _ => panic!("NavigationLink index {} out of bounds (count: 1)", index),
        }
    }

    fn get_icon(&self, index: usize) -> &Icon {
        match index {
            0 => self.0.icon(),
            _ => panic!("NavigationLink index {} out of bounds (count: 1)", index),
        }
    }

    fn build_content(&self, index: usize) -> Box<dyn View> {
        match index {
            0 => self.0.build_content(),
            _ => panic!("NavigationLink index {} out of bounds (count: 1)", index),
        }
    }
}

// Implement NavigationLinkTuple for 2-tuple
impl NavigationLinkTuple for (NavigationLink, NavigationLink) {
    fn count(&self) -> usize {
        2
    }

    fn get_label(&self, index: usize) -> &str {
        match index {
            0 => self.0.label(),
            1 => self.1.label(),
            _ => panic!("NavigationLink index {} out of bounds (count: 2)", index),
        }
    }

    fn get_icon(&self, index: usize) -> &Icon {
        match index {
            0 => self.0.icon(),
            1 => self.1.icon(),
            _ => panic!("NavigationLink index {} out of bounds (count: 2)", index),
        }
    }

    fn build_content(&self, index: usize) -> Box<dyn View> {
        match index {
            0 => self.0.build_content(),
            1 => self.1.build_content(),
            _ => panic!("NavigationLink index {} out of bounds (count: 2)", index),
        }
    }
}

// Implement NavigationLinkTuple for 3-tuple
impl NavigationLinkTuple for (NavigationLink, NavigationLink, NavigationLink) {
    fn count(&self) -> usize {
        3
    }

    fn get_label(&self, index: usize) -> &str {
        match index {
            0 => self.0.label(),
            1 => self.1.label(),
            2 => self.2.label(),
            _ => panic!("NavigationLink index {} out of bounds (count: 3)", index),
        }
    }

    fn get_icon(&self, index: usize) -> &Icon {
        match index {
            0 => self.0.icon(),
            1 => self.1.icon(),
            2 => self.2.icon(),
            _ => panic!("NavigationLink index {} out of bounds (count: 3)", index),
        }
    }

    fn build_content(&self, index: usize) -> Box<dyn View> {
        match index {
            0 => self.0.build_content(),
            1 => self.1.build_content(),
            2 => self.2.build_content(),
            _ => panic!("NavigationLink index {} out of bounds (count: 3)", index),
        }
    }
}

// Implement NavigationLinkTuple for 4-tuple
impl NavigationLinkTuple
    for (
        NavigationLink,
        NavigationLink,
        NavigationLink,
        NavigationLink,
    )
{
    fn count(&self) -> usize {
        4
    }

    fn get_label(&self, index: usize) -> &str {
        match index {
            0 => self.0.label(),
            1 => self.1.label(),
            2 => self.2.label(),
            3 => self.3.label(),
            _ => panic!("NavigationLink index {} out of bounds (count: 4)", index),
        }
    }

    fn get_icon(&self, index: usize) -> &Icon {
        match index {
            0 => self.0.icon(),
            1 => self.1.icon(),
            2 => self.2.icon(),
            3 => self.3.icon(),
            _ => panic!("NavigationLink index {} out of bounds (count: 4)", index),
        }
    }

    fn build_content(&self, index: usize) -> Box<dyn View> {
        match index {
            0 => self.0.build_content(),
            1 => self.1.build_content(),
            2 => self.2.build_content(),
            3 => self.3.build_content(),
            _ => panic!("NavigationLink index {} out of bounds (count: 4)", index),
        }
    }
}

// Implement NavigationLinkTuple for 5-tuple
impl NavigationLinkTuple
    for (
        NavigationLink,
        NavigationLink,
        NavigationLink,
        NavigationLink,
        NavigationLink,
    )
{
    fn count(&self) -> usize {
        5
    }

    fn get_label(&self, index: usize) -> &str {
        match index {
            0 => self.0.label(),
            1 => self.1.label(),
            2 => self.2.label(),
            3 => self.3.label(),
            4 => self.4.label(),
            _ => panic!("NavigationLink index {} out of bounds (count: 5)", index),
        }
    }

    fn get_icon(&self, index: usize) -> &Icon {
        match index {
            0 => self.0.icon(),
            1 => self.1.icon(),
            2 => self.2.icon(),
            3 => self.3.icon(),
            4 => self.4.icon(),
            _ => panic!("NavigationLink index {} out of bounds (count: 5)", index),
        }
    }

    fn build_content(&self, index: usize) -> Box<dyn View> {
        match index {
            0 => self.0.build_content(),
            1 => self.1.build_content(),
            2 => self.2.build_content(),
            3 => self.3.build_content(),
            4 => self.4.build_content(),
            _ => panic!("NavigationLink index {} out of bounds (count: 5)", index),
        }
    }
}
