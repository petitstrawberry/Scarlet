//! Input-panel ownership and activation lifetime, independent of IME selection.
use sws_protocol::input_panel::Context;
use sws_protocol::text_input_content_hints::NO_AUTO_SOFTWARE_KEYBOARD;
#[derive(Clone, Copy, Debug)]
pub struct Panel {
    pub provider: Option<(usize, u32)>,
    pub context: Context,
    pub visible: bool,
    explicit_show: bool,
}
impl Panel {
    pub const fn new() -> Self {
        Self {
            provider: None,
            context: Context {
                context_id: 0,
                window_id: 0,
                generation: 0,
                content_hint: 0,
                content_purpose: 0,
            },
            visible: false,
            explicit_show: false,
        }
    }
    pub fn register(&mut self, client: usize, window: u32) -> bool {
        if self.provider.is_some_and(|owner| owner != (client, window)) {
            return false;
        }
        self.provider = Some((client, window));
        true
    }
    pub fn unregister(&mut self, client: usize) -> bool {
        if !self.provider.is_some_and(|owner| owner.0 == client) {
            return false;
        }
        self.provider = None;
        self.visible = false;
        true
    }
    /// Window removal only releases the exact provider, not other windows of its client.
    pub fn unregister_window(&mut self, client: usize, window: u32) -> bool {
        if self.provider != Some((client, window)) {
            return false;
        }
        self.unregister(client)
    }
    pub fn update(&mut self, mut context: Context) -> bool {
        let new_activation = (context.context_id, context.window_id)
            != (self.context.context_id, self.context.window_id)
            || (context.content_hint ^ self.context.content_hint) & NO_AUTO_SOFTWARE_KEYBOARD != 0;
        context.generation = if new_activation {
            self.context.generation.wrapping_add(1)
        } else {
            self.context.generation
        };
        if context == self.context {
            return false;
        }
        self.context = context;
        if new_activation {
            self.visible = false;
            self.explicit_show = false;
        }
        true
    }
    /// Provider metadata includes an explicit-show override for this activation.
    /// Raw `context` retains the editor's preference for IME and later focus.
    pub fn provider_context(&self) -> Context {
        let mut delivered = self.context;
        if self.explicit_show {
            delivered.content_hint &= !NO_AUTO_SOFTWARE_KEYBOARD;
        }
        delivered
    }
    /// The caller must authenticate the editor. Explicit requests override only
    /// provider snapshots; context/window or auto-keyboard policy changes reset it.
    pub fn request_show(&mut self, context_id: u32) -> Option<Context> {
        if !self.context.active() || self.context.context_id != context_id {
            return None;
        }
        self.context.generation = self.context.generation.wrapping_add(1);
        self.explicit_show = true;
        Some(self.provider_context())
    }
    pub fn accepts(&self, client: usize, context: u32, generation: u32) -> bool {
        self.provider.is_some_and(|owner| owner.0 == client)
            && self.context.active()
            && self.context.context_id == context
            && self.context.generation == generation
    }
    pub fn show(&mut self, client: usize, context: u32, generation: u32, visible: bool) -> bool {
        if !self.accepts(client, context, generation) {
            return false;
        }
        let changed = self.visible != visible;
        self.visible = visible;
        changed
    }
}
/// Output-space panel bounds, target surface bounds, and physical/logical scale.
/// Keep the geometry in the cache as well as the resulting local intersection:
/// moves/resizes must refresh clients even when that intersection is identical.
type OcclusionTarget = (
    sws_protocol::input_panel::Occlusion,
    (i32, i32, u32, u32),
    u32,
);

#[derive(Default)]
pub(super) struct OcclusionTracker {
    target: Option<OcclusionTarget>,
}

