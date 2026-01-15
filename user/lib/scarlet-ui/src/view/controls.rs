//! Control views (UI widgets with reactive state support)
//!
//! All controls support two-way binding via `Binding<T>` for reactive updates.

use super::traits::{View, Size, Focus, Hoverable};
use crate::graphics::{measure_text_sized, Canvas, Rect};
use crate::Color;
use crate::event::{Event, EventKind, MouseButton};
use crate::state::{State, Binding, ViewRefreshHandle};
use scarlet_std::string::String;
use scarlet_std::sync::Arc;
use scarlet_std::vec::Vec;
use super::traits::ViewBox;
use scarlet_std::boxed::Box;

/// Text label view
pub struct Label {
    text: String,
    color: Color,
    font_size: u32,
    needs_redraw: bool,
}

impl Label {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            color: Color::WHITE,
            font_size: 16,
            needs_redraw: false,
        }
    }

    /// Set text color
    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    /// Set font size
    pub fn font_size(mut self, size: u32) -> Self {
        self.font_size = size;
        self
    }

    /// Update text content
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.needs_redraw = true;
    }
}

impl View for Label {
    fn layout(&mut self, _available: Size) -> Size {
        let (w, h) = measure_text_sized(&self.text, self.font_size as f32);
        Size::new(w, h)
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        canvas.draw_text_sized(frame.x, frame.y, &self.text, self.color, self.font_size as f32);
    }

    fn needs_draw(&self) -> bool {
        self.needs_redraw
    }

    fn set_needs_draw(&mut self) {
        self.needs_redraw = true;
    }

    fn clear_needs_draw(&mut self) {
        self.needs_redraw = false;
    }
}

/// Reactive text view (SwiftUI-like `Text`)
///
/// This view recomputes its text when any watched `State<T>` changes.
/// Use the `text!()` / `label!()` macros for ergonomic formatting.
///
/// Includes throttling to reduce flicker during rapid state changes.
pub struct Text {
    formatter: Arc<dyn Fn() -> String + Send + Sync>,
    color: Color,
    font_size: u32,
    refresh_handle: ViewRefreshHandle,
    cached_text: String,
    /// Frame counter for throttling
    frame_counter: u32,
    /// Last frame when text was updated
    last_update_frame: u32,
    /// Minimum frames between updates (0 = no throttling)
    throttle_frames: u32,
}

impl Text {
    pub fn new<F>(formatter: F) -> Self
    where
        F: Fn() -> String + Send + Sync + 'static,
    {
        let formatter = Arc::new(formatter);
        let cached_text = formatter();
        Self {
            formatter,
            color: Color::WHITE,
            font_size: 16,
            refresh_handle: ViewRefreshHandle::new(),
            cached_text,
            frame_counter: 0,
            last_update_frame: 0,
            throttle_frames: 0,
        }
    }

    /// Construct a `Text` using a pre-created refresh handle.
    ///
    /// This is used by macros to subscribe multiple states to the same handle.
    pub fn from_refresh_handle<F>(refresh_handle: ViewRefreshHandle, formatter: F) -> Self
    where
        F: Fn() -> String + Send + Sync + 'static,
    {
        let formatter = Arc::new(formatter);
        let cached_text = formatter();
        Self {
            formatter,
            color: Color::WHITE,
            font_size: 16,
            refresh_handle,
            cached_text,
            frame_counter: 0,
            last_update_frame: 0,
            throttle_frames: 0,
        }
    }

    /// Watch a state. When it changes, the text is marked dirty.
    pub fn watch<T>(self, state: State<T>) -> Self {
        state.subscribe_view(&self.refresh_handle);
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    pub fn font_size(mut self, size: u32) -> Self {
        self.font_size = size;
        self
    }
    
    /// Set throttling to reduce update frequency.
    /// 
    /// `frames` is the minimum number of frames between updates.
    /// Use this for rapidly changing values like sliders or timers.
    pub fn throttle(mut self, frames: u32) -> Self {
        self.throttle_frames = frames;
        self
    }
}

impl View for Text {
    fn layout(&mut self, _available: Size) -> Size {
        self.frame_counter = self.frame_counter.wrapping_add(1);
        
        if self.refresh_handle.take_dirty() {
            // Check throttling
            let frames_since_update = self.frame_counter.wrapping_sub(self.last_update_frame);
            if self.throttle_frames == 0 || frames_since_update >= self.throttle_frames {
                self.cached_text = (self.formatter)();
                self.last_update_frame = self.frame_counter;
            } else {
                // Re-mark as dirty to update later
                self.refresh_handle.mark_dirty();
            }
        }
        let (w, h) = measure_text_sized(&self.cached_text, self.font_size as f32);
        Size::new(w, h)
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        canvas.draw_text_sized(
            frame.x,
            frame.y,
            &self.cached_text,
            self.color,
            self.font_size as f32,
        );
    }

