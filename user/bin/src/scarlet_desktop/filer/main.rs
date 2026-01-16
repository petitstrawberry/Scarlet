//! Scarlet Filer (File Manager)
//!
//! File manager application for Scarlet Desktop with real file system operations

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use scarlet_ui::{
    Application, Button, Color, HStack, Label, Padding, RectView, Spacer, StackAlignment, State,
    VStack, ViewModifier, Window, WindowKind,
};
use std::{format, println, string::String, vec::Vec};

/// Color palette for modern file manager
const BG: Color = Color::rgb(248, 250, 252);
const SURFACE: Color = Color::rgb(255, 255, 255);
const SURFACE_VAR: Color = Color::rgb(241, 245, 249);
const BORDER: Color = Color::rgb(226, 232, 240);
const PRIMARY: Color = Color::rgb(59, 130, 246);
const PRIMARY_HOVER: Color = Color::rgb(37, 99, 235);
const TEXT_MAIN: Color = Color::rgb(15, 23, 42);
const TEXT_SUB: Color = Color::rgb(100, 116, 139);
const TEXT_MUTE: Color = Color::rgb(148, 163, 184);

/// File type enumeration
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Directory,
    File,
    Symlink,
    Executable,
}

impl FileType {
    /// Get icon for this file type
    fn icon(self) -> &'static str {
        match self {
            FileType::Directory => "[DIR]",
            FileType::File => "[FILE]",
            FileType::Symlink => "[LINK]",
            FileType::Executable => "[EXEC]",
        }
    }

    /// Get color for this file type
    fn color(self) -> Color {
        match self {
            FileType::Directory => Color::rgb(245, 158, 11), // Amber
            FileType::File => Color::rgb(100, 116, 139),     // Slate
            FileType::Symlink => Color::rgb(59, 130, 246),   // Blue
            FileType::Executable => Color::rgb(34, 197, 94), // Green
        }
    }

    /// Convert from fs file type byte
    fn from_fs_type(fs_type: u8) -> Self {
        match fs_type {
            0 => FileType::File,
            1 => FileType::Directory,
            2 => FileType::Symlink,
            _ => FileType::File,
        }
    }
}

/// File entry representing a file or directory
#[derive(Clone)]
pub struct FileEntry {
    pub name: String,
    pub file_type: FileType,
    pub size: u64,
}

impl FileEntry {
    /// Format file size for display
    fn format_size(&self) -> String {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;

        if self.size >= GB {
            format!("{:.1} GB", self.size as f64 / GB as f64)
        } else if self.size >= MB {
            format!("{:.1} MB", self.size as f64 / MB as f64)
        } else if self.size >= KB {
            format!("{:.1} KB", self.size as f64 / KB as f64)
        } else {
            format!("{} B", self.size)
        }
    }

    /// Create a clickable button view for this file entry
    fn to_button_view(&self) -> Button<impl FnMut() + 'static> {
        let name = self.name.clone();
        let size_str = self.format_size();

        Button::new(
            HStack::new()
                .spacing(12)
                .alignment(StackAlignment::Center)
                .child(Label::new(self.file_type.icon()).font_size(24))
                .child(
                    VStack::new()
                        .spacing(2)
                        .alignment(StackAlignment::Start)
                        .child(Label::new(&self.name).color(TEXT_MAIN).font_size(14))
                        .child(Label::new(&size_str).color(TEXT_MUTE).font_size(11)),
                )
                .child(Spacer::new()),
            move || {
                println!("[filer] Selected: {}", name);
            },
        )
        .background(SURFACE)
        .text_color(TEXT_MAIN)
        .corner_radius(8)
        .padding(12)
    }
}

