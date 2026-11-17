//! Local display-layout editor. Dragging never sends live layout transactions.
use super::*;
use std::rc::Rc;
use std::cell::Cell;
use sws_protocol::outputs::{self as outputs, OutputConfiguration, OutputExperience, OutputSnapshot};
use scarlet_ui::event::{Event, MouseButton, MouseEvent, TouchPhase, TouchDragAxis, TouchGesture};

pub(super) const SCALES: &[u32] = &[1000, 1250, 1500, 1750, 2000, 2500, 3000];
const WIDTH: f32 = 544.0;
const HEIGHT: f32 = 220.0;

#[derive(Clone)]
pub(super) struct Editor {
    pub snapshot: Option<OutputSnapshot>,
    pub draft: Vec<OutputConfiguration>,
    pub selected: usize,
    pub status: String,
    pub dirty: bool,
    drag_before: Option<Vec<OutputConfiguration>>,
    drag: Option<(u32, i32, i32, i32, i32, (f32, f32, f32))>,
}
impl Default for Editor { fn default() -> Self { Self::new() } }
impl Editor {
    pub fn new() -> Self { Self { snapshot: None, draft: Vec::new(), selected: 0, status: String::from("Loading displays…"), dirty: false, drag: None, drag_before: None } }
    fn adopt(&mut self, snapshot: OutputSnapshot) {
        let selected_id = self.snapshot.as_ref().and_then(|snapshot| snapshot.outputs.get(self.selected)).map(|output| output.monitor_key.clone());
        self.selected = selected_id.and_then(|key| snapshot.outputs.iter().position(|output| output.monitor_key == key)).unwrap_or(0);
        self.draft = snapshot.configuration.clone();
        self.snapshot = Some(snapshot);
        self.dirty = false; self.drag = None; self.drag_before = None;
    }
    fn geometry(&self, viewport_width: f32) -> Option<(f32, f32, f32)> {
        // Keep the preview coordinate system fixed while the pointer moves.
        // Re-fitting to the moving card would move it away from the drag anchor.
        if let Some((_, _, _, _, _, geometry)) = self.drag { return Some(geometry); }
        let snapshot = self.snapshot.as_ref()?;
        let mut left = 0i64; let mut top = 0i64; let mut right = 1i64; let mut bottom = 1i64;
        for config in &self.draft {
            let output = snapshot.outputs.iter().find(|output| output.id == config.id)?;
            let (width, height) = output.mode.logical_size(config.scale_milli).ok()?;
            left = left.min(i64::from(config.x)); top = top.min(i64::from(config.y));
            right = right.max(i64::from(config.x) + i64::from(width)); bottom = bottom.max(i64::from(config.y) + i64::from(height));
        }
        let zoom = ((viewport_width - 48.0) / (right - left) as f32).min((HEIGHT - 48.0) / (bottom - top) as f32).max(0.001);
        Some(((viewport_width - (right - left) as f32 * zoom) * 0.5 - left as f32 * zoom,
            (HEIGHT - (bottom - top) as f32 * zoom) * 0.5 - top as f32 * zoom, zoom))
    }
    fn card(&self, index: usize, viewport_width: f32) -> Option<(f32, f32, f32, f32)> {
        let snapshot = self.snapshot.as_ref()?; let output = snapshot.outputs.get(index)?;
        let config = self.draft.iter().find(|config| config.id == output.id)?;
        let (width, height) = output.mode.logical_size(config.scale_milli).ok()?;
        let (x, y, zoom) = self.geometry(viewport_width)?;
        Some((x + config.x as f32 * zoom, y + config.y as f32 * zoom, width as f32 * zoom, height as f32 * zoom))
    }
    fn settle(&mut self, id: u32) {
        let Some(snapshot) = &self.snapshot else { return; };
        let Some(current) = self.draft.iter().find(|config| config.id == id).copied() else { return; };
        match outputs::snapped_position(&snapshot.outputs, &self.draft, id, (current.x, current.y)) {
            Ok((x, y)) => {
                if let Some(config) = self.draft.iter_mut().find(|config| config.id == id) { config.x = x; config.y = y; }
                if let Ok(canonical) = outputs::canonical_layout(&snapshot.outputs, &self.draft) { self.draft = canonical; }
                self.dirty = self.draft != snapshot.configuration;
                self.status = String::from("Ready to apply. Displays must touch at an edge or corner.");
            }
            Err(_) => { self.draft = snapshot.configuration.clone(); self.dirty = false; self.status = String::from("This arrangement would overlap or disconnect displays."); }
        }
    }
}

