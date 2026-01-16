//! Menu system for application menu bars
//!
//! This module provides a macOS-style menu system where:
//! - Applications can register their menus
//! - The taskbar displays menus for the focused app
//! - Default Scarlet menus are shown when no app is focused

use scarlet_std::string::String;
use scarlet_std::vec::Vec;
use scarlet_std::boxed::Box;

/// A menu bar menu (e.g., "File", "Edit", "View")
pub struct Menu {
    /// Menu title (e.g., "File", "Edit", "View")
    pub title: String,

    /// Menu items in this menu
    pub items: Vec<MenuItem>,
}

impl Menu {
    /// Create a new menu with the given title
    pub fn new(title: &str) -> Self {
        Self {
            title: String::from(title),
            items: Vec::new(),
        }
    }

    /// Add a menu item
    pub fn item(mut self, item: MenuItem) -> Self {
        self.items.push(item);
        self
    }

    /// Add a separator
    pub fn separator(mut self) -> Self {
        self.items.push(MenuItem::separator());
        self
    }

    /// Add multiple items
    pub fn items(mut self, items: Vec<MenuItem>) -> Self {
        self.items.extend(items);
        self
    }
}

/// A menu item (e.g., "New", "Open", "Save")
pub struct MenuItem {
    /// Item label
    pub label: String,

    /// Keyboard shortcut (e.g., "Cmd+N", "Cmd+O")
    pub shortcut: Option<String>,

    /// Item type
    pub item_type: MenuItemType,
}

/// Menu item type
pub enum MenuItemType {
    /// Normal action item
    Action {
        /// Callback when the item is activated
        callback: Option<Box<dyn FnMut() + Send + 'static>>,
    },

    /// Separator line
    Separator,

    /// Submenu
    Submenu {
        /// Submenu items
        menus: Vec<Menu>,
    },
}

impl MenuItem {
    /// Create a new action menu item
    pub fn action(label: &str) -> Self {
        Self {
            label: String::from(label),
            shortcut: None,
            item_type: MenuItemType::Action {
                callback: None,
            },
        }
    }

    /// Create a new action menu item with a callback
    pub fn action_with_callback(label: &str, callback: impl FnMut() + Send + 'static) -> Self {
        Self {
            label: String::from(label),
            shortcut: None,
            item_type: MenuItemType::Action {
                callback: Some(Box::new(callback)),
            },
        }
    }

    /// Set the keyboard shortcut
    pub fn shortcut(mut self, shortcut: &str) -> Self {
        self.shortcut = Some(String::from(shortcut));
        self
    }

    /// Create a separator item
    pub fn separator() -> Self {
        Self {
            label: String::new(),
            shortcut: None,
            item_type: MenuItemType::Separator,
        }
    }

    /// Create a submenu item
    pub fn submenu(label: &str, menus: Vec<Menu>) -> Self {
        Self {
            label: String::from(label),
            shortcut: None,
            item_type: MenuItemType::Submenu { menus },
        }
    }

    /// Check if this item is a separator
    pub fn is_separator(&self) -> bool {
        matches!(self.item_type, MenuItemType::Separator)
    }

    /// Execute the callback if this is an action item
    pub fn execute(&mut self) {
        if let MenuItemType::Action { callback } = &mut self.item_type {
            if let Some(cb) = callback {
                cb();
            }
        }
    }
}

/// Application menu structure
///
/// Contains all menus for an application.
/// The first menu should typically be the application name menu.
pub struct ApplicationMenu {
    /// Application menus
    pub menus: Vec<Menu>,
}

impl ApplicationMenu {
    /// Create a new application menu
    pub fn new() -> Self {
        Self {
            menus: Vec::new(),
        }
    }

    /// Add a menu
    pub fn menu(mut self, menu: Menu) -> Self {
        self.menus.push(menu);
        self
    }

    /// Add multiple menus
    pub fn menus(mut self, menus: Vec<Menu>) -> Self {
        self.menus.extend(menus);
        self
    }

    /// Get the number of menus
    pub fn len(&self) -> usize {
        self.menus.len()
    }

    /// Check if there are no menus
    pub fn is_empty(&self) -> bool {
        self.menus.is_empty()
    }
}

impl Default for ApplicationMenu {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a default "Scarlet" menu for when no app is focused
pub fn default_scarlet_menu() -> ApplicationMenu {
    use scarlet_std::vec;

    ApplicationMenu {
        menus: vec![
            Menu::new("Scarlet")
                .item(MenuItem::action("About Scarlet"))
                .separator()
                .item(MenuItem::action("Settings..."))
                .separator()
                .item(MenuItem::action("Quit")),
            Menu::new("File")
                .item(MenuItem::action("New Window"))
                .separator()
                .item(MenuItem::action("Close Window")),
            Menu::new("Edit")
                .item(MenuItem::action("Undo").shortcut("Cmd+Z"))
                .item(MenuItem::action("Redo").shortcut("Cmd+Shift+Z"))
                .separator()
                .item(MenuItem::action("Cut").shortcut("Cmd+X"))
                .item(MenuItem::action("Copy").shortcut("Cmd+C"))
                .item(MenuItem::action("Paste").shortcut("Cmd+V"))
                .item(MenuItem::action("Select All").shortcut("Cmd+A")),
            Menu::new("View")
                .item(MenuItem::action("Enter Fullscreen")),
            Menu::new("Window")
                .item(MenuItem::action("Minimize"))
                .item(MenuItem::action("Zoom"))
                .separator()
                .item(MenuItem::action("Bring All to Front")),
            Menu::new("Help")
                .item(MenuItem::action("Scarlet Help")),
        ],
    }
}
