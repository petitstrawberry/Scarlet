//! Cross-platform SGFX frontend and execution-backend selector.
//!
//! Portable renderers record resources and command buffers through
//! [`sgfx_core`]. Applications and platform composition roots use this crate
//! to select one complete execution backend. A selected backend continues to
//! own physical resources, command lowering, transport limits, and submission.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(all(
    not(feature = "std"),
    target_os = "scarlet",
    feature = "legacy-scarlet-std"
))]
extern crate scarlet_std as std;

use core::fmt;

pub use sgfx_core::{backend, ir};

#[cfg(all(not(target_os = "scarlet"), feature = "backend-wgpu"))]
mod host;
#[cfg(all(target_os = "scarlet", feature = "backend-scarlet-virgl"))]
mod scarlet;

#[cfg(all(not(target_os = "scarlet"), feature = "backend-wgpu"))]
pub use host::{Executor, MappedTargetSession, WindowContext};
#[cfg(all(target_os = "scarlet", feature = "backend-scarlet-virgl"))]
pub use scarlet::{Capabilities, Context, Device, Executor, Handle, ImageRef, MappedTargetSession};

/// Environment variable used to override automatic SGFX backend selection.
pub const BACKEND_ENV: &str = "SGFX_BACKEND";

/// A complete execution backend that may be selected by the SGFX frontend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    /// SGFX execution through WGPU.
    Wgpu,
    /// Direct SGFX execution through Metal.
    Metal,
    /// SGFX VirGL execution through the Scarlet GPU ABI.
    ScarletVirgl,
    /// SGFX AGX execution through the Scarlet GPU ABI.
    ScarletAgx,
}

impl BackendKind {
    /// Return the stable configuration name for this backend.
    ///
    /// # Returns
    ///
    /// A value accepted by [`BACKEND_ENV`].
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wgpu => "wgpu",
            Self::Metal => "metal",
            Self::ScarletVirgl => "scarlet-virgl",
            Self::ScarletAgx => "scarlet-agx",
        }
    }
}

impl fmt::Display for BackendKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Backend selection requested by an application or the process environment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BackendPreference {
    /// Select the best compiled backend for the active target and device.
    #[default]
    Auto,
    /// Require WGPU execution.
    Wgpu,
    /// Require direct Metal execution.
    Metal,
    /// Require Scarlet/VirGL execution.
    ScarletVirgl,
    /// Require Scarlet/AGX execution.
    ScarletAgx,
}

impl BackendPreference {
    /// Parse one backend preference name.
    ///
    /// # Arguments
    ///
    /// * `value` - `auto` or one stable [`BackendKind`] name.
    ///
    /// # Returns
    ///
    /// The parsed preference, or [`Error::InvalidBackendPreference`].
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "wgpu" => Ok(Self::Wgpu),
            "metal" => Ok(Self::Metal),
            "scarlet-virgl" | "virgl" => Ok(Self::ScarletVirgl),
            "scarlet-agx" | "agx" => Ok(Self::ScarletAgx),
            _ => Err(Error::InvalidBackendPreference),
        }
    }

    /// Read the process backend preference.
    ///
    /// # Returns
    ///
    /// The parsed [`BACKEND_ENV`] value, or [`BackendPreference::Auto`] when
    /// the variable is absent.
    pub fn from_environment() -> Result<Self> {
        match backend_environment_value() {
            Some(value) => Self::parse(value.as_str()),
            None => Ok(Self::Auto),
        }
    }
}

/// Failure returned by the cross-platform SGFX frontend.
#[derive(Debug)]
pub enum Error {
    /// The requested backend name is invalid.
    InvalidBackendPreference,
    /// The requested backend was not compiled for this target.
    BackendUnavailable(BackendKind),
    /// WGPU initialization, materialization, execution, or presentation failed.
    #[cfg(all(not(target_os = "scarlet"), feature = "backend-wgpu"))]
    Wgpu(sgfx_backend_wgpu::Error),
    /// A Scarlet/VirGL device or context operation failed.
    #[cfg(all(target_os = "scarlet", feature = "backend-scarlet-virgl"))]
    ScarletVirglHandle(sgfx_backend_scarlet_virgl::HandleError),
    /// A Scarlet/VirGL IR materialization or execution operation failed.
    #[cfg(all(target_os = "scarlet", feature = "backend-scarlet-virgl"))]
    ScarletVirglIr(sgfx_backend_scarlet_virgl::IrSubmitError),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBackendPreference => {
                formatter.write_str("invalid SGFX backend preference")
            }
            Self::BackendUnavailable(backend) => {
                write!(formatter, "SGFX backend {backend} is unavailable")
            }
            #[cfg(all(not(target_os = "scarlet"), feature = "backend-wgpu"))]
            Self::Wgpu(error) => write!(formatter, "SGFX WGPU backend failed: {error}"),
            #[cfg(all(target_os = "scarlet", feature = "backend-scarlet-virgl"))]
            Self::ScarletVirglHandle(error) => {
                write!(formatter, "SGFX Scarlet/VirGL device failed: {error:?}")
            }
            #[cfg(all(target_os = "scarlet", feature = "backend-scarlet-virgl"))]
            Self::ScarletVirglIr(error) => {
                write!(formatter, "SGFX Scarlet/VirGL execution failed: {error:?}")
            }
        }
    }
}

