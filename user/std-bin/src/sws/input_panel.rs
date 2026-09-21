//! Input-panel ownership and activation lifetime, independent of IME selection.
use sws_protocol::input_panel::Context;
#[derive(Clone, Copy, Debug)]
pub struct Panel {
    pub provider: Option<(usize, u32)>,
    pub context: Context,
    pub visible: bool,
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
    pub fn update(&mut self, mut context: Context) -> bool {
        let new_activation = (context.context_id, context.window_id)
            != (self.context.context_id, self.context.window_id);
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
        }
        true
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
#[cfg(test)]
mod tests {
    use super::*;
    fn editor(id: u32) -> Context {
        Context {
            context_id: id,
            window_id: id + 10,
            ..Context::default()
        }
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
}
