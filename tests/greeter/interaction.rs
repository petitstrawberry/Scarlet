#[path = "../../user/std-bin/src/greeter_ui.rs"]
mod greeter_ui;
#[path = "../../user/std-bin/src/sws/login_access.rs"]
mod login_access;
use greeter_ui::{Actions, Screen};
use scarlet_ui::{BitmapImage, RenderingPipeline, prelude::*};

fn form(w: u32, h: u32, field: u8, covered: f32) -> (RenderingPipeline, Actions, State<String>) {
    scarlet_ui::graphics::set_default_font(include_bytes!(
        "../../bundles/desktop/fs/share/fonts/Mplus1-Regular.ttf"
    ))
    .unwrap();
    let mut display = [0; 36];
    display[..4].copy_from_slice(&[field, 0, if field == 1 { 3 } else { 0 }, 5]);
    display[4..9].copy_from_slice(b"alice");
    let screen = Screen {
        width: w,
        height: h,
        covered,
        display,
    };
    let actions = Actions::default();
    let value = State::new(scarlet_ui::generate_state_id(), screen.value());
    let image = BitmapImage::from_bgra(vec![0xff171719], 1, 1);
    let mut p = RenderingPipeline::new();
    p.set_root(
        screen
            .window(
                actions.clone(),
                image,
                value.clone(),
                true,
                State::new(scarlet_ui::generate_state_id(), "09:41".to_owned()),
            )
            .create_element(),
    );
    p.layout_initial();
    p.render().unwrap();
    (p, actions, value)
}

#[test]
fn focused_shell_field_uses_the_standard_panel_and_respects_actual_occlusion() {
    for (w, h, covered) in [
        (320, 360, 141.),
        (800, 480, 260.),
        (600, 1024, 240.),
        (1280, 720, 300.),
    ] {
        for field in [0, 1] {
            let (p, _, _) = form(w, h, field, covered);
            let state = p.focused_text_input_state().expect("focused login editor");
            assert!(state.automatic_software_keyboard);
            assert_eq!(
                state.surrounding_text,
                if field == 0 { "alice" } else { "•••" }
            );
            assert!(state.input_rect.origin.y + state.input_rect.size.height <= h as f32 - covered);
        }
        assert!(form(w, h, 4, 0.).0.focused_text_input_state().is_none());
    }
}

#[test]
fn login_clients_have_distinct_roles_and_no_desktop_services() {
    use login_access::{LoginPeer::*, login_request_allowed};
    let allows =
        |role, msg: &sws_protocol::ClientMessageRef<'_>| login_request_allowed(role, msg, &[7]);
    use sws_protocol::{ClientMessageRef::*, input_panel::Request as Panel};
    let key = InputPanel(Panel::Key {
        context_id: 1,
        generation: 2,
        code: 30,
        modifiers: 0,
    });
    assert!(allows(Editor, &TextInputEnable { context_id: 1 }));
    assert!(!allows(Panel, &TextInputEnable { context_id: 1 }));
    assert!(allows(Panel, &key));
    assert!(!allows(Editor, &key));
    for role in [Editor, Panel] {
        assert!(allows(role, &DestroyWindow { window_id: 7 }));
        assert!(!allows(role, &DestroyWindow { window_id: 1 }));
        assert!(!allows(role, &SetFullscreen { window_id: 1 }));
        assert!(!allows(role, &ImeGetMethods {}));
        assert!(!allows(role, &GetWindowList {}));
        assert!(!allows(role, &GetActiveApp {}));
        assert!(!allows(
            role,
            &Clipboard(sws_protocol::clipboard::Request::Get { window_id: 1 })
        ));
    }
}

#[test]
fn normalized_key_stream_reaches_the_broker_once_per_character() {
    use scarlet_ui::event::{KeyCode, KeyEvent, KeyModifiers};
    let modifiers = KeyModifiers::default();
    let keys = [
        KeyEvent::Pressed {
            keycode: KeyCode::Char('A'),
            modifiers,
        },
        KeyEvent::Char { c: 'A' },
        KeyEvent::Released {
            keycode: KeyCode::Char('A'),
            modifiers,
        },
        KeyEvent::Char { c: '!' },
        KeyEvent::Pressed {
            keycode: KeyCode::Backspace,
            modifiers,
        },
        KeyEvent::Pressed {
            keycode: KeyCode::Tab,
            modifiers,
        },
        KeyEvent::Char { c: '\t' },
        KeyEvent::Pressed {
            keycode: KeyCode::Enter,
            modifiers,
        },
    ];
    assert_eq!(
        keys.iter()
            .filter_map(greeter_ui::broker_key)
            .collect::<Vec<_>>(),
        b"A!\x08\t\n"
    );
}

