//! macOS configuration for the shared hosted SGFX backend.
#[path = "../../../shared/hosted/sgfx/mod.rs"]
mod common;
pub use common::{Config, Endpoint, Host, Worker, create_scanout, upload_scanout};
pub use common::run_offscreen as run_offscreen_configured;

pub(crate) fn config() -> Config {
    crate::latency_trace::init();
    Config {
        irq: 22,
        trace: Some(|id, fields| crate::latency_trace::gpu_receive(id, || fields)),
    }
}

pub fn run_offscreen(
    boot: impl FnOnce(Option<Endpoint>) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    common::run_offscreen(config(), boot)
}
