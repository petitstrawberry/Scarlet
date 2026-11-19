#[path = "../../user/std-bin/src/greeter_ui.rs"]
mod greeter_ui;
#[allow(dead_code)]
#[path = "../../user/std-bin/src/soft_keyboard/layout.rs"]
mod keyboard_layout;
#[path = "../../user/std-bin/src/seat_keys.rs"]
mod seat_keys;
#[path = "../../user/std-bin/src/seat_pointer.rs"]
mod seat_pointer;
use greeter_ui::{Action, Screen};
use keyboard_layout::{Action as KeyAction, Layer};
use scarlet_ui::{Element, Point};

fn screen(w: u32, h: u32) -> Screen {
    let mut state = [0; 36];
    state[3] = 5;
    state[4..9].copy_from_slice(b"alice");
    Screen {
        width: w,
        height: h,
        state,
        keyboard: true,
        shift: false,
        layer: Layer::Letters,
    }
}
fn controls(element: &dyn Element, origin: Point, out: &mut Vec<(String, i32, i32, f32)>) {
    let at = Point::new(
        origin.x + element.position().x,
        origin.y + element.position().y,
    );
    let control = element.as_any().downcast_ref::<scarlet_ui::RenderElement<scarlet_ui::Button, scarlet_ui::views::ButtonRenderObject>>().map(|e| e.view().label())
        .or_else(|| element.as_any().is::<scarlet_ui::RenderElement<scarlet_ui::TextField, scarlet_ui::views::TextFieldRenderObject>>().then_some("field"));
    if let Some(label) = control {
        let size = element.bounds().size;
        out.push((
            label.to_owned(),
            (at.x + size.width / 2.0) as i32,
            (at.y + size.height / 2.0) as i32,
            size.height,
        ));
    }
    for c in element.children() {
        controls(c.as_ref(), at, out);
    }
}
#[test]
fn protocol_is_bounded_and_contains_only_display_state() {
    let s = screen(800, 600);
    let bytes = s.encode();
    assert_eq!(bytes.len(), 48);
    assert!(Screen::decode(&bytes).is_ok());
    for (i, v) in [
        (0, 5),
        (1, 3),
        (2, 73),
        (3, 33),
        (39, 1),
        (44, 2),
        (45, 2),
        (46, 3),
        (47, 1),
    ] {
        let mut b = bytes;
        b[i] = v;
        assert!(Screen::decode(&b).is_err(), "{i}");
    }
}
#[test]
fn login_controls_dispatch_and_keyboard_stays_below_form() {
    scarlet_ui::graphics::set_default_font(include_bytes!(
        "../../bundles/desktop/fs/share/fonts/Mplus1-Regular.ttf"
    ))
    .unwrap();
    for (w, h) in [(800, 480), (1280, 720), (600, 1024), (320, 360)] {
        for field in [0, 1, 3, 4] {
            let mut s = screen(w, h);
            s.state[0] = field;
            let targets = s.targets();
            let pipeline = s.pipeline();
            let mut controls_seen = Vec::new();
            controls(
                pipeline.element_tree().root().unwrap(),
                Point::ZERO,
                &mut controls_seen,
            );
            let mut actions = Vec::new();
            let mut disabled = 0;
            for (label, x, y, _) in controls_seen {
                assert!(
                    (0..w as i32).contains(&x) && (0..h as i32).contains(&y),
                    "outside {w}x{h} field {field}: {label:?} @ {x},{y}"
                );
                let Some(action) = targets.at(x, y) else {
                    assert!(
                        field < 3 && (label == "あ/A" || label.is_empty()),
                        "missing {label:?} @ {x},{y}"
                    );
                    disabled += 1;
                    continue;
                };
                assert_eq!(
                    matches!(action, Action::Key(_)),
                    y as f32 >= h as f32 - s.keyboard_height(),
                    "overlap at {w}x{h}: {label}"
                );
                if label == "field" {
                    assert!(
                        action
                            == if field == 0 {
                                Action::User
                            } else {
                                Action::Password
                            }
                    );
                }
                actions.push(action);
            }
            if field < 3 {
                assert!(actions.contains(&Action::Submit) && actions.contains(&Action::Accounts));
                assert!(actions.contains(&Action::Key(KeyAction::Key(16, 0))));
                assert_eq!(disabled, 2);
            } else {
                assert_eq!(s.keyboard_height(), 0.);
                assert!(actions.contains(&Action::Manual) && actions.contains(&Action::Choose));
                assert_eq!(actions.contains(&Action::Next), field == 4);
                assert_eq!(actions.contains(&Action::Previous), field == 4);
            }
        }
    }
}

#[test]
fn physical_and_panel_keys_reach_the_same_private_broker_bytes() {
    use seat_keys::{PanelInput, key_byte};
    let mut panel = PanelInput::new();
    let mut s = screen(800, 480);
    assert_eq!(panel.activate(&mut s, Action::Password), Some(18));
    assert_eq!(
        panel.activate(
            &mut s,
            Action::Key(KeyAction::Modifier(sws_protocol::input_panel::SHIFT))
        ),
        None
    );
    for (code, expected) in [(30, b'A'), (2, b'1'), (14, 8), (57, b' '), (28, b'\n')] {
        assert_eq!(
            panel.activate(&mut s, Action::Key(KeyAction::Key(code, 0))),
            Some(expected)
        );
        assert_eq!(key_byte(code, expected == b'A', false), Some(expected));
    }
    assert_eq!(key_byte(30, false, true), Some(b'A'));
    assert_eq!(key_byte(30, true, true), Some(b'a'));
    assert_eq!(
        panel.activate(
            &mut s,
            Action::Key(KeyAction::Key(2, sws_protocol::input_panel::SHIFT))
        ),
        Some(b'!')
    );
    assert_eq!(panel.activate(&mut s, Action::Submit), Some(b'\n'));
}

#[test]
fn login_keyboard_has_the_desktop_key_bounds_on_each_page() {
    use scarlet_ui::prelude::*;
    use std::rc::Rc;
    #[path = "../../user/std-bin/src/soft_keyboard/view.rs"]
    mod desktop_keyboard;
    for (width, height) in [(800, 480), (600, 1024), (320, 360)] {
        for layer in [Layer::Letters, Layer::Symbols, Layer::MoreSymbols] {
            let mut s = screen(width, height);
            s.layer = layer;
            let h = s.keyboard_height();
            let size = Size::new(width as f32, h);
            let view = desktop_keyboard::keyboard(size, 0, layer, false, Rc::new(|_| {}), |_| {
                desktop_keyboard::KeyTarget::Desktop
            });
            let mut panel = scarlet_ui::ElementTree::new();
            panel.set_root(view.create_element());
            panel.layout(LayoutConstraints::tight(size.width, size.height));
            let mut desktop = Vec::new();
            controls(
                panel.root().unwrap(),
                Point::new(0., height as f32 - h),
                &mut desktop,
            );
            let mut login = Vec::new();
            controls(
                s.pipeline().element_tree().root().unwrap(),
                Point::ZERO,
                &mut login,
            );
            login.retain(|(_, _, y, _)| *y as f32 >= height as f32 - h);
            assert_eq!(desktop, login);
        }
    }
}