    fn needs_draw(&self) -> bool {
        self.refresh_handle.is_dirty()
    }

    fn set_needs_draw(&mut self) {
        self.refresh_handle.mark_dirty();
    }

    fn clear_needs_draw(&mut self) {
        self.refresh_handle.take_dirty();
    }
}

/// Reactive label that automatically updates when state changes
///
/// This label is bound to a `State<T>` and automatically redraws
/// whenever the state value changes. Use a formatter function to
/// convert the state value to display text.
///
/// # Example
///
/// ```no_run
/// use scarlet_ui::{State, ReactiveLabel};
///
/// let counter = State::new(0);
///
/// // Label automatically updates when counter changes
/// ReactiveLabel::new(counter.clone(), |count| format!("Count: {}", count))
/// ```
pub struct ReactiveLabel<T: Clone + 'static> {
    state: State<T>,
    formatter: Arc<dyn Fn(&T) -> String + Send + Sync>,
    color: Color,
    font_size: u32,
    refresh_handle: ViewRefreshHandle,
    cached_text: String,
}

impl<T: Clone + 'static> ReactiveLabel<T> {
    /// Create a new reactive label
    ///
    /// The formatter function converts the state value to display text.
    pub fn new<F>(state: State<T>, formatter: F) -> Self
    where
        F: Fn(&T) -> String + Send + Sync + 'static,
    {
        let refresh_handle = ViewRefreshHandle::new();
        state.subscribe_view(&refresh_handle);
        
        // Get initial text
        let cached_text = state.with(|v| formatter(v));
        
        Self {
            state,
            formatter: Arc::new(formatter),
            color: Color::WHITE,
            font_size: 16,
            refresh_handle,
            cached_text,
        }
    }

    /// Set text color
    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    /// Set font size
    pub fn font_size(mut self, size: u32) -> Self {
        self.font_size = size;
        self
    }

    fn update_text(&mut self) {
        self.cached_text = self.state.with(|v| (self.formatter)(v));
    }
}

impl<T: Clone + 'static> View for ReactiveLabel<T> {
    fn layout(&mut self, _available: Size) -> Size {
        // Check if state changed and update text
        if self.refresh_handle.take_dirty() {
            self.update_text();
        }
        let (w, h) = measure_text_sized(&self.cached_text, self.font_size as f32);
        Size::new(w, h)
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        canvas.draw_text_sized(frame.x, frame.y, &self.cached_text, self.color, self.font_size as f32);
    }

    fn needs_draw(&self) -> bool {
        self.refresh_handle.is_dirty()
    }

    fn set_needs_draw(&mut self) {
        self.refresh_handle.mark_dirty();
    }

    fn clear_needs_draw(&mut self) {
        self.refresh_handle.take_dirty();
    }
}

/// Button view with click action
pub enum ButtonLabel {
    Text(String),
    View(ViewBox),
}

impl From<String> for ButtonLabel {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for ButtonLabel {
    fn from(value: &str) -> Self {
        Self::Text(value.into())
    }
}

impl<V: View + 'static> From<V> for ButtonLabel {
    fn from(value: V) -> Self {
        Self::View(Box::new(value))
    }
}

pub struct Button<F: FnMut() + 'static> {
    label: ButtonLabel,
    on_click: F,
    background: Color,
    text_color: Color,
    corner_radius: u32,
    padding: u32,
    is_hovered: bool,
    is_pressed: bool,
    label_size: Size,
    needs_redraw: bool,
}

impl<F: FnMut() + 'static> Button<F> {
    pub fn new(label: impl Into<ButtonLabel>, on_click: F) -> Self {
        Self {
            label: label.into(),
            on_click,
            background: Color::rgb(60, 60, 60),
            text_color: Color::WHITE,
            corner_radius: 4,
            padding: 12,
            is_hovered: false,
            is_pressed: false,
            label_size: Size::ZERO,
            needs_redraw: false,
        }
    }

    /// Set background color
    pub fn background(mut self, color: Color) -> Self {
        self.background = color;
        self
    }

    /// Set text color
    pub fn text_color(mut self, color: Color) -> Self {
        self.text_color = color;
        self
    }

    /// Set corner radius
    pub fn corner_radius(mut self, radius: u32) -> Self {
        self.corner_radius = radius;
        self
    }

    /// Set padding
    pub fn padding(mut self, padding: u32) -> Self {
        self.padding = padding;
        self
    }

    fn current_background(&self) -> Color {
        if self.is_pressed {
            Color::rgb(
                self.background.r.saturating_sub(30),
                self.background.g.saturating_sub(30),
                self.background.b.saturating_sub(30),
            )
        } else if self.is_hovered {
            Color::rgb(
                self.background.r.saturating_add(20),
                self.background.g.saturating_add(20),
                self.background.b.saturating_add(20),
            )
        } else {
            self.background
        }
    }
}