fn button_center(element: &dyn Element, origin: Point, label: &str) -> Option<(i32, i32)> {
    let at = Point::new(
        origin.x + element.position().x,
        origin.y + element.position().y,
    );
    if let Some(button) = element
        .as_any()
        .downcast_ref::<scarlet_ui::RenderElement<Button, scarlet_ui::views::ButtonRenderObject>>()
        && button.view().label() == label
    {
        let size = element.bounds().size;
        return Some((
            (at.x + size.width / 2.) as i32,
            (at.y + size.height / 2.) as i32,
        ));
    }
    element
        .children()
        .iter()
        .find_map(|child| button_center(child.as_ref(), at, label))
}

#[test]
fn account_selection_manual_entry_and_submit_dispatch_from_visible_controls() {
    use greeter_ui::Action;
    use scarlet_ui::event::{Event, MouseButton, MouseEvent};
    for (w, h, covered) in [(800, 480, 240.), (600, 1024, 260.)] {
        for (field, label, expected) in [
            (4, "alice", Action::Choose),
            (4, "Other user", Action::Manual),
            (0, "Continue", Action::Submit),
            (1, "Sign in", Action::Submit),
        ] {
            let (mut p, actions, _) = form(w, h, field, if field < 2 { covered } else { 0. });
            let (x, y) =
                button_center(p.element_tree().root().unwrap(), Point::new(0., 0.), label).unwrap();
            p.handle_event(&Event::Mouse(MouseEvent::ButtonPressed {
                x,
                y,
                button: MouseButton::Left,
                click_count: 1,
            }));
            p.handle_event(&Event::Mouse(MouseEvent::ButtonReleased {
                x,
                y,
                button: MouseButton::Left,
                click_count: 1,
            }));
            assert_eq!(actions.take(), Some(expected), "{label} on {w}×{h}");
            use scarlet_ui::event::{TouchChange, TouchFrame, TouchPhase};
            for (serial, phase) in [(1, TouchPhase::Down), (2, TouchPhase::Up)] {
                let time_ns = serial * 16_000_000;
                p.handle_event(&Event::TouchFrame(TouchFrame {
                    seat_id: 0,
                    serial,
                    time_ns,
                    changes: vec![TouchChange {
                        seat_id: 0,
                        serial,
                        time_ns,
                        id: 8,
                        phase,
                        x,
                        y,
                        pressure: None,
                        touch_major: None,
                    }],
                }));
            }
            assert_eq!(
                actions.take(),
                Some(expected),
                "direct tap on {label} at {w}×{h}"
            );
        }
    }
}

#[test]
fn broker_updates_the_mounted_password_field_without_resetting_focus() {
    let (mut pipeline, _, value) = form(800, 480, 1, 250.);
    for length in [4, 3, 0] {
        let mask = "•".repeat(length);
        value.set(mask.clone());
        pipeline.request_redraw();
        pipeline.render().unwrap();
        let field = pipeline.focused_text_input_state().unwrap();
        assert_eq!(field.surrounding_text, mask);
        assert!(field.automatic_software_keyboard);
    }
}

#[test]
fn clock_and_account_actions_share_the_centered_stack_and_yield_to_the_editor() {
    fn clock_bounds(element: &dyn Element, origin: Point) -> Option<(Point, Size)> {
        let at = Point::new(
            origin.x + element.position().x,
            origin.y + element.position().y,
        );
        if let Some(text) = element
            .as_any()
            .downcast_ref::<scarlet_ui::RenderElement<Text, scarlet_ui::views::TextRenderObject>>()
            && text.view().content() == "09:41"
        {
            return Some((at, element.bounds().size));
        }
        element
            .children()
            .iter()
            .find_map(|c| clock_bounds(c.as_ref(), at))
    }
    for (w, h) in [(320, 360), (800, 480), (600, 1024)] {
        let (p, _, _) = form(w, h, 4, 0.);
        let root = p.element_tree().root().unwrap();
        let (at, size) = clock_bounds(root, Point::ZERO).unwrap();
        assert!((at.x + size.width / 2. - w as f32 / 2.).abs() <= 1.);
        assert!(at.y > 0. && at.y + size.height / 2. < h as f32 / 2.);
        let (_, last_y) = button_center(root, Point::ZERO, "Other user").unwrap();
        // The last action has a 44-point hit target, centered on its label.
        assert!((at.y - (h as f32 - last_y as f32 - 22.)).abs() < 2.);
        let (_, account_y) = button_center(root, Point::ZERO, "alice").unwrap();
        assert!(account_y as f32 > at.y + size.height);
        let (p, _, _) = form(w, h, 1, 120.);
        assert!(clock_bounds(p.element_tree().root().unwrap(), Point::ZERO).is_none());
    }
}
