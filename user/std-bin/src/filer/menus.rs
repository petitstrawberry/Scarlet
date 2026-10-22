//! Application menus published through ScarletUI to the desktop shell.
use super::*;
use scarlet_ui::{MenuBarModel, MenuEntry, MenuItemModel};

impl FilerApp {
    pub(super) fn menu_item(
        &self,
        id: &str,
        title: &str,
        shortcut: &str,
        enabled: bool,
        action: impl Fn(&Self) + Send + Sync + 'static,
    ) -> MenuEntry {
        let app = self.clone();
        MenuEntry::Item(
            MenuItemModel::new(id, title)
                .shortcut(shortcut)
                .enabled(enabled)
                .on_activate(Arc::new(move || action(&app))),
        )
    }

    pub(super) fn browser_commands_ready(&self) -> bool {
        self.prompt.get().is_none() && !self.operation_busy.get() && !self.details_busy.get()
    }

    pub(super) fn browser_menu_bar(&self) -> MenuBarModel {
        let ready = self.browser_commands_ready();
        let selected = self.selected_entry();
        let has_selection = ready && selected.is_some();
        let can_move = move_supported();
        let clipboard = self.clipboard.get();
        let mut history = self.history.get();
        let can_go_back = history.back(&self.current_path.get()).is_some();
        let mut history = self.history.get();
        let can_go_forward = history.forward(&self.current_path.get()).is_some();
        MenuBarModel::new(vec![
            MenuItemModel::new("file", "File").children(vec![
                self.menu_item("open", "Open", "Enter", has_selection, |a| {
                    a.open_selected(false)
                }),
                self.menu_item("new-folder", "New Folder…", "Ctrl+Shift+N", ready, |a| {
                    a.create_folder()
                }),
                self.menu_item(
                    "rename",
                    "Rename…",
                    "F2",
                    has_selection && can_move,
                    |a| a.rename_selected(),
                ),
                MenuEntry::Separator,
                self.menu_item("details", "Details…", "Ctrl+I", ready, |a| {
                    a.show_details()
                }),
                self.menu_item(
                    "browse-archive",
                    "Browse Archive…",
                    "",
                    ready
                        && selected.as_ref().is_some_and(|entry| {
                            !entry.is_directory && archive::supported(Path::new(&entry.path))
                        }),
                    |a| a.open_selected(false),
                ),
                MenuEntry::Separator,
                self.menu_item("delete", "Delete…", "Delete", has_selection, |a| {
                    a.delete_selected()
                }),
            ]),
            MenuItemModel::new("edit", "Edit").children(vec![
                self.menu_item("cut", "Cut", "Ctrl+X", has_selection && can_move, |a| {
                    a.copy_selected(true)
                }),
                self.menu_item("copy", "Copy", "Ctrl+C", has_selection, |a| {
                    a.copy_selected(false)
                }),
                self.menu_item(
                    "paste",
                    "Paste",
                    "Ctrl+V",
                    ready && clipboard.is_some_and(|item| !item.cut || can_move),
                    |a| a.paste(),
                ),
                MenuEntry::Separator,
                self.menu_item("find", "Filter This Folder…", "Ctrl+F", ready, |a| {
                    a.focus_search()
                }),
            ]),
            MenuItemModel::new("view", "View").children(vec![
                self.menu_item("refresh", "Refresh", "F5", ready, |a| a.refresh()),
                self.menu_item(
                    "hidden",
                    if self.show_hidden.get() {
                        "Hide Hidden Files"
                    } else {
                        "Show Hidden Files"
                    },
                    "Ctrl+H",
                    ready,
                    |a| a.show_hidden.set(!a.show_hidden.get()),
                ),
                MenuEntry::Item(
                    MenuItemModel::new("sort", "Sort By")
                        .enabled(ready)
                        .children(
                            ["Name A–Z", "Name Z–A", "Size, Largest First"]
                                .into_iter()
                                .enumerate()
                                .map(|(index, title)| {
                                    self.menu_item(
                                        &format!("sort-{index}"),
                                        title,
                                        "",
                                        ready && self.sort_index.get() != index,
                                        move |a| a.sort_index.set(index),
                                    )
                                })
                                .collect(),
                        ),
                ),
            ]),
            MenuItemModel::new("go", "Go").children(vec![
                self.menu_item("back", "Back", "Alt+Left", ready && can_go_back, |a| {
                    a.navigate_history(false, false)
                }),
                self.menu_item(
                    "forward",
                    "Forward",
                    "Alt+Right",
                    ready && can_go_forward,
                    |a| a.navigate_history(true, false),
                ),
                self.menu_item(
                    "up",
                    "Enclosing Folder",
                    "Alt+Up",
                    ready && Path::new(&self.current_path.get()).parent().is_some(),
                    |a| a.navigate_up(),
                ),
                MenuEntry::Separator,
                self.menu_item("home", "Home", "", ready, |a| a.navigate_to(home_path())),
                self.menu_item("computer", "Computer", "", ready, |a| {
                    a.navigate_to(ROOT_PATH)
                }),
                self.menu_item("location", "Go to Folder…", "Ctrl+L", ready, |a| {
                    a.focus_location()
                }),
            ]),
        ])
    }

    pub(super) fn focus_location(&self) {
        self.search_focus.set(0);
        self.location_focus
            .set(self.location_focus.get().wrapping_add(1).max(1));
    }

    pub(super) fn focus_search(&self) {
        self.location_focus.set(0);
        self.search_focus
            .set(self.search_focus.get().wrapping_add(1).max(1));
    }
}
