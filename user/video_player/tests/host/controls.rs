#[cfg(test)]
mod tests {
    use super::*;
    use scarlet_ui::event::KeyModifiers;
    #[test]
    fn streaming_accepts_the_switch_stateless_h264_build() {
        assert_eq!(
            streaming_hardware_codec_supported(VideoCodec::H264),
            cfg!(feature = "h264-stateful-hw") || cfg!(feature = "h264-stateless-hw")
        );
        assert_eq!(
            streaming_hardware_codec_supported(VideoCodec::Hevc),
            cfg!(feature = "hevc-stateful-hw")
        );
        assert!(!streaming_hardware_codec_supported(VideoCodec::Vp9));
        assert!(!streaming_hardware_codec_supported(VideoCodec::Av1));
    }

    #[test]
    fn native_video_panel_matches_full_viewport_controls_and_hit_targets() {
        let frame = VideoFrameData {
            image: None,
            pixels: Vec::new(),
            width: 1920,
            height: 1080,
            current_frame: 60,
            total_frames: 600,
        };
        for touch in [false, true] {
            TOUCH_MODE.store(touch, Ordering::Relaxed);
            for milli in [1000, 2000] {
                graphics::set_current_scale_milli(milli);
                let scale = UiScale::current();
                let (width, height) = (1280, 720);
                let c = ControlsOverlay::new(false);
                c.update_canvas_size(width, height);
                c.set_media_duration_us(60_000_000);
                c.desired_position_us.store(6_000_000);
                c.set_buffered_position_us(30_000_000);
                c.control_focus_visible.store(true, Ordering::Release);
                let mut panel = None;
                for focus in 0..4 {
                    c.control_focus.store(focus, Ordering::Release);
                    render_controls_panel(&mut panel, width, height, &frame, &c);
                    let panel = panel.as_ref().unwrap();
                    assert_eq!(panel.logical_height(), controls_panel_height());
                    let mut reference = Buffer::from_logical_dimensions(width, height);
                    let (w, h) = (reference.width(), reference.height());
                    reference.data_mut().fill(0);
                    draw_seek_bar(
                        reference.data_mut(),
                        w,
                        h,
                        width,
                        height,
                        0,
                        &frame,
                        &c,
                        scale,
                    );
                    let start = (h - panel.height()) as usize * w as usize * 4;
                    assert_eq!(panel.data(), &reference.data()[start..]);
                    let origin_y = height - panel.logical_height();
                    for (target, (x, y)) in [
                        (1, play_pause_button_origin(width, height).unwrap()),
                        (2, loop_button_origin(width, height).unwrap()),
                        (3, fullscreen_button_origin(width, height).unwrap()),
                    ] {
                        assert_eq!(pointer_control(&c, x as i32 + 1, y as i32 + 1), target);
                        let mut bright = false;
                        for row in scale.physical_pos(y - origin_y)
                            ..scale.physical_pos(y - origin_y + control_button_size())
                        {
                            for col in
                                scale.physical_pos(x)..scale.physical_pos(x + control_button_size())
                            {
                                let at = (row * panel.width() + col) as usize * 4;
                                bright |= panel.data()[at..at + 3].iter().any(|&value| value > 100);
                            }
                        }
                        assert!(
                            bright,
                            "missing icon for target {target}, touch={touch}, scale={milli}"
                        );
                    }
                    assert_eq!(
                        panel.data()[3],
                        112,
                        "panel background must remain translucent"
                    );
                }
                c.hide();
                render_controls_panel(&mut panel, width, height, &frame, &c);
                assert!(panel.is_none());
                c.show_for_activity();
                render_controls_panel(&mut panel, 80, 60, &frame, &c);
                assert!(panel.as_ref().unwrap().data().iter().all(|&byte| byte == 0));
            }
        }
        graphics::set_current_scale_milli(1000);
        TOUCH_MODE.store(false, Ordering::Relaxed);
    }

