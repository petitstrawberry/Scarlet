//! macOS registration for Scarlet's hosted Ethernet device.

#[path = "../../../shared/hosted/network/mod.rs"]
mod hosted_network;

use scarlet::interrupt::InterruptId;

pub const COMPLETION_IRQ: InterruptId = 19;

scarlet::late_initcall!(register_ethernet);

/// Retain the BSP's Ethernet initcall in the native executable.
pub fn link_boot_ethernet() {
    std::hint::black_box(&__LATE_INITCALL__);
}

fn register_ethernet() {
    let library = std::env::var_os("SCARLET_HOSTED_SLIRP")
        .or_else(|| option_env!("SCARLET_HOSTED_SLIRP").map(Into::into))
        .unwrap_or_else(|| "libslirp.0.dylib".into());
    if let Err(error) = hosted_network::attach(0, COMPLETION_IRQ, library.into()) {
        scarlet::println!("[hosted Ethernet] {}", error);
    }
}
