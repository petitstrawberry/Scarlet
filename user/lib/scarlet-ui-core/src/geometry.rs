//! Geometry primitives for ScarletUI
//!
//! Provides basic types for 2D geometry operations.

use core::ops::{Add, AddAssign, Sub};
use libm;

/// 2D size with width and height
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const ZERO: Self = Self {
        width: 0.0,
        height: 0.0,
    };

    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    /// Check if size is infinite in any dimension
    pub fn is_infinite(&self) -> bool {
        self.width.is_infinite() || self.height.is_infinite()
    }

    /// Check if size is zero
    pub fn is_zero(&self) -> bool {
        self.width == 0.0 && self.height == 0.0
    }

    /// Constrain this size within bounds
    pub fn constrain(&self, min: Size, max: Size) -> Size {
        Size {
            width: self.width.clamp(min.width, max.width),
            height: self.height.clamp(min.height, max.height),
        }
    }
}

/// 2D point with x and y coordinates
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// Calculate distance to another point
    pub fn distance_to(&self, other: Point) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        libm::sqrtf(dx * dx + dy * dy)
    }

    /// Translate point by offset
    pub fn translate(&self, offset: Offset) -> Point {
        Point {
            x: self.x + offset.dx,
            y: self.y + offset.dy,
        }
    }
}

impl Add<Offset> for Point {
    type Output = Point;

    fn add(self, offset: Offset) -> Point {
        self.translate(offset)
    }
}

impl AddAssign<Offset> for Point {
    fn add_assign(&mut self, offset: Offset) {
        self.x += offset.dx;
        self.y += offset.dy;
    }
}

/// 2D offset with dx and dy components
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Offset {
    pub dx: f32,
    pub dy: f32,
}

impl Offset {
    pub const ZERO: Self = Self { dx: 0.0, dy: 0.0 };

    pub const fn new(dx: f32, dy: f32) -> Self {
        Self { dx, dy }
    }

    pub fn from_size(size: Size) -> Self {
        Offset {
            dx: size.width,
            dy: size.height,
        }
    }
}

impl Add for Offset {
    type Output = Offset;

    fn add(self, other: Offset) -> Offset {
        Offset {
            dx: self.dx + other.dx,
            dy: self.dy + other.dy,
        }
    }
}

impl Sub for Offset {
    type Output = Offset;

    fn sub(self, other: Offset) -> Offset {
        Offset {
            dx: self.dx - other.dx,
            dy: self.dy - other.dy,
        }
    }
}

/// 2D rectangle with origin and size
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

impl Rect {
    /// Create a zero-sized rect at origin
    pub const fn zero() -> Self {
        Self {
            origin: Point::ZERO,
            size: Size::ZERO,
        }
    }

    pub const fn new(origin: Point, size: Size) -> Self {
        Self { origin, size }
    }

    /// Create rect from x, y, width, height
    pub const fn from_xywh(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            origin: Point { x, y },
            size: Size { width, height },
        }
    }

    /// Create rect from origin point and size
    pub fn from_point_size(point: Point, size: Size) -> Self {
        Self {
            origin: point,
            size,
        }
    }

    /// Get the left edge
    pub fn left(&self) -> f32 {
        self.origin.x
    }

    /// Get the right edge
    pub fn right(&self) -> f32 {
        self.origin.x + self.size.width
    }

    /// Get the top edge
    pub fn top(&self) -> f32 {
        self.origin.y
    }

    /// Get the bottom edge
    pub fn bottom(&self) -> f32 {
        self.origin.y + self.size.height
    }

    /// Get the width
    pub fn width(&self) -> f32 {
        self.size.width
    }

    /// Get the height
    pub fn height(&self) -> f32 {
        self.size.height
    }

    /// Check if rect contains a point
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.left()
            && point.x < self.right()
            && point.y >= self.top()
            && point.y < self.bottom()
    }

    /// Check if rect overlaps with another
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.left() < other.right()
            && self.right() > other.left()
            && self.top() < other.bottom()
            && self.bottom() > other.top()
    }

    /// Inset rect by insets on all sides
    pub fn inset(&self, insets: EdgeInsets) -> Rect {
        Rect {
            origin: Point {
                x: self.origin.x + insets.left,
                y: self.origin.y + insets.top,
            },
            size: Size {
                width: self.size.width - insets.left - insets.right,
                height: self.size.height - insets.top - insets.bottom,
            },
        }
    }

    /// Get the center point of the rect
    pub fn center(&self) -> Point {
        Point {
            x: self.origin.x + self.size.width / 2.0,
            y: self.origin.y + self.size.height / 2.0,
        }
    }
}

