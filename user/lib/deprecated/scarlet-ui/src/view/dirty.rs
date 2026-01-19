//! Dirty flag tracking for incremental updates
//!
//! This module provides bitflags for tracking what aspects of a view
//! need to be updated. This enables efficient incremental rendering.

use core::fmt;

/// Dirty flags for tracking what needs updating
///
/// These flags indicate which aspects of a view have changed and
/// need to be recalculated or redrawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DirtyFlags(u8);

impl DirtyFlags {
    /// No changes - view is clean
    pub const CLEAN: Self = Self(0);

    /// Layout needs recalculation
    ///
    /// Set when the view's size or the size of its children changes.
    pub const LAYOUT: Self = Self(1 << 0);

    /// Paint needed - view needs redraw
    ///
    /// Set when the view's appearance changes but size doesn't.
    pub const PAINT: Self = Self(1 << 1);

    /// Children changed - child list was modified
    ///
    /// Set when children are added, removed, or reordered.
    pub const CHILDREN: Self = Self(1 << 2);

    /// All flags - everything needs updating
    pub const ALL: Self = Self(0b111);

    /// Create new dirty flags
    pub const fn new() -> Self {
        Self::CLEAN
    }

    /// Check if layout is dirty
    pub const fn is_layout_dirty(self) -> bool {
        self.0 & Self::LAYOUT.0 != 0
    }

    /// Check if paint is dirty
    pub const fn is_paint_dirty(self) -> bool {
        self.0 & Self::PAINT.0 != 0
    }

    /// Check if children are dirty
    pub const fn are_children_dirty(self) -> bool {
        self.0 & Self::CHILDREN.0 != 0
    }

    /// Check if any flag is set (view needs any update)
    pub const fn is_dirty(self) -> bool {
        self.0 != 0
    }

    /// Mark layout as dirty
    pub const fn layout() -> Self {
        Self::LAYOUT
    }

    /// Mark paint as dirty
    pub const fn paint() -> Self {
        Self::PAINT
    }

    /// Mark children as dirty
    pub const fn children() -> Self {
        Self::CHILDREN
    }

    /// Set flags
    pub fn set(&mut self, flags: Self) {
        self.0 |= flags.0;
    }

    /// Clear flags
    pub fn clear(&mut self, flags: Self) {
        self.0 &= !flags.0;
    }

    /// Clear all flags
    pub fn clear_all(&mut self) {
        self.0 = 0;
    }
}

impl Default for DirtyFlags {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for DirtyFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        if self.is_layout_dirty() {
            write!(f, "LAYOUT")?;
            first = false;
        }
        if self.is_paint_dirty() {
            if !first {
                write!(f, " | ")?;
            }
            write!(f, "PAINT")?;
            first = false;
        }
        if self.are_children_dirty() {
            if !first {
                write!(f, " | ")?;
            }
            write!(f, "CHILDREN")?;
            first = false;
        }

        if first {
            write!(f, "CLEAN")
        } else {
            Ok(())
        }
    }
}

// Implement bitwise OR for combining flags
impl core::ops::BitOr for DirtyFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for DirtyFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dirty_flags() {
        let mut flags = DirtyFlags::new();
        assert!(!flags.is_dirty());
        assert!(!flags.is_layout_dirty());
        assert!(!flags.is_paint_dirty());

        flags.set(DirtyFlags::LAYOUT);
        assert!(flags.is_dirty());
        assert!(flags.is_layout_dirty());
        assert!(!flags.is_paint_dirty());

        flags.set(DirtyFlags::PAINT);
        assert!(flags.is_paint_dirty());

        flags.clear(DirtyFlags::LAYOUT);
        assert!(!flags.is_layout_dirty());
        assert!(flags.is_paint_dirty());
    }

    #[test]
    fn test_dirty_flags_or() {
        let flags = DirtyFlags::LAYOUT | DirtyFlags::PAINT;
        assert!(flags.is_layout_dirty());
        assert!(flags.is_paint_dirty());
        assert!(!flags.are_children_dirty());
    }

    #[test]
    fn test_dirty_flags_clear_all() {
        let mut flags = DirtyFlags::LAYOUT | DirtyFlags::PAINT;
        flags.clear_all();
        assert!(!flags.is_dirty());
    }
}
