//! Layout constraints and layout system
//!
//! This module provides the layout system with constraints-based sizing
//! (similar to Flutter's BoxConstraints or Druid's constraints).
//!
//! ## Layout Phases
//!
//! The layout process has two phases:
//! 1. **Measure** (`LayoutConstraints::constrain()`): Given constraints, calculate desired size
//! 2. **Arrange** (parent positions child): Place the view at a specific position
//!
//! ## Layout Constraints
//!
//! - **Tight**: min == max (exact size)
//! - **Loose**: min == 0 (any size up to max)
//! - **Unbounded**: max == u32::MAX (no upper limit)

use crate::graphics::Rect;

/// Size in 2D space
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const ZERO: Size = Size::new(0, 0);

    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Constrain this size to fit within the given constraints
    pub fn constrain(&self, constraints: LayoutConstraints) -> Size {
        Size {
            width: self
                .width
                .clamp(constraints.min_width, constraints.max_width),
            height: self
                .height
                .clamp(constraints.min_height, constraints.max_height),
        }
    }
}

/// Layout constraints define the minimum and maximum size for a view
///
/// Similar to Flutter's BoxConstraints or Druid's BoxConstraints.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LayoutConstraints {
    pub min_width: u32,
    pub max_width: u32,
    pub min_height: u32,
    pub max_height: u32,
}

impl LayoutConstraints {
    /// Create new constraints with explicit min/max values
    pub const fn new(min_width: u32, max_width: u32, min_height: u32, max_height: u32) -> Self {
        Self {
            min_width,
            max_width,
            min_height,
            max_height,
        }
    }

    /// Create tight constraints (exact size)
    ///
    /// The view must be exactly this size.
    pub fn tight(size: Size) -> Self {
        Self {
            min_width: size.width,
            max_width: size.width,
            min_height: size.height,
            max_height: size.height,
        }
    }

    /// Create loose constraints (any size up to max)
    ///
    /// The view can be any size from (0, 0) to (max_width, max_height).
    pub fn loose(max: Size) -> Self {
        Self {
            min_width: 0,
            max_width: max.width,
            min_height: 0,
            max_height: max.height,
        }
    }

    /// Create unconstrained constraints (any size)
    ///
    /// The view can be any size. Use with caution - this can lead to
    /// overflow issues if the view requests an enormous size.
    pub fn unconstrained() -> Self {
        Self {
            min_width: 0,
            max_width: u32::MAX,
            min_height: 0,
            max_height: u32::MAX,
        }
    }

    /// Constrain a size to fit within these constraints
    pub fn constrain(&self, size: Size) -> Size {
        size.constrain(*self)
    }

    /// Check if constraints are tight (min == max)
    pub fn is_tight(&self) -> bool {
        self.min_width == self.max_width && self.min_height == self.max_height
    }

    /// Check if constraints are bounded (has a finite max)
    pub fn is_bounded(&self) -> bool {
        self.max_width != u32::MAX || self.max_height != u32::MAX
    }
}

/// Layout trait for views that participate in layout
///
/// This is a two-pass layout system:
/// 1. `measure()`: Given constraints, calculate desired size
/// 2. `arrange()`: Given a frame, position children within that frame
///
/// This separation allows for efficient layout caching and
/// incremental updates.
pub trait Layout {
    /// Calculate the desired size given constraints
    ///
    /// This is the first pass of layout. The view should return
    /// the size it wants to be, constrained by the given constraints.
    fn measure(&mut self, constraints: LayoutConstraints) -> Size;

    /// Position children within the given frame
    ///
    /// This is the second pass of layout. The parent calls this
    /// after determining the child's size, passing the actual frame
    /// the child should occupy.
    fn arrange(&mut self, frame: Rect);
}

/// Cross-axis alignment for flex containers (VStack, HStack)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CrossAxisAlignment {
    /// Align to the start of the cross axis
    Start,
    /// Align to the center of the cross axis
    Center,
    /// Align to the end of the cross axis
    End,
    /// Stretch to fill the cross axis
    Stretch,
}

/// Main-axis alignment for flex containers
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MainAxisAlignment {
    /// Align to the start of the main axis
    Start,
    /// Align to the center of the main axis
    Center,
    /// Align to the end of the main axis
    End,
    /// Distribute space evenly between children
    SpaceBetween,
    /// Distribute space evenly around children
    SpaceAround,
    /// Distribute space evenly with equal spacing
    SpaceEvenly,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_size_constrain() {
        let size = Size::new(100, 200);
        let constraints = LayoutConstraints::loose(Size::new(50, 50));
        let constrained = size.constrain(constraints);
        assert_eq!(constrained, Size::new(50, 50));
    }

    #[test]
    fn test_tight_constraints() {
        let size = Size::new(100, 100);
        let constraints = LayoutConstraints::tight(size);
        assert!(constraints.is_tight());
        let result = constraints.constrain(Size::new(50, 200));
        assert_eq!(result, Size::new(100, 100));
    }

    #[test]
    fn test_loose_constraints() {
        let constraints = LayoutConstraints::loose(Size::new(100, 100));
        assert!(!constraints.is_tight());
        let result = constraints.constrain(Size::new(50, 50));
        assert_eq!(result, Size::new(50, 50));
    }

    #[test]
    fn test_unconstrained() {
        let constraints = LayoutConstraints::unconstrained();
        assert!(!constraints.is_tight());
        assert!(!constraints.is_bounded());
    }
}