pub(super) fn watch(state: State<Editor>) {
    thread::spawn(move || loop {
        let Ok(connection) = Connection::connect_default() else { thread::sleep(Duration::from_secs(1)); continue; };
        let Ok(snapshot) = connection.outputs() else {
            state.update(|editor| editor.status = String::from("Display configuration is unavailable.")); return;
        };
        state.update(|editor| { editor.adopt(snapshot); editor.status = String::from("Drag displays to arrange them, then apply."); });
        loop {
            if connection.wait_for_window_events(Duration::from_millis(500)).is_err() || connection.dispatch().is_err() { break; }
            while let Some(event) = connection.poll_event() {
                if let sws_client::event::Event::Output(outputs::OutputEvent::Snapshot(snapshot)) = event {
                    if state.get().snapshot.as_ref().is_some_and(|old| old.generation == snapshot.generation) { continue; }
                    state.update(|editor| { editor.adopt(snapshot); editor.status = String::from("Display configuration updated."); });
                }
            }
        }
    });
}

pub(super) fn canvas(state: State<Editor>) -> impl View + Clone {
    let paint = state.clone();
    let viewport = Rc::new(Cell::new(WIDTH));
    let paint_viewport = viewport.clone();
    let contact = Cell::new(None);
    scarlet_ui::views::CanvasView::new(WIDTH, HEIGHT, Rc::new(move |buffer, width, height| {
        let editor = paint.get();
        let mut canvas = scarlet_ui::graphics::Canvas::new(buffer, width, height);
        canvas.fill_rect(0, 0, width, height, Color::rgb(35, 38, 45));
        let scale = height as f32 / HEIGHT;
        let viewport_width = width as f32 / scale.max(0.001);
        paint_viewport.set(viewport_width);
        let count = editor.snapshot.as_ref().map_or(0, |snapshot| snapshot.outputs.len());
        for index in 0..count {
            let Some((x, y, w, h)) = editor.card(index, viewport_width) else { continue; };
            let (x, y, w, h) = ((x * scale) as i32, (y * scale) as i32, (w * scale) as u32, (h * scale) as u32);
            let color = if index == editor.selected { Color::rgb(44, 116, 220) } else { Color::rgb(91, 101, 118) };
            canvas.fill_rect(x, y, w, h, Color::rgb(180, 192, 210));
            canvas.fill_rect(x + 2, y + 2, w.saturating_sub(4), h.saturating_sub(4), color);
            canvas.draw_text_sized(x + (w / 2) as i32 - (5.0 * scale) as i32, y + (h / 2) as i32 - (9.0 * scale) as i32,
                &format!("{}", index + 1), Color::WHITE, 18.0 * scale);
        }
    })).touch_drag_axis(TouchDragAxis::Both).on_event(move |event| {
        let viewport_width = viewport.get();
        // Canvas owns a single contact; a second finger cannot move or finish
        // the first finger's display arrangement.
        let input = match *event {
            Event::Mouse(MouseEvent::ButtonPressed { button: MouseButton::Left, x, y, .. }) => (0, x, y),
            Event::Mouse(MouseEvent::Moved { x, y }) => (1, x, y),
            Event::Mouse(MouseEvent::ButtonReleased { button: MouseButton::Left, x, y, .. }) => (2, x, y),
            Event::Mouse(MouseEvent::ButtonCancelled { .. }) => (3, 0, 0),
            Event::Touch(change) => {
                if change.phase == TouchPhase::Down && contact.get().is_none() { contact.set(Some(change.id)); }
                if contact.get() != Some(change.id) { return false; }
                let phase = match change.phase { TouchPhase::Down => 0, TouchPhase::Move => 1, TouchPhase::Up => 2, TouchPhase::Cancel => 3 };
                if phase >= 2 { contact.set(None); }
                (phase, change.x, change.y)
            }
            Event::TouchGesture(TouchGesture::Pinch { .. }) => { contact.set(None); (3, 0, 0) }
            _ => return false,
        };
        match input {
            (0, x, y) => {
                let editor = state.get();
                let count = editor.snapshot.as_ref().map_or(0, |snapshot| snapshot.outputs.len());
                for index in (0..count).rev() {
                    let Some((left, top, width, height)) = editor.card(index, viewport_width) else { continue; };
                    if x as f32 >= left && y as f32 >= top && (x as f32) < left + width && (y as f32) < top + height {
                        let id = editor.snapshot.as_ref().unwrap().outputs[index].id;
                        let config = editor.draft.iter().find(|config| config.id == id).copied().unwrap();
                        let geometry = editor.geometry(viewport_width).unwrap();
                        state.update(|editor| { editor.selected = index; editor.drag_before = Some(editor.draft.clone()); editor.drag = Some((id, x, y, config.x, config.y, geometry)); });
                        return true;
                    }
                }
                false
            }
            (1, x, y) => {
                let Some((id, start_x, start_y, origin_x, origin_y, (_, _, zoom))) = state.get().drag else { return false; };
                state.update(|editor| if let Some(config) = editor.draft.iter_mut().find(|config| config.id == id) {
                    config.x = origin_x.saturating_add(((x - start_x) as f32 / zoom) as i32);
                    config.y = origin_y.saturating_add(((y - start_y) as f32 / zoom) as i32);
                });
                true
            }
            (2, _, _) => {
                let Some((id, ..)) = state.get().drag else { return false; };
                state.update(|editor| { editor.drag = None; editor.settle(id); editor.drag_before = None; }); true
            }
            (3, _, _) => {
                if state.get().drag.is_none() { return false; }
                state.update(|editor| { editor.drag = None; if let Some(draft) = editor.drag_before.take() { editor.draft = draft; } }); true
            }
            _ => false,
        }
    }).frame_height(HEIGHT)
}

