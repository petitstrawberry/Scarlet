//! Debug logging flags for ScarletUI.

use core::sync::atomic::{AtomicBool, Ordering};

static DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);

/// Enable or disable debug logging.
pub fn set_enabled(enabled: bool) {
    DEBUG_ENABLED.store(enabled, Ordering::SeqCst);
}

/// Check if debug logging is enabled.
pub fn is_enabled() -> bool {
    DEBUG_ENABLED.load(Ordering::SeqCst)
}
