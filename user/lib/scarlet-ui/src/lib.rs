#![no_std]

extern crate scarlet_std as std;

mod buffer;
mod dirty;
mod event;
mod geometry;
mod layout;
mod node_id;
mod state;
mod traits;

pub use buffer::Buffer;
pub use dirty::DirtyFlags;
pub use event::{Event, EventContext, EventPhase, HitResult, KeyEvent, MouseEvent, MouseEventKind};
pub use geometry::{Point, Rect, Size};
pub use layout::{Alignment, LayoutConstraints};
pub use node_id::NodeId;
pub use state::{State, SubscriptionId};
pub use traits::{RenderNode, UpdateResult, View};

pub mod prelude {
    pub use crate::event::{Event, EventContext, EventPhase};
    pub use crate::layout::{Alignment, LayoutConstraints};
    pub use crate::state::{State, SubscriptionId};
    pub use crate::traits::{RenderNode, View};
    pub use crate::{NodeId, Point, Rect, Size};
}
