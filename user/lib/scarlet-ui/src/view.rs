//! View trait - the core abstraction for ScarletUI
//!
//! Views are blueprints for UI elements. They implement the Factory Method pattern
//! where create_element() manufactures the corresponding Element.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::any::Any;

use crate::element::Element;

/// Factory trait for creating UI elements
///
/// Views are immutable descriptions of UI that create Elements when mounted.
/// This follows the Factory Method pattern combined with the Component pattern.
pub trait View: Any {
    /// Create an Element from this View
    ///
    /// This is called when the View is first mounted into the element tree.
    fn create_element(&self) -> Box<dyn Element>;

    /// Get all Listenable dependencies (State references) from this View
    ///
    /// The framework uses this to track when the View needs to rebuild.
    fn listenables(&self) -> Vec<&dyn crate::state::Listenable> {
        Vec::new()
    }

    /// Get this View as Any for downcasting
    fn as_any(&self) -> &dyn Any;

    /// Get the type name of this View (for debugging)
    fn type_name(&self) -> &str {
        core::any::type_name::<Self>()
    }
}

/// Helper trait for Views that can be cloned
///
/// Many Views need to be cloneable to work with the reconciliation system.
pub trait ViewClone: View {
    fn clone_view(&self) -> Box<dyn View>;
}

/// Blanket implementation for Clone + View types
impl<V: View + Clone + 'static> ViewClone for V {
    fn clone_view(&self) -> Box<dyn View> {
        Box::new(self.clone())
    }
}
