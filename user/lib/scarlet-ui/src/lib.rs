#![no_std]

extern crate scarlet_std as std;

mod buffer;
mod containers;
mod dirty;
mod event;
mod geometry;
mod layout;
mod node_id;
mod state;
mod traits;
mod views;

mod app;

pub use buffer::Buffer;
pub use dirty::DirtyFlags;
pub use event::{Event, EventContext, EventDispatcher, EventPhase, HitResult, KeyEvent, MouseEvent, MouseEventKind};
pub use geometry::{Point, Rect, Size};
pub use layout::{Alignment, LayoutConstraints};
pub use node_id::NodeId;
pub use containers::VStack;
pub use state::{State, SubscriptionId};
pub use traits::{RenderNode, UpdateResult, View};
pub use views::{Button, ButtonColors, Rectangle, Slider, Spacer, Text, TextField, Toggle};
pub use app::{App, Application};

// Re-export macros
extern crate scarlet_ui_macros;
pub use scarlet_ui_macros::{view, View, vstack};

pub mod prelude {
    pub use crate::event::{Event, EventContext, EventPhase};
    pub use crate::layout::{Alignment, LayoutConstraints};
    pub use crate::state::{State, SubscriptionId};
    pub use crate::traits::{RenderNode, View};
    pub use crate::{NodeId, Point, Rect, Size};
}
