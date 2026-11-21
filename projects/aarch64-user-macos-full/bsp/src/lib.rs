//! Native macOS primitives for the hosted Scarlet BSP.

#[cfg(not(all(target_arch = "aarch64", target_os = "macos")))]
compile_error!("This BSP requires AArch64 macOS");

pub mod audio;
#[path = "../../../shared/hosted/block.rs"]
pub mod block;
pub mod gpu;
pub(crate) mod latency_trace;
pub mod network;
#[path = "../../../shared/hosted/root_image.rs"]
pub mod root_image;
#[path = "../../../shared/hosted/terminal.rs"]
pub mod terminal;
pub mod video;
pub mod window;
pub use scarlet_hosted_runtime::{context, ram, run_gate};