impl OcclusionTracker {
    pub(super) fn update(
        &mut self,
        target: Option<OcclusionTarget>,
    ) -> Vec<sws_protocol::input_panel::Occlusion> {
        use sws_protocol::input_panel::Occlusion;
        if self.target == target {
            return Vec::new();
        }
        let mut events = Vec::new();
        if let Some((old, _, _)) = self.target
            && target.is_none_or(|(next, _, _)| next.window_id != old.window_id)
        {
            events.push(Occlusion {
                window_id: old.window_id,
                ..Default::default()
            });
        }
        if let Some((panel, surface, _)) = target {
            // i64 keeps signed origins plus unsigned extents from overflowing.
            let left = i64::from(panel.x).max(i64::from(surface.0));
            let top = i64::from(panel.y).max(i64::from(surface.1));
            let right = (i64::from(panel.x) + i64::from(panel.width))
                .min(i64::from(surface.0) + i64::from(surface.2));
            let bottom = (i64::from(panel.y) + i64::from(panel.height))
                .min(i64::from(surface.1) + i64::from(surface.3));
            events.push(if left < right && top < bottom {
                Occlusion {
                    window_id: panel.window_id,
                    x: (left - i64::from(surface.0)).min(i64::from(i32::MAX)) as i32,
                    y: (top - i64::from(surface.1)).min(i64::from(i32::MAX)) as i32,
                    width: (right - left) as u32,
                    height: (bottom - top) as u32,
                }
            } else {
                Occlusion {
                    window_id: panel.window_id,
                    ..Default::default()
                }
            });
        }
        self.target = target;
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sws_protocol::input_panel::Occlusion;

    fn keyboard(window_id: u32) -> Occlusion {
        Occlusion {
            window_id,
            x: 0,
            y: 600,
            width: 1200,
            height: 300,
        }
    }

    #[test]
    fn occlusion_is_clipped_to_split_and_floating_surfaces() {
        let mut tracker = OcclusionTracker::default();
        assert_eq!(
            tracker.update(Some((keyboard(1), (604, 32, 596, 868), 1000))),
            vec![Occlusion {
                window_id: 1,
                x: 0,
                y: 568,
                width: 596,
                height: 300
            }]
        );
        assert_eq!(
            tracker.update(Some((keyboard(1), (-20, 500, 400, 200), 1000))),
            vec![Occlusion {
                window_id: 1,
                x: 20,
                y: 100,
                width: 380,
                height: 100
            }]
        );
    }

    #[test]
    fn disjoint_surface_gets_clear_and_geometry_changes_resend() {
        let mut tracker = OcclusionTracker::default();
        let clear = Occlusion {
            window_id: 1,
            ..Default::default()
        };
        assert_eq!(
            tracker.update(Some((keyboard(1), (0, 32, 600, 400), 1000))),
            vec![clear]
        );
        assert!(
            tracker
                .update(Some((keyboard(1), (0, 32, 600, 400), 1000)))
                .is_empty()
        );
        assert_eq!(
            tracker.update(Some((keyboard(1), (0, 40, 600, 400), 1000))),
            vec![clear]
        );
        assert_eq!(
            tracker.update(Some((keyboard(1), (0, 40, 600, 400), 2000))),
            vec![clear]
        );
        assert_eq!(
            tracker.update(Some((keyboard(1), (0, 40, 600, 700), 2000)))[0].height,
            140
        );
    }

    #[test]
    fn target_handoff_clears_old_editor_before_new_editor_and_hide_clears_once() {
        let mut tracker = OcclusionTracker::default();
        tracker.update(Some((keyboard(1), (0, 32, 1200, 868), 1000)));
        let events = tracker.update(Some((keyboard(2), (100, 550, 400, 200), 1000)));
        assert_eq!(
            events[0],
            Occlusion {
                window_id: 1,
                ..Default::default()
            }
        );
        assert_eq!(
            events[1],
            Occlusion {
                window_id: 2,
                x: 0,
                y: 50,
                width: 400,
                height: 150
            }
        );
        assert_eq!(
            tracker.update(None),
            vec![Occlusion {
                window_id: 2,
                ..Default::default()
            }]
        );
        assert!(tracker.update(None).is_empty());
    }

    #[test]
    fn extreme_output_origins_do_not_wrap_the_intersection() {
        let mut tracker = OcclusionTracker::default();
        let panel = Occlusion {
            window_id: 1,
            x: i32::MAX - 10,
            y: 0,
            width: 100,
            height: 20,
        };
        assert_eq!(
            tracker.update(Some((panel, (i32::MAX - 5, 0, 10, 10), 1000))),
            vec![Occlusion {
                window_id: 1,
                x: 0,
                y: 0,
                width: 10,
                height: 10
            }]
        );
    }

    fn editor(id: u32) -> Context {
        Context {
            context_id: id,
            window_id: id + 10,
            ..Context::default()
        }
    }
    #[test]
    fn automatic_keyboard_policy_changes_invalidate_keys_but_keep_text_context() {
        let mut panel = Panel::new();
        panel.register(1, 50);
        panel.update(editor(3));
        let initial = panel.context;
        panel.show(1, 3, initial.generation, true);
        let mut suppressed = editor(3);
        suppressed.content_hint = NO_AUTO_SOFTWARE_KEYBOARD;
        assert!(panel.update(suppressed));
        assert!(!panel.visible);
        assert!(panel.context.active());
        assert_eq!(panel.context.context_id, initial.context_id);
        assert!(!panel.accepts(1, 3, initial.generation));
        let generation = panel.context.generation;
        assert!(panel.accepts(1, 3, generation));
        assert!(!panel.update(suppressed));
        assert!(panel.update(editor(3)));
        assert!(!panel.accepts(1, 3, generation));
        assert_eq!(panel.context.content_hint, 0);
    }

    #[test]
    fn explicit_keyboard_show_only_clears_the_delivered_suppression_hint() {
        let mut panel = Panel::new();
        panel.register(1, 50);
        assert!(panel.request_show(0).is_none());
        let mut suppressed = editor(3);
        suppressed.content_hint = NO_AUTO_SOFTWARE_KEYBOARD
            | sws_protocol::text_input_content_hints::SENSITIVE_DATA;
        panel.update(suppressed);
        let generation = panel.context.generation;
        assert!(panel.request_show(4).is_none());
        assert_eq!(panel.context.generation, generation);
        let delivered = panel.request_show(3).unwrap();
        assert_eq!(
            delivered.content_hint,
            sws_protocol::text_input_content_hints::SENSITIVE_DATA
        );
        assert_eq!(panel.context.content_hint, suppressed.content_hint);
        assert!(!panel.accepts(1, 3, generation));
        assert!(panel.show(1, 3, delivered.generation, true));
        // Ordinary text commits do not immediately dismiss an explicitly opened panel.
        assert!(!panel.update(suppressed));
        assert!(panel.visible);
        // Other metadata changes must retain the explicit override as well.
        suppressed.content_purpose = sws_protocol::text_input_content_purpose::NUMBER;
        assert!(panel.update(suppressed));
        assert_eq!(
            panel.provider_context().content_hint,
            sws_protocol::text_input_content_hints::SENSITIVE_DATA
        );
        assert_eq!(panel.context.content_hint, suppressed.content_hint);
        // Changing the per-field policy clears the override even on the same context.
        let mut allowed = suppressed;
        allowed.content_hint &= !NO_AUTO_SOFTWARE_KEYBOARD;
        panel.update(allowed);
        panel.update(suppressed);
        assert_eq!(panel.provider_context().content_hint, suppressed.content_hint);
        // The next focused editor still has its own original policy.
        panel.update(editor(4));
        panel.update(suppressed);
        assert!(!panel.visible);
        assert_eq!(panel.context.content_hint, suppressed.content_hint);
        assert_eq!(panel.provider_context().content_hint, suppressed.content_hint);
    }
    #[test]
    fn exclusive_provider_and_stale_keys() {
        let mut panel = Panel::new();
        assert!(panel.register(1, 50));
        assert!(!panel.register(2, 51));
        panel.update(editor(3));
        let old = panel.context;
        assert!(panel.accepts(1, 3, old.generation));
        assert!(!panel.accepts(2, 3, old.generation));
        panel.show(1, 3, old.generation, true);
        assert!(panel.visible);
        panel.update(Context::default());
        assert!(!panel.visible);
        assert!(!panel.accepts(1, 3, old.generation));
        panel.update(editor(3));
        assert!(!panel.accepts(1, 3, old.generation));
    }
    #[test]
    fn editor_updates_keep_activation_but_disconnect_hides_panel() {
        let mut panel = Panel::new();
        panel.register(1, 50);
        panel.update(editor(3));
        let generation = panel.context.generation;
        panel.show(1, 3, generation, true);
        let mut state = editor(3);
        state.content_purpose = 8;
        panel.update(state);
        assert!(panel.visible);
        assert_eq!(panel.context.generation, generation);
        assert!(!panel.unregister(2));
        assert!(panel.unregister(1));
        assert!(!panel.visible);
        assert!(!panel.accepts(1, 3, generation));
        assert!(panel.register(2, 51));
    }
    #[test]
    fn queued_registration_is_released_by_later_lifecycle_events() {
        let mut panel = Panel::new();
        // Register was queued before the IPC thread observed disconnect.
        assert!(panel.register(1, 50));
        panel.update(editor(3));
        let context = panel.context;
        panel.show(1, context.context_id, context.generation, true);
        assert!(panel.unregister(1)); // compositor's later ClientDisconnected
        assert!(!panel.visible);
        assert!(!panel.accepts(1, context.context_id, context.generation));
        assert!(panel.register(2, 60));
        assert!(!panel.unregister_window(2, 61));
        assert!(!panel.unregister_window(1, 60));
        assert_eq!(panel.provider, Some((2, 60)));
        assert!(panel.unregister_window(2, 60)); // later DestroyWindow
        assert!(panel.register(3, 70));
    }
}
