//! Gesture Recognizer - Pointer gesture recognition
//!
//! This module provides gesture recognizers for common pointer gestures
//! like tap, drag, long press, etc.

use crate::event::MouseEvent;
use crate::geometry::Point;
use alloc::boxed::Box;
use alloc::vec::Vec;

// Use libm for sqrt in no_std environment
use libm;

/// Gesture recognizer trait
pub trait GestureRecognizer {
    /// Process a mouse event and return recognized gesture if complete
    fn process_event(&mut self, event: &MouseEvent) -> Option<Gesture>;

    /// Reset the recognizer state
    fn reset(&mut self);

    /// Update time-based recognizers (call each frame with delta time in ms)
    fn update(&mut self, _delta_ms: u64) -> Option<Gesture> {
        None
    }

    /// Clone the recognizer
    fn clone_box(&self) -> Box<dyn GestureRecognizer>;
}

/// Types of gestures that can be recognized
#[derive(Clone, Debug, PartialEq)]
pub enum Gesture {
    /// Tap gesture (click/tap)
    Tap { position: Point, count: usize },
    /// Drag gesture
    Drag {
        start_position: Point,
        current_position: Point,
        delta: Point,
    },
    /// Long press gesture
    LongPress { position: Point, duration_ms: u64 },
    /// Pinch gesture (for touch displays)
    Pinch { center: Point, scale: f32 },
    /// Rotation gesture (for touch displays)
    Rotation { center: Point, angle: f32 },
}

impl Gesture {
    /// Get the position of the gesture
    pub fn position(&self) -> Point {
        match self {
            Gesture::Tap { position, .. } => *position,
            Gesture::Drag {
                current_position, ..
            } => *current_position,
            Gesture::LongPress { position, .. } => *position,
            Gesture::Pinch { center, .. } => *center,
            Gesture::Rotation { center, .. } => *center,
        }
    }
}

/// Tap gesture recognizer
pub struct TapGestureRecognizer {
    start_position: Option<Point>,
    tap_count: usize,
    max_distance: f32,
    is_active: bool,
}

impl TapGestureRecognizer {
    /// Create a new tap gesture recognizer
    pub fn new() -> Self {
        Self {
            start_position: None,
            tap_count: 1,
            max_distance: 10.0, // Maximum movement distance for tap
            is_active: false,
        }
    }

    /// Set the tap count (1 for single tap, 2 for double tap, etc.)
    pub fn with_tap_count(mut self, count: usize) -> Self {
        self.tap_count = count;
        self
    }

    /// Set the maximum distance for tap recognition
    pub fn with_max_distance(mut self, distance: f32) -> Self {
        self.max_distance = distance;
        self
    }
}

impl Default for TapGestureRecognizer {
    fn default() -> Self {
        Self::new()
    }
}

impl GestureRecognizer for TapGestureRecognizer {
    fn process_event(&mut self, event: &MouseEvent) -> Option<Gesture> {
        match event {
            MouseEvent::ButtonPressed { x, y, .. } => {
                self.start_position = Some(Point {
                    x: *x as f32,
                    y: *y as f32,
                });
                self.is_active = true;
                None
            }
            MouseEvent::ButtonReleased { x, y, .. } => {
                if let (Some(start), true) = (self.start_position, self.is_active) {
                    let end = Point {
                        x: *x as f32,
                        y: *y as f32,
                    };
                    let dx = end.x - start.x;
                    let dy = end.y - start.y;
                    let distance = libm::sqrtf(dx * dx + dy * dy);

                    self.is_active = false;
                    self.start_position = None;

                    if distance <= self.max_distance {
                        return Some(Gesture::Tap {
                            position: end,
                            count: self.tap_count,
                        });
                    }
                }
                None
            }
            MouseEvent::Moved { .. } | MouseEvent::Entered { .. } | MouseEvent::Exited { .. } => {
                // Movement doesn't cancel tap, just check distance on release
                None
            }
            MouseEvent::Wheel { .. } => {
                // Wheel event cancels tap
                self.reset();
                None
            }
        }
    }

