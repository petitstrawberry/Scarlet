//! Menu components for ScarletUI
//!
//! Provides macOS-style menu bar components:
//! - MenuItem: Individual menu items in the menu bar
//! - MenuBar: Container for menu items (horizontal layout)
//! - Menu: Dropdown menu content

mod menu;
mod menu_bar;
mod menu_item;

pub use menu::{Menu, MenuAction, MenuItemContent, MenuRenderObject};
pub use menu_bar::{MenuBar, MenuBarElement};
pub use menu_item::MenuItem;
pub(crate) use menu_item::MenuItemRenderObject;