    #[test]
    fn overlays_blend_over_transparent_and_opaque_pixels() {
        let mut transparent = [0; 4];
        blend_pixel_bgra(&mut transparent, [90, 120, 180, 112], 112);
        assert_eq!(transparent, [90, 120, 180, 112]);
        blend_pixel_bgra(&mut transparent, [0, 0, 0, 96], 96);
        assert_eq!(transparent[3], 166);
        let mut opaque = [100, 150, 200, 255];
        blend_pixel_bgra(&mut opaque, [0, 0, 0, 112], 112);
        assert_eq!(opaque, [56, 84, 112, 255]);
    }
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
    fn touch(c: &ControlsOverlay, id: u64, phase: TouchPhase, x: i32, y: i32) {
        touch_at(c, id, phase, x, y, 0);
    }
    fn touch_at(c: &ControlsOverlay, id: u64, phase: TouchPhase, x: i32, y: i32, time_ns: u64) {
        handle_canvas_event(
            &Event::Touch(TouchChange {
                seat_id: 0,
                serial: 1,
                time_ns,
                id,
                phase,
                x,
                y,
                pressure: None,
                touch_major: None,
            }),
            c,
            &PaintSignal,
        );
    }
    fn idle_until_hidden(c: &ControlsOverlay, timer: &mut ControlsAutoHide) {
        // First tick observes the latest input; the following ticks measure idle time.
        timer.tick(c);
        for _ in 0..CONTROLS_HIDE_IDLE_TICKS - 1 {
            assert!(!timer.tick(c));
            assert!(c.is_visible());
        }
        assert!(timer.tick(c));
        assert!(!c.is_visible());
    }
    #[test]
    fn controller_navigation_and_resume_do_not_pin_the_overlay() {
        let c = ControlsOverlay::new(false);
        let mut timer = ControlsAutoHide::new(&c);
        key(&c, KeyCode::Down, true);
        key(&c, KeyCode::Down, false);
        idle_until_hidden(&c, &mut timer);
        key(&c, KeyCode::Up, true);
        key(&c, KeyCode::Enter, true);
        key(&c, KeyCode::Enter, false);
        assert!(c.is_paused());
        for _ in 0..CONTROLS_HIDE_IDLE_TICKS * 2 {
            assert!(!timer.tick(&c));
        }
        key(&c, KeyCode::Enter, true);
        for _ in 0..CONTROLS_HIDE_IDLE_TICKS * 2 {
            assert!(!timer.tick(&c));
            assert!(c.is_visible());
        }
        key(&c, KeyCode::Enter, false);
        assert!(!c.is_paused());
        idle_until_hidden(&c, &mut timer);
    }
    #[test]
    fn stationary_mouse_notifications_do_not_keep_the_overlay_visible() {
        let c = ControlsOverlay::new(false);
        let mut timer = ControlsAutoHide::new(&c);
        let event = Event::Mouse(MouseEvent::Moved { x: 200, y: 100 });
        handle_canvas_event(&event, &c, &PaintSignal);
        timer.tick(&c);
        for _ in 0..CONTROLS_HIDE_IDLE_TICKS {
            handle_canvas_event(&event, &c, &PaintSignal);
            timer.tick(&c);
        }
        assert!(!c.is_visible());
        handle_canvas_event(&event, &c, &PaintSignal);
        assert!(!c.is_visible());
        handle_canvas_event(
            &Event::Mouse(MouseEvent::Moved { x: 201, y: 100 }),
            &c,
            &PaintSignal,
        );
        assert!(c.is_visible());
        idle_until_hidden(&c, &mut timer);
    }
    #[test]
    fn native_touch_reveals_without_activation_and_blank_taps_toggle_visibility() {
        // Native touch works even in Normal posture, without synthetic mouse motion.
        TOUCH_MODE.store(false, Ordering::Relaxed);
        let c = ControlsOverlay::new(false);
        c.update_canvas_size(390, 844);
        c.hide();
        let (x, y) = play_pause_button_origin(390, 844).unwrap();
        let (x, y) = (x as i32 + 5, y as i32 + 5);
        touch(&c, 1, TouchPhase::Down, x, y);
        touch(&c, 1, TouchPhase::Up, x, y);
        assert!(c.is_visible());
        assert!(!c.is_paused());
        touch(&c, 2, TouchPhase::Down, 200, 100);
        touch(&c, 2, TouchPhase::Up, 200, 100);
        assert!(!c.is_visible());
        touch(&c, 3, TouchPhase::Down, 200, 100);
        touch(&c, 3, TouchPhase::Up, 200, 100);
        assert!(c.is_visible());
        touch(&c, 4, TouchPhase::Down, x, y);
        touch(&c, 4, TouchPhase::Up, x, y);
        assert!(c.is_paused());
        touch(&c, 5, TouchPhase::Down, 200, 100);
        touch(&c, 5, TouchPhase::Up, 200, 100);
        assert!(!c.is_visible());
        assert!(c.is_paused());
    }
    #[test]
    fn touch_button_resume_and_release_restart_auto_hide() {
        let c = ControlsOverlay::new(false);
        c.update_canvas_size(640, 360);
        c.paused.store(true, Ordering::Release);
        let mut timer = ControlsAutoHide::new(&c);
        let (x, y) = play_pause_button_origin(640, 360).unwrap();
        touch(&c, 1, TouchPhase::Down, x as i32 + 5, y as i32 + 5);
        for _ in 0..CONTROLS_HIDE_IDLE_TICKS * 2 {
            assert!(!timer.tick(&c));
        }
        touch(&c, 1, TouchPhase::Up, x as i32 + 5, y as i32 + 5);
        assert!(!c.is_paused());
        idle_until_hidden(&c, &mut timer);
    }
    #[test]
    fn touch_seek_holds_overlay_until_release_and_ignores_other_contacts() {
        TOUCH_MODE.store(true, Ordering::Relaxed);
        let c = ControlsOverlay::new(false);
        c.update_canvas_size(768, 1024);
        c.set_media_duration_us(60_000_000);
        let mut timer = ControlsAutoHide::new(&c);
        let y = (1024 - seek_track_bottom_inset()) as i32;
        touch(&c, 1, TouchPhase::Down, 200, y);
        touch(&c, 2, TouchPhase::Down, 100, 100);
        touch(&c, 2, TouchPhase::Up, 100, 100);
        assert!(c.is_scrubbing());
        touch(&c, 1, TouchPhase::Move, 500, y);
        let preview = c.desired_position_us.load();
        assert!(preview > 30_000_000);
        for _ in 0..CONTROLS_HIDE_IDLE_TICKS * 2 {
            assert!(!timer.tick(&c));
        }
        touch(&c, 1, TouchPhase::Up, 500, y);
        assert!(!c.is_scrubbing());
        assert_eq!(c.current_seek_target_us(), preview);
        idle_until_hidden(&c, &mut timer);
        TOUCH_MODE.store(false, Ordering::Relaxed);
    }
    #[test]
    fn touch_cancel_resize_and_swipe_do_not_activate_or_commit() {
        let c = ControlsOverlay::new(false);
        c.update_canvas_size(640, 360);
        c.set_media_duration_us(60_000_000);
        c.last_video_pts_us.store(7_000_000);
        let mut timer = ControlsAutoHide::new(&c);
        let y = (360 - seek_track_bottom_inset()) as i32;
        touch(&c, 1, TouchPhase::Down, 200, y);
        touch(&c, 1, TouchPhase::Move, 500, y);
        touch(&c, 1, TouchPhase::Cancel, 500, y);
        assert!(!c.is_scrubbing());
        assert_eq!(c.desired_position_us.load(), 7_000_000);
        assert_eq!(c.current_seek_epoch(), 0);
        idle_until_hidden(&c, &mut timer);
        c.show_for_activity();
        touch(&c, 2, TouchPhase::Down, 200, y);
        c.update_canvas_size(768, 1024);
        touch(&c, 2, TouchPhase::Up, 500, y);
        assert_eq!(c.current_seek_epoch(), 0);
        let (x, y) = play_pause_button_origin(768, 1024).unwrap();
        let (x, y) = (x as i32 + 5, y as i32 + 5);
        touch(&c, 3, TouchPhase::Down, x, y);
        touch(&c, 3, TouchPhase::Move, 200, 100);
        touch(&c, 3, TouchPhase::Up, x, y);
        assert!(!c.is_paused());
        touch(&c, 4, TouchPhase::Down, 200, 100);
        touch(&c, 4, TouchPhase::Move, 300, 100);
        touch(&c, 4, TouchPhase::Up, 200, 100);
        assert!(c.is_visible());
    }
    #[test]
    fn redundant_fullscreen_confirmation_does_not_reveal_or_delay_overlay() {
        let c = ControlsOverlay::new(false);
        let mut timer = ControlsAutoHide::new(&c);
        for _ in 0..CONTROLS_HIDE_IDLE_TICKS {
            c.confirm_fullscreen(false);
            timer.tick(&c);
        }
        assert!(!c.is_visible());
        c.confirm_fullscreen(false);
        assert!(!c.is_visible());
        c.confirm_fullscreen(true);
        assert!(c.is_visible());
        idle_until_hidden(&c, &mut timer);
    }
    #[test]
    fn mouse_in_tablet_posture_remains_mouse_input_and_resumes_idle_hiding() {
        TOUCH_MODE.store(true, Ordering::Relaxed);
        let c = ControlsOverlay::new(false);
        c.update_canvas_size(640, 360);
        let mut timer = ControlsAutoHide::new(&c);
        pointer(&c, 200, 100, true);
        pointer(&c, 200, 100, false);
        assert!(c.is_visible());
        idle_until_hidden(&c, &mut timer);
        c.show_for_activity();
        let (x, y) = play_pause_button_origin(640, 360).unwrap();
        let (x, y) = (x as i32 + 5, y as i32 + 5);
        pointer(&c, x, y, true);
        pointer(&c, x, y, false);
        assert!(c.is_paused());
        pointer(&c, x, y, true);
        pointer(&c, x, y, false);
        assert!(!c.is_paused());
        idle_until_hidden(&c, &mut timer);
        TOUCH_MODE.store(false, Ordering::Relaxed);
    }
    #[test]
    fn touch_long_press_and_unrelated_mouse_motion_do_not_activate_controls() {
        let c = ControlsOverlay::new(false);
        c.update_canvas_size(640, 360);
        c.set_media_duration_us(60_000_000);
        touch_at(&c, 1, TouchPhase::Down, 200, 100, 0);
        touch_at(&c, 1, TouchPhase::Up, 200, 100, 700_000_000);
        assert!(c.is_visible());
        let (x, y) = play_pause_button_origin(640, 360).unwrap();
        touch_at(&c, 2, TouchPhase::Down, x as i32 + 5, y as i32 + 5, 0);
        touch_at(
            &c,
            2,
            TouchPhase::Up,
            x as i32 + 5,
            y as i32 + 5,
            700_000_000,
        );
        assert!(!c.is_paused());
        let y = (360 - seek_track_bottom_inset()) as i32;
        touch(&c, 3, TouchPhase::Down, 200, y);
        let preview = c.desired_position_us.load();
        handle_canvas_event(
            &Event::Mouse(MouseEvent::Moved { x: 500, y }),
            &c,
            &PaintSignal,
        );
        pointer(&c, 500, y, false);
        assert!(c.is_scrubbing());
        assert_eq!(c.desired_position_us.load(), preview);
        assert_eq!(c.current_seek_epoch(), 0);
        touch(&c, 3, TouchPhase::Cancel, 200, y);
        assert!(!c.is_scrubbing());
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
        assert!(c.control_focus_visible.load(Ordering::Acquire));
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
