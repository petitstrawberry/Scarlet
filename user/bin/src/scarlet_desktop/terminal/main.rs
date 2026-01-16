//! Scarlet Terminal
//!
//! Terminal emulator application for Scarlet Desktop
//!
//! Note: Full PTY support requires libc functions not available in current no_std environment.
//! This version provides the UI framework for future PTY integration.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use scarlet_ui::{
    Application, Color, Label, Padding, State, VStack, Window, WindowKind,
};
use std::println;

/// Terminal state
struct TerminalState {
    output: scarlet_std::string::String,
    command: scarlet_std::string::String,
}

impl TerminalState {
    fn new() -> Self {
        Self {
            output: scarlet_std::string::String::from(
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
                 Press Ctrl+C to exit.\n"
            ),
            command: scarlet_std::string::String::new(),
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
                self.output = scarlet_std::string::String::from("Terminal cleared.\n");
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
                self.output.push_str("Current date/time: [Time implementation needed]\n");
            }
            "" => {}
            _ => {
                self.output.push_str("Command not found: ");
                self.output.push_str(cmd);
                self.output.push_str("\nType 'help' for available commands.\n");
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[terminal] Starting Scarlet Terminal");

    let terminal_state = State::new(TerminalState::new());

    let mut app = match Application::new() {
        Ok(mut app) => {
            app.app_id("org.scarlet-os.desktop.terminal");
            app
        }
        Err(e) => {
            println!("[terminal] Failed to connect to SWS: {}", e);
            return 1;
        }
    };

    // Create terminal state clone for input handling
    let state_for_input = terminal_state.clone();

    // Create terminal window
    let window = Window::new("Terminal", 800, 600)
        .min_size(600, 400)
        .background(Color::rgb(20, 20, 20))
        .window_type(WindowKind::Normal)
        .main_window()
        .content(
            VStack::new()
                .spacing(0)
                .child(
                    // Output display area
                    Padding::new(
                        Label::new(terminal_state.map(|s| s.output.clone()))
                            .color(Color::rgb(0, 255, 0))
                            .font_size(13)
                    )
                    .all(8),
                )
                .child(
                    // Simple status line
                    Padding::new(
                        Label::new(terminal_state.map(|s| {
                            if !s.command.is_empty() {
                                let mut result = scarlet_std::string::String::from("$ ");
                                result.push_str(&s.command);
                                result
                            } else {
                                scarlet_std::string::String::from("$")
                            }
                        }))
                        .color(Color::rgb(0, 200, 0))
                        .font_size(14)
                    )
                    .all(8),
                )
        );

    // Simulate some initial terminal output
    terminal_state.update(|state| {
        state.execute_command("help");
    });

    if let Err(e) = app.add_window(window) {
        println!("[terminal] Failed to add window: {}", e);
        return 1;
    }

    println!("[terminal] Running terminal app");
    println!("[terminal] Note: PTY integration pending libc support");
    app.run();
}
