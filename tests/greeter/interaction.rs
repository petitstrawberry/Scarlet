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
        (0, 3),
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
        let s = screen(w, h);
        let targets = s.targets();
        let p = s.pipeline();
        // Inspect the production tree, then exercise every rendered Button.
        let mut buttons = Vec::new();
        controls(p.element_tree().root().unwrap(), Point::ZERO, &mut buttons);
        assert!(buttons.len() > 30);
        let mut fields = 0;
        for (label, x, y, height) in &buttons {
            assert!(
                *x >= 0 && *x < w as i32 && *y >= 0 && *y < h as i32,
                "button outside {w}x{h}: {x},{y}"
            );
            assert!(*height >= 28.0, "small target at {w}x{h}: {height}");
            let action = targets.at(*x, *y).expect("visible control must activate");
            if !matches!(action, Action::Key(_)) {
                assert!(
                    *y as f32 + height / 2.0 <= h as f32 - s.keyboard_height() + 0.5,
                    "overlap at {w}x{h}: {label}"
                );
            }
            if label == "field" {
                assert!(action == [Action::User, Action::Password][fields]);
                fields += 1;
                assert!(*y as f32 + height / 2.0 <= h as f32 - s.keyboard_height());
            } else if label == "q" {
                assert!(action == Action::Key(KeyAction::Key(16, 0)));
            }
        }
        assert_eq!(fields, 2);
    }
}

#[test]
fn physical_and_panel_keys_reach_the_same_private_broker_bytes() {
    use seat_keys::{activate, key_byte};
    let mut s = screen(800, 480);
    assert_eq!(activate(&mut s, Action::Password), Some(18));
    assert_eq!(
        activate(
            &mut s,
            Action::Key(KeyAction::Modifier(sws_protocol::input_panel::SHIFT))
        ),
        None
    );
    for (code, expected) in [(30, b'A'), (2, b'1'), (14, 8), (57, b' '), (28, b'\n')] {
        assert_eq!(
            activate(&mut s, Action::Key(KeyAction::Key(code, 0))),
            Some(expected)
        );
        assert_eq!(key_byte(code, expected == b'A', false), Some(expected));
    }
    assert_eq!(key_byte(30, false, true), Some(b'A'));
    assert_eq!(key_byte(30, true, true), Some(b'a'));
    assert_eq!(
        activate(
            &mut s,
            Action::Key(KeyAction::Key(2, sws_protocol::input_panel::SHIFT))
        ),
        Some(b'!')
    );
    assert_eq!(activate(&mut s, Action::Submit), Some(b'\n'));
}
