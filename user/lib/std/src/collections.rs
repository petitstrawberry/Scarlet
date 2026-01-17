//! Collection types for Scarlet
//!
//! This module provides:
//! - HashMap and HashSet from hashbrown for general use
//! - BTreeMap and BTreeSet from alloc for static contexts and ordered access

extern crate alloc;

pub use hashbrown::HashMap;
pub use hashbrown::HashSet;

// Re-export alloc collections for convenience
pub use alloc::collections::BTreeMap;
pub use alloc::collections::BTreeSet;