/// Insets for padding/border on each side
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct EdgeInsets {
    pub top: f32,
    pub left: f32,
    pub bottom: f32,
    pub right: f32,
}

impl EdgeInsets {
    pub const ZERO: Self = Self {
        top: 0.0,
        left: 0.0,
        bottom: 0.0,
        right: 0.0,
    };

    pub const fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            top,
            left,
            bottom,
            right,
        }
    }

    pub const fn all(value: f32) -> Self {
        Self {
            top: value,
            left: value,
            bottom: value,
            right: value,
        }
    }

    pub const fn symmetric(vertical: f32, horizontal: f32) -> Self {
        Self {
            top: vertical,
            left: horizontal,
            bottom: vertical,
            right: horizontal,
        }
    }

    pub const fn only_top(value: f32) -> Self {
        Self {
            top: value,
            left: 0.0,
            bottom: 0.0,
            right: 0.0,
        }
    }

    pub const fn only_left(value: f32) -> Self {
        Self {
            top: 0.0,
            left: value,
            bottom: 0.0,
            right: 0.0,
        }
    }

    pub const fn only_right(value: f32) -> Self {
        Self {
            top: 0.0,
            left: 0.0,
            bottom: 0.0,
            right: value,
        }
    }

    pub const fn only_bottom(value: f32) -> Self {
        Self {
            top: 0.0,
            left: 0.0,
            bottom: value,
            right: 0.0,
        }
    }

    pub const fn horizontal(value: f32) -> Self {
        Self {
            top: 0.0,
            left: value,
            bottom: 0.0,
            right: value,
        }
    }

    pub const fn vertical(value: f32) -> Self {
        Self {
            top: value,
            left: 0.0,
            bottom: value,
            right: 0.0,
        }
    }
}

/// Alignment for positioning children in containers
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Alignment {
    #[default]
    Center,
    Leading,
    Trailing,
    Top,
    Bottom,
    TopLeading,
    TopTrailing,
    BottomLeading,
    BottomTrailing,
}

impl Alignment {
    /// Apply alignment to position a child in a parent
    pub fn apply(&self, parent_size: Size, child_size: Size) -> Point {
        match self {
            Alignment::Center => Point {
                x: (parent_size.width - child_size.width) / 2.0,
                y: (parent_size.height - child_size.height) / 2.0,
            },
            Alignment::Leading => Point {
                x: 0.0,
                y: (parent_size.height - child_size.height) / 2.0,
            },
            Alignment::Trailing => Point {
                x: parent_size.width - child_size.width,
                y: (parent_size.height - child_size.height) / 2.0,
            },
            Alignment::Top => Point {
                x: (parent_size.width - child_size.width) / 2.0,
                y: 0.0,
            },
            Alignment::Bottom => Point {
                x: (parent_size.width - child_size.width) / 2.0,
                y: parent_size.height - child_size.height,
            },
            Alignment::TopLeading => Point::ZERO,
            Alignment::TopTrailing => Point {
                x: parent_size.width - child_size.width,
                y: 0.0,
            },
            Alignment::BottomLeading => Point {
                x: 0.0,
                y: parent_size.height - child_size.height,
            },
            Alignment::BottomTrailing => Point {
                x: parent_size.width - child_size.width,
                y: parent_size.height - child_size.height,
            },
        }
    }

    /// Apply alignment for horizontal axis (for VStack cross-axis)
    pub fn align_x(&self, parent_width: f32, child_width: f32) -> f32 {
        match self {
            Alignment::Leading | Alignment::TopLeading | Alignment::BottomLeading => 0.0,
            Alignment::Trailing | Alignment::TopTrailing | Alignment::BottomTrailing => {
                parent_width - child_width
            }
            Alignment::Center | Alignment::Top | Alignment::Bottom => {
                (parent_width - child_width) / 2.0
            }
        }
    }

    /// Apply alignment for vertical axis (for HStack cross-axis)
    pub fn align_y(&self, parent_height: f32, child_height: f32) -> f32 {
        match self {
            Alignment::Top | Alignment::TopLeading | Alignment::TopTrailing => 0.0,
            Alignment::Bottom | Alignment::BottomLeading | Alignment::BottomTrailing => {
                parent_height - child_height
            }
            Alignment::Center | Alignment::Leading | Alignment::Trailing => {
                (parent_height - child_height) / 2.0
            }
        }
    }
}
