//! Display-only ScarletUI tree shared by the anonymous painter and trusted
//! seat's hit testing. Never put credentials or physical key events in this tree.
use scarlet_ui::prelude::*;
use scarlet_ui::{
    ElementTree, RenderingPipeline, ViewKey, generate_state_id, views::containers::ViewTuple,
};
use std::{cell::RefCell, io, rc::Rc};

use crate::keyboard_layout::{Action as KeyAction, EDGE_UNITS, GAP_UNITS, GRID_UNITS, Layer};

pub const REQUEST_BYTES: usize = 48;
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Action {
    User,
    Password,
    Submit,
    Keyboard,
    Key(KeyAction),
}
type Actions = Rc<RefCell<Vec<Action>>>;

fn target(view: impl View + Clone + 'static, actions: &Actions, action: Action) -> Box<dyn View> {
    let mut actions = actions.borrow_mut();
    let id = actions.len();
    actions.push(action);
    Box::new(ViewExt::key(view, id))
}

pub struct Targets {
    tree: ElementTree,
    actions: Actions,
}
impl Targets {
    pub fn at(&self, x: i32, y: i32) -> Option<Action> {
        let hit = self.tree.hit_test(Point::new(x as f32, y as f32))?;
        let path = self.tree.find_path_ids(hit.id())?;
        path.into_iter()
            .rev()
            .find_map(|id| match self.tree.find_element(id)?.view_key()? {
                ViewKey::Value(i) => self.actions.borrow().get(*i as usize).copied(),
                _ => None,
            })
    }
}
impl Drop for Targets {
    fn drop(&mut self) {
        self.tree.clear_root();
    }
}
#[derive(Clone)]
pub struct Screen {
    pub width: u32,
    pub height: u32,
    pub state: [u8; 36],
    pub keyboard: bool,
    pub shift: bool,
    pub layer: Layer,
}
impl Screen {
    pub fn encode(&self) -> [u8; REQUEST_BYTES] {
        let mut bytes = [0; REQUEST_BYTES];
        bytes[..36].copy_from_slice(&self.state);
        bytes[36..40].copy_from_slice(&self.width.to_le_bytes());
        bytes[40..44].copy_from_slice(&self.height.to_le_bytes());
        bytes[44] = u8::from(self.keyboard);
        bytes[45] = u8::from(self.shift);
        bytes[46] = match self.layer {
            Layer::Symbols => 1,
            Layer::MoreSymbols => 2,
            _ => 0,
        };
        bytes
    }
    pub fn decode(bytes: &[u8; REQUEST_BYTES]) -> io::Result<Self> {
        let invalid =
            || io::Error::new(io::ErrorKind::InvalidData, "invalid greeter display state");
        let screen = Self {
            width: u32::from_le_bytes(bytes[36..40].try_into().unwrap()),
            height: u32::from_le_bytes(bytes[40..44].try_into().unwrap()),
            state: bytes[..36].try_into().unwrap(),
            keyboard: bytes[44] != 0,
            shift: bytes[45] != 0,
            layer: match bytes[46] {
                0 => Layer::Letters,
                1 => Layer::Symbols,
                2 => Layer::MoreSymbols,
                _ => return Err(invalid()),
            },
        };
        let [field, failed, count, len] = screen.state[..4].try_into().unwrap();
        if !(320..=1440).contains(&screen.width)
            || !(360..=1440).contains(&screen.height)
            || field > 2
            || failed > 2
            || count > 72
            || len > 32
            || bytes[44] > 1
            || bytes[45] > 1
            || bytes[47] != 0
        {
            return Err(invalid());
        }
        let name = &screen.state[4..4 + usize::from(len)];
        if !name
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
            || screen.state[4 + name.len()..].iter().any(|b| *b != 0)
            || (field == 2 && name.is_empty())
        {
            return Err(invalid());
        }
        Ok(screen)
    }
    pub fn pixel_bytes(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }
    pub fn keyboard_height(&self) -> f32 {
        if self.keyboard && self.state[1] != 2 {
            (self.height as f32 * 0.42)
                .min(self.width as f32 * 0.45)
                .min(360.0)
                .min(self.height as f32 - 226.0)
        } else {
            0.0
        }
    }
    pub fn targets(&self) -> Targets {
        let actions = Actions::default();
        let mut tree = ElementTree::new();
        tree.set_root(self.window(actions.clone()).create_element());
        tree.layout(LayoutConstraints::tight(
            self.width as f32,
            self.height as f32,
        ));
        Targets { tree, actions }
    }
    pub fn pipeline(&self) -> RenderingPipeline {
        let mut pipeline = RenderingPipeline::new();
        pipeline.set_root(self.window(Actions::default()).create_element());
        pipeline.layout_initial();
        pipeline
    }
    fn window(&self, actions: Actions) -> impl View + Clone {
        let (w, h) = (self.width as f32, self.height as f32);
        let panel_h = self.keyboard_height();
        let form_w = (w - 32.0).min(440.0);
        let small = h - panel_h < 300.0;
        let field_h = if small { 34.0 } else { 44.0 };
        let field = self.state[0];
        let failed = self.state[1];
        let name =
            String::from_utf8(self.state[4..4 + usize::from(self.state[3])].to_vec()).unwrap();
        let accent = Color::rgb(161u8, 43u8, 67u8);
        let mut form: Vec<Box<dyn View>> = vec![Box::new(
            Text::new(if field == 2 {
                "Unlock session"
            } else {
                "Sign in"
            })
            .font_size(if small { 22.0 } else { 28.0 }),
        )];
        if failed == 2 {
            form.push(Box::new(
                Text::new("No local account is available.").font_size(16.0),
            ));
            form.push(Box::new(
                Text::new("Configure an image user to sign in.").font_size(14.0),
            ));
            form.push(button(
                "Check again",
                Action::Submit,
                form_w,
                44.0,
                actions.clone(),
            ));
        } else {
            for (label, text, selected, action) in [
                ("User", name, field == 0, Action::User),
                (
                    "Password",
                    "•".repeat(usize::from(self.state[2])),
                    field != 0,
                    Action::Password,
                ),
            ] {
                form.push(Box::new(Text::new(label).font_size(14.0)));

                form.push(target(
                    TextField::new(State::new(generate_state_id(), text))
                        .automatic_software_keyboard(false)
                        .font_size(18.0)
                        .border_color(if selected {
                            accent
                        } else {
                            Color::rgb(100u8, 104u8, 116u8)
                        })
                        .frame(form_w, field_h),
                    &actions,
                    action,
                ));
            }
            form.push(Box::new(
                Text::new(if failed == 1 {
                    "Authentication failed. Try again."
                } else {
                    " "
                })
                .font_size(14.0)
                .color(accent),
            ));
            form.push(Box::new(
                HStack::new(Views(vec![
                    button(
                        if field == 0 {
                            "Continue"
                        } else if field == 2 {
                            "Unlock"
                        } else {
                            "Sign in"
                        },
                        Action::Submit,
                        form_w * 0.56,
                        field_h,
                        actions.clone(),
                    ),
                    button(
                        if self.keyboard {
                            "Hide keyboard"
                        } else {
                            "Keyboard"
                        },
                        Action::Keyboard,
                        form_w * 0.44 - 8.0,
                        field_h,
                        actions.clone(),
                    ),
                ]))
                .spacing(8.0),
            ));
        }
        let mut content: Vec<Box<dyn View>> = vec![Box::new(
            VStack::new(Views(form))
                .spacing(if small { 3.0 } else { 6.0 })
                .alignment(Alignment::Leading)
                .padding_insets(EdgeInsets::new(
                    (w - form_w) / 2.0,
                    ((h - panel_h - if small { 216.0 } else { 277.0 }) * 0.5).max(4.0),
                    0.0,
                    0.0,
                ))
                .frame(w, h - panel_h),
        )];
        if panel_h > 0.0 {
            content.push(self.keyboard_view(w, panel_h, actions));
        }
        Window::new("Sign in", VStack::new(Views(content)).spacing(0.0))
            .decorated(false)
            .size(Size::new(w, h))
    }
    fn keyboard_view(&self, w: f32, h: f32, actions: Actions) -> Box<dyn View> {
        let width = w.min(1100.0);
        let unit = width / GRID_UNITS;
        let gap = unit * GAP_UNITS;
        let pad = unit * EDGE_UNITS;
        let mut layout = crate::keyboard_layout::rows(false, self.layer);
        // Share US keys and symbol pages, without desktop/IME/clipboard tools.
        for row in &mut layout {
            row.keys.retain(|key| {
                !matches!(
                    key.action,
                    KeyAction::Key(85, _) | KeyAction::SetLayer(Layer::Tools | Layer::Function)
                )
            });
        }
        // The desktop bottom row has two layer switches around the IME/tools
        // buttons. Give the private panel one switch, a spacebar and Hide.
        let bottom = layout.last_mut().unwrap();
        let hide = bottom.keys.pop().unwrap();
        bottom.keys.truncate(2);
        bottom.keys.push(hide);
        for (key, weight) in bottom.keys.iter_mut().zip([
            2.35,
            GRID_UNITS - 4.7 - 2.0 * (EDGE_UNITS + GAP_UNITS),
            2.35,
        ]) {
            key.weight = weight;
        }
        let row_h = (h - pad * 2.0 - gap * 3.0) / 4.0;
        let mut rows = Vec::new();
        for row in layout {
            let inset = row.inset * unit;
            let keys = row
                .keys
                .into_iter()
                .map(|key| {
                    let label = match (key.action, self.shift) {
                        (KeyAction::Key(14, _), _) => "Del",
                        (KeyAction::Modifier(_), shifted) if width < 420.0 => {
                            if shifted {
                                "AA"
                            } else {
                                "Aa"
                            }
                        }
                        (KeyAction::Modifier(_), true) => "SHIFT",
                        (KeyAction::Key(28, _), _) if width < 420.0 => "Enter",
                        (_, true) => key.shifted,
                        _ => key.plain,
                    };
                    button(
                        label,
                        Action::Key(key.action),
                        key.weight * unit,
                        row_h,
                        actions.clone(),
                    )
                })
                .collect();
            rows.push(Box::new(
                HStack::new(Views(keys))
                    .spacing(gap)
                    .alignment(Alignment::TopLeading)
                    .padding_insets(EdgeInsets::new(inset, 0.0, 0.0, 0.0)),
            ) as Box<dyn View>);
        }
        Box::new(
            VStack::new(Views(rows))
                .spacing(gap)
                .alignment(Alignment::TopLeading)
                .padding(pad)
                .frame(w, h)
                .background(Color::rgb(36u8, 38u8, 46u8)),
        )
    }
}
fn button(label: &str, action: Action, w: f32, h: f32, actions: Actions) -> Box<dyn View> {
    target(
        (if action == Action::Submit {
            Button::new(label)
                .background_color(Color::rgb(148u8, 43u8, 64u8))
                .text_color(Color::WHITE)
        } else {
            Button::new(label)
        })
        .font_size(if label.chars().count() > 2 {
            14.0
        } else {
            20.0
        })
        .padding(2.0)
        .frame(w, h),
        &actions,
        action,
    )
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
    fn collect_listenables<'a>(&'a self, out: &mut Vec<&'a dyn Listenable>) {
        for view in &self.0 {
            out.extend(view.listenables());
        }
    }
}
