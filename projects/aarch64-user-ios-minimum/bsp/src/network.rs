//! Register the bundled native NAT transport at the hosted Ethernet boundary.

#[path = "../../../shared/hosted/network/mod.rs"]
mod hosted_network;

use scarlet::interrupt::InterruptId;
use std::{path::{Path, PathBuf}, sync::OnceLock};

const ETHERNET_IRQ: InterruptId = 19;
static LIBRARY: OnceLock<PathBuf> = OnceLock::new();

scarlet::late_initcall!(register_ethernet);

pub fn configure(bundle: &Path) {
    let library = bundle.join("Frameworks/ScarletSlirp.framework/ScarletSlirp");
    if library.is_file() {
        let _ = LIBRARY.set(library);
    }
    std::hint::black_box(&__LATE_INITCALL__);
}

fn register_ethernet() {
    let Some(library) = LIBRARY.get() else {
        return;
    };
    match hosted_network::attach(0, ETHERNET_IRQ, library.clone()) {
        Ok(()) => scarlet::println!("[hosted Ethernet] eth0 connected to the native NAT"),
        Err(error) => scarlet::println!("[hosted Ethernet] {error}"),
    }
}
