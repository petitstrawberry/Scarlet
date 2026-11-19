//! Display-only ScarletUI tree shared by the anonymous painter and trusted
//! seat's hit testing. Never put credentials or physical key events in this tree.
use scarlet_ui::prelude::*;
use scarlet_ui::{
    ElementTree, RenderingPipeline, ViewKey, generate_state_id, views::containers::ViewTuple,
};
use std::{cell::RefCell, io, rc::Rc};

use crate::keyboard_layout::{Action as KeyAction, Layer};

#[path = "soft_keyboard/view.rs"]
mod keyboard_view;

#[path = "scarlet_shell/appearance.rs"]
mod appearance;
#[path = "scarlet_shell/wallpaper.rs"]
mod wallpaper;
use appearance::{ACCENT, ACTION_HEIGHT, CARD, FROST, LINE, MUTED, RADIUS, TEXT};
use scarlet_ui::BitmapImage;

thread_local! {
    // Only the anonymous painter loads the system theme. The trusted tree
    // uses an empty background with identical bounds for hit testing.
    static WALLPAPER: RefCell<Option<BitmapImage>> = const { RefCell::new(None) };
}

pub const REQUEST_BYTES: usize = 48;
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Action {
    User,
    Password,
    Submit,
    Keyboard,
    Accounts,
    Previous,
    Next,
    Manual,
    Choose,
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
            || field > 4
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
            || (field >= 2 && name.is_empty())
            || (field >= 3 && (count != 0 || failed != 0))
        {
            return Err(invalid());
        }
        Ok(screen)
    }
    pub fn pixel_bytes(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }
    pub fn keyboard_height(&self) -> f32 {
        if self.keyboard && self.state[0] < 3 && self.state[1] != 2 {
            crate::keyboard_layout::panel_height(self.width as f32, self.height as f32)
                .min(self.height as f32 - 164.0)
        } else {
            0.0
        }
    }
    pub fn targets(&self) -> Targets {
        let actions = Actions::default();
        let mut tree = ElementTree::new();
        tree.set_root(self.window(actions.clone(), None).create_element());
        tree.layout(LayoutConstraints::tight(
            self.width as f32,
            self.height as f32,
        ));
        Targets { tree, actions }
    }
    pub fn pipeline(&self) -> RenderingPipeline {
        let mut pipeline = RenderingPipeline::new();
        let bitmap = WALLPAPER.with_borrow_mut(|cached| {
            if cached
                .as_ref()
                .is_none_or(|i| (i.width(), i.height()) != (self.width, self.height))
            {
                *cached =
                    Some(wallpaper::load_background_state().login_bitmap(self.width, self.height));
            }
            cached.as_ref().unwrap().clone()
        });
        pipeline.set_root(
            self.window(Actions::default(), Some(bitmap))
                .create_element(),
        );
        pipeline.layout_initial();
        pipeline
    }
    fn window(&self, actions: Actions, wallpaper: Option<BitmapImage>) -> impl View + Clone {
        let (w, h) = (self.width as f32, self.height as f32);
        let panel_h = self.keyboard_height();
        let free = h - panel_h;
        let compact = free < 250.0;
        let inset = if compact { 10.0 } else { 22.0 };
        let width = (w - 32.0).min(360.0) - inset * 2.0;
        let gap = if compact { 4.0 } else { 10.0 };
        let row_h = if compact { 36.0 } else { ACTION_HEIGHT };
        let field = self.state[0];
        let failed = self.state[1];
        let name = std::str::from_utf8(&self.state[4..4 + usize::from(self.state[3])]).unwrap();
        let mut form: Vec<Box<dyn View>> = Vec::new();
        let title = if failed == 2 {
            "No accounts available"
        } else {
            match field {
                3 | 4 => "Choose a user",
                2 => "Unlock session",
                1 => name,
                _ => "Other user",
            }
        };
        form.push(Box::new(
            HStack::new((
                IconView::new(if field >= 3 { Icon::Users } else { Icon::User })
                    .size(IconSize::Pixels(if compact { 20 } else { 24 }))
                    .color(TEXT),
                Text::new(title)
                    .line_limit(Some(1))
                    .color(TEXT)
                    .font_size(if compact { 19.0 } else { 24.0 }),
            ))
            .spacing(10.0),
        ));
        let control =
            |label: &str, action: Action, size: f32| button(label, action, size, row_h, &actions);
        if failed == 2 {
            form.push(control("Check again", Action::Submit, width));
        } else if field >= 3 {
            let mut accounts = Vec::new();
            if field == 4 {
                accounts.push(control("", Action::Previous, ACTION_HEIGHT));
            }
            let name_width = if field == 4 {
                width - 2.0 * (ACTION_HEIGHT + gap)
            } else {
                width
            };
            accounts.push(control(name, Action::Choose, name_width));
            if field == 4 {
                accounts.push(control("", Action::Next, ACTION_HEIGHT));
            }
            form.push(Box::new(HStack::new(Views(accounts)).spacing(gap)));
            form.push(control("Other user", Action::Manual, width));
        } else {
            let user = field == 0;
            let value = if user {
                name.to_owned()
            } else {
                "•".repeat(usize::from(self.state[2]))
            };
            form.push(target(
                TextField::new(State::new(generate_state_id(), value))
                    .automatic_software_keyboard(false)
                    .placeholder(if user { "Username" } else { "Password" })
                    .font_size(18.0)
                    .background_color(FROST)
                    .text_color(TEXT)
                    .placeholder_color(MUTED)
                    .border_color(LINE)
                    .frame(width, row_h),
                &actions,
                if user { Action::User } else { Action::Password },
            ));
            if failed == 1 {
                form.push(Box::new(
                    Text::new("Authentication failed. Try again.")
                        .color(TEXT)
                        .font_size(14.0),
                ));
            }
            let title = if user {
                "Continue"
            } else if field == 2 {
                "Unlock"
            } else {
                "Sign in"
            };
            let mut row = Vec::new();
            if field != 2 {
                row.push(control("", Action::Accounts, row_h));
            }
            let slots = row.len() as f32 + 1.0;
            row.push(control(
                title,
                Action::Submit,
                width - slots * (row_h + gap),
            ));
            row.push(control("", Action::Keyboard, row_h));
            form.push(Box::new(HStack::new(Views(row)).spacing(gap)));
        }
        let card = Surface::floating(
            VStack::new(Views(form))
                .alignment(Alignment::Leading)
                .spacing(gap)
                .padding(inset),
        )
        .fill(CARD)
        .border_color(LINE)
        .corner_radius(RADIUS)
        .elevation(ElevationRole::Flat);
        let mut content: Vec<Box<dyn View>> = vec![Box::new(
            ZStack::new((card,))
                .alignment(Alignment::Center)
                .frame(w, free),
        )];
        if panel_h > 0.0 {
            content.push(keyboard_view::keyboard(
                Size::new(w, panel_h),
                if self.shift {
                    sws_protocol::input_panel::SHIFT
                } else {
                    0
                },
                self.layer,
                false,
                Rc::new(|_| {}),
                |action| {
                    // Desktop-only commands stay in their usual locations, visibly disabled.
                    if matches!(
                        action,
                        KeyAction::Key(85, _) | KeyAction::SetLayer(Layer::Tools | Layer::Function)
                    ) {
                        return keyboard_view::KeyTarget::Disabled;
                    }
                    let mut actions = actions.borrow_mut();
                    let id = actions.len();
                    actions.push(Action::Key(action));
                    keyboard_view::KeyTarget::Seat(id)
                },
            ));
        }
        let foreground = VStack::new(Views(content)).spacing(0.0);
        let background = wallpaper
            .map(Image::from_bitmap)
            .unwrap_or_else(|| Image::placeholder(1, 1));
        Window::new(
            "Sign in",
            ZStack::new((background.frame(w, h), foreground))
                .frame(w, h)
                .background(Color::BLACK),
        )
        .decorated(false)
        .size(Size::new(w, h))
    }
}

fn button(label: &str, action: Action, w: f32, h: f32, actions: &Actions) -> Box<dyn View> {
    let icon = match action {
        Action::Accounts => Some(Icon::Users),
        Action::Keyboard => Some(Icon::KeyboardHide),
        Action::Choose => Some(Icon::User),
        Action::Previous => Some(Icon::ChevronLeft),
        Action::Next => Some(Icon::ChevronRight),
        _ => None,
    };
    let mut button = Button::new(label);
    if let Some(icon) = icon {
        button = if label.is_empty() {
            Button::icon_only(icon)
        } else {
            button.icon(icon)
        };
        button = button.icon_color(TEXT).icon_size(IconSize::Pixels(20));
    }
    let button = button.header_style().text_color(TEXT).font_size(17.0);
    let primary = matches!(action, Action::Choose | Action::Submit);
    target(
        appearance::action_surface(
            button,
            Size::new(w, h),
            if primary {
                appearance::FLOATING_ACTIVE
            } else {
                FROST
            },
            if primary { ACCENT } else { LINE },
            primary,
        ),
        actions,
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
