//! Local display-layout editor. Dragging never sends live layout transactions.
pub(super) use super::settings_display_editor::{Editor, SCALES};
use super::settings_display_editor::{HEIGHT, WIDTH};
use super::*;
use scarlet_ui::event::{Event, MouseButton, MouseEvent, TouchDragAxis, TouchGesture, TouchPhase};
use std::cell::Cell;
use std::rc::Rc;
use sws_protocol::outputs::{self as outputs, OutputSnapshot};

fn mode_catalog(
    connection: &Connection,
    snapshot: &OutputSnapshot,
) -> Vec<(u32, Vec<outputs::OutputMode>)> {
    snapshot
        .outputs
        .iter()
        .map(|output| {
            (
                output.id,
                connection
                    .output_modes(output.id)
                    .unwrap_or_else(|_| vec![output.mode]),
            )
        })
        .collect()
}
pub(super) fn watch(state: State<Editor>) {
    thread::spawn(move || {
        loop {
            let Ok(connection) = Connection::connect_default() else {
                thread::sleep(Duration::from_secs(1));
                continue;
            };
            let Ok(snapshot) = connection.outputs() else {
                state.update(|editor| {
                    editor.status = String::from("Display configuration is unavailable.")
                });
                return;
            };
            let supports_roles = connection
                .get_capabilities()
                .is_ok_and(|c| c.supports_output_roles());
            let modes = mode_catalog(&connection, &snapshot);
            state.update(|editor| {
                editor.adopt(snapshot);
                editor.supports_roles = supports_roles;
                editor.modes = modes;
                editor.status.clear();
            });
            loop {
                if connection
                    .wait_for_window_events(Duration::from_millis(500))
                    .is_err()
                    || connection.dispatch().is_err()
                {
                    break;
                }
                while let Some(event) = connection.poll_event() {
                    if let sws_client::event::Event::Output(outputs::OutputEvent::Snapshot(
                        snapshot,
                    )) = event
                    {
                        if state
                            .get()
                            .snapshot
                            .as_ref()
                            .is_some_and(|old| old.generation > snapshot.generation)
                        {
                            continue;
                        }
                        let modes = mode_catalog(&connection, &snapshot);
                        state.update(|editor| {
                            if editor
                                .snapshot
                                .as_ref()
                                .is_some_and(|old| old.generation > snapshot.generation)
                            {
                                return;
                            }
                            editor.observe(snapshot);
                            editor.modes = modes;
                        });
                    }
                }
            }
        }
    });
}