pub(super) fn apply(state: &State<Editor>) {
    let editor = state.get(); let Some(snapshot) = editor.snapshot else { return; };
    let persistent = outputs::topology_key(&snapshot.outputs).is_some();
    let result = Connection::connect_default().and_then(|connection| connection.configure_outputs(outputs::OutputTransaction { base_generation: snapshot.generation, configuration: editor.draft }));
    state.update(|editor| match result {
        Ok(snapshot) => { editor.adopt(snapshot); editor.status = String::from(if persistent { "Applied and remembered for this display combination." } else { "Applied for this session. A display has no unique identity, so this combination is not restored automatically." }); }
        Err(error) => editor.status = format!("Not applied: {:?}. Refresh if displays changed.", error),
    });
}

pub(super) fn scale(state: &State<Editor>, index: usize) {
    let Some(scale) = SCALES.get(index).copied() else { return; };
    state.update(|editor| {
        let Some(id) = editor.snapshot.as_ref().and_then(|snapshot| snapshot.outputs.get(editor.selected)).map(|output| output.id) else { return; };
        if let Some(config) = editor.draft.iter_mut().find(|config| config.id == id) { config.scale_milli = scale; }
        editor.settle(id);
    });
}
pub(super) fn experience(state: &State<Editor>, index: usize) {
    state.update(|editor| {
        let Some(id) = editor.snapshot.as_ref().and_then(|snapshot| snapshot.outputs.get(editor.selected)).map(|output| output.id) else { return; };
        if let Some(config) = editor.draft.iter_mut().find(|config| config.id == id) { config.experience = match index { 1 => OutputExperience::Desktop, 2 => OutputExperience::Tablet, _ => OutputExperience::Auto }; editor.dirty = true; }
    });
}
