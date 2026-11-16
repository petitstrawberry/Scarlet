//! Scarlet remote desktop service.
//!
//! One SWS capture session is shared by all connected RFB clients. SWS remains
//! independent of RFB, TCP, X11 key symbols, and remote-client lifecycle.

mod sws;
mod vnc;

use clap::{Parser, Subcommand};
use std::io::Read;
use std::net::TcpListener;
use std::path::PathBuf;
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
        /// File containing a 1-8 byte ASCII VNC password (for macOS Screen Sharing).
        #[arg(long, value_name = "PATH")]
        password_file: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Server {
            listen,
            password_file,
        } => run_server(&listen, password_file),
    }
}

fn run_server(listen_address: &str, password_file: Option<PathBuf>) -> ExitCode {
    let password = match password_file
        .map(|path| {
            let mut bytes = Vec::new();
            std::fs::File::open(path)?
                .take(11)
                .read_to_end(&mut bytes)?;
            vnc::VncPassword::from_bytes(&bytes)
        })
        .transpose()
    {
        Ok(password) => password,
        Err(error) => {
            eprintln!("remote-desktop: failed to load VNC password: {error}");
            return ExitCode::from(1);
        }
    };
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

    println!("[remote-desktop] RFB 3.3/3.7/3.8 listening on {listen_address}");
    if password.is_some() {
        println!("[remote-desktop] Security type: VNC Authentication");
    } else {
        println!("[remote-desktop] Security type: None (trusted networks only)");
    }
    vnc::serve(listener, state, password)
}