pub(super) fn canvas(state: State<Editor>) -> impl View + Clone {
    let paint = state.clone();
    let viewport = Rc::new(Cell::new(WIDTH));
    let paint_viewport = viewport.clone();
    scarlet_ui::views::CanvasView::new(
        WIDTH,
        HEIGHT,
        Rc::new(move |buffer, width, height| {
            let editor = paint.get();
            let mut canvas = scarlet_ui::graphics::Canvas::new(buffer, width, height);
            canvas.fill_rect(0, 0, width, height, Color::rgb(35, 38, 45));
            let scale = height as f32 / HEIGHT;
            let viewport_width = width as f32 / scale.max(0.001);
            paint_viewport.set(viewport_width);
            if let Some((origin_x, origin_y, zoom)) = editor.geometry(viewport_width) {
                let guide_color = Color::rgb(113, 206, 255);
                if let Some(snap) = editor.drag_snap.0 {
                    let x = libm::roundf((origin_x + snap.guide as f32 * zoom) * scale) as i32;
                    canvas.fill_rect(
                        x,
                        (12.0 * scale) as i32,
                        libm::ceilf(scale) as u32,
                        height.saturating_sub((24.0 * scale) as u32),
                        guide_color,
                    );
                }
                if let Some(snap) = editor.drag_snap.1 {
                    let y = libm::roundf((origin_y + snap.guide as f32 * zoom) * scale) as i32;
                    canvas.fill_rect(
                        (12.0 * scale) as i32,
                        y,
                        width.saturating_sub((24.0 * scale) as u32),
                        libm::ceilf(scale) as u32,
                        guide_color,
                    );
                }
            }
            let Some(snapshot) = &editor.snapshot else {
                return;
            };
            let selected = editor.selected_desktop();
            for (index, output) in snapshot.outputs.iter().enumerate() {
                let Some((x, y, w, h)) = editor.card(index, viewport_width) else {
                    continue;
                };
                let (x, y, w, h) = (
                    (x * scale) as i32,
                    (y * scale) as i32,
                    (w * scale) as u32,
                    (h * scale) as u32,
                );
                let color = if Some(output.id) == selected {
                    Color::rgb(44, 116, 220)
                } else {
                    Color::rgb(91, 101, 118)
                };
                canvas.fill_rect(x, y, w, h, Color::rgb(180, 192, 210));
                canvas.fill_rect(
                    x + 2,
                    y + 2,
                    w.saturating_sub(4),
                    h.saturating_sub(4),
                    color,
                );
                let label = editor.card_label(index);
                let font_size = 18.0 * scale;
                let (tw, th) = scarlet_ui::measure_text_sized(&label, font_size);
                canvas.draw_text_sized_clipped(
                    x + (w as i32 - tw as i32) / 2,
                    y + (h as i32 - th as i32) / 2,
                    &label,
                    Color::WHITE,
                    font_size,
                    Rect::from_xywh(x as f32, y as f32, w as f32, h as f32),
                    0.0,
                );
            }
        }),
    )
    .touch_drag_axis(TouchDragAxis::Both)
    .on_event(move |event| {
        if state.get().applying {
            return false;
        }
        let viewport_width = viewport.get();
        // Canvas owns a single contact; a second finger cannot move or finish
        // the first finger's display arrangement.
        let owner = state.get().touch_contact;
        if matches!(event, Event::Mouse(_)) && owner.is_some() {
            return false;
        }
        let touch_contact = match *event {
            Event::Touch(change) => Some((change.seat_id, change.id)),
            _ => None,
        };
        let input = match *event {
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                x,
                y,
                ..
            }) => (0, x, y),
            Event::Mouse(MouseEvent::Moved { x, y }) => (1, x, y),
            Event::Mouse(MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                x,
                y,
                ..
            }) => (2, x, y),
            Event::Mouse(MouseEvent::ButtonCancelled { .. }) => (3, 0, 0),
            Event::Touch(change) => {
                if change.phase == TouchPhase::Down {
                    if owner.is_some() {
                        return false;
                    }
                } else if owner != touch_contact {
                    return false;
                }
                let phase = match change.phase {
                    TouchPhase::Down => 0,
                    TouchPhase::Move => 1,
                    TouchPhase::Up => 2,
                    TouchPhase::Cancel => 3,
                };
                (phase, change.x, change.y)
            }
            Event::TouchGesture(TouchGesture::Pinch {
                first_id,
                second_id,
                ..
            }) if owner.is_some_and(|(_, id)| id == first_id || id == second_id) => (3, 0, 0),
            _ => return false,
        };
        match input {
            (0, x, y) => {
                let editor = state.get();
                let count = editor
                    .snapshot
                    .as_ref()
                    .map_or(0, |snapshot| snapshot.outputs.len());
                for index in (0..count).rev() {
                    let Some((left, top, width, height)) = editor.card(index, viewport_width)
                    else {
                        continue;
                    };
                    if x as f32 >= left
                        && y as f32 >= top
                        && (x as f32) < left + width
                        && (y as f32) < top + height
                    {
                        let id = editor.snapshot.as_ref().unwrap().outputs[index].id;
                        let config = editor
                            .draft
                            .iter()
                            .find(|config| config.id == id)
                            .copied()
                            .unwrap();
                        let geometry = editor.geometry(viewport_width).unwrap();
                        state.update(|editor| {
                            editor.selected = index;
                            if !editor.has_other_desktop(id) {
                                return;
                            }
                            editor.touch_contact = touch_contact;
                            editor.drag_snap = (None, None);
                            editor.drag_before = Some(editor.draft.clone());
                            editor.drag = Some((id, x, y, config.x, config.y, geometry));
                        });
                        return true;
                    }
                }
                false
            }
            (1, x, y) => {
                let Some((id, start_x, start_y, origin_x, origin_y, (_, _, zoom))) =
                    state.get().drag
                else {
                    return false;
                };
                let desired = (
                    origin_x.saturating_add(((x - start_x) as f32 / zoom) as i32),
                    origin_y.saturating_add(((y - start_y) as f32 / zoom) as i32),
                );
                state.update(|editor| editor.move_preview(id, desired, zoom));
                true
            }
            (2, _, _) => {
                let Some((id, ..)) = state.get().drag else {
                    return false;
                };
                state.update(|editor| {
                    editor.drag = None;
                    editor.touch_contact = None;
                    editor.drag_snap = (None, None);
                    editor.settle(id);
                    editor.drag_before = None;
                });
                true
            }
            (3, _, _) => {
                if state.get().drag.is_none() {
                    return false;
                }
                state.update(|editor| {
                    editor.drag = None;
                    editor.touch_contact = None;
                    editor.drag_snap = (None, None);
                    if let Some(draft) = editor.drag_before.take() {
                        editor.draft = draft;
                    }
                });
                true
            }
            _ => false,
        }
    })
    .frame_height(HEIGHT)
}

