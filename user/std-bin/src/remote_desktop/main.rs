//! Scarlet remote desktop service.
//!
//! One SWS capture session is shared by all connected RFB clients. SWS remains
//! independent of RFB, TCP, X11 key symbols, and remote-client lifecycle.

mod sws;
mod vnc;

use clap::{Parser, Subcommand};
use std::net::TcpListener;
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;

use sws::DesktopState;

const DEFAULT_LISTEN_ADDRESS: &str = "0.0.0.0:5900";

#[derive(Debug, Parser)]
#[command(
    name = "remote-desktop",
    version,
    about = "Scarlet remote desktop client and server"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run an RFB/VNC server backed by the SWS desktop.
    Server {
        /// RFB listen address.
        #[arg(long, value_name = "ADDRESS:PORT", default_value = DEFAULT_LISTEN_ADDRESS)]
        listen: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Server { listen } => run_server(&listen),
    }
}

fn run_server(listen_address: &str) -> ExitCode {
    let listener = match TcpListener::bind(listen_address) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!(
                "remote-desktop: failed to listen on {}: {}",
                listen_address, error
            );
            return ExitCode::from(1);
        }
    };

    let state = Arc::new(DesktopState::new());
    let capture_state = Arc::clone(&state);
    if thread::Builder::new()
        .spawn(move || sws::capture_loop(capture_state))
        .is_err()
    {
        eprintln!("remote-desktop: failed to start SWS capture thread");
        return ExitCode::from(1);
    }

    println!("[remote-desktop] RFB 3.8 listening on {listen_address}");
    println!("[remote-desktop] Security type: None (trusted networks only)");
    vnc::serve(listener, state)
}