/// Read directory entries from the file system
fn read_directory(path: &str) -> Vec<FileEntry> {
    match std::fs::list_directory(path) {
        Ok(entries) => {
            let mut file_entries = Vec::new();

            if path != "/" && path != "." {
                file_entries.push(FileEntry {
                    name: String::from(".."),
                    file_type: FileType::Directory,
                    size: 0,
                });
            }

            for entry in entries {
                if entry.name == "." || entry.name == ".." {
                    continue;
                }

                let file_type = FileType::from_fs_type(entry.file_type);
                file_entries.push(FileEntry {
                    name: entry.name,
                    file_type,
                    size: entry.size,
                });
            }

            file_entries.sort_by(|a, b| match (a.file_type, b.file_type) {
                (FileType::Directory, FileType::Directory) => a.name.cmp(&b.name),
                (FileType::Directory, _) => std::cmp::Ordering::Less,
                (_, FileType::Directory) => std::cmp::Ordering::Greater,
                _ => a.name.cmp(&b.name),
            });

            file_entries
        }
        Err(e) => {
            println!("[filer] Failed to read directory '{}': {}", path, e);
            Vec::new()
        }
    }
}

/// Navigate to a directory and return its entries
fn navigate_to(path: &str) -> Vec<FileEntry> {
    println!("[filer] Navigating to: {}", path);
    read_directory(path)
}

/// Build the navigation toolbar
fn build_navigation_bar() -> HStack {
    HStack::new()
        .spacing(8)
        .alignment(StackAlignment::Center)
        .child(
            Button::new("◀ Back", || {
                println!("[filer] Back button (stub)");
            })
            .background(SURFACE_VAR)
            .text_color(TEXT_MAIN)
            .corner_radius(8)
            .padding(8),
        )
        .child(
            Button::new("▶ Forward", || {
                println!("[filer] Forward button (stub)");
            })
            .background(SURFACE_VAR)
            .text_color(TEXT_MAIN)
            .corner_radius(8)
            .padding(8),
        )
        .child(
            Button::new("Up", || {
                println!("[filer] Up button (stub)");
            })
            .background(SURFACE_VAR)
            .text_color(TEXT_MAIN)
            .corner_radius(8)
            .padding(8),
        )
        .child(
            Button::new("Refresh", || {
                println!("[filer] Refresh button (stub)");
            })
            .background(SURFACE_VAR)
            .text_color(TEXT_MAIN)
            .corner_radius(8)
            .padding(8),
        )
        .child(Spacer::new())
}

/// Build the sidebar with common locations
fn build_sidebar() -> VStack {
    VStack::new()
        .spacing(4)
        .alignment(StackAlignment::Start)
        .child(
            Label::new("Locations")
                .color(TEXT_MUTE)
                .font_size(11)
                .padding_hv(8, 0),
        )
        .child(
            Button::new("Home", || {
                println!("[filer] Navigate to: /home/user");
            })
            .background(SURFACE)
            .text_color(TEXT_MAIN)
            .corner_radius(8)
            .padding(10),
        )
        .child(
            Button::new("Desktop", || {
                println!("[filer] Navigate to: Desktop");
            })
            .background(SURFACE)
            .text_color(TEXT_MAIN)
            .corner_radius(8)
            .padding(10),
        )
        .child(
            Button::new("Documents", || {
                println!("[filer] Navigate to: Documents");
            })
            .background(SURFACE)
            .text_color(TEXT_MAIN)
            .corner_radius(8)
            .padding(10),
        )
        .child(
            Button::new("Downloads", || {
                println!("[filer] Navigate to: Downloads");
            })
            .background(SURFACE)
            .text_color(TEXT_MAIN)
            .corner_radius(8)
            .padding(10),
        )
        .child(
            Button::new("Pictures", || {
                println!("[filer] Navigate to: Pictures");
            })
            .background(SURFACE)
            .text_color(TEXT_MAIN)
            .corner_radius(8)
            .padding(10),
        )
        .child(
            Button::new("Music", || {
                println!("[filer] Navigate to: Music");
            })
            .background(SURFACE)
            .text_color(TEXT_MAIN)
            .corner_radius(8)
            .padding(10),
        )
        .child(
            Button::new("Videos", || {
                println!("[filer] Navigate to: Videos");
            })
            .background(SURFACE)
            .text_color(TEXT_MAIN)
            .corner_radius(8)
            .padding(10),
        )
        .child(Spacer::new())
}

