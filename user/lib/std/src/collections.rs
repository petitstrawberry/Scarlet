//! Additional collection types beyond those in alloc
//!
//! This module provides HashMap and HashSet from hashbrown for use in Scarlet.
//! Note that HashMap cannot be used in static contexts (use BTreeMap from alloc::collections instead).

pub use hashbrown::HashMap;
pub use hashbrown::HashSet;
