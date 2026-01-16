//! Scarlet Filer (File Manager)
//!
//! File manager application for Scarlet Desktop with real file system operations

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use scarlet_ui::{
    Application, Button, Color, HStack, Label, Padding, RectView, Spacer,
    StackAlignment, State, VStack, Window, WindowKind,
};
use std::{format, println, string::String, vec::Vec};

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
            FileType::Directory => "📁",
            FileType::File => "📄",
            FileType::Symlink => "🔗",
            FileType::Executable => "⚙️",
        }
    }

    /// Get color for this file type
    fn color(self) -> Color {
        match self {
            FileType::Directory => Color::rgb(255, 200, 100),
            FileType::File => Color::rgb(224, 224, 224),
            FileType::Symlink => Color::rgb(100, 200, 255),
            FileType::Executable => Color::rgb(100, 255, 100),
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
        let file_type = self.file_type;
        let size_str = self.format_size();

        Button::new(
            HStack::new()
                .spacing(12)
                .alignment(StackAlignment::Center)
                .child(
                    Label::new(self.file_type.icon())
                        .font_size(20),
                )
                .child(
                    VStack::new()
                        .spacing(2)
                        .alignment(StackAlignment::Start)
                        .child(
                            Label::new(&self.name)
                                .color(self.file_type.color())
                                .font_size(14),
                        )
                        .child(
                            Label::new(&size_str)
                                .color(Color::rgb(136, 136, 136))
                                .font_size(11),
                        ),
                )
                .child(Spacer::new()),
            move || {
                println!("[filer] Selected: {}", name);
                // TODO: Implement selection
            }
        )
        .background(Color::rgb(45, 45, 45))
        .text_color(Color::rgb(200, 200, 200))
        .corner_radius(4)
        .padding(8)
    }
}

