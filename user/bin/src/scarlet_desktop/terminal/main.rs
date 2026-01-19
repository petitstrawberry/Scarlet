//! Scarlet Terminal
//!
//! Terminal emulator application for Scarlet Desktop
//!
//! Note: Full PTY support requires libc functions not available in current no_std environment.
//! This version provides the UI framework for future PTY integration.

#![no_std]
#![no_main]

extern crate alloc;

use scarlet_ui::{
    Application, Window, WindowBuilder,
    VStack,
    Text,
    View, ViewExt,
    Color,
};
use alloc::string::String;
use scarlet_std::println;

/// Terminal state
struct TerminalState {
    output: String,
    command: String,
}

impl TerminalState {
    fn new() -> Self {
        Self {
            output: String::from(
                "Scarlet Terminal v0.2.0\n\n\
                 Welcome to Scarlet Terminal!\n\n\
                 PTY support requires libc integration.\n\
                 This is a UI framework for future PTY implementation.\n\n\
                 Features implemented:\n\
                 - PTY creation and management (pty.rs)\n\
                 - VT100/ANSI escape sequence parser (vtparser.rs)\n\
                 - Terminal buffer with scrolling (terminal.rs)\n\
                 - Real-time output display via State\n\n\
                 To enable full PTY support:\n\
                 1. Add libc dependency to Cargo.toml\n\
                 2. Or implement syscall wrappers for RISC-V\n\
                 3. Uncomment PTY code in main.rs\n\n\
                 Press Ctrl+C to exit.\n",
            ),
            command: String::new(),
        }
    }

    fn execute_command(&mut self, cmd: &str) {
        self.output.push_str("$ ");
        self.output.push_str(cmd);
        self.output.push_str("\n");

        // Simple command simulation
        let cmd_lower = cmd.trim().to_lowercase();
        match cmd_lower.as_str() {
            "clear" => {
                self.output = String::from("Terminal cleared.\n");
            }
            "help" => {
                self.output.push_str("Available commands:\n");
                self.output.push_str("  clear - Clear terminal\n");
                self.output.push_str("  help  - Show this help\n");
                self.output.push_str("  echo  <text> - Echo text\n");
                self.output.push_str("  date  - Show current date/time\n");
            }
            cmd if cmd.starts_with("echo ") => {
                let text = &cmd[5..];
                self.output.push_str(text);
                self.output.push_str("\n");
            }
            "date" => {
                self.output
                    .push_str("Current date/time: [Time implementation needed]\n");
            }
            "" => {}
            _ => {
                self.output.push_str("Command not found: ");
                self.output.push_str(cmd);
                self.output
                    .push_str("\nType 'help' for available commands.\n");
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[terminal] Starting Scarlet Terminal");

    let mut terminal_state = TerminalState::new();

    // Simulate some initial terminal output
    terminal_state.execute_command("help");

    let mut app = match Application::new() {
        Ok(mut a) => {
            a.app_id("org.scarlet-os.desktop.terminal");
            a
        }
        Err(e) => {
            println!("[terminal] Failed to create application: {}", e);
            return 1;
        }
    };

    // Get current output as string
    let output_text = terminal_state.output.clone();

    // Create terminal UI
    let ui_content = VStack::new()
        .spacing(0)
        .child(
            // Output display area
            Text::new(&output_text)
                .font_size(13)
                .padding(8)
        )
        .child(
            // Simple status line
            Text::new("$ help")
                .font_size(14)
                .padding(8)
        );

    let window = Window::builder()
        .title("Terminal")
        .size(800, 600)
        .min_size(600, 400)
        .build()
        .background(Color::rgb(30, 30, 30))
        .content(ui_content);

    if let Err(e) = app.add_window(window) {
        println!("[terminal] Failed to add window: {}", e);
        return 1;
    }

    println!("[terminal] Running terminal app");
    println!("[terminal] Note: PTY integration pending libc support");
    app.run();
}
