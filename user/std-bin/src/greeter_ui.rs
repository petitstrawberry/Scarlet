//! Scarlet shell login form. The editor state contains a login name or mask glyphs.
use scarlet_ui::{BitmapImage, prelude::*, views::containers::ViewTuple};
use std::{cell::Cell, rc::Rc};
#[path = "scarlet_shell/appearance.rs"]
mod appearance;
use appearance::{ACTION_HEIGHT, LINE, MUTED, TEXT};

/// Both physical and input-panel strokes arrive as normal ScarletUI key events.
/// Printable keys also emit a Char event, so only editing keys use Pressed.
pub fn broker_key(key: &scarlet_ui::event::KeyEvent) -> Option<u8> {
    use scarlet_ui::event::{KeyCode, KeyEvent};
    match key {
        KeyEvent::Char { c } if c.is_ascii() && !c.is_ascii_control() => Some(*c as u8),
        KeyEvent::Pressed { keycode, .. } => match keycode {
            KeyCode::Enter => Some(b'\n'),
            KeyCode::Backspace => Some(8),
            KeyCode::Tab => Some(9),
            KeyCode::Escape => Some(27),
            _ => None,
        },
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Submit,
    Accounts,
    Previous,
    Next,
    Manual,
    Choose,
}
impl Action {
    pub fn broker_byte(self) -> u8 {
        match self {
            Self::Submit | Self::Choose => b'\n',
            Self::Accounts => 19,
            Self::Previous => 20,
            Self::Next => 21,
            Self::Manual => 22,
        }
    }
}
pub type Actions = Rc<Cell<Option<Action>>>;
pub struct Screen {
    pub width: u32,
    pub height: u32,
    pub covered: f32,
    pub display: [u8; 36],
}
impl Screen {
    pub fn value(&self) -> String {
        if self.display[0] == 0 {
            String::from_utf8_lossy(&self.display[4..4 + usize::from(self.display[3])]).into_owned()
        } else {
            "•".repeat(usize::from(self.display[2]))
        }
    }
    pub fn window(
        &self,
        actions: Actions,
        wallpaper: BitmapImage,
        value: State<String>,
        focus: bool,
        time_label: State<String>,
    ) -> impl View + Clone {
        let (w, h) = (self.width as f32, self.height as f32);
        let free = (h - self.covered).max(1.);
        let compact = free < 280.;
        let tight = free < 180.;
        let gap = if tight {
            3.
        } else if compact {
            7.
        } else {
            14.
        };
        let width = (w - 36.).clamp(120., 312.);
        let field = self.display[0];
        let unavailable = self.display[1] == 2;
        let choosing = field >= 3 && !unavailable;
        let panel_open = self.covered > 0. && !choosing && !unavailable;
        let name = std::str::from_utf8(&self.display[4..4 + usize::from(self.display[3])]).unwrap();
        let title = if unavailable {
            "No accounts available"
        } else if field == 0 {
            "Other user"
        } else {
            name
        };
        let mut form: Vec<Box<dyn View>> = Vec::new();
        let diameter = if choosing {
            44.
        } else if tight {
            32.
        } else if compact {
            46.
        } else {
            88.
        };
        let avatar = avatar(title, diameter, field == 0 || unavailable);
        let title_size = if choosing {
            22.
        } else if compact {
            23.
        } else {
            32.
        };
        let identity: Box<dyn View> = if choosing {
            let pending = actions.clone();
            let choose = Button::new(name)
                .header_style()
                .text_color(TEXT)
                .font_size(title_size)
                .on_click(move || pending.set(Some(Action::Choose)));
            let pending = actions.clone();
            Box::new(
                HStack::new((avatar, choose))
                    .spacing(6.)
                    .on_click(move || pending.set(Some(Action::Choose))),
            )
        } else {
            let label = Text::new(title)
                .font_size(title_size)
                .color(TEXT)
                .line_limit(Some(1));
            let account: Box<dyn View> = if panel_open && field != 2 {
                // The account heading also opens the chooser while typing.
                let pending = actions.clone();
                let select = HStack::new((
                    label,
                    Button::icon_only(Icon::ChevronDown)
                        .header_style()
                        .icon_size(IconSize::Small)
                        .icon_color(TEXT)
                        .on_click(move || pending.set(Some(Action::Accounts))),
                ))
                .spacing(2.);
                Box::new(select)
            } else {
                Box::new(label)
            };
            let contents = Views(vec![Box::new(avatar), account]);
            let header: Box<dyn View> = if compact {
                Box::new(HStack::new(contents).spacing(12.))
            } else {
                Box::new(VStack::new(contents).spacing(14.))
            };
            if panel_open && field != 2 {
                let pending = actions.clone();
                Box::new(
                    ZStack::new(Views(vec![header]))
                        .on_click(move || pending.set(Some(Action::Accounts))),
                )
            } else {
                header
            }
        };
        if choosing && field == 4 {
            // Symmetric side controls keep the selected identity centered.
            form.push(Box::new(
                HStack::new((
                    action_button("", Some(Icon::ChevronLeft), Action::Previous, &actions),
                    ZStack::new(Views(vec![identity])).frame(width - 2. * ACTION_HEIGHT, 54.),
                    action_button("", Some(Icon::ChevronRight), Action::Next, &actions),
                ))
                .spacing(0.),
            ));
        } else {
            form.push(identity);
        }

        if unavailable {
            form.push(Box::new(action_button(
                "Check again",
                None,
                Action::Submit,
                &actions,
            )));
        } else if choosing {
            form.push(Box::new(action_button(
                "Other user",
                None,
                Action::Manual,
                &actions,
            )));
        } else {
            let user = field == 0;
            let submit_label = if user {
                "Continue"
            } else if field == 2 {
                "Unlock"
            } else {
                "Sign in"
            };
            let entry = TextField::new(value)
                .autofocus(focus)
                .placeholder(if user { "Username" } else { "Password" })
                .font_size(17.)
                .padding(10.)
                .background_color(Color::rgba_f32(0.12, 0.12, 0.15, 0.88))
                .text_color(TEXT)
                .placeholder_color(MUTED)
                .border_color(LINE)
                .focused_border_color(Color::rgba_f32(0.86, 0.87, 0.91, 0.68))
                .frame(width - 88., ACTION_HEIGHT);
            let entry_row = Box::new(
                HStack::new((
                    entry,
                    action_button(submit_label, None, Action::Submit, &actions),
                ))
                .spacing(8.),
            );
            let error: Option<Box<dyn View>> = (self.display[1] == 1).then(|| {
                Box::new(
                    Text::new("Authentication failed. Try again.")
                        .color(TEXT)
                        .font_size(14.),
                ) as Box<dyn View>
            });
            if panel_open {
                form.extend(error);
                form.push(entry_row);
            } else {
                form.push(entry_row);
                form.extend(error);
            }
            if field != 2 && !panel_open {
                form.push(Box::new(action_button(
                    "Switch user",
                    None,
                    Action::Accounts,
                    &actions,
                )));
            }
        }
        let controls = VStack::new(Views(form)).spacing(gap);
        let content: Box<dyn View> = if choosing && free >= 300. {
            let font = (w * 0.17).min(free * 0.17).clamp(42., 104.);
            Box::new(
                VStack::new((
                    Text::from_state(time_label).font_size(font).color(TEXT),
                    controls,
                ))
                .spacing(font * 0.46),
            )
        } else {
            Box::new(controls)
        };
        // Keep the editing row at the display midline when it fits. Its lower
        // edge follows the panel only when the panel would cover the row.
        let (alignment, height) = if panel_open {
            (
                Alignment::Bottom,
                (h / 2. + ACTION_HEIGHT / 2.).min(free - gap).max(1.),
            )
        } else {
            (Alignment::Center, free)
        };
        let foreground = ZStack::new(Views(vec![content]))
            .alignment(alignment)
            .frame(w, height);
        Window::new(
            "Sign in",
            ZStack::new((
                Image::from_bitmap(wallpaper)
                    .fit_mode(ImageFit::Cover)
                    .frame(w, h),
                ZStack::new(())
                    .frame(w, h)
                    .background(Color::rgba_f32(0.025, 0.025, 0.035, 0.34)),
                foreground,
            ))
            .alignment(Alignment::TopLeading)
            .frame(w, h)
            .background(Color::BLACK),
        )
        .decorated(false)
        .size(Size::new(w, h))
    }
}

fn avatar(name: &str, diameter: f32, generic: bool) -> impl View + Clone {
    // Public account names have no portrait metadata. Keep a stable monogram
    // for each account; the manual-entry screen carries the shell's user icon.
    let colors = [
        Color::rgb(93u8, 112u8, 139u8),
        Color::rgb(110u8, 98u8, 133u8),
        Color::rgb(81u8, 118u8, 114u8),
        Color::rgb(145u8, 94u8, 92u8),
    ];
    let color = if generic {
        Color::rgb(86u8, 91u8, 106u8)
    } else {
        colors[name.bytes().map(usize::from).sum::<usize>() % colors.len()]
    };
    let glyph: Box<dyn View> = if generic {
        Box::new(
            IconView::new(Icon::User)
                .size(IconSize::Pixels((diameter * 0.48) as u16))
                .color(TEXT),
        )
    } else {
        Box::new(
            Text::new(
                name.chars()
                    .next()
                    .unwrap_or(' ')
                    .to_uppercase()
                    .to_string(),
            )
            .font_size(diameter * 0.47)
            .color(TEXT),
        )
    };
    Surface::section(
        ZStack::new(Views(vec![glyph]))
            .alignment(Alignment::Center)
            .frame(diameter, diameter),
    )
    .fill(color)
    .bordered(false)
    .corner_radius(diameter / 2.)
    .border_rounded(Color::rgba_f32(1., 1., 1., 0.22), 1., diameter / 2.)
}

fn action_button(
    label: &str,
    icon: Option<Icon>,
    action: Action,
    actions: &Actions,
) -> impl View + Clone {
    let primary = action == Action::Submit;
    let button = match icon {
        Some(icon) => Button::icon_only(icon)
            .icon_size(IconSize::Pixels(17))
            .icon_color(TEXT),
        None => Button::new(label),
    };
    let target = actions.clone();
    let button = button
        .header_style()
        .text_color(if primary {
            Color::rgb(31u8, 32u8, 38u8)
        } else {
            TEXT
        })
        .font_size(15.)
        .on_click(move || target.set(Some(action)));
    let width = if icon.is_some() {
        ACTION_HEIGHT
    } else if primary {
        80.
    } else {
        122.
    };
    let target = actions.clone();
    Surface::section(
        ZStack::new((button,))
            .alignment(Alignment::Center)
            .frame(width, ACTION_HEIGHT),
    )
    .fill(if primary {
        Color::rgb(231u8, 232u8, 238u8)
    } else if icon.is_some() {
        Color::rgba_f32(0.31, 0.32, 0.37, 0.42)
    } else {
        Color::TRANSPARENT
    })
    .bordered(false)
    .corner_radius(if primary { 12. } else { ACTION_HEIGHT / 2. })
    .on_click(move || target.set(Some(action)))
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