    fn reset(&mut self) {
        self.start_position = None;
        self.is_active = false;
    }

    fn clone_box(&self) -> Box<dyn GestureRecognizer> {
        Box::new(self.clone())
    }
}

impl Clone for TapGestureRecognizer {
    fn clone(&self) -> Self {
        Self {
            start_position: self.start_position,
            tap_count: self.tap_count,
            max_distance: self.max_distance,
            is_active: self.is_active,
        }
    }
}

/// Drag gesture recognizer
pub struct DragGestureRecognizer {
    start_position: Option<Point>,
    current_position: Option<Point>,
    minimum_distance: f32,
    is_active: bool,
    has_started: bool,
}

impl DragGestureRecognizer {
    /// Create a new drag gesture recognizer
    pub fn new() -> Self {
        Self {
            start_position: None,
            current_position: None,
            minimum_distance: 5.0, // Minimum movement to start drag
            is_active: false,
            has_started: false,
        }
    }

    /// Set the minimum distance to start drag
    pub fn with_minimum_distance(mut self, distance: f32) -> Self {
        self.minimum_distance = distance;
        self
    }
}

impl Default for DragGestureRecognizer {
    fn default() -> Self {
        Self::new()
    }
}

impl GestureRecognizer for DragGestureRecognizer {
    fn process_event(&mut self, event: &MouseEvent) -> Option<Gesture> {
        match event {
            MouseEvent::ButtonPressed { x, y, .. } => {
                self.start_position = Some(Point {
                    x: *x as f32,
                    y: *y as f32,
                });
                self.current_position = self.start_position;
                self.is_active = true;
                self.has_started = false;
                None
            }
            MouseEvent::Moved { x, y } => {
                if let (Some(start), true) = (self.start_position, self.is_active) {
                    let current = Point {
                        x: *x as f32,
                        y: *y as f32,
                    };
                    let delta = Point {
                        x: current.x - start.x,
                        y: current.y - start.y,
                    };
                    let dist_sq = delta.x * delta.x + delta.y * delta.y;
                    let distance = libm::sqrtf(dist_sq);

                    self.current_position = Some(current);

                    if distance >= self.minimum_distance || self.has_started {
                        self.has_started = true;
                        return Some(Gesture::Drag {
                            start_position: start,
                            current_position: current,
                            delta,
                        });
                    }
                }
                None
            }
            MouseEvent::Entered { .. } | MouseEvent::Exited { .. } => None,
            MouseEvent::ButtonReleased { .. } => {
                self.reset();
                None
            }
            MouseEvent::Wheel { .. } => {
                self.reset();
                None
            }
        }
    }

    fn reset(&mut self) {
        self.start_position = None;
        self.current_position = None;
        self.is_active = false;
        self.has_started = false;
    }

    fn clone_box(&self) -> Box<dyn GestureRecognizer> {
        Box::new(self.clone())
    }
}

impl Clone for DragGestureRecognizer {
    fn clone(&self) -> Self {
        Self {
            start_position: self.start_position,
            current_position: self.current_position,
            minimum_distance: self.minimum_distance,
            is_active: self.is_active,
            has_started: self.has_started,
        }
    }
}

/// Long press gesture recognizer
pub struct LongPressGestureRecognizer {
    start_position: Option<Point>,
    duration_ms: u64,
    required_duration_ms: u64,
    max_distance: f32,
    is_active: bool,
    has_triggered: bool,
}

impl LongPressGestureRecognizer {
    /// Create a new long press gesture recognizer
    pub fn new() -> Self {
        Self {
            start_position: None,
            duration_ms: 0,
            required_duration_ms: 500, // 500ms for long press
            max_distance: 10.0,
            is_active: false,
            has_triggered: false,
        }
    }

    /// Set the required duration for long press (in milliseconds)
    pub fn with_duration(mut self, duration_ms: u64) -> Self {
        self.required_duration_ms = duration_ms;
        self
    }

