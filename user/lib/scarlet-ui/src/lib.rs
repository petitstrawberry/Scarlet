//! ScarletUI facade crate.
//!
//! UI implementation lives in `scarlet-ui-core`; platform backends live in
//! separate backend crates. This crate preserves the app-facing `scarlet_ui`
//! API and selects the requested backend by feature.
//!
//! Dependency direction:
//!
//! ```text
//! scarlet-ui-platform-sws  ----.
//!                              v
//! apps --> scarlet-ui ---> scarlet-ui-core
//!                              ^
//! scarlet-ui-platform-winit ---'
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(all(feature = "std", feature = "legacy-scarlet-std"))]
compile_error!("scarlet-ui features `std` and `legacy-scarlet-std` are mutually exclusive");

#[cfg(all(feature = "platform-sws", feature = "platform-winit"))]
compile_error!(
    "scarlet-ui platform features `platform-sws` and `platform-winit` are mutually exclusive"
);

#[cfg(not(any(feature = "std", feature = "legacy-scarlet-std")))]
compile_error!("scarlet-ui requires either the `std` or `legacy-scarlet-std` feature");

pub use scarlet_ui_core::*;

#[cfg(feature = "platform-sws")]
pub use scarlet_ui_platform_sws::{SWSPlatformWindow, SwsBackend};
#[cfg(feature = "platform-winit")]
pub use scarlet_ui_platform_winit::{WinitBackend, WinitPlatformWindow};

/// Extension methods for running applications with the selected backend.
pub trait ApplicationRunExt: Application {
    /// Run with the platform backend selected by this crate's features.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the application exits normally.
    #[cfg(any(feature = "platform-sws", feature = "platform-winit"))]
    fn run(&mut self) -> Result<()>
    where
        Self: Sized + View,
    {
        selected_platform::run(self)
    }
}

impl<T: Application> ApplicationRunExt for T {}

#[cfg(any(feature = "platform-sws", feature = "platform-winit"))]
mod selected_platform {
    use alloc::boxed::Box;

    use scarlet_ui_core::{Application, ApplicationRunner, Result, View};

    pub(super) fn run<A>(app: &mut A) -> Result<()>
    where
        A: Application + View,
    {
        ApplicationRunner::new(selected_backend()).run(app)
    }

    #[cfg(feature = "platform-sws")]
    fn selected_backend() -> Box<dyn scarlet_ui_core::PlatformBackend> {
        Box::new(scarlet_ui_platform_sws::SwsBackend::new())
    }

    #[cfg(feature = "platform-winit")]
    fn selected_backend() -> Box<dyn scarlet_ui_core::PlatformBackend> {
        Box::new(scarlet_ui_platform_winit::WinitBackend::new())
    }
}

/// Prelude module for convenient imports.
pub mod prelude {
    pub use scarlet_ui_core::prelude::*;

    pub use crate::ApplicationRunExt;
}