/// Result returned by the SGFX frontend.
pub type Result<T> = core::result::Result<T, Error>;

/// Selected SGFX execution environment.
pub struct Instance {
    backend: BackendKind,
}

impl Instance {
    /// Select an SGFX backend using [`BACKEND_ENV`] and target defaults.
    ///
    /// # Returns
    ///
    /// A selected instance, or a configuration error.
    pub fn new() -> Result<Self> {
        Self::with_preference(BackendPreference::from_environment()?)
    }

    /// Select an SGFX backend from an explicit preference.
    ///
    /// # Arguments
    ///
    /// * `preference` - Automatic or required backend selection.
    ///
    /// # Returns
    ///
    /// A selected instance, or [`Error::BackendUnavailable`].
    pub fn with_preference(preference: BackendPreference) -> Result<Self> {
        Ok(Self {
            backend: resolve_backend(preference)?,
        })
    }

    /// Return the selected complete backend.
    ///
    /// # Returns
    ///
    /// The stable selected backend identity.
    pub const fn backend(&self) -> BackendKind {
        self.backend
    }
}

fn resolve_backend(preference: BackendPreference) -> Result<BackendKind> {
    match preference {
        BackendPreference::Auto => default_backend(),
        BackendPreference::Wgpu => require_backend(BackendKind::Wgpu),
        BackendPreference::Metal => require_backend(BackendKind::Metal),
        BackendPreference::ScarletVirgl => require_backend(BackendKind::ScarletVirgl),
        BackendPreference::ScarletAgx => require_backend(BackendKind::ScarletAgx),
    }
}

fn default_backend() -> Result<BackendKind> {
    #[cfg(all(target_os = "scarlet", feature = "backend-scarlet-virgl"))]
    {
        return Ok(BackendKind::ScarletVirgl);
    }
    #[cfg(all(not(target_os = "scarlet"), feature = "backend-wgpu"))]
    {
        return Ok(BackendKind::Wgpu);
    }
    #[allow(unreachable_code)]
    Err(Error::BackendUnavailable(default_backend_kind()))
}

const fn default_backend_kind() -> BackendKind {
    if cfg!(target_os = "scarlet") {
        BackendKind::ScarletVirgl
    } else {
        BackendKind::Wgpu
    }
}

fn require_backend(backend: BackendKind) -> Result<BackendKind> {
    let available = match backend {
        BackendKind::Wgpu => cfg!(all(not(target_os = "scarlet"), feature = "backend-wgpu")),
        BackendKind::Metal => false,
        BackendKind::ScarletVirgl => cfg!(all(
            target_os = "scarlet",
            feature = "backend-scarlet-virgl"
        )),
        BackendKind::ScarletAgx => false,
    };
    if available {
        Ok(backend)
    } else {
        Err(Error::BackendUnavailable(backend))
    }
}

#[cfg(feature = "std")]
fn backend_environment_value() -> Option<std::string::String> {
    std::env::var(BACKEND_ENV).ok()
}

#[cfg(all(
    not(feature = "std"),
    target_os = "scarlet",
    feature = "legacy-scarlet-std"
))]
fn backend_environment_value() -> Option<std::string::String> {
    std::env::var(BACKEND_ENV)
}

#[cfg(not(any(
    feature = "std",
    all(target_os = "scarlet", feature = "legacy-scarlet-std")
)))]
fn backend_environment_value() -> Option<alloc::string::String> {
    None
}

#[cfg(test)]
mod tests {
    use super::{BackendKind, BackendPreference, Error, Instance};

    #[test]
    fn parses_stable_backend_names() {
        assert_eq!(
            BackendPreference::parse("wgpu").unwrap(),
            BackendPreference::Wgpu
        );
        assert_eq!(
            BackendPreference::parse("virgl").unwrap(),
            BackendPreference::ScarletVirgl
        );
        assert!(matches!(
            BackendPreference::parse("unknown"),
            Err(Error::InvalidBackendPreference)
        ));
    }

    #[test]
    fn host_auto_selects_wgpu() {
        #[cfg(not(target_os = "scarlet"))]
        assert_eq!(
            Instance::with_preference(BackendPreference::Auto)
                .unwrap()
                .backend(),
            BackendKind::Wgpu
        );
    }

    #[test]
    fn unavailable_backend_is_not_silently_substituted() {
        assert!(matches!(
            Instance::with_preference(BackendPreference::Metal),
            Err(Error::BackendUnavailable(BackendKind::Metal))
        ));
    }
}