pub(super) fn apply(state: &State<Editor>) {
    let mut pending = None;
    state.update(|editor| pending = editor.begin_apply());
    let Some(changes) = pending else { return };
    let original = changes.original;
    let state = state.clone();
    thread::spawn(move || {
        let result = (|| -> core::result::Result<OutputSnapshot, String> {
            let connection = Connection::connect_default().map_err(|e| format!("{e:?}"))?;
            let read = || connection.outputs().map_err(|e| format!("{e:?}"));
            let mut live = read()?;
            if live.generation != original.generation {
                return Err("Displays changed before applying".into());
            }
            for (id, mode) in &changes.modes {
                connection
                    .set_output_mode(*id, *mode)
                    .map_err(|e| format!("{e:?}"))?;
            }
            // The driver acknowledges a request before the new timing is live.
            // Validate layout against the confirmed modes, including fractional scales.
            let start = scarlet_os::time::monotonic_time_ns();
            while !changes
                .modes
                .iter()
                .all(|(id, mode)| live.outputs.iter().any(|o| o.id == *id && o.mode == *mode))
            {
                if scarlet_os::time::monotonic_time_ns().saturating_sub(start) > 10_000_000_000 {
                    return Err("Display did not confirm the selected resolution".into());
                }
                thread::sleep(Duration::from_millis(100));
                live = read()?;
                if live.outputs.len() != original.outputs.len()
                    || !original.outputs.iter().all(|old| {
                        live.outputs.iter().any(|o| {
                            o.id == old.id
                                && o.connector_key == old.connector_key
                                && o.monitor_key == old.monitor_key
                        })
                    })
                {
                    return Err("Connected displays changed".into());
                }
                // Modesetting may move displays, but preserves their preferences.
                // Avoid resubmitting an old preference under the new generation.
                if !original.configuration.iter().all(|old| {
                    live.configuration.iter().any(|current| {
                        current.id == old.id
                            && current.role == old.role
                            && current.experience == old.experience
                            && current.scale_milli == old.scale_milli
                    })
                }) {
                    return Err("Display preferences changed while setting resolution".into());
                }
            }
            connection
                .configure_outputs(outputs::OutputTransaction {
                    base_generation: live.generation,
                    configuration: changes.configuration,
                })
                .map_err(|e| format!("{e:?}"))
        })();
        let status = match &result {
            Ok(_) => String::new(),
            Err(e) => format!("Display settings: {e}."),
        };
        let snapshot = result
            .ok()
            .or_else(|| Connection::connect_default().ok()?.outputs().ok());
        state.update(|editor| editor.finish_apply(snapshot, status));
    });
}