/// Read directory entries from the file system
fn read_directory(path: &str) -> Vec<FileEntry> {
    match std::fs::list_directory(path) {
        Ok(entries) => {
            let mut file_entries = Vec::new();

            // Add parent directory entry if not at root
            if path != "/" && path != "." {
                file_entries.push(FileEntry {
                    name: String::from(".."),
                    file_type: FileType::Directory,
                    size: 0,
                });
            }

            // Convert fs entries to FileEntry
            for entry in entries {
                // Skip . and .. entries (we handle .. specially)
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

            // Sort: directories first, then alphabetically
            file_entries.sort_by(|a, b| {
                match (a.file_type, b.file_type) {
                    (FileType::Directory, FileType::Directory) => a.name.cmp(&b.name),
                    (FileType::Directory, _) => std::cmp::Ordering::Less,
                    (_, FileType::Directory) => std::cmp::Ordering::Greater,
                    _ => a.name.cmp(&b.name),
                }
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
                // TODO: Navigate back in history
            })
            .background(Color::rgb(60, 60, 60))
            .text_color(Color::rgb(224, 224, 224))
            .corner_radius(6)
            .padding(8),
        )
        .child(
            Button::new("▶ Forward", || {
                println!("[filer] Forward button (stub)");
                // TODO: Navigate forward in history
            })
            .background(Color::rgb(60, 60, 60))
            .text_color(Color::rgb(224, 224, 224))
            .corner_radius(6)
            .padding(8),
        )
        .child(
            Button::new("⬆ Up", || {
                println!("[filer] Up button (stub)");
                // TODO: Navigate to parent directory
            })
            .background(Color::rgb(60, 60, 60))
            .text_color(Color::rgb(224, 224, 224))
            .corner_radius(6)
            .padding(8),
        )
        .child(
            Button::new("🔄 Refresh", || {
                println!("[filer] Refresh button (stub)");
                // TODO: Refresh current directory
            })
            .background(Color::rgb(60, 60, 60))
            .text_color(Color::rgb(224, 224, 224))
            .corner_radius(6)
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
                .color(Color::rgb(136, 136, 136))
                .font_size(12),
        )
        .child(
            Button::new("🏠 Home", || {
                println!("[filer] Navigate to: /home/user");
                // TODO: Navigate to home directory
            })
            .background(Color::rgb(50, 50, 50))
            .text_color(Color::rgb(200, 200, 200))
            .corner_radius(6)
            .padding(10),
        )
        .child(
            Button::new("📁 Desktop", || {
                println!("[filer] Navigate to: Desktop");
            })
            .background(Color::rgb(50, 50, 50))
            .text_color(Color::rgb(200, 200, 200))
            .corner_radius(6)
            .padding(10),
        )
        .child(
            Button::new("📄 Documents", || {
                println!("[filer] Navigate to: Documents");
            })
            .background(Color::rgb(50, 50, 50))
            .text_color(Color::rgb(200, 200, 200))
            .corner_radius(6)
            .padding(10),
        )
        .child(
            Button::new("⬇️ Downloads", || {
                println!("[filer] Navigate to: Downloads");
            })
            .background(Color::rgb(50, 50, 50))
            .text_color(Color::rgb(200, 200, 200))
            .corner_radius(6)
            .padding(10),
        )
        .child(
            Button::new("🖼️ Pictures", || {
                println!("[filer] Navigate to: Pictures");
            })
            .background(Color::rgb(50, 50, 50))
            .text_color(Color::rgb(200, 200, 200))
            .corner_radius(6)
            .padding(10),
        )
        .child(
            Button::new("🎵 Music", || {
                println!("[filer] Navigate to: Music");
            })
            .background(Color::rgb(50, 50, 50))
            .text_color(Color::rgb(200, 200, 200))
            .corner_radius(6)
            .padding(10),
        )
        .child(
            Button::new("🎬 Videos", || {
                println!("[filer] Navigate to: Videos");
            })
            .background(Color::rgb(50, 50, 50))
            .text_color(Color::rgb(200, 200, 200))
            .corner_radius(6)
            .padding(10),
        )
        .child(Spacer::new())
}

/// Build the action toolbar
fn build_action_toolbar() -> HStack {
    let text_color = Color::rgb(200, 200, 200);

    HStack::new()
        .spacing(8)
        .alignment(StackAlignment::Center)
        .child(
            Button::new("New Folder", || {
                println!("[filer] New folder (stub)");
            })
            .background(Color::rgb(60, 60, 60))
            .text_color(text_color)
            .corner_radius(6)
            .padding(8),
        )
        .child(
            Button::new("Rename", || {
                println!("[filer] Rename (stub)");
            })
            .background(Color::rgb(60, 60, 60))
            .text_color(text_color)
            .corner_radius(6)
            .padding(8),
        )
        .child(Spacer::new())
}

/// Build the file list view with real file entries
fn build_file_list(entries: Vec<FileEntry>) -> VStack {
    let mut list = VStack::new()
        .spacing(4)
        .alignment(StackAlignment::Start);

    if entries.is_empty() {
        list = list.child(
            Label::new("This folder is empty")
                .color(Color::rgb(136, 136, 136))
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
                .color(Color::rgb(136, 136, 136))
                .font_size(11),
        )
        .child(
            Label::new(&format!("Path: {}", path))
                .color(Color::rgb(136, 136, 136))
                .font_size(11),
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

    // Start in home directory
    let current_path = "/home/user";

    // Read actual file entries from the file system
    let file_entries = navigate_to(current_path);
    let entry_count = file_entries.len();

    // Create file manager window
    let window = Window::new("Filer", 900, 600)
        .min_size(700, 500)
        .background(Color::rgb(30, 30, 30))
        .window_type(WindowKind::Normal)
        .main_window()
        .content(
            VStack::new()
                .spacing(12)
                .alignment(StackAlignment::Start)
                .child(
                    // Navigation bar
                    build_navigation_bar(),
                )
                .child(
                    // Main content area with sidebar and file list
                    HStack::new()
                        .spacing(12)
                        .alignment(StackAlignment::Start)
                        .child(
                            // Sidebar
                            RectView::new(Color::rgb(35, 35, 35))
                                .width(160)
                                .corner_radius(8),
                        )
                        .child(
                            // Sidebar content
                            VStack::new()
                                .spacing(8)
                                .alignment(StackAlignment::Start)
                                .child(build_sidebar()),
                        )
                        .child(
                            // File list area
                            VStack::new()
                                .spacing(8)
                                .alignment(StackAlignment::Start)
                                .child(
                                    // Path display
                                    RectView::new(Color::rgb(40, 40, 40))
                                        .height(36)
                                        .corner_radius(6),
                                )
                                .child(
                                    // Path label
                                    Padding::new(
                                        Label::new(&format!("📍 {}", current_path))
                                            .color(Color::rgb(200, 200, 200))
                                            .font_size(13),
                                    )
                                    .horizontal(12)
                                    .vertical(8),
                                )
                                .child(
                                    // Action toolbar
                                    build_action_toolbar(),
                                )
                                .child(
                                    // File list header
                                    Padding::new(
                                        HStack::new()
                                            .spacing(12)
                                            .alignment(StackAlignment::Start)
                                            .child(
                                                Label::new("Name")
                                                    .color(Color::rgb(180, 180, 180))
                                                    .font_size(12),
                                            )
                                            .child(Spacer::new())
                                            .child(
                                                Label::new("Size")
                                                    .color(Color::rgb(180, 180, 180))
                                                    .font_size(12),
                                            ),
                                    )
                                    .all(8),
                                )
                                .child(
                                    // File entries
                                    build_file_list(file_entries),
                                )
                                .child(Spacer::new()),
                        )
                )
                .child(
                    // Status bar
                    Padding::new(
                        build_status_bar(current_path, entry_count)
                    )
                    .all(12)
                )
        );

    if let Err(e) = app.add_window(window) {
        println!("[filer] Failed to add window: {}", e);
        return 1;
    }

    println!("[filer] Running file manager");
    app.run();
}