/// Build the action toolbar
fn build_action_toolbar() -> HStack {
    HStack::new()
        .spacing(8)
        .alignment(StackAlignment::Center)
        .child(
            Button::new("New Folder", || {
                println!("[filer] New folder (stub)");
            })
            .background(SURFACE_VAR)
            .text_color(TEXT_MAIN)
            .corner_radius(8)
            .padding(8),
        )
        .child(
            Button::new("Rename", || {
                println!("[filer] Rename (stub)");
            })
            .background(SURFACE_VAR)
            .text_color(TEXT_MAIN)
            .corner_radius(8)
            .padding(8),
        )
        .child(Spacer::new())
}

/// Build the file list view with real file entries
fn build_file_list(entries: Vec<FileEntry>) -> VStack {
    let mut list = VStack::new().spacing(4).alignment(StackAlignment::Start);

    if entries.is_empty() {
        list = list.child(
            Label::new("This folder is empty")
                .color(TEXT_MUTE)
                .font_size(14),
        );
    } else {
        for entry in entries {
            list = list.child(entry.to_button_view());
        }
    }

    list
}

/// Build the status bar
fn build_status_bar(path: &str, file_count: usize) -> HStack {
    HStack::new()
        .spacing(16)
        .alignment(StackAlignment::Center)
        .child(
            Label::new(&format!("{} items", file_count))
                .color(TEXT_MUTE)
                .font_size(12),
        )
        .child(
            Label::new(&format!("Path: {}", path))
                .color(TEXT_MUTE)
                .font_size(12),
        )
        .child(Spacer::new())
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[filer] Starting Scarlet Filer");

    let mut app = match Application::new() {
        Ok(mut app) => {
            app.app_id("org.scarlet-os.desktop.filer");
            app
        }
        Err(e) => {
            println!("[filer] Failed to connect to SWS: {}", e);
            return 1;
        }
    };

    let current_path = "/home/user";
    let file_entries = navigate_to(current_path);
    let entry_count = file_entries.len();

    let window = Window::new("Filer", 950, 650)
        .min_size(750, 550)
        .background(BG)
        .window_type(WindowKind::Normal)
        .main_window()
        .content(
            VStack::new()
                .spacing(16)
                .alignment(StackAlignment::Start)
                .child(
                    Padding::new(
                        VStack::new()
                            .spacing(12)
                            .child(
                                HStack::new()
                                    .spacing(12)
                                    .alignment(StackAlignment::Center)
                                    .child(
                                        Label::new("File Manager").color(TEXT_MAIN).font_size(24),
                                    )
                                    .child(Spacer::new())
                                    .child(
                                        Label::new(&format!("{}", current_path))
                                            .color(TEXT_SUB)
                                            .font_size(13)
                                            .padding_hv(8, 12)
                                            .corner_radius_with_background(6, SURFACE_VAR),
                                    ),
                            )
                            .child(build_navigation_bar())
                            .child(build_action_toolbar()),
                    )
                    .all(20),
                )
                .child(
                    HStack::new()
                        .spacing(16)
                        .alignment(StackAlignment::Start)
                        .child(
                            VStack::new()
                                .spacing(8)
                                .child(build_sidebar())
                                .padding_hv(16, 16)
                                .corner_radius_with_background(12, SURFACE)
                                .border(1, BORDER),
                        )
                        .child(
                            VStack::new()
                                .spacing(8)
                                .child(
                                    HStack::new()
                                        .spacing(12)
                                        .child(Label::new("Name").color(TEXT_SUB).font_size(12))
                                        .child(Spacer::new())
                                        .child(Label::new("Size").color(TEXT_SUB).font_size(12)),
                                )
                                .child(RectView::new(BORDER).height(1))
                                .child(build_file_list(file_entries))
                                .child(Spacer::new()),
                        ),
                )
                .child(
                    Padding::new(RectView::new(BORDER).height(1))
                        .horizontal(20)
                        .vertical(0),
                )
                .child(Padding::new(build_status_bar(current_path, entry_count)).all(20)),
        );

    if let Err(e) = app.add_window(window) {
        println!("[filer] Failed to add window: {}", e);
        return 1;
    }

    println!("[filer] Running file manager");
    app.run();
}
