//! Host harness for production geometry/state code; no compositor or OS calls.
//! Run test-tablet-geometry.sh through the Scarlet Nix environment on AArch64.
#[path = "../frame_callback.rs"]
mod frame_callback;
#[path = "../input_panel.rs"]
mod input_panel;
#[path = "../tablet_motion.rs"]
mod tablet_motion;
#[path = "../tablet_navigation.rs"]
mod tablet_navigation;
#[path = "../window.rs"]
mod window;
#[path = "../workspace_geometry.rs"]
mod workspace_geometry;
mod compositor {
    pub fn is_sws_debug_enabled() -> bool {
        false
    }
}

#[test]
fn tablet_keyboard_cycles_preserve_single_and_both_split_allocations() {
    use sws_protocol::input_panel::Occlusion;
    use sws_protocol::workspace::{SplitAxis, TabletLayout};
    let workarea = (0, 32, 1200, 868);
    for layout in [
        TabletLayout::Single { window_id: 1 },
        TabletLayout::Split {
            axis: SplitAxis::Horizontal,
            first_window_id: 1,
            second_window_id: 2,
            ratio_milli: 500,
        },
        TabletLayout::Split {
            axis: SplitAxis::Vertical,
            first_window_id: 1,
            second_window_id: 2,
            ratio_milli: 500,
        },
    ] {
        let mut tracker = input_panel::OcclusionTracker::default();
        for window_id in [1, 2] {
            let Some(slot) = workspace_geometry::tablet_slot(layout, window_id, workarea) else {
                continue;
            };
            let mut app = window::Window::new(window_id, 100, 100, 800, 600);
            app.set_tablet_managed(true);
            let restore = app.workspace_restore_geometry;
            (app.x, app.y, app.width, app.height) = slot;
            for _ in 0..3 {
                let panel = Occlusion {
                    window_id,
                    x: 0,
                    y: 600,
                    width: 1200,
                    height: 300,
                };
                let shown = tracker.update(Some((panel, app.surface_geometry(), 1000)));
                let local = shown.last().unwrap();
                assert!(local.x >= 0 && local.y >= 0);
                assert!(local.x as u32 + local.width <= slot.2);
                assert!(local.y as u32 + local.height <= slot.3);
                assert_eq!(app.surface_geometry(), slot);
                assert_eq!(app.workspace_restore_geometry, restore);
                assert_eq!(
                    tracker.update(None),
                    vec![Occlusion {
                        window_id,
                        ..Default::default()
                    }]
                );
                assert_eq!(
                    workspace_geometry::tablet_slot(layout, window_id, workarea),
                    Some(slot)
                );
            }
        }
    }
}
