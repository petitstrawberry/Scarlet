#![no_std]

pub extern crate scarlet_std as std;

mod buffer;
mod containers;
mod dirty;
mod event;
mod geometry;
pub mod graphics;
mod layout;
mod node_id;
mod state;
pub mod theme;
mod traits;
mod views;

mod app;

pub use buffer::Buffer;
pub use dirty::DirtyFlags;
pub use event::{Event, EventContext, EventDispatcher, EventPhase, HitResult, KeyEvent, MouseEvent, MouseEventKind};
pub use geometry::{Color, Point, Rect, Size};
pub use layout::{Alignment, LayoutConstraints};
pub use node_id::NodeId;
pub use containers::{HStack, VStack, Window, ZStack};
pub use state::{State, SubscriptionId};
pub use theme::{ColorScheme, Theme, get_theme, set_theme, with_theme, init as init_theme};
pub use traits::{RenderNode, UpdateResult, View};
pub use views::{Button, ButtonColors, Image, Rectangle, Slider, Text, TextField, Toggle};
pub use views::spacer::Spacer;  // Export as marker type (not a View)
pub use app::{App, Application};

// Re-export macros
extern crate scarlet_ui_macros;
pub use scarlet_ui_macros::{hstack, view, View, vstack, zstack};

pub mod prelude {
    pub use crate::app::{App, Application};
    pub use crate::containers::{HStack, VStack, Window, ZStack};
    pub use crate::event::{Event, EventContext, EventPhase};
    pub use crate::geometry::Color;
    pub use crate::layout::{Alignment, LayoutConstraints};
    pub use crate::state::{State, SubscriptionId};
    pub use crate::theme::{ColorScheme, Theme, get_theme, set_theme, with_theme};
    pub use crate::traits::{RenderNode, View};
    pub use crate::views::{Button, Image, Rectangle, Slider, Spacer, Text, TextField, Toggle};
    pub use crate::{NodeId, Point, Rect, Size};
    pub use scarlet_ui_macros::{hstack, vstack, View, zstack};
}
