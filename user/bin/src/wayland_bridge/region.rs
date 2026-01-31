//! Region management for Wayland surfaces
//!
//! A region is a collection of rectangles that can be used to define
//! opaque regions or input regions for a surface.

use std::collections::BTreeMap;
use std::vec::Vec;

/// A rectangle with integer coordinates
#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width: width.max(0),
            height: height.max(0),
        }
    }

    /// Check if this rectangle intersects with another
    pub fn intersects(&self, other: &Rect) -> bool {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = (self.x + self.width).min(other.x + other.width);
        let y2 = (self.y + self.height).min(other.y + other.height);
        x1 < x2 && y1 < y2
    }

    /// Check if a point is inside this rectangle
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// A region is a collection of rectangles
#[derive(Debug, Clone)]
pub struct Region {
    pub id: u32,
    pub rects: Vec<Rect>,
}

impl Region {
    pub fn new(id: u32) -> Self {
        Self {
            id,
            rects: Vec::new(),
        }
    }

    /// Add a rectangle to the region
    pub fn add(&mut self, x: i32, y: i32, width: i32, height: i32) {
        // Simplified: just add the rectangle
        // Full implementation would merge overlapping rectangles
        self.rects.push(Rect::new(x, y, width, height));
    }

    /// Subtract a rectangle from the region
    pub fn subtract(&mut self, x: i32, y: i32, width: i32, height: i32) {
        // Simplified: remove rectangles that are completely contained
        // Full implementation would split overlapping rectangles
        let remove_rect = Rect::new(x, y, width, height);
        self.rects.retain(|rect| {
            // Keep rectangles that don't intersect or aren't fully contained
            !rect.intersects(&remove_rect)
                || rect.x < remove_rect.x
                || rect.y < remove_rect.y
                || rect.x + rect.width > remove_rect.x + remove_rect.width
                || rect.y + rect.height > remove_rect.y + remove_rect.height
        });
    }

    /// Check if a point is inside the region
    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.rects.iter().any(|rect| rect.contains(x, y))
    }

    /// Check if the region is empty
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }
}

/// Manages regions for the Wayland bridge
pub struct RegionManager {
    regions: BTreeMap<u32, Region>,
    next_id: u32,
}

impl RegionManager {
    pub fn new() -> Self {
        Self {
            regions: BTreeMap::new(),
            next_id: 1,
        }
    }

    /// Create a new region and return its ID
    pub fn create_region(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.regions.insert(id, Region::new(id));
        id
    }

    /// Create a region with a specific ID (used by WaylandBridge)
    pub fn create_region_with_id(&mut self, id: u32) {
        // Update next_id to avoid collisions
        if id >= self.next_id {
            self.next_id = id + 1;
        }
        self.regions.insert(id, Region::new(id));
    }

    /// Get a reference to a region
    pub fn get_region(&self, id: u32) -> Option<&Region> {
        self.regions.get(&id)
    }

    /// Get a mutable reference to a region
    pub fn get_region_mut(&mut self, id: u32) -> Option<&mut Region> {
        self.regions.get_mut(&id)
    }

    /// Destroy a region
    pub fn destroy_region(&mut self, id: u32) {
        self.regions.remove(&id);
    }

    /// Add a rectangle to a region
    pub fn add_to_region(&mut self, region_id: u32, x: i32, y: i32, width: i32, height: i32) {
        if let Some(region) = self.regions.get_mut(&region_id) {
            region.add(x, y, width, height);
        }
    }

    /// Subtract a rectangle from a region
    pub fn subtract_from_region(
        &mut self,
        region_id: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) {
        if let Some(region) = self.regions.get_mut(&region_id) {
            region.subtract(x, y, width, height);
        }
    }
}
