#[cfg(test)]
mod tests {
    use super::*;
    fn key(controls: &ControlsOverlay, keycode: KeyCode, down: bool) {
        let event = if down {
            KeyEvent::Pressed {
                keycode,
                modifiers: KeyModifiers::empty(),
            }
        } else {
            KeyEvent::Released {
                keycode,
                modifiers: KeyModifiers::empty(),
            }
        };
        handle_key_event(event, controls, &PaintSignal);
    }
    fn pointer(controls: &ControlsOverlay, x: i32, y: i32, down: bool) {
        let event = if down {
            MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                x,
                y,
                click_count: 1,
            }
        } else {
            MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                x,
                y,
                click_count: 1,
            }
        };
        handle_canvas_event(&Event::Mouse(event), controls, &PaintSignal);
    }
    #[test]
    fn confirm_has_one_action_per_press_and_navigation_has_visible_focus() {
        let c = ControlsOverlay::new(false);
        key(&c, KeyCode::Enter, true);
        assert!(c.is_paused());
        for _ in 0..10 {
            key(&c, KeyCode::Enter, true);
        }
        assert!(c.is_paused());
        key(&c, KeyCode::Enter, false);
        key(&c, KeyCode::Enter, true);
        assert!(!c.is_paused());
        key(&c, KeyCode::Enter, false);
        key(&c, KeyCode::Down, true);
        assert_eq!(c.control_focus.load(Ordering::Acquire), 1);
        assert!(c.is_visible());
        assert!(c.controls_pinned.load(Ordering::Acquire));
        key(&c, KeyCode::Enter, true);
        assert!(c.is_loop_enabled());
        key(&c, KeyCode::Enter, false);
        key(&c, KeyCode::Down, true);
        assert_eq!(c.control_focus.load(Ordering::Acquire), 2);
        key(&c, KeyCode::Enter, true);
        assert_eq!(DISMISSALS.swap(0, Ordering::Relaxed), 0);
        key(&c, KeyCode::Escape, true);
        assert_eq!(DISMISSALS.swap(0, Ordering::Relaxed), 1);
    }
    #[test]
    fn repeated_seeks_coalesce_clamp_and_retain_paused_preview() {
        let c = ControlsOverlay::new(false);
        c.set_media_duration_us(20_000_000);
        c.paused.store(true, Ordering::Release);
        for _ in 0..3 {
            key(&c, KeyCode::Right, true);
        }
        assert_eq!(c.current_seek_target_us(), 15_000_000);
        assert!(!c.is_paused());
        c.mark_video_ready_for_seek(c.current_seek_epoch());
        assert!(c.is_paused());
        for _ in 0..10 {
            key(&c, KeyCode::Left, true);
        }
        assert_eq!(c.current_seek_target_us(), 0);
        for _ in 0..10 {
            key(&c, KeyCode::Right, true);
        }
        assert_eq!(c.current_seek_target_us(), 19_999_999);
    }
    #[test]
    fn hidden_controls_reveal_without_action_and_release_requires_matching_press() {
        let c = ControlsOverlay::new(false);
        c.update_canvas_size(390, 844);
        c.hide();
        let (x, y) = play_pause_button_origin(390, 844).unwrap();
        pointer(&c, x as i32 + 20, y as i32 + 20, true);
        pointer(&c, x as i32 + 20, y as i32 + 20, false);
        assert!(c.is_visible());
        assert!(!c.is_paused());
        pointer(&c, x as i32 + 20, y as i32 + 20, false);
        assert!(!c.is_paused());
        pointer(&c, x as i32 + 20, y as i32 + 20, true);
        pointer(&c, 200, 100, false);
        assert!(!c.is_paused());
        pointer(&c, x as i32 + 20, y as i32 + 20, true);
        pointer(&c, x as i32 + 20, y as i32 + 20, false);
        assert!(c.is_paused());
    }
    #[test]
    fn cancelled_drag_and_rotation_do_not_commit_a_seek() {
        let c = ControlsOverlay::new(false);
        c.update_canvas_size(768, 1024);
        c.set_media_duration_us(60_000_000);
        pointer(&c, 300, 994, true);
        assert!(c.is_scrubbing());
        assert!(c.desired_position_us.load() > 0);
        handle_canvas_event(
            &Event::Mouse(MouseEvent::ButtonCancelled {
                button: MouseButton::Left,
                x: 300,
                y: 994,
            }),
            &c,
            &PaintSignal,
        );
        assert!(!c.is_scrubbing());
        assert_eq!(c.current_seek_epoch(), 0);
        c.update_canvas_size(1024, 768);
        pointer(&c, 300, 738, false);
        assert_eq!(c.current_seek_epoch(), 0);
    }
    #[test]
    fn resizing_during_a_drag_cancels_preview_and_never_seeks_on_release() {
        let c = ControlsOverlay::new(false);
        c.update_canvas_size(768, 1024);
        c.set_media_duration_us(60_000_000);
        c.last_video_pts_us.store(7_000_000);
        pointer(&c, 300, (1024 - seek_track_bottom_inset()) as i32, true);
        assert!(c.is_scrubbing());
        c.update_canvas_size(1024, 768);
        assert!(!c.is_scrubbing());
        assert_eq!(c.desired_position_us.load(), 7_000_000);
        pointer(&c, 300, (768 - seek_track_bottom_inset()) as i32, false);
        assert_eq!(c.current_seek_epoch(), 0);
    }
    #[test]
    fn compact_desktop_and_tablet_targets_remain_separate_in_all_shapes() {
        for touch in [false, true] {
            TOUCH_MODE.store(touch, Ordering::Relaxed);
            let size = control_button_size();
            assert_eq!(size, if touch { 44 } else { 28 });
            for (w, h) in [
                (200, 160),
                (320, 240),
                (390, 844),
                (768, 1024),
                (1024, 768),
                (1280, 720),
            ] {
                let c = ControlsOverlay::new(false);
                c.update_canvas_size(w, h);
                c.set_media_duration_us(10_000_000);
                let (x, y) = play_pause_button_origin(w, h).unwrap();
                assert_eq!(pointer_control(&c, x as i32 + 1, y as i32 + 1), 1);
                assert_eq!(
                    pointer_control(&c, (x + size - 1) as i32, (y + size - 1) as i32),
                    1
                );
                let (fx, fy) = fullscreen_button_origin(w, h).unwrap();
                assert_eq!(pointer_control(&c, fx as i32 + 1, fy as i32 + 1), 3);
                assert_eq!(
                    pointer_control(&c, (fx + size - 1) as i32, (fy + size - 1) as i32),
                    3
                );
                assert!(fx >= loop_button_left_inset() + size + 12);
                assert_eq!(
                    pointer_control(&c, 150, (h - seek_track_bottom_inset()) as i32),
                    4
                );
                assert!(
                    y + size
                        < h - seek_track_bottom_inset()
                            - seek_track_hit_inset().max(SEEK_KNOB_HEIGHT)
                );
                assert_eq!(seek_target_from_track_x(&c, -100), 0);
                assert_eq!(seek_target_from_track_x(&c, w as i32 + 100), 10_000_000);
            }
        }
        TOUCH_MODE.store(false, Ordering::Relaxed);
    }
    #[test]
    fn replay_and_back_clear_scrubbing_and_keep_session_state_sane() {
        let c = ControlsOverlay::new(true);
        c.set_media_duration_us(30_000_000);
        c.mark_finished();
        key(&c, KeyCode::Enter, true);
        assert!(!c.is_finished());
        assert!(!c.is_paused());
        assert_eq!(c.current_replay_epoch(), 1);
        assert_eq!(c.current_seek_target_us(), 0);
        c.set_scrubbing(true);
        key(&c, KeyCode::Escape, true);
        assert!(!c.is_scrubbing());
        assert_eq!(DISMISSALS.swap(0, Ordering::Relaxed), 1);
        c.reset_for_media(false);
        assert!(!c.is_loop_enabled());
        assert_eq!(c.media_duration_us(), 0);
        assert!(c.is_visible());
    }
    #[test]
    fn native_control_icons_use_centered_masks_at_both_densities() {
        for milli in [1000, 2000] {
            let scale = UiScale { milli };
            let width = scale.physical_len(44);
            for icon in [
                scarlet_ui::Icon::PlayerPlay,
                scarlet_ui::Icon::PlayerPause,
                scarlet_ui::Icon::Repeat,
                scarlet_ui::Icon::ArrowsMaximize,
                scarlet_ui::Icon::ArrowsMinimize,
            ] {
                let mut pixels = vec![0; (width * width * 4) as usize];
                draw_native_control_icon(&mut pixels, width, width, 12, 12, icon, [255; 4], scale);
                let occupied: Vec<_> = pixels
                    .chunks_exact(4)
                    .enumerate()
                    .filter(|(_, p)| p[0] > 0)
                    .map(|(i, _)| ((i as u32) % width, (i as u32) / width))
                    .collect();
                assert!(!occupied.is_empty());
                assert!(occupied.iter().all(|&(x, y)| x >= scale.physical_pos(12)
                    && x < scale.physical_pos(28)
                    && y >= scale.physical_pos(12)
                    && y < scale.physical_pos(28)));
                draw_native_control_icon(&mut pixels, width, width, 43, 43, icon, [255; 4], scale);
            }
        }
    }

    #[test]
    fn fullscreen_keyboard_holds_and_cancel_do_not_close_the_video() {
        DISMISSALS.store(0, Ordering::Relaxed);
        let c = ControlsOverlay::new(false);
        c.paused.store(true, Ordering::Release);
        key(&c, KeyCode::F(11), true);
        assert!(c.fullscreen_requested.load(Ordering::Acquire));
        assert!(!c.fullscreen.load(Ordering::Acquire));
        for _ in 0..10 {
            key(&c, KeyCode::F(11), true);
        }
        assert!(c.fullscreen_requested.load(Ordering::Acquire));
        c.fullscreen_request_pending.store(false, Ordering::Release);
        c.confirm_fullscreen(true);
        key(&c, KeyCode::F(11), false);
        key(&c, KeyCode::Escape, true);
        assert!(!c.fullscreen_requested.load(Ordering::Acquire));
        c.fullscreen_request_pending.store(false, Ordering::Release);
        c.confirm_fullscreen(false);
        for _ in 0..10 {
            key(&c, KeyCode::Escape, true);
        }
        assert_eq!(DISMISSALS.load(Ordering::Relaxed), 0);
        assert!(c.is_paused());
        key(&c, KeyCode::Escape, false);
        key(&c, KeyCode::Escape, true);
        assert_eq!(DISMISSALS.swap(0, Ordering::Relaxed), 1);
    }
    #[test]
    fn fullscreen_controller_focus_and_pointer_use_the_same_action() {
        let c = ControlsOverlay::new(false);
        c.update_canvas_size(768, 432);
        for _ in 0..3 {
            key(&c, KeyCode::Down, true);
        }
        assert_eq!(c.control_focus.load(Ordering::Acquire), 3);
        key(&c, KeyCode::Enter, true);
        key(&c, KeyCode::Enter, false);
        c.fullscreen_request_pending.store(false, Ordering::Release);
        c.confirm_fullscreen(true);
        c.update_canvas_size(1920, 1080);
        assert_eq!(c.control_focus.load(Ordering::Acquire), 3);
        let (x, y) = fullscreen_button_origin(1920, 1080).unwrap();
        pointer(&c, x as i32 + 10, y as i32 + 10, true);
        pointer(&c, 100, 100, false);
        assert!(c.fullscreen_requested.load(Ordering::Acquire));
        pointer(&c, x as i32 + 10, y as i32 + 10, true);
        pointer(&c, x as i32 + 10, y as i32 + 10, false);
        assert!(!c.fullscreen_requested.load(Ordering::Acquire));
        assert!(!c.is_paused());
        key(&c, KeyCode::Down, true);
        assert_eq!(c.control_focus.load(Ordering::Acquire), 0);
        key(&c, KeyCode::Up, true);
        assert_eq!(c.control_focus.load(Ordering::Acquire), 3);
    }
    #[test]
    fn fullscreen_cancel_pending_entry_and_rejection_preserve_confirmed_state() {
        let c = ControlsOverlay::new(false);
        c.last_video_pts_us.store(7_000_000);
        c.set_scrubbing(true);
        c.desired_position_us.store(9_000_000);
        c.request_fullscreen(true);
        assert!(!c.is_scrubbing());
        assert_eq!(c.desired_position_us.load(), 7_000_000);
        key(&c, KeyCode::Escape, true);
        assert!(!c.fullscreen_requested.load(Ordering::Acquire));
        assert_eq!(DISMISSALS.swap(0, Ordering::Relaxed), 0);
        c.request_fullscreen(true);
        c.fullscreen_request_pending.store(false, Ordering::Release);
        c.confirm_fullscreen(false);
        assert!(!c.fullscreen.load(Ordering::Acquire));
        assert!(!c.fullscreen_requested.load(Ordering::Acquire));
        c.request_fullscreen(true);
        c.fullscreen_request_pending.store(false, Ordering::Release);
        c.confirm_fullscreen(true);
        c.reset_for_media(false);
        assert!(c.fullscreen.load(Ordering::Acquire));
        assert!(c.fullscreen_requested.load(Ordering::Acquire));
    }
}
