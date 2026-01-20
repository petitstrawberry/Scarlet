use crate::traits::render_node::RenderNode;

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

    fn build(&self) -> std::boxed::Box<dyn RenderNode>;

    fn as_any(&self) -> &dyn std::any::Any;
}
