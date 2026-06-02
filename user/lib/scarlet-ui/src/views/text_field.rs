//! Text field View - editable single-line text input.
//!
//! TextField owns keyboard editing behavior for focused text input controls.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::any::Any;

use crate::buffer::Buffer;
use crate::color::{Color, ColorPalette};
use crate::element::{
    Element, ElementRenderObject, LayoutConstraints, RenderElement, TextInputElementState,
};
use crate::event::{FocusEvent, KeyCode, KeyEvent};
use crate::geometry::{Point, Rect, Size};
use crate::graphics;
use crate::state::{Listenable, State};
use crate::view::View;

/// Single-line editable text input.
#[derive(Clone)]
pub struct TextField {
    text: State<String>,
    placeholder: String,
    on_submit: Option<Arc<dyn Fn() + 'static>>,
    blur_on_submit: bool,
    background_color: Color,
    border_color: Color,
    focused_border_color: Color,
    text_color: Color,
    placeholder_color: Color,
    font_size: f32,
    padding: f32,
}

impl TextField {
    /// Create a new text field bound to the supplied text state.
    pub fn new(text: State<String>) -> Self {
        let palette = ColorPalette::default();
        Self {
            text,
            placeholder: String::new(),
            on_submit: None,
            blur_on_submit: false,
            background_color: Color::rgb(248u8, 249u8, 251u8),
            border_color: Color::rgb(190u8, 196u8, 205u8),
            focused_border_color: Color::rgb(35u8, 95u8, 160u8),
            text_color: palette.text_primary(),
            placeholder_color: Color::gray(0.55),
            font_size: 14.0,
            padding: 8.0,
        }
    }

    /// Set placeholder text shown while the value is empty.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Set whether Enter should remove focus after submitting.
    pub fn blur_on_submit(mut self, blur: bool) -> Self {
        self.blur_on_submit = blur;
        self
    }

    /// Set the callback invoked when Enter is pressed while focused.
    pub fn on_submit(mut self, callback: impl Fn() + 'static) -> Self {
        self.on_submit = Some(Arc::new(callback));
        self
    }

    /// Set the font size.
    pub fn font_size(mut self, size: f32) -> Self {
        self.font_size = size;
        self
    }

    /// Set the padding.
    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = padding;
        self
    }

    /// Return the bound text state.
    pub fn text_state(&self) -> &State<String> {
        &self.text
    }

    /// Invoke the submit callback if present.
    pub fn invoke_submit(&self) {
        if let Some(callback) = self.on_submit.as_ref() {
            callback();
        }
    }

    pub(crate) fn text_input_state(&self, preedit: &str) -> TextInputElementState {
        let text = self.text.get();
        let mut display = text.clone();
        display.push_str(preedit);
        let (text_width, _) = graphics::measure_text_sized(&display, self.font_size);
        TextInputElementState {
            cursor_rect: Rect::from_xywh(
                self.padding + text_width as f32,
                self.padding * 0.5,
                1.0,
                self.font_size * 1.25,
            ),
            surrounding_text: text,
        }
    }
}

