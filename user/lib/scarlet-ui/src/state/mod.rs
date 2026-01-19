//! State management modules
//!
//! This module provides SwiftUI-style state management for ScarletUI.

extern crate alloc;
use alloc::sync::Arc;
use scarlet_std::sync::Mutex;

pub mod data;
pub mod local;
pub mod observable;

pub use data::DataContext;
pub use local::Local;
pub use observable::{
    Observable,
    ObservableNotifier,
    StateObject,
    Observed,
};
