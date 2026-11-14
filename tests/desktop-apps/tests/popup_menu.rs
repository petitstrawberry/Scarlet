//! Exercise the actual shell menu renderer, including its native surface metadata.
#[path = "../../../user/std-bin/src/scarlet_shell/popup_menu.rs"]
mod popup_menu;

use popup_menu::PopupMenuRenderer;
use scarlet_ui::element::{ElementRenderObject, LayoutConstraints};
use scarlet_ui::views::menu::MenuRenderObject;
use scarlet_ui::views::{MenuAction, MenuItemContent};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use sws_protocol::surface_regions::{BACKDROP, INPUT};

#[test]
fn glass_menu_has_translucent_rounded_body_and_shadow_at_each_scale() {
    for scale in [1000, 1250, 2000] {
        let mut renderer = PopupMenuRenderer::new(
            vec![
                MenuItemContent::new("Open"),
                MenuItemContent::separator(),
                MenuItemContent::new("Quit"),
            ],
            28.0,
            220.0,
            220.0,
            scale,
        );
        let geometry = renderer.window_geometry();
        let regions = renderer.surface_regions();
        assert_eq!(regions.len(), 1);
        let region = regions[0];
        assert_eq!(region.flags, INPUT | BACKDROP);
        assert!(region.corner_radius > 0 && region.blur_radius > 0);
        assert_eq!(
            (region.x, region.y, region.width, region.height),
            (geometry.x, geometry.y, geometry.width, geometry.height)
        );
        let buffer = renderer.buffer().unwrap();
        let (width, height) = renderer.physical_size();
        assert_eq!((buffer.width(), buffer.height()), (width, height));
        assert!(width > geometry.width && height > geometry.height);
        assert_eq!(
            buffer.as_slice()[0] >> 24,
            0,
            "outer padding must stay transparent"
        );
        let alpha =
            |x: i32, y: i32| buffer.as_slice()[y as usize * width as usize + x as usize] >> 24;
        let center_x = geometry.x + geometry.width as i32 / 2;
        // An empty band inside the menu keeps the shared translucent material.
        let body = alpha(center_x, geometry.y + geometry.height as i32 - 2);
        assert!(body > 150 && body < 255, "material alpha: {body}");
        assert!(
            alpha(geometry.x, geometry.y) < body,
            "corners must be rounded"
        );
        let shadow = alpha(center_x, geometry.y + geometry.height as i32 + 2);
        assert!(
            shadow > 0 && shadow < body,
            "floating shadow must survive surface padding"
        );
        let before = buffer.as_slice().to_vec();
        let origin = renderer.origin();
        assert!(renderer.handle_move(origin.x + 30.0, origin.y + 10.0));
        renderer.render();
        assert_ne!(renderer.buffer().unwrap().as_slice(), before);
        assert!(renderer.handle_move(-1.0, -1.0));
        renderer.render();
        assert_eq!(
            renderer.buffer().unwrap().as_slice(),
            before,
            "hover must not accumulate paint"
        );
    }
}

#[test]
fn shadow_padding_does_not_shift_rows_or_invoke_actions() {
    let invoked = Arc::new(AtomicUsize::new(0));
    let action = invoked.clone();
    let items = vec![
        MenuItemContent::new("Submenu")
            .action(MenuAction::Submenu)
            .callback(move || {
                action.fetch_add(1, Ordering::Relaxed);
            }),
        MenuItemContent::separator(),
        MenuItemContent::new("Disabled")
            .enabled(false)
            .callback(|| panic!("disabled menu item invoked")),
    ];
    let mut reference = MenuRenderObject::new(items.clone(), 48.0, 320.0);
    let size = reference.layout(LayoutConstraints::unconstrained());
    let row = reference.item_rect(0).unwrap();
    let separator = reference.item_rect(1).unwrap();
    let disabled = reference.item_rect(2).unwrap();
    let mut renderer = PopupMenuRenderer::new(items, 48.0, 320.0, 320.0, 1500);
    let origin = renderer.origin();
    assert_eq!(renderer.size(), size);
    renderer.handle_click(0.0, 0.0);
    renderer.handle_click(origin.x - 1.0, origin.y + 20.0);
    renderer.handle_click(origin.x + 10.0, origin.y + 1.0);
    renderer.handle_click(origin.x + 10.0, origin.y + separator.origin.y + 1.0);
    renderer.handle_click(origin.x + 10.0, origin.y + disabled.origin.y + 1.0);
    assert_eq!(invoked.load(Ordering::Relaxed), 0);
    for y in [
        row.origin.y,
        row.origin.y + 24.0,
        row.origin.y + row.size.height - 0.1,
    ] {
        renderer.handle_click(origin.x + 10.0, origin.y + y);
    }
    assert_eq!(invoked.load(Ordering::Relaxed), 3);
    assert!(renderer.handle_move(origin.x + 10.0, origin.y + 20.0));
    assert!(!renderer.handle_move(origin.x + 10.0, origin.y + 30.0));
    assert!(renderer.handle_move(0.0, 0.0));
}

#[test]
fn popup_menu_grows_to_fit_label_and_shortcut() {
    let items = vec![
        MenuItemContent::new("Open Markdown Preview to Side").shortcut("Ctrl+K V"),
        MenuItemContent::new("Command Palette...").shortcut("Ctrl+Shift+P"),
    ];
    let preferred = MenuRenderObject::new(items.clone(), 28.0, 220.0).preferred_width();
    assert!(preferred > 220.0);
    let renderer = PopupMenuRenderer::new(items.clone(), 28.0, 220.0, 2000.0, 1000);
    assert!(renderer.size().width >= preferred);
    // Never wider than the usable screen width.
    let capped = PopupMenuRenderer::new(items, 28.0, 220.0, 240.0, 1000);
    assert_eq!(capped.size().width, 240.0);
}