    /// Set the maximum distance for long press recognition
    pub fn with_max_distance(mut self, distance: f32) -> Self {
        self.max_distance = distance;
        self
    }
}

impl Default for LongPressGestureRecognizer {
    fn default() -> Self {
        Self::new()
    }
}

impl GestureRecognizer for LongPressGestureRecognizer {
    fn process_event(&mut self, event: &MouseEvent) -> Option<Gesture> {
        match event {
            MouseEvent::ButtonPressed { x, y, .. } => {
                self.start_position = Some(Point {
                    x: *x as f32,
                    y: *y as f32,
                });
                self.duration_ms = 0;
                self.is_active = true;
                self.has_triggered = false;
                None
            }
            MouseEvent::Moved { x, y } => {
                if let (Some(start), true) = (self.start_position, self.is_active) {
                    let current = Point {
                        x: *x as f32,
                        y: *y as f32,
                    };
                    let dx = current.x - start.x;
                    let dy = current.y - start.y;
                    let distance = libm::sqrtf(dx * dx + dy * dy);

                    if distance > self.max_distance {
                        self.reset();
                    }
                }
                None
            }
            MouseEvent::Entered { .. } | MouseEvent::Exited { .. } => None,
            MouseEvent::ButtonReleased { .. } => {
                self.reset();
                None
            }
            MouseEvent::Wheel { .. } => {
                self.reset();
                None
            }
        }
    }

    fn update(&mut self, delta_ms: u64) -> Option<Gesture> {
        if self.is_active && !self.has_triggered {
            self.duration_ms += delta_ms;

            if self.duration_ms >= self.required_duration_ms {
                if let Some(position) = self.start_position {
                    self.has_triggered = true;
                    return Some(Gesture::LongPress {
                        position,
                        duration_ms: self.duration_ms,
                    });
                }
            }
        }
        None
    }

    fn reset(&mut self) {
        self.start_position = None;
        self.duration_ms = 0;
        self.is_active = false;
        self.has_triggered = false;
    }

    fn clone_box(&self) -> Box<dyn GestureRecognizer> {
        Box::new(self.clone())
    }
}

impl Clone for LongPressGestureRecognizer {
    fn clone(&self) -> Self {
        Self {
            start_position: self.start_position,
            duration_ms: self.duration_ms,
            required_duration_ms: self.required_duration_ms,
            max_distance: self.max_distance,
            is_active: self.is_active,
            has_triggered: self.has_triggered,
        }
    }
}

/// Gesture manager that handles multiple gesture recognizers
pub struct GestureManager {
    recognizers: Vec<Box<dyn GestureRecognizer>>,
}

impl GestureManager {
    /// Create a new gesture manager
    pub fn new() -> Self {
        Self {
            recognizers: Vec::new(),
        }
    }

    /// Add a gesture recognizer
    pub fn add(&mut self, recognizer: Box<dyn GestureRecognizer>) {
        self.recognizers.push(recognizer);
    }

    /// Process an event and return recognized gestures
    pub fn process_event(&mut self, event: &MouseEvent) -> Vec<Gesture> {
        let mut gestures = Vec::new();

        for recognizer in &mut self.recognizers {
            if let Some(gesture) = recognizer.process_event(event) {
                gestures.push(gesture);
            }
        }

        gestures
    }

    /// Update time-based recognizers (call each frame with delta time in ms)
    pub fn update(&mut self, delta_ms: u64) -> Vec<Gesture> {
        let mut gestures = Vec::new();

        for recognizer in &mut self.recognizers {
            if let Some(gesture) = recognizer.update(delta_ms) {
                gestures.push(gesture);
            }
        }

        gestures
    }

    /// Reset all recognizers
    pub fn reset_all(&mut self) {
        for recognizer in &mut self.recognizers {
            recognizer.reset();
        }
    }
}

impl Default for GestureManager {
    fn default() -> Self {
        Self::new()
    }
}
