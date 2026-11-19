//! The system software keyboard's ScarletUI view. Only the input destination
//! and available actions depend on the owner of the seat.
use crate::keyboard_layout::{
    Action, EDGE_UNITS, GAP_UNITS, GRID_UNITS, Layer, RowGeometry, Tone, VerticalGeometry, rows,
};
use scarlet_ui::{element::ComponentElement, prelude::*, views::containers::ViewTuple};
use std::{rc::Rc, time::Duration};
use sws_protocol::input_panel;
pub enum KeyTarget {
    Desktop,
    Seat(usize),
    Disabled,
}
#[derive(Clone)]
struct Views(Vec<Box<dyn View>>);
impl ViewTuple for Views {
    fn create_elements(&self) -> Vec<Box<dyn Element>> {
        self.0.iter().map(|v| v.create_element()).collect()
    }
    fn clone_views(&self) -> Vec<Box<dyn View>> {
        self.0.clone()
    }
    fn collect_listenables<'a>(&'a self, out: &mut Vec<&'a dyn scarlet_ui::state::Listenable>) {
        for view in &self.0 {
            out.extend(view.listenables());
        }
    }
}
#[derive(Clone)]
struct KeyboardKeyView {
    label: &'static str,
    action: Action,
    enabled: bool,
    background: Color,
    text_color: Color,
    width: f32,
    height: f32,
    font_size: f32,
    dispatch: Rc<dyn Fn(Action)>,
}
impl KeyboardKeyView {
    fn body(&self) -> Box<dyn View> {
        let dispatch = self.dispatch.clone();
        let action = self.action;
        let icon = match action {
            Action::Hide => Some(Icon::KeyboardHide),
            Action::SetLayer(Layer::Tools) => Some(Icon::Dots),
            _ => None,
        };
        let button = if let Some(icon) = icon {
            Button::icon_only(icon)
                .icon_size(IconSize::Medium)
                .icon_color(self.text_color)
        } else {
            Button::new(self.label)
        };
        let button = button
            .header_style()
            .font_size(self.font_size)
            .padding(2.0)
            .background_color(self.background)
            .text_color(self.text_color)
            .enabled(self.enabled)
            .on_click(move || dispatch(action))
            // All touch keys activate on DOWN. Utility actions must not
            // depend on an UP routed through a tree replaced after painting.
            // The pure keyboard tracker owns actual repeat timing; this
            // Button option supplies press activation and suppresses the
            // release tap without ever firing a UI-owned repeat.
            .repeat_while_pressed(Duration::MAX, Duration::MAX);
        Box::new(button.frame(self.width, self.height))
    }
}
fn keyboard_key_body(view: &KeyboardKeyView) -> Box<dyn View> {
    view.body()
}
impl View for KeyboardKeyView {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(ComponentElement::new_with_builder(
            self.clone(),
            keyboard_key_body,
        ))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
pub fn keyboard(
    size: Size,
    modifiers: u32,
    layer: Layer,
    numeric: bool,
    dispatch: Rc<dyn Fn(Action)>,
    identify: impl Fn(Action) -> KeyTarget,
) -> Box<dyn View> {
    let layout = rows(numeric, layer);
    // One shared unit keeps Q/A/Z widths consistent across staggered rows.
    let tablet_layout = layout.iter().any(|row| row.geometry == RowGeometry::Tablet);
    let tablet_unit = size.width / GRID_UNITS;
    let horizontal_padding = if tablet_layout {
        tablet_unit * EDGE_UNITS
    } else {
        10.0
    };
    let tablet_gap = tablet_unit * GAP_UNITS;
    let fill_gap = if tablet_layout { tablet_gap } else { 7.0 };
    let vertical_gap = if tablet_layout { tablet_gap } else { 7.0 };
    let vertical_padding = if tablet_layout {
        tablet_unit * EDGE_UNITS
    } else {
        10.0
    };
    let content_width = (size.width - horizontal_padding * 2.0).max(1.0);
    let vertical = VerticalGeometry::new(size.height, vertical_padding, vertical_gap, &layout);
    let mut views: Vec<Box<dyn View>> = Vec::new();
    for row in layout {
        let row_height = vertical.row_unit * row.height;
        let weight: f32 = row.keys.iter().map(|key| key.weight).sum();
        let gap = if row.geometry == RowGeometry::Tablet {
            tablet_gap
        } else {
            fill_gap
        };
        let unit = if row.geometry == RowGeometry::Tablet {
            tablet_unit
        } else {
            ((content_width - gap * (row.keys.len() - 1) as f32) / weight).max(1.0)
        };
        let inset = row.inset * unit;
        let keys = row
            .keys
            .into_iter()
            .map(|key| {
                let label = if modifiers & input_panel::SHIFT != 0 {
                    key.shifted
                } else {
                    key.plain
                };
                let active = matches!(key.action,Action::Modifier(flag) if modifiers & flag!=0)
                    || (key.action == Action::ToggleFunction && layer == Layer::Function);
                let (background, text_color) = if active {
                    (Color::rgb(224, 225, 229), Color::rgb(28, 29, 33))
                } else {
                    (
                        match key.tone {
                            Tone::Character => Color::rgba_f32(0.46, 0.46, 0.49, 0.96),
                            Tone::Utility => Color::rgba_f32(0.29, 0.29, 0.33, 0.96),
                            Tone::Toolbar => Color::rgba_f32(1.0, 1.0, 1.0, 0.045),
                        },
                        Color::WHITE,
                    )
                };
                let width = unit * key.weight;
                let font_size = if row.height < 0.8 {
                    (row_height * 0.32).clamp(12.0, 16.0)
                } else if label.chars().count() > 2 || key.action == Action::ToggleFunction {
                    (row_height * 0.25).clamp(14.0, 19.0)
                } else {
                    (row_height * 0.41).clamp(20.0, 30.0)
                };
                let id = identify(key.action);
                let view = KeyboardKeyView {
                    label,
                    action: key.action,
                    enabled: !matches!(id, KeyTarget::Disabled),
                    background,
                    text_color,
                    width,
                    height: row_height,
                    font_size,
                    dispatch: dispatch.clone(),
                };
                match id {
                    KeyTarget::Seat(id) => Box::new(ViewExt::key(view, id)) as Box<dyn View>,
                    _ => Box::new(view),
                }
            })
            .collect();
        views.push(Box::new(
            HStack::new(Views(keys))
                .spacing(gap)
                .alignment(Alignment::TopLeading)
                .padding_insets(EdgeInsets::new(inset, 0.0, 0.0, 0.0)),
        ));
    }
    Box::new(
        VStack::new(Views(views))
            .spacing(vertical_gap)
            .alignment(Alignment::TopLeading)
            .padding_insets(EdgeInsets::new(
                horizontal_padding,
                vertical_padding,
                horizontal_padding,
                vertical.bottom_padding,
            ))
            .background(Color::rgba_f32(0.10, 0.10, 0.13, 0.96))
            .frame(size.width, size.height),
    )
}