impl<F: FnMut() + 'static> View for Button<F> {
    fn layout(&mut self, _available: Size) -> Size {
        let inner_available = Size::new(u32::MAX, u32::MAX);
        self.label_size = match &mut self.label {
            ButtonLabel::Text(text) => {
                let (w, h) = measure_text_sized(text, 16.0);
                Size::new(w, h)
            }
            ButtonLabel::View(v) => v.layout(inner_available),
        };
        Size::new(
            self.label_size.width + self.padding * 2,
            self.label_size.height + self.padding * 2,
        )
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        // Draw background with rounded corners
        canvas.fill_rounded_rect(frame.x, frame.y, frame.width, frame.height, self.corner_radius, self.current_background());

        // Draw border with rounded corners
        let border_color = if self.is_hovered {
            Color::rgb(150, 150, 150)
        } else {
            Color::rgb(100, 100, 100)
        };
        canvas.draw_rounded_rect(frame.x, frame.y, frame.width, frame.height, self.corner_radius, border_color);

        // Draw label
        let label_x = frame.x + self.padding as i32;
        let label_y = frame.y + self.padding as i32;
        match &self.label {
            ButtonLabel::Text(text) => {
                canvas.draw_text(label_x, label_y, text, self.text_color);
            }
            ButtonLabel::View(v) => {
                let label_frame = Rect::new(label_x, label_y, self.label_size.width, self.label_size.height);
                v.draw(canvas, label_frame);
            }
        }
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        match event.kind {
            EventKind::MouseDown { button: MouseButton::Left } => {
                if frame.contains(event.x(), event.y()) {
                    self.is_pressed = true;
                    self.needs_redraw = true;
                    true
                } else {
                    // Click outside clears pressed state
                    if self.is_pressed {
                        self.is_pressed = false;
                        self.needs_redraw = true;
                    }
                    false
                }
            }
            EventKind::MouseUp { button: MouseButton::Left } => {
                if self.is_pressed {
                    self.is_pressed = false;
                    self.needs_redraw = true;
                    if frame.contains(event.x(), event.y()) {
                        (self.on_click)();
                    }
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn update_hover_state(&mut self, mouse_in_frame: bool) -> bool {
        let was_hovered = self.is_hovered;
        self.is_hovered = mouse_in_frame;
        if was_hovered != self.is_hovered {
            self.needs_redraw = true;
            true
        } else {
            false
        }
    }

    fn children(&self) -> Vec<(&dyn View, Rect)> {
        match &self.label {
            ButtonLabel::View(v) => {
                let child_frame = Rect::new(self.padding as i32, self.padding as i32, self.label_size.width, self.label_size.height);
                let mut out = Vec::new();
                out.push((v.as_ref() as &dyn View, child_frame));
                out
            }
            _ => Vec::new(),
        }
    }

    fn children_mut(&mut self) -> Vec<(&mut dyn View, Rect)> {
        match &mut self.label {
            ButtonLabel::View(v) => {
                let child_frame = Rect::new(self.padding as i32, self.padding as i32, self.label_size.width, self.label_size.height);
                let mut out = Vec::new();
                out.push((v.as_mut() as &mut dyn View, child_frame));
                out
            }
            _ => Vec::new(),
        }
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&dyn View, Rect) -> bool) {
        if let ButtonLabel::View(v) = &self.label {
            let child_frame = Rect::new(self.padding as i32, self.padding as i32, self.label_size.width, self.label_size.height);
            let _ = visitor(v.as_ref() as &dyn View, child_frame);
        }
    }

    fn visit_children_mut(&mut self, visitor: &mut dyn FnMut(&mut dyn View, Rect) -> bool) {
        if let ButtonLabel::View(v) = &mut self.label {
            let child_frame = Rect::new(self.padding as i32, self.padding as i32, self.label_size.width, self.label_size.height);
            let _ = visitor(v.as_mut() as &mut dyn View, child_frame);
        }
    }

    fn needs_draw(&self) -> bool {
        if self.needs_redraw {
            return true;
        }
        match &self.label {
            ButtonLabel::View(v) => v.needs_draw(),
            _ => false,
        }
    }

    fn set_needs_draw(&mut self) {
        self.needs_redraw = true;
    }

    fn clear_needs_draw(&mut self) {
        self.needs_redraw = false;
    }
}

impl<F: FnMut() + 'static> Hoverable for Button<F> {
    fn is_hovered(&self) -> bool {
        self.is_hovered
    }
}

/// Flexible spacer - takes up available space
pub struct Spacer {
    min_length: u32,
}

impl Spacer {
    pub fn new() -> Self {
        Self { min_length: 0 }
    }

    /// Set minimum length
    pub fn min_length(mut self, length: u32) -> Self {
        self.min_length = length;
        self
    }
}

impl Default for Spacer {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Spacer {
    fn flex_factor(&self) -> u32 {
        1
    }

    fn layout(&mut self, available: Size) -> Size {
        if available.width == 0 {
            Size::new(0, available.height.max(self.min_length))
        } else if available.height == 0 {
            Size::new(available.width.max(self.min_length), 0)
        } else {
            Size::new(
                available.width.max(self.min_length),
                available.height.max(self.min_length),
            )
        }
    }

    fn draw(&self, _canvas: &mut Canvas, _frame: Rect) {
        // Spacer is invisible
    }
}

/// Rectangle view with optional rounded corners
pub struct RectView {
    color: Color,
    width: Option<u32>,
    height: Option<u32>,
    corner_radius: u32,
    border_width: u32,
    border_color: Option<Color>,
}

impl RectView {
    pub fn new(color: Color) -> Self {
        Self {
            color,
            width: None,
            height: None,
            corner_radius: 0,
            border_width: 0,
            border_color: None,
        }
    }

    /// Set fixed width
    pub fn width(mut self, width: u32) -> Self {
        self.width = Some(width);
        self
    }

    /// Set fixed height
    pub fn height(mut self, height: u32) -> Self {
        self.height = Some(height);
        self
    }

    /// Set corner radius for rounded corners
    pub fn corner_radius(mut self, radius: u32) -> Self {
        self.corner_radius = radius;
        self
    }

    /// Set border
    pub fn border(mut self, width: u32, color: Color) -> Self {
        self.border_width = width;
        self.border_color = Some(color);
        self
    }
}

impl View for RectView {
    fn layout(&mut self, available: Size) -> Size {
        Size::new(
            self.width.unwrap_or(available.width),
            self.height.unwrap_or(available.height),
        )
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        // Draw with rounded corners if specified
        if self.corner_radius > 0 {
            canvas.fill_rounded_rect(frame.x, frame.y, frame.width, frame.height, self.corner_radius, self.color);
            if let Some(border_color) = self.border_color {
                for i in 0..self.border_width {
                    canvas.draw_rounded_rect(
                        frame.x + i as i32,
                        frame.y + i as i32,
                        frame.width - i * 2,
                        frame.height - i * 2,
                        self.corner_radius.saturating_sub(i),
                        border_color,
                    );
                }
            }
        } else {
            canvas.fill_rect(frame.x, frame.y, frame.width, frame.height, self.color);
            if let Some(border_color) = self.border_color {
                for i in 0..self.border_width {
                    canvas.draw_rect(
                        frame.x + i as i32,
                        frame.y + i as i32,
                        frame.width - i * 2,
                        frame.height - i * 2,
                        border_color,
                    );
                }
            }
        }
    }
}

// ============================================================================
// Bound Controls - Controls that work with Binding<T>
// The old non-bound versions are removed. Use State::new() + .binding() pattern.
// ============================================================================

/// TextField - text input control with two-way binding
///
/// # Example
///
/// ```no_run
/// use scarlet_ui::{State, TextField};
///
/// let text = State::new(String::from(""));
///
/// // State can be passed directly (no .binding() needed)
/// TextField::new("Enter text...", text)
/// ```
pub struct TextField {
    binding: Binding<String>,
    placeholder: String,
    is_focused: bool,
    cursor_pos: usize,
    text_color: Color,
    background: Color,
    border_color: Color,
    corner_radius: u32,
    padding: u32,
    refresh_handle: ViewRefreshHandle,
    cached_text: String,
}

impl TextField {
    pub fn new(placeholder: impl Into<String>, binding: impl Into<Binding<String>>) -> Self {
        let binding = binding.into();
        let cached_text = binding.get();
        let cursor_pos = cached_text.len();
        Self {
            binding,
            placeholder: placeholder.into(),
            is_focused: false,
            cursor_pos,
            text_color: Color::BLACK,
            background: Color::WHITE,
            border_color: Color::rgb(180, 180, 180),
            corner_radius: 4,
            padding: 8,
            refresh_handle: ViewRefreshHandle::new(),
            cached_text,
        }
    }

    /// Set text color
    pub fn text_color(mut self, color: Color) -> Self {
        self.text_color = color;
        self
    }

    /// Set background color
    pub fn background(mut self, color: Color) -> Self {
        self.background = color;
        self
    }

    /// Set border color
    pub fn border_color(mut self, color: Color) -> Self {
        self.border_color = color;
        self
    }

    /// Set corner radius
    pub fn corner_radius(mut self, radius: u32) -> Self {
        self.corner_radius = radius;
        self
    }

    fn sync_from_binding(&mut self) {
        let new_text = self.binding.get();
        if new_text != self.cached_text {
            self.cached_text = new_text;
            self.cursor_pos = self.cursor_pos.min(self.cached_text.len());
        }
    }
}

impl View for TextField {
    fn layout(&mut self, available: Size) -> Size {
        if self.refresh_handle.take_dirty() {
            self.sync_from_binding();
        }

        // If the parent provides a width constraint, respect it.
        // When `available.width == 0`, treat it as "unconstrained" and use a
        // reasonable intrinsic width.
        let width = if available.width == 0 { 150 } else { available.width };
        Size::new(width, 32)
    }

    fn flex_factor(&self) -> u32 {
        // Make TextField consume remaining space in HStack/VStack.
        1
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        // Background with rounded corners
        canvas.fill_rounded_rect(frame.x, frame.y, frame.width, frame.height, self.corner_radius, self.background);
        
        // Border (thicker if focused)
        let border_color = if self.is_focused {
            Color::rgb(100, 150, 255)
        } else {
            self.border_color
        };
        canvas.draw_rounded_rect(frame.x, frame.y, frame.width, frame.height, self.corner_radius, border_color);
        if self.is_focused {
            canvas.draw_rounded_rect(frame.x + 1, frame.y + 1, frame.width - 2, frame.height - 2, self.corner_radius.saturating_sub(1), border_color);
        }

        // Text or placeholder
        let display_text = if self.cached_text.is_empty() {
            &self.placeholder
        } else {
            &self.cached_text
        };
        
        let text_color = if self.cached_text.is_empty() {
            Color::rgb(150, 150, 150)
        } else {
            self.text_color
        };

        if !display_text.is_empty() {
            canvas.draw_text(
                frame.x + self.padding as i32,
                frame.y + self.padding as i32,
                display_text,
                text_color,
            );
        }
        
        // Draw cursor if focused
        if self.is_focused {
            let cursor_text = if self.cursor_pos <= self.cached_text.len() {
                &self.cached_text[..self.cursor_pos]
            } else {
                &self.cached_text
            };
            let (cursor_x, _) = measure_text_sized(cursor_text, 16.0);
            canvas.fill_rect(
                frame.x + self.padding as i32 + cursor_x as i32,
                frame.y + self.padding as i32,
                2,
                16,
                self.text_color,
            );
        }
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        match event.kind {
            EventKind::MouseDown { button: MouseButton::Left } => {
                let was_focused = self.is_focused;
                self.is_focused = frame.contains(event.x(), event.y());
                if was_focused != self.is_focused {
                    self.refresh_handle.mark_dirty();
                }
                was_focused != self.is_focused
            }
            EventKind::Blur => {
                let was_focused = self.is_focused;
                self.is_focused = false;
                if was_focused {
                    self.refresh_handle.mark_dirty();
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn needs_draw(&self) -> bool {
        self.refresh_handle.is_dirty()
    }

    fn set_needs_draw(&mut self) {
        self.refresh_handle.mark_dirty();
    }

    fn clear_needs_draw(&mut self) {
        self.refresh_handle.take_dirty();
    }
}

impl Focus for TextField {
    fn on_focus_gain(&mut self) -> bool {
        let was_focused = self.is_focused;
        self.is_focused = true;
        if !was_focused {
            self.refresh_handle.mark_dirty();
        }
        !was_focused
    }

    fn on_focus_loss(&mut self) -> bool {
        let was_focused = self.is_focused;
        self.is_focused = false;
        if was_focused {
            self.refresh_handle.mark_dirty();
        }
        was_focused
    }

    fn is_focused(&self) -> bool {
        self.is_focused
    }
}

/// CheckBox - boolean toggle control with two-way binding
///
/// # Example
///
/// ```no_run
/// use scarlet_ui::{State, CheckBox};
///
/// let checked = State::new(false);
///
/// // State can be passed directly (no .binding() needed)
/// CheckBox::new("Enable feature", checked)
/// ```
pub struct CheckBox {
    binding: Binding<bool>,
    label: String,
    check_color: Color,
    label_color: Color,
    corner_radius: u32,
    refresh_handle: ViewRefreshHandle,
}

impl CheckBox {
    pub fn new(label: impl Into<String>, binding: impl Into<Binding<bool>>) -> Self {
        Self {
            binding: binding.into(),
            label: label.into(),
            check_color: Color::rgb(50, 150, 255),
            label_color: Color::BLACK,
            corner_radius: 3,
            refresh_handle: ViewRefreshHandle::new(),
        }
    }

    /// Set check color
    pub fn check_color(mut self, color: Color) -> Self {
        self.check_color = color;
        self
    }

    /// Set label color
    pub fn label_color(mut self, color: Color) -> Self {
        self.label_color = color;
        self
    }

    /// Set corner radius for checkbox
    pub fn corner_radius(mut self, radius: u32) -> Self {
        self.corner_radius = radius;
        self
    }
}

impl View for CheckBox {
    fn layout(&mut self, _available: Size) -> Size {
        let (label_w, label_h) = measure_text_sized(&self.label, 16.0);
        Size::new(24 + 8 + label_w, 24.max(label_h))
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        let checked = self.binding.get();
        
        let box_size = 20;
        let box_y = frame.y + (frame.height as i32 - box_size as i32) / 2;
        
        // Background with rounded corners
        canvas.fill_rounded_rect(frame.x, box_y, box_size, box_size, self.corner_radius, Color::WHITE);
        // Border
        canvas.draw_rounded_rect(frame.x, box_y, box_size, box_size, self.corner_radius, Color::rgb(180, 180, 180));
        
        // Check mark if checked
        if checked {
            for i in 0..3 {
                for j in 0..5 {
                    canvas.put_pixel(
                        frame.x + 4 + j,
                        box_y + 10 + j - i,
                        self.check_color,
                    );
                }
                for j in 0..8 {
                    canvas.put_pixel(
                        frame.x + 8 + j,
                        box_y + 14 - j - i,
                        self.check_color,
                    );
                }
            }
        }
        
        // Draw label
        canvas.draw_text(
            frame.x + box_size as i32 + 8,
            frame.y + (frame.height as i32 - 16) / 2,
            &self.label,
            self.label_color,
        );
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        match event.kind {
            EventKind::MouseDown { button: MouseButton::Left } => {
                if frame.contains(event.x(), event.y()) {
                    let current = self.binding.get();
                    self.binding.set(!current);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn needs_draw(&self) -> bool {
        self.refresh_handle.is_dirty()
    }

    fn set_needs_draw(&mut self) {
        self.refresh_handle.mark_dirty();
    }

    fn clear_needs_draw(&mut self) {
        self.refresh_handle.take_dirty();
    }
}

/// Slider - value selection control with two-way binding
///
/// # Example
///
/// ```no_run
/// use scarlet_ui::{State, Slider};
///
/// let value = State::new(0.5f32);
///
/// // State can be passed directly (no .binding() needed)
/// Slider::new(0.0, 1.0, value)
/// ```
pub struct Slider {
    binding: Binding<f32>,
    min: f32,
    max: f32,
    track_color: Color,
    thumb_color: Color,
    is_dragging: bool,
    /// Pending value during drag - only committed on mouse up or periodically
    pending_value: Option<f32>,
    /// Last time we committed the value (for throttling during drag)
    last_commit_frame: u32,
    /// Frame counter for throttling
    frame_counter: u32,
    refresh_handle: ViewRefreshHandle,
}

impl Slider {
    pub fn new(min: f32, max: f32, binding: impl Into<Binding<f32>>) -> Self {
        Self {
            binding: binding.into(),
            min,
            max,
            track_color: Color::rgb(200, 200, 200),
            thumb_color: Color::rgb(50, 150, 255),
            is_dragging: false,
            pending_value: None,
            last_commit_frame: 0,
            frame_counter: 0,
            refresh_handle: ViewRefreshHandle::new(),
        }
    }

    /// Set track color
    pub fn track_color(mut self, color: Color) -> Self {
        self.track_color = color;
        self
    }

    /// Set thumb color
    pub fn thumb_color(mut self, color: Color) -> Self {
        self.thumb_color = color;
        self
    }

    fn update_value_from_mouse(&mut self, mouse_x: i32, frame: Rect) {
        let thumb_radius = 8;
        let track_start = frame.x + thumb_radius;
        let track_end = frame.x + frame.width as i32 - thumb_radius;
        let track_width = (track_end - track_start) as f32;
        
        if track_width > 0.0 {
            let position = (mouse_x - track_start).max(0).min(track_width as i32) as f32;
            let ratio = position / track_width;
            let new_value = self.min + ratio * (self.max - self.min);
            let clamped = new_value.clamp(self.min, self.max);
            
            // Check if value actually changed (with small threshold to avoid noise)
            let old_value = self.pending_value.unwrap_or_else(|| self.binding.get());
            let value_changed = (clamped - old_value).abs() > 0.001;
            
            if value_changed {
                // Store pending value instead of immediately committing
                self.pending_value = Some(clamped);
                
                // Throttle: only commit every 5 frames during drag to reduce flicker
                self.frame_counter = self.frame_counter.wrapping_add(1);
                if self.frame_counter.wrapping_sub(self.last_commit_frame) >= 5 {
                    self.commit_pending_value();
                }
                
                // Mark dirty only when value actually changed
                self.refresh_handle.mark_dirty();
            }
        }
    }
    
    fn commit_pending_value(&mut self) {
        if let Some(value) = self.pending_value.take() {
            self.binding.set(value);
            self.last_commit_frame = self.frame_counter;
        }
    }
}

impl View for Slider {
    fn layout(&mut self, available: Size) -> Size {
        Size::new(available.width.max(100), 32)
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        // Use pending value during drag for immediate visual feedback,
        // fall back to binding value otherwise
        let value = self.pending_value
            .unwrap_or_else(|| self.binding.get())
            .clamp(self.min, self.max);
        
        let track_height = 4;
        let thumb_radius = 8;
        let track_y = frame.y + (frame.height as i32 - track_height as i32) / 2;
        
        // Draw track with rounded ends
        canvas.fill_rounded_rect(
            frame.x + thumb_radius,
            track_y,
            frame.width - thumb_radius as u32 * 2,
            track_height,
            track_height / 2,
            self.track_color,
        );
        
        // Calculate thumb position
        let track_width = frame.width as f32 - thumb_radius as f32 * 2.0;
        let ratio = if self.max > self.min {
            (value - self.min) / (self.max - self.min)
        } else {
            0.0
        };
        let thumb_x = frame.x + thumb_radius + (track_width * ratio) as i32;
        let thumb_y = frame.y + frame.height as i32 / 2;
        
        // Draw thumb (circle)
        for dy in -(thumb_radius as i32)..=(thumb_radius as i32) {
            for dx in -(thumb_radius as i32)..=(thumb_radius as i32) {
                if dx * dx + dy * dy <= (thumb_radius * thumb_radius) as i32 {
                    canvas.put_pixel(thumb_x + dx, thumb_y + dy, self.thumb_color);
                }
            }
        }
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        match event.kind {
            EventKind::MouseDown { button: MouseButton::Left } => {
                if frame.contains(event.x(), event.y()) {
                    self.is_dragging = true;
                    self.update_value_from_mouse(event.x(), frame);
                    true
                } else {
                    false
                }
            }
            EventKind::MouseMove => {
                if self.is_dragging {
                    self.update_value_from_mouse(event.x(), frame);
                    true
                } else {
                    false
                }
            }
            EventKind::MouseUp { button: MouseButton::Left } => {
                if self.is_dragging {
                    self.is_dragging = false;
                    // Commit any pending value on mouse up
                    self.commit_pending_value();
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn needs_draw(&self) -> bool {
        self.refresh_handle.is_dirty()
    }

    fn set_needs_draw(&mut self) {
        self.refresh_handle.mark_dirty();
    }

    fn clear_needs_draw(&mut self) {
        self.refresh_handle.take_dirty();
    }
}

/// ProgressBar - progress indicator with reactive state
///
/// Features smooth animation interpolation to reduce flicker during
/// rapid value changes.
///
/// # Example
///
/// ```no_run
/// use scarlet_ui::{State, ProgressBar};
///
/// let progress = State::new(0.5f32);
///
/// ProgressBar::new(progress)
/// ```
pub struct ProgressBar {
    state: State<f32>,
    track_color: Color,
    fill_color: Color,
    corner_radius: u32,
    height: u32,
    refresh_handle: ViewRefreshHandle,
    /// Current displayed value (for smooth animation)
    display_value: f32,
    /// Whether animation is enabled
    animate: bool,
}

impl ProgressBar {
    pub fn new(state: State<f32>) -> Self {
        let refresh_handle = ViewRefreshHandle::new();
        state.subscribe_view(&refresh_handle);
        let initial = state.get().clamp(0.0, 1.0);
        Self {
            state,
            track_color: Color::rgb(230, 230, 230),
            fill_color: Color::rgb(50, 150, 255),
            corner_radius: 4,
            height: 16,
            refresh_handle,
            display_value: initial,
            // Disable animation by default to prevent flicker
            animate: false,
        }
    }
    
    /// Enable smooth animation (may cause flicker in some scenarios)
    pub fn animated(mut self) -> Self {
        self.animate = true;
        self
    }

    /// Set track color
    pub fn track_color(mut self, color: Color) -> Self {
        self.track_color = color;
        self
    }

    /// Set fill color
    pub fn fill_color(mut self, color: Color) -> Self {
        self.fill_color = color;
        self
    }

    /// Set corner radius
    pub fn corner_radius(mut self, radius: u32) -> Self {
        self.corner_radius = radius;
        self
    }

    /// Set height
    pub fn height(mut self, height: u32) -> Self {
        self.height = height;
        self
    }
    
    /// Disable animation (instant updates)
    /// Note: Animation is disabled by default, so this is typically not needed.
    #[deprecated(note = "Animation is now disabled by default. Use .animated() to enable.")]
    pub fn no_animation(mut self) -> Self {
        self.animate = false;
        self
    }
}

impl View for ProgressBar {
    fn layout(&mut self, available: Size) -> Size {
        // Update display value with interpolation
        let target = self.state.get().clamp(0.0, 1.0);
        
        if self.animate {
            // Lerp towards target (smooth animation)
            let diff = target - self.display_value;
            // Use a larger threshold to reduce flicker from tiny updates
            if diff.abs() < 0.01 {
                self.display_value = target;
            } else {
                // Move 50% of the way each frame (faster convergence)
                self.display_value += diff * 0.5;
                // Keep refreshing until we reach target
                self.refresh_handle.mark_dirty();
            }
        } else {
            self.display_value = target;
        }
        
        Size::new(available.width.max(100), self.height)
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        let progress = self.display_value;
        
        // Draw track with rounded corners
        canvas.fill_rounded_rect(frame.x, frame.y, frame.width, frame.height, self.corner_radius, self.track_color);
        
        // Draw filled portion with rounded corners
        let fill_width = (frame.width as f32 * progress) as u32;
        if fill_width > 0 {
            canvas.fill_rounded_rect(frame.x, frame.y, fill_width, frame.height, self.corner_radius, self.fill_color);
        }
        
        // Draw border
        canvas.draw_rounded_rect(frame.x, frame.y, frame.width, frame.height, self.corner_radius, Color::rgb(180, 180, 180));
    }

    fn needs_draw(&self) -> bool {
        self.refresh_handle.is_dirty()
    }

    fn set_needs_draw(&mut self) {
        self.refresh_handle.mark_dirty();
    }

    fn clear_needs_draw(&mut self) {
        self.refresh_handle.take_dirty();
    }
}

/// Toggle - switch control with two-way binding
///
/// # Example
///
/// ```no_run
/// use scarlet_ui::{State, Toggle};
///
/// let enabled = State::new(false);
///
/// // State can be passed directly (no .binding() needed)
/// Toggle::new(enabled)
/// ```
pub struct Toggle {
    binding: Binding<bool>,
    on_color: Color,
    off_color: Color,
    thumb_color: Color,
    is_hovered: bool,
    refresh_handle: ViewRefreshHandle,
}

impl Toggle {
    pub fn new(binding: impl Into<Binding<bool>>) -> Self {
        Self {
            binding: binding.into(),
            on_color: Color::rgb(50, 200, 100),
            off_color: Color::rgb(180, 180, 180),
            thumb_color: Color::WHITE,
            is_hovered: false,
            refresh_handle: ViewRefreshHandle::new(),
        }
    }

    /// Set on color
    pub fn on_color(mut self, color: Color) -> Self {
        self.on_color = color;
        self
    }

    /// Set off color
    pub fn off_color(mut self, color: Color) -> Self {
        self.off_color = color;
        self
    }

    /// Set thumb color
    pub fn thumb_color(mut self, color: Color) -> Self {
        self.thumb_color = color;
        self
    }
}

impl View for Toggle {
    fn layout(&mut self, _available: Size) -> Size {
        Size::new(50, 28)
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        let enabled = self.binding.get();
        
        let track_height = 24;
        let track_radius = track_height / 2;
        let thumb_radius = 10;
        
        let track_color = if enabled {
            self.on_color
        } else {
            self.off_color
        };
        
        // Draw track with rounded ends (pill shape)
        canvas.fill_rounded_rect(
            frame.x,
            frame.y + 2,
            frame.width,
            track_height,
            track_radius,
            track_color,
        );
        
        // Calculate thumb position
        let thumb_x = if enabled {
            frame.x + frame.width as i32 - track_radius as i32 - 2
        } else {
            frame.x + track_radius as i32 + 2
        };
        let thumb_y = frame.y + frame.height as i32 / 2;
        
        // Draw thumb (circle)
        for dy in -(thumb_radius as i32)..=(thumb_radius as i32) {
            for dx in -(thumb_radius as i32)..=(thumb_radius as i32) {
                if dx * dx + dy * dy <= (thumb_radius * thumb_radius) as i32 {
                    canvas.put_pixel(thumb_x + dx, thumb_y + dy, self.thumb_color);
                }
            }
        }
        
        // Draw border if hovered
        if self.is_hovered {
            canvas.draw_rounded_rect(frame.x, frame.y + 2, frame.width, track_height, track_radius, Color::rgb(100, 100, 100));
        }
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        match event.kind {
            EventKind::MouseMove => {
                let was_hovered = self.is_hovered;
                self.is_hovered = frame.contains(event.x(), event.y());
                was_hovered != self.is_hovered
            }
            EventKind::MouseDown { button: MouseButton::Left } => {
                if frame.contains(event.x(), event.y()) {
                    let current = self.binding.get();
                    self.binding.set(!current);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn needs_draw(&self) -> bool {
        self.refresh_handle.is_dirty()
    }

    fn set_needs_draw(&mut self) {
        self.refresh_handle.mark_dirty();
    }

    fn clear_needs_draw(&mut self) {
        self.refresh_handle.take_dirty();
    }
}
