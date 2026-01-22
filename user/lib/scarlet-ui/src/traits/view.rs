use crate::traits::render_object::RenderObject;
use std::boxed::Box;
use std::vec::Vec;

// View trait is dyn-compatible (object-safe)
// Concrete View types should implement Clone separately
pub trait View: 'static {
    // Note: Default impl uses Self, which requires monomorphization.
    // When called through trait object (&dyn View), this uses the
    // concrete type's TypeId, not Object:: typeid.
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }

    fn type_name(&self) -> &'static str;

    fn build(&self) -> Box<dyn RenderObject>;

    fn as_any(&self) -> &dyn std::any::Any;

    /// Get child views for this view
    /// Default: no children (leaf view)
    fn children(&self) -> Vec<Box<dyn View>> {
        Vec::new()
    }

    /// Subscribe to state changes for automatic view rebuilding
    /// Default: no states to watch
    fn subscribe_states(&self, _callback: std::sync::Arc<dyn Fn() + Send + Sync>) {}
}