impl View for TextField {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            TextFieldRenderObject::from_view(self),
        ))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn Listenable> {
        alloc::vec![&self.text]
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// TextField RenderObject.
pub struct TextFieldRenderObject {
    text: String,
    preedit: String,
    focused: bool,
    placeholder: String,
    background_color: Color,
    border_color: Color,
    focused_border_color: Color,
    text_color: Color,
    placeholder_color: Color,
    font_size: f32,
    padding: f32,
    size: Size,
    buffer: Option<Buffer>,
}

impl TextFieldRenderObject {
    /// Create a render object from a TextField view.
    pub fn from_view(view: &TextField) -> Self {
        Self {
            text: view.text.get(),
            preedit: String::new(),
            focused: false,
            placeholder: view.placeholder.clone(),
            background_color: view.background_color,
            border_color: view.border_color,
            focused_border_color: view.focused_border_color,
            text_color: view.text_color,
            placeholder_color: view.placeholder_color,
            font_size: view.font_size,
            padding: view.padding,
            size: Size::ZERO,
            buffer: None,
        }
    }
}

impl ElementRenderObject for TextFieldRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let text = if self.text.is_empty() {
            self.placeholder.as_str()
        } else {
            self.text.as_str()
        };
        let (measured_width, measured_height) = graphics::measure_text_sized(text, self.font_size);
        let intrinsic = Size {
            width: measured_width as f32 + self.padding * 2.0,
            height: measured_height as f32 + self.padding * 2.0,
        };
        self.size = constraints.constrain(intrinsic);
        let width = libm::ceilf(self.size.width.max(1.0)) as u32;
        let height = libm::ceilf(self.size.height.max(1.0)) as u32;
        let needs_resize = self
            .buffer
            .as_ref()
            .map_or(true, |b| b.logical_width() != width || b.logical_height() != height);
        if needs_resize {
            self.buffer = Some(Buffer::from_logical_dimensions(width, height));
        }
        self.size
    }

    fn size(&self) -> Size {
        self.size
    }

    fn hit_test(&self, point: Point) -> bool {
        point.x >= 0.0 && point.y >= 0.0 && point.x < self.size.width && point.y < self.size.height
    }

    fn render(&mut self) {
        if let Some(buffer) = self.buffer.as_mut() {
            let mut canvas = graphics::Canvas::for_buffer(buffer);
            let width = canvas.width();
            let height = canvas.height();
            let border = if self.focused {
                self.focused_border_color
            } else {
                self.border_color
            };
            canvas.fill_rect(0, 0, width, height, self.background_color);
            canvas.draw_rect(0, 0, width, height, border);

            let display = if self.text.is_empty() && self.preedit.is_empty() {
                self.placeholder.clone()
            } else if self.focused {
                let mut display = self.text.clone();
                display.push_str(&self.preedit);
                display.push('|');
                display
            } else {
                self.text.clone()
            };
            let color = if self.text.is_empty() && self.preedit.is_empty() {
                self.placeholder_color
            } else {
                self.text_color
            };
            let x = self.padding as i32;
            let y = ((height as f32 - self.font_size * 1.2) / 2.0).max(0.0) as i32;
            canvas.draw_text_sized(x, y, &display, color, self.font_size);
            if self.focused && !self.preedit.is_empty() {
                let (committed_width, _) = graphics::measure_text_sized(&self.text, self.font_size);
                let (preedit_width, _) = graphics::measure_text_sized(&self.preedit, self.font_size);
                let underline_y = (y as f32 + self.font_size * 1.15).max(0.0) as i32;
                let underline_x = x + committed_width as i32;
                canvas.draw_line(
                    underline_x,
                    underline_y,
                    underline_x + preedit_width as i32,
                    underline_y,
                    self.focused_border_color,
                );
            }
        }
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn clear_buffer(&mut self) {
        self.buffer = None;
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn update(&mut self, new_view: &dyn View) -> crate::element::UpdateResult {
        let Some(view) = new_view.as_any().downcast_ref::<TextField>() else {
            return crate::element::UpdateResult::Replaced;
        };
        let focused = self.focused;
        let preedit = self.preedit.clone();
        *self = TextFieldRenderObject::from_view(view);
        self.focused = focused;
        self.preedit = preedit;
        crate::element::UpdateResult::Updated
    }
}

impl TextFieldRenderObject {
    pub(crate) fn is_focused(&self) -> bool {
        self.focused
    }

    pub(crate) fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            self.preedit.clear();
        }
    }

    pub(crate) fn preedit(&self) -> &str {
        &self.preedit
    }

    pub(crate) fn set_preedit(&mut self, preedit: &str) {
        self.preedit.clear();
        self.preedit.push_str(preedit);
    }
}

pub(crate) fn handle_text_field_keyboard(
    field: &TextField,
    render_object: &mut TextFieldRenderObject,
    event: KeyEvent,
) -> bool {
    if !render_object.is_focused() {
        return false;
    }
    match event {
        KeyEvent::Char { c } if !c.is_control() => {
            render_object.set_preedit("");
            let mut text = field.text.get();
            text.push(c);
            field.text.set(text);
            true
        }
        KeyEvent::Pressed {
            keycode: KeyCode::Backspace,
        } => {
            let mut text = field.text.get();
            text.pop();
            field.text.set(text);
            true
        }
        KeyEvent::Pressed {
            keycode: KeyCode::Enter,
        } => {
            field.invoke_submit();
            if field.blur_on_submit {
                render_object.set_focused(false);
            }
            true
        }
        KeyEvent::Pressed {
            keycode: KeyCode::Escape,
        }
        | KeyEvent::Pressed {
            keycode: KeyCode::Tab,
        } => {
            render_object.set_focused(false);
            true
        }
        _ => false,
    }
}

pub(crate) fn handle_text_field_focus(
    render_object: &mut TextFieldRenderObject,
    event: FocusEvent,
) -> bool {
    match event {
        FocusEvent::Gained => render_object.set_focused(true),
        FocusEvent::Lost => render_object.set_focused(false),
    }
    true
}

pub(crate) fn handle_text_field_text_input(
    field: &TextField,
    render_object: &mut TextFieldRenderObject,
    event: &crate::event::Event,
) -> bool {
    if !render_object.is_focused() {
        return false;
    }

    match event {
        crate::event::Event::TextInputCommit { text, .. } => {
            render_object.set_preedit("");
            let mut current = field.text.get();
            current.push_str(text);
            field.text.set(current);
            true
        }
        crate::event::Event::TextInputPreedit { text, .. } => {
            render_object.set_preedit(text);
            true
        }
        crate::event::Event::TextInputDeleteSurroundingText {
            before_bytes,
            after_bytes,
            ..
        } => {
            render_object.set_preedit("");
            delete_surrounding_text_at_end(field, *before_bytes, *after_bytes);
            true
        }
        crate::event::Event::TextInputDone { .. } => true,
        _ => false,
    }
}

fn delete_surrounding_text_at_end(field: &TextField, before_bytes: u32, after_bytes: u32) {
    if after_bytes != 0 {
        return;
    }

    let mut text = field.text.get();
    let delete_bytes = before_bytes as usize;
    if delete_bytes == 0 {
        return;
    }

    let mut remove_from = text.len().saturating_sub(delete_bytes);
    while remove_from > 0 && !text.is_char_boundary(remove_from) {
        remove_from -= 1;
    }
    text.truncate(remove_from);
    field.text.set(text);
}
