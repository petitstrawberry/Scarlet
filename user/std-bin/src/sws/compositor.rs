//! Compositor module - manages window composition and rendering

use super::config;
use super::cursor::Cursor;
use super::cursor_theme::CursorTheme;
use super::damage::{DamageRect, PresentDamage, WindowGeometrySnapshot, changed_geometry_damage};
use super::gpu_compositor::{GpuCompositor, SgfxBufferError, SgfxBufferIdentity, SgfxCommitToken};
use super::input::{
    CompositorInputEvent, ConsumedKeys, GestureEvent, GestureRecognizer, HeldKeys, InputManager,
    KeyRepeatState, KeyboardSource, ModifierTapState, PointerSource, TOUCH_COORD_MAX, TouchFrame,
    TouchPolicyEvent, forward_to_binary_key_protocol, is_initial_press, is_physical_key_value,
    key_codes,
};
use super::input_environment;
use super::ipc::{IpcEvent, IpcServer, send_message_to_client, send_response_to_client};
use super::pointer_lock::{
    CorrelatedReply, PointerInteractionState, PointerLockDenial, PointerLockState, captured_window,
    confirmed_lock_state, correlated_reply, cursor_visible, input_route, validate_request,
};
use super::remote::capture::CaptureSession;
use super::remote::server::{RemoteEvent, RemoteServer};
use super::window::{
    PresentationInstance, PresentationTransform, WindowManager, WindowType, maximized_geometry_for,
    rounded_rect_contains_point, rounded_rect_row_span,
};
use core::sync::atomic::{AtomicU8, Ordering};
use core::time::Duration;
use framebuffer::{DisplayPresentRegion, DisplaySurface};
use scarlet_os::handle::Handle;
use scarlet_os::handle::capability::memory_mapping::munmap;
use scarlet_os::poll::{POLLIN, PollHandle, poll};
use scarlet_os::time::monotonic_time_ns;
use std::env;
use std::println;
use std::string::String;
use std::thread;
use std::vec::Vec;
use sws_protocol;

pub(super) fn is_sws_debug_enabled() -> bool {
    static LOG_CACHE: AtomicU8 = AtomicU8::new(u8::MAX);
    let cached = LOG_CACHE.load(Ordering::Relaxed);
    if cached != u8::MAX {
        return cached != 0;
    }
    let enabled = match env::var("SWS_LOG").ok() {
        Some(val) => matches!(
            val.as_str(),
            "debug" | "DEBUG" | "3" | "trace" | "TRACE" | "4"
        ),
        None => false,
    };
    LOG_CACHE.store(enabled as u8, Ordering::Relaxed);
    enabled
}

fn overview_workspace_region_for(
    workarea: (i32, i32, u32, u32),
    tablet_mode: bool,
    scale_milli: u32,
    presentation: sws_protocol::workspace::ShellPresentation,
) -> (i32, i32, u32, u32) {
    let (x, y, width, height) = workarea;
    let total_height = sws_protocol::workspace::workspace_region_height(
        height,
        tablet_mode,
        presentation,
        scale_milli,
    );
    (x, y, width, total_height)
}

fn intersect_compositor_rects(
    first: (i32, i32, u32, u32),
    second: (i32, i32, u32, u32),
) -> Option<(i32, i32, u32, u32)> {
    let left = i64::from(first.0).max(i64::from(second.0));
    let top = i64::from(first.1).max(i64::from(second.1));
    let right = i64::from(first.0)
        .saturating_add(i64::from(first.2))
        .min(i64::from(second.0).saturating_add(i64::from(second.2)));
    let bottom = i64::from(first.1)
        .saturating_add(i64::from(first.3))
        .min(i64::from(second.1).saturating_add(i64::from(second.3)));
    if right <= left || bottom <= top {
        return None;
    }
    Some((
        left.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        top.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        u32::try_from(right - left).unwrap_or(u32::MAX),
        u32::try_from(bottom - top).unwrap_or(u32::MAX),
    ))
}

fn presentation_instance_contains_point(instance: &PresentationInstance, px: i32, py: i32) -> bool {
    if let Some(clip) = instance.clip
        && !rounded_rect_contains_point(clip, instance.clip_radius, px, py)
    {
        return false;
    }
    let transform = instance.transform;
    px >= transform.x
        && px < transform.x.saturating_add(transform.width as i32)
        && py >= transform.y
        && py < transform.y.saturating_add(transform.height as i32)
}

fn layout_overview_cards(
    workarea: (i32, i32, u32, u32),
    tablet_mode: bool,
    scale_milli: u32,
    presentation: sws_protocol::workspace::ShellPresentation,
    workspace_ids: &[u32],
    active_workspace: u32,
) -> Vec<(u32, (i32, i32, u32, u32))> {
    if workspace_ids.is_empty() {
        return Vec::new();
    }
    let (_, _, work_width, work_height) = workarea;
    let (region_x, region_y, region_width, region_height) =
        overview_workspace_region_for(workarea, tablet_mode, scale_milli, presentation);
    let scale_milli = scale_milli.max(1);
    let scale_length = |logical: u32| {
        ((u64::from(logical) * u64::from(scale_milli)) / 1000)
            .max(1)
            .min(u64::from(u32::MAX)) as u32
    };
    let compact = !tablet_mode
        || matches!(
            presentation,
            sws_protocol::workspace::ShellPresentation::Home
        );
    let target_width = scale_length(if compact { 170 } else { 320 });
    let active_index = workspace_ids
        .iter()
        .position(|workspace_id| *workspace_id == active_workspace)
        .unwrap_or(0);

    let gap = if compact {
        (region_width / 128).clamp(scale_length(8), scale_length(14))
    } else {
        (region_width / 80).clamp(scale_length(12), scale_length(28))
    };
    let maximum_card_width = scale_length(if compact { 176 } else { 620 });
    let preferred_pitch = target_width.saturating_add(gap).max(1);
    // The rail is a full-width horizontal ScrollView. Choose a pitch whose
    // snap points put the next card centre close to either viewport edge. A
    // clipped neighbour therefore remains legible without manufacturing
    // special one-off rectangles at the sides.
    let maximum_edge_steps = if compact { 5 } else { 3 };
    let edge_steps = ((u64::from(region_width).saturating_add(u64::from(preferred_pitch)))
        / u64::from(preferred_pitch.saturating_mul(2).max(1)))
    .clamp(1, maximum_edge_steps) as u32;
    let pitch = (region_width / edge_steps.saturating_mul(2).max(1)).max(1);
    let effective_gap = gap.min(pitch.saturating_sub(1));
    let mut card_width = target_width
        .min(maximum_card_width)
        .min(pitch.saturating_sub(effective_gap).max(1))
        .min(region_width.max(1));
    let mut card_height = ((u64::from(card_width) * u64::from(work_height.max(1)))
        / u64::from(work_width.max(1))) as u32;
    let maximum_card_height = region_height
        .saturating_sub(scale_length(if compact { 20 } else { 32 }))
        .max(1);
    if card_height > maximum_card_height {
        card_height = maximum_card_height;
        card_width = ((u64::from(card_height) * u64::from(work_width.max(1)))
            / u64::from(work_height.max(1))) as u32;
    }
    card_width = card_width.max(1);
    card_height = card_height.max(1);

    let row_y = region_y.saturating_add(
        region_height
            .saturating_sub(card_height)
            .checked_div(2)
            .unwrap_or(0) as i32,
    );
    let item_count = u64::try_from(workspace_ids.len()).unwrap_or(u64::MAX);
    let viewport_width = u64::from(region_width);
    // This is the compositor equivalent of a full-width horizontal
    // ScrollView containing one fixed-pitch HStack. The extra pitch split
    // across both ends is scroll content padding; `max` is the HStack's
    // min-width alignment, not a second layout mode.
    let intrinsic_content_width = u64::from(pitch).saturating_mul(item_count.saturating_add(1));
    let scroll_content_width = intrinsic_content_width.max(viewport_width);
    let content_alignment = scroll_content_width.saturating_sub(intrinsic_content_width) / 2;
    let maximum_scroll = scroll_content_width.saturating_sub(viewport_width);
    let active_center = content_alignment.saturating_add(
        u64::from(pitch).saturating_mul(
            u64::try_from(active_index)
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        ),
    );
    let scroll_offset = active_center
        .saturating_sub(viewport_width / 2)
        .min(maximum_scroll);
    let viewport_left = i64::from(region_x);
    let viewport_right = viewport_left.saturating_add(i64::from(region_width));
    let half_card_width = i64::from(card_width / 2);

    workspace_ids
        .iter()
        .enumerate()
        .filter_map(|(index, workspace_id)| {
            let center = viewport_left
                .saturating_add(i64::try_from(content_alignment).unwrap_or(i64::MAX))
                .saturating_add(
                    i64::from(pitch)
                        .saturating_mul(i64::try_from(index).unwrap_or(i64::MAX).saturating_add(1)),
                )
                .saturating_sub(i64::try_from(scroll_offset).unwrap_or(i64::MAX));
            let x = center.saturating_sub(half_card_width);
            let right = x.saturating_add(i64::from(card_width));
            if right <= viewport_left || x >= viewport_right {
                return None;
            }
            Some((
                *workspace_id,
                (
                    x.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
                    row_y,
                    card_width,
                    card_height,
                ),
            ))
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OverviewSpreadSlot {
    window_id: u32,
    rect: (i32, i32, u32, u32),
}

type OverviewShadowLayer = ((i32, i32, u32, u32), u32, [u8; 4]);

fn push_overview_shadow_layers(
    layers: &mut Vec<OverviewShadowLayer>,
    rect: (i32, i32, u32, u32),
    radius: u32,
    strong: bool,
) {
    // A small stack of low-opacity layers approximates a soft elevation
    // shadow on both render paths. Keeping the outer layers very faint avoids
    // the hard halo produced by the former two-rectangle implementation.
    let short_side = rect.2.min(rect.3);
    let maximum_spread = if strong {
        (short_side / 42).clamp(10, 16)
    } else {
        (short_side / 56).clamp(6, 12)
    };
    let vertical_offset = if strong {
        (maximum_spread / 4).max(2)
    } else {
        (maximum_spread / 5).max(1)
    };
    let alphas = if strong {
        [2u8, 3, 4, 5, 7, 10]
    } else {
        [1u8, 2, 3, 4, 5, 7]
    };
    let layer_count = alphas.len() as u32;

    for (index, alpha) in alphas.into_iter().enumerate() {
        let index = index as u32;
        let spread = maximum_spread.saturating_sub(
            maximum_spread.saturating_sub(1).saturating_mul(index) / layer_count.saturating_sub(1),
        );
        let x = rect.0.saturating_sub(spread as i32);
        let y = rect
            .1
            .saturating_sub(spread as i32)
            .saturating_add(vertical_offset as i32);
        layers.push((
            (
                x,
                y,
                rect.2.saturating_add(spread.saturating_mul(2)),
                rect.3.saturating_add(spread.saturating_mul(2)),
            ),
            radius.saturating_add(spread),
            [0, 0, 0, alpha],
        ));
    }
}

fn needs_overview_fallback_shadow(insets: sws_protocol::WindowGeometryInsets) -> bool {
    insets.horizontal() == 0 && insets.vertical() == 0
}

fn fit_overview_rect(source: (u32, u32), bounds: (u32, u32)) -> (u32, u32) {
    let source_width = source.0.max(1);
    let source_height = source.1.max(1);
    let bound_width = bounds.0.max(1);
    let bound_height = bounds.1.max(1);
    let fitted = if u64::from(source_width) * u64::from(bound_height)
        > u64::from(source_height) * u64::from(bound_width)
    {
        (
            bound_width,
            ((u64::from(source_height) * u64::from(bound_width)) / u64::from(source_width)).max(1)
                as u32,
        )
    } else {
        (
            ((u64::from(source_width) * u64::from(bound_height)) / u64::from(source_height)).max(1)
                as u32,
            bound_height,
        )
    };
    (fitted.0.min(source_width), fitted.1.min(source_height))
}

fn laptop_overview_window_stage_for(
    workarea: (i32, i32, u32, u32),
    workspace_region: (i32, i32, u32, u32),
    scale_milli: u32,
) -> (i32, i32, u32, u32) {
    let scale_milli = scale_milli.max(1);
    let scale_length = |logical: u32| {
        ((u64::from(logical) * u64::from(scale_milli)) / 1000)
            .max(1)
            .min(u64::from(u32::MAX)) as u32
    };
    let horizontal_margin = scale_length(40).min(workarea.2 / 4);
    let region_gap = scale_length(sws_protocol::workspace::OVERVIEW_REGION_GAP);
    let drawer_reserve =
        scale_length(sws_protocol::workspace::DRAWER_SHEET_LIP_HEIGHT).saturating_add(region_gap);
    let x = workarea.0.saturating_add(horizontal_margin as i32);
    let y = workspace_region
        .1
        .saturating_add(workspace_region.3 as i32)
        .saturating_add(region_gap as i32);
    let work_bottom = workarea.1.saturating_add(workarea.3 as i32);
    let bottom = work_bottom.saturating_sub(drawer_reserve as i32);
    (
        x,
        y,
        workarea
            .2
            .saturating_sub(horizontal_margin.saturating_mul(2))
            .max(1),
        bottom.saturating_sub(y).max(1) as u32,
    )
}

fn layout_overview_window_spread(
    stage: (i32, i32, u32, u32),
    scale_milli: u32,
    windows: &[(u32, u32, u32)],
) -> Vec<OverviewSpreadSlot> {
    if windows.is_empty() {
        return Vec::new();
    }
    let gap = ((u64::from(24u32) * u64::from(scale_milli.max(1))) / 1000)
        .max(1)
        .min(u64::from(u32::MAX)) as u32;
    let mut best_columns = 1usize;
    let mut best_score = 0u64;
    let mut best_aspect_error = u64::MAX;
    for columns in 1..=windows.len() {
        let rows = windows.len().div_ceil(columns);
        let cell_width = stage
            .2
            .saturating_sub(gap.saturating_mul(columns.saturating_sub(1) as u32))
            / columns as u32;
        let cell_height = stage
            .3
            .saturating_sub(gap.saturating_mul(rows.saturating_sub(1) as u32))
            / rows as u32;
        if cell_width == 0 || cell_height == 0 {
            continue;
        }
        let score = windows.iter().fold(0u64, |score, (_, width, height)| {
            let fitted = fit_overview_rect((*width, *height), (cell_width, cell_height));
            score.saturating_add(u64::from(fitted.0) * u64::from(fitted.1))
        });
        let aspect_error = (u64::try_from(columns)
            .unwrap_or(u64::MAX)
            .saturating_mul(u64::from(stage.3)))
        .abs_diff(
            u64::try_from(rows)
                .unwrap_or(u64::MAX)
                .saturating_mul(u64::from(stage.2)),
        );
        if score > best_score || score == best_score && aspect_error < best_aspect_error {
            best_score = score;
            best_aspect_error = aspect_error;
            best_columns = columns;
        }
    }

    let rows = windows.len().div_ceil(best_columns);
    let cell_width = stage
        .2
        .saturating_sub(gap.saturating_mul(best_columns.saturating_sub(1) as u32))
        / best_columns as u32;
    let cell_height = stage
        .3
        .saturating_sub(gap.saturating_mul(rows.saturating_sub(1) as u32))
        / rows as u32;
    let mut slots = Vec::with_capacity(windows.len());
    for (index, (window_id, width, height)) in windows.iter().copied().enumerate() {
        let row = index / best_columns;
        let column = index % best_columns;
        let row_start = row.saturating_mul(best_columns);
        let row_count = windows.len().saturating_sub(row_start).min(best_columns);
        let row_width = cell_width
            .saturating_mul(row_count as u32)
            .saturating_add(gap.saturating_mul(row_count.saturating_sub(1) as u32));
        let row_x = stage
            .0
            .saturating_add(stage.2.saturating_sub(row_width) as i32 / 2);
        let cell_x = row_x.saturating_add(
            column
                .saturating_mul(cell_width.saturating_add(gap) as usize)
                .min(i32::MAX as usize) as i32,
        );
        let cell_y = stage.1.saturating_add(
            row.saturating_mul(cell_height.saturating_add(gap) as usize)
                .min(i32::MAX as usize) as i32,
        );
        let fitted = fit_overview_rect((width, height), (cell_width, cell_height));
        slots.push(OverviewSpreadSlot {
            window_id,
            rect: (
                cell_x.saturating_add(cell_width.saturating_sub(fitted.0) as i32 / 2),
                cell_y.saturating_add(cell_height.saturating_sub(fitted.1) as i32 / 2),
                fitted.0.max(1),
                fitted.1.max(1),
            ),
        });
    }
    slots
}

fn overview_drag_progress_milli(start_y: i32, current_y: i32, rail_bottom: i32) -> u32 {
    let travelled = i64::from(start_y)
        .saturating_sub(i64::from(current_y))
        .max(0);
    let travel_to_rail = i64::from(start_y)
        .saturating_sub(i64::from(rail_bottom))
        .max(1);
    travelled
        .saturating_mul(1000)
        .checked_div(travel_to_rail)
        .unwrap_or(0)
        .clamp(0, 1000) as u32
}

fn interpolate_overview_dimension(from: u32, to: u32, progress_milli: u32) -> u32 {
    let progress = u64::from(progress_milli.min(1000));
    let inverse = 1000u64.saturating_sub(progress);
    ((u64::from(from.max(1))
        .saturating_mul(inverse)
        .saturating_add(u64::from(to.max(1)).saturating_mul(progress)))
        / 1000)
        .max(1)
        .min(u64::from(u32::MAX)) as u32
}

fn overview_dragged_root_rect(
    base: (i32, i32, u32, u32),
    thumbnail_size: (u32, u32),
    start_pointer: (i32, i32),
    current_pointer: (i32, i32),
    progress_milli: u32,
) -> (i32, i32, u32, u32) {
    let width = interpolate_overview_dimension(base.2, thumbnail_size.0, progress_milli);
    let height = interpolate_overview_dimension(base.3, thumbnail_size.1, progress_milli);
    let grab_x = i64::from(start_pointer.0)
        .saturating_sub(i64::from(base.0))
        .clamp(0, i64::from(base.2));
    let grab_y = i64::from(start_pointer.1)
        .saturating_sub(i64::from(base.1))
        .clamp(0, i64::from(base.3));
    let x = i64::from(current_pointer.0)
        .saturating_sub(grab_x.saturating_mul(i64::from(width)) / i64::from(base.2.max(1)));
    let y = i64::from(current_pointer.1)
        .saturating_sub(grab_y.saturating_mul(i64::from(height)) / i64::from(base.3.max(1)));
    (
        x.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        y.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        width,
        height,
    )
}

fn project_overview_rect_between_roots(
    rect: (i32, i32, u32, u32),
    source_root: (i32, i32, u32, u32),
    target_root: (i32, i32, u32, u32),
) -> (i32, i32, u32, u32) {
    let map_offset = |value: i32, source_origin: i32, source_extent: u32, target_extent: u32| {
        i64::from(value)
            .saturating_sub(i64::from(source_origin))
            .saturating_mul(i64::from(target_extent))
            .checked_div(i64::from(source_extent.max(1)))
            .unwrap_or(0)
    };
    let x = i64::from(target_root.0).saturating_add(map_offset(
        rect.0,
        source_root.0,
        source_root.2,
        target_root.2,
    ));
    let y = i64::from(target_root.1).saturating_add(map_offset(
        rect.1,
        source_root.1,
        source_root.3,
        target_root.3,
    ));
    let width = (u64::from(rect.2).saturating_mul(u64::from(target_root.2))
        / u64::from(source_root.2.max(1)))
    .max(1)
    .min(u64::from(u32::MAX)) as u32;
    let height = (u64::from(rect.3).saturating_mul(u64::from(target_root.3))
        / u64::from(source_root.3.max(1)))
    .max(1)
    .min(u64::from(u32::MAX)) as u32;
    (
        x.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        y.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        width,
        height,
    )
}

fn map_overview_drop_position(
    preview_origin: (i32, i32),
    target_card: (i32, i32, u32, u32),
    workarea: (i32, i32, u32, u32),
    surface_size: (u32, u32),
) -> (i32, i32) {
    let map_axis = |preview: i32,
                    card_origin: i32,
                    card_extent: u32,
                    work_origin: i32,
                    work_extent: u32,
                    surface_extent: u32| {
        let relative = i64::from(preview).saturating_sub(i64::from(card_origin));
        let projected = i64::from(work_origin).saturating_add(
            relative.saturating_mul(i64::from(work_extent)) / i64::from(card_extent.max(1)),
        );
        let minimum = i64::from(work_origin);
        let maximum = minimum.saturating_add(i64::from(work_extent.saturating_sub(surface_extent)));
        projected.clamp(minimum, maximum) as i32
    };

    (
        map_axis(
            preview_origin.0,
            target_card.0,
            target_card.2,
            workarea.0,
            workarea.2,
            surface_size.0,
        ),
        map_axis(
            preview_origin.1,
            target_card.1,
            target_card.3,
            workarea.1,
            workarea.3,
            surface_size.1,
        ),
    )
}

#[cfg(test)]
mod touch_modality_tests {
    use super::*;
    use crate::input::{abs_codes, event_types};

    #[test]
    fn direct_touch_hides_cursor_without_changing_pointer_position() {
        let pointer_position = (417, 233);
        let mut modality = InputModality::default();
        assert!(modality.direct_touch());
        assert!(modality.cursor_hidden_by_touch);
        assert_eq!(pointer_position, (417, 233));
    }

    #[test]
    fn touchpad_or_mouse_motion_shows_cursor_again() {
        let mut modality = InputModality {
            cursor_hidden_by_touch: true,
        };
        assert!(modality.pointer_motion());
        assert!(!modality.cursor_hidden_by_touch);
    }

    #[test]
    fn frame_callbacks_wait_for_visibility_and_a_new_presentation_boundary() {
        assert!(frame_callback_is_ready(true, false, 0, 0));
        assert!(!frame_callback_is_ready(false, false, 1, 0));
        assert!(!frame_callback_is_ready(true, true, 4, 4));
        assert!(frame_callback_is_ready(true, true, 5, 4));
    }

    #[test]
    fn overview_shadow_uses_a_soft_monotonic_falloff() {
        let mut layers = Vec::new();
        push_overview_shadow_layers(&mut layers, (100, 80, 480, 300), 12, true);

        assert_eq!(layers.len(), 6);
        assert!(layers.windows(2).all(|pair| {
            let outer = pair[0];
            let inner = pair[1];
            outer.0.2 > inner.0.2
                && outer.0.3 > inner.0.3
                && outer.1 > inner.1
                && outer.2[3] < inner.2[3]
        }));
        assert_eq!(layers.first().map(|layer| layer.2[3]), Some(2));
        assert_eq!(layers.last().map(|layer| layer.2[3]), Some(10));
    }

    #[test]
    fn overview_does_not_stack_a_fallback_over_client_shadow_outsets() {
        assert!(needs_overview_fallback_shadow(
            sws_protocol::WindowGeometryInsets::default()
        ));
        assert!(!needs_overview_fallback_shadow(
            sws_protocol::WindowGeometryInsets {
                left: 11,
                top: 6,
                right: 11,
                bottom: 16,
            }
        ));
    }

    #[test]
    fn normalized_direct_coordinates_hit_screen_edges() {
        assert_eq!(normalized_touch_to_screen(0, 1920), 0);
        assert_eq!(normalized_touch_to_screen(TOUCH_COORD_MAX, 1920), 1919);
    }

    #[test]
    fn overview_lays_out_complete_cards_when_every_workspace_fits() {
        let ids = [1, 2, 3, 4, 5, 6, 7];
        let cards = layout_overview_cards(
            (0, 32, 1920, 1048),
            false,
            1000,
            sws_protocol::workspace::ShellPresentation::Overview,
            &ids,
            4,
        );
        assert_eq!(cards.len(), 7);
        assert!(cards.iter().any(|(workspace_id, _)| *workspace_id == 4));
        for (_, (x, y, width, height)) in cards {
            assert!(x >= 0);
            assert!(y >= 32);
            assert!(x.saturating_add(width as i32) <= 1920);
            assert!(y.saturating_add(height as i32) <= 32 + 1048);
            assert!(width >= 150);
            assert!(height >= 80);
        }
    }

    #[test]
    fn overview_window_tracks_the_active_workspace_at_row_edges() {
        let ids = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let first = layout_overview_cards(
            (0, 32, 1366, 736),
            false,
            1000,
            sws_protocol::workspace::ShellPresentation::Overview,
            &ids,
            1,
        );
        let last = layout_overview_cards(
            (0, 32, 1366, 736),
            false,
            1000,
            sws_protocol::workspace::ShellPresentation::Overview,
            &ids,
            12,
        );
        assert_eq!(
            first.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6, 7, 8]
        );
        assert_eq!(
            last.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![5, 6, 7, 8, 9, 10, 11, 12]
        );
    }

    #[test]
    fn overview_keeps_one_neighbor_peeking_at_each_viewport_edge() {
        let ids = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let cards = layout_overview_cards(
            (0, 32, 1366, 736),
            false,
            1000,
            sws_protocol::workspace::ShellPresentation::Overview,
            &ids,
            7,
        );

        assert_eq!(
            cards.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![3, 4, 5, 6, 7, 8, 9, 10, 11]
        );
        let left = cards.first().expect("left neighbor").1;
        let right = cards.last().expect("right neighbor").1;
        assert!(left.0 < 0);
        assert!(left.0.saturating_add(left.2 as i32) > 0);
        assert!(right.0 < 1366);
        assert!(right.0.saturating_add(right.2 as i32) > 1366);
        let left_visible = left.0.saturating_add(left.2 as i32).max(0) as u32;
        let right_visible = 1366i32.saturating_sub(right.0).max(0) as u32;
        assert!(left_visible.saturating_mul(3) >= left.2);
        assert!(left_visible.saturating_mul(3) <= left.2.saturating_mul(2));
        assert!(right_visible.saturating_mul(3) >= right.2);
        assert!(right_visible.saturating_mul(3) <= right.2.saturating_mul(2));
    }

    #[test]
    fn overview_scrolls_the_creation_card_into_the_final_viewport() {
        let ids = [1, 2, 3, 4, 0];
        let first = layout_overview_cards(
            (0, 32, 640, 736),
            false,
            1000,
            sws_protocol::workspace::ShellPresentation::Overview,
            &ids,
            1,
        );
        let last = layout_overview_cards(
            (0, 32, 640, 736),
            false,
            1000,
            sws_protocol::workspace::ShellPresentation::Overview,
            &ids,
            4,
        );

        assert_eq!(
            first.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert_eq!(
            last.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![2, 3, 4, 0]
        );
    }

    #[test]
    fn overview_workspace_thumbnail_instances_are_hit_testable() {
        let instance = PresentationInstance {
            transform: PresentationTransform {
                x: 120,
                y: 60,
                width: 160,
                height: 90,
                opacity: 1.0,
            },
            clip: Some((100, 40, 200, 130)),
            clip_radius: 12,
        };

        assert!(presentation_instance_contains_point(&instance, 180, 100));
        assert!(!presentation_instance_contains_point(&instance, 119, 100));
        assert!(!presentation_instance_contains_point(&instance, 280, 100));

        let clipped = PresentationInstance {
            clip: Some((150, 80, 80, 40)),
            ..instance
        };
        assert!(!presentation_instance_contains_point(&clipped, 140, 90));
        assert!(presentation_instance_contains_point(&clipped, 180, 100));
    }

    #[test]
    fn overview_drop_maps_back_to_the_corresponding_freeform_position() {
        assert_eq!(
            map_overview_drop_position(
                (350, 250),
                (100, 100, 500, 300),
                (0, 32, 1000, 600),
                (200, 100),
            ),
            (500, 332)
        );
        assert_eq!(
            map_overview_drop_position(
                (590, 390),
                (100, 100, 500, 300),
                (0, 32, 1000, 600),
                (300, 200),
            ),
            (700, 432)
        );
    }

    #[test]
    fn overview_drag_shrinks_continuously_toward_the_workspace_rail() {
        assert_eq!(overview_drag_progress_milli(330, 330, 200), 0);
        assert_eq!(overview_drag_progress_milli(330, 265, 200), 500);
        assert_eq!(overview_drag_progress_milli(330, 200, 200), 1000);
        assert_eq!(overview_drag_progress_milli(330, 120, 200), 1000);

        assert_eq!(
            overview_dragged_root_rect(
                (100, 300, 800, 600),
                (160, 120),
                (500, 330),
                (500, 265),
                500,
            ),
            (260, 247, 480, 360)
        );
        assert_eq!(
            overview_dragged_root_rect(
                (100, 300, 800, 600),
                (160, 120),
                (500, 330),
                (500, 200),
                1000,
            ),
            (420, 194, 160, 120)
        );
    }

    #[test]
    fn overview_drag_projects_transients_with_the_shrinking_root() {
        assert_eq!(
            project_overview_rect_between_roots(
                (300, 400, 200, 100),
                (100, 300, 800, 600),
                (420, 194, 160, 120),
            ),
            (460, 214, 40, 20)
        );
    }

    #[test]
    fn overview_workspace_region_uses_the_shared_ratio_at_every_scale() {
        let logical = overview_workspace_region_for(
            (0, 32, 1920, 1048),
            false,
            1000,
            sws_protocol::workspace::ShellPresentation::Overview,
        );
        let scaled = overview_workspace_region_for(
            (0, 40, 2400, 1310),
            false,
            1250,
            sws_protocol::workspace::ShellPresentation::Overview,
        );
        assert_eq!(logical, (0, 32, 1920, 104));
        assert_eq!(scaled, (0, 40, 2400, 131));
    }

    #[test]
    fn overview_spread_never_enlarges_a_window() {
        assert_eq!(fit_overview_rect((320, 200), (1200, 800)), (320, 200));
        assert_eq!(fit_overview_rect((1200, 600), (400, 400)), (400, 200));
    }

    #[test]
    fn overview_spread_uses_distinct_aspect_preserving_slots() {
        let slots = layout_overview_window_spread(
            (40, 152, 1840, 800),
            1000,
            &[(10, 1200, 700), (11, 640, 480), (12, 320, 240)],
        );
        assert_eq!(slots.len(), 3);
        for (index, slot) in slots.iter().enumerate() {
            assert!(slot.rect.0 >= 40);
            assert!(slot.rect.1 >= 152);
            assert!(slot.rect.0.saturating_add(slot.rect.2 as i32) <= 1880);
            assert!(slot.rect.1.saturating_add(slot.rect.3 as i32) <= 952);
            let source = [(1200, 700), (640, 480), (320, 240)][index];
            assert!(slot.rect.2 <= source.0);
            assert!(slot.rect.3 <= source.1);
        }
        for (index, first) in slots.iter().enumerate() {
            for second in slots.iter().skip(index + 1) {
                assert!(intersect_compositor_rects(first.rect, second.rect).is_none());
            }
        }
    }

    #[test]
    fn overview_card_tokens_scale_with_the_output() {
        let ids = [1, 2];
        let logical = layout_overview_cards(
            (0, 32, 1920, 1048),
            false,
            1000,
            sws_protocol::workspace::ShellPresentation::Overview,
            &ids,
            1,
        );
        let hidpi = layout_overview_cards(
            (0, 32, 1920, 1048),
            false,
            2000,
            sws_protocol::workspace::ShellPresentation::Overview,
            &ids,
            1,
        );
        assert!(hidpi[0].1.2 > logical[0].1.2);
        assert_eq!(hidpi.len(), 2);
    }

    #[test]
    fn direct_touch_press_and_move_update_coordinates_before_button_or_sync() {
        assert_eq!(
            direct_legacy_event_sequence(12, 34, DirectLegacyEventKind::Press),
            vec![
                (event_types::EV_ABS, abs_codes::ABS_X, 12),
                (event_types::EV_ABS, abs_codes::ABS_Y, 34),
                (event_types::EV_KEY, key_codes::BTN_LEFT, 1),
                (event_types::EV_SYN, 0, 0),
            ]
        );
        assert_eq!(
            direct_legacy_event_sequence(56, 78, DirectLegacyEventKind::Move),
            vec![
                (event_types::EV_ABS, abs_codes::ABS_X, 56),
                (event_types::EV_ABS, abs_codes::ABS_Y, 78),
                (event_types::EV_SYN, 0, 0),
            ]
        );
    }

    #[test]
    fn direct_touch_release_commits_at_contact_then_clears_hover() {
        assert_eq!(
            direct_legacy_event_sequence(12, 34, DirectLegacyEventKind::Release),
            vec![
                (event_types::EV_ABS, abs_codes::ABS_X, 12),
                (event_types::EV_ABS, abs_codes::ABS_Y, 34),
                (event_types::EV_KEY, key_codes::BTN_LEFT, 0),
                (event_types::EV_SYN, 0, 0),
                (event_types::EV_ABS, abs_codes::ABS_X, -1),
                (event_types::EV_ABS, abs_codes::ABS_Y, -1),
                (event_types::EV_SYN, 0, 0),
            ]
        );
    }

    #[test]
    fn direct_touch_cancel_releases_only_after_pointer_is_outside() {
        assert_eq!(
            direct_legacy_event_sequence(12, 34, DirectLegacyEventKind::Cancel),
            vec![
                (event_types::EV_ABS, abs_codes::ABS_X, -1),
                (event_types::EV_ABS, abs_codes::ABS_Y, -1),
                (event_types::EV_KEY, key_codes::BTN_LEFT, 0),
                (event_types::EV_SYN, 0, 0),
            ]
        );
    }

    #[test]
    fn direct_touch_can_authorize_move_without_mouse_button_state() {
        assert_eq!(
            interactive_move_grab_origin(false, Some((640, 360)), None, (20, 30)),
            Some((640, 360))
        );
    }

    #[test]
    fn interactive_move_requires_a_mouse_or_direct_touch_grab() {
        assert_eq!(
            interactive_move_grab_origin(false, None, Some((10, 20)), (30, 40)),
            None
        );
        assert_eq!(
            interactive_move_grab_origin(true, None, Some((10, 20)), (30, 40)),
            Some((10, 20))
        );
    }

    #[test]
    fn maximized_normal_window_tracks_shell_workarea() {
        assert_eq!(
            maximized_geometry_for(
                super::super::window::WindowType::Normal,
                Some((0, 56, 1920, 1024)),
                1920,
                1080,
            ),
            (10, 66, 1900, 1004)
        );
        assert_eq!(
            maximized_geometry_for(
                super::super::window::WindowType::Taskbar,
                Some((0, 56, 1920, 1024)),
                1920,
                1080,
            ),
            (0, 0, 1920, 1080)
        );
    }

    #[test]
    fn resize_outline_excludes_client_reported_window_decoration() {
        let insets = sws_protocol::WindowGeometryInsets {
            left: 10,
            top: 6,
            right: 10,
            bottom: 14,
        };

        assert_eq!(
            resize_outline_for_surface(90, 74, 324, 260, insets),
            (100, 80, 304, 240)
        );
        assert_eq!(
            resize_outline_for_surface(90, 74, 344, 280, insets),
            (100, 80, 324, 260)
        );
    }
}

macro_rules! sws_debug {
    ($($arg:tt)*) => {
        if is_sws_debug_enabled() {
            std::println!($($arg)*);
        }
    };
}

fn sgfx_error_code(error: SgfxBufferError) -> u32 {
    match error {
        SgfxBufferError::Unavailable => sws_protocol::error_codes::SGFX_UNAVAILABLE,
        SgfxBufferError::InvalidBuffer => sws_protocol::error_codes::INVALID_SGFX_BUFFER,
        SgfxBufferError::StaleGeneration => sws_protocol::error_codes::STALE_SGFX_GENERATION,
        SgfxBufferError::BufferBusy => sws_protocol::error_codes::SGFX_BUFFER_BUSY,
        SgfxBufferError::ImportFailed => sws_protocol::error_codes::SGFX_IMPORT_FAILED,
    }
}

fn send_sgfx_protocol_error(client_id: usize, request_id: u8, code: u32) {
    let payload = sws_protocol::payload_error(code).to_vec();
    if request_id == 0 {
        send_message_to_client(client_id, sws_protocol::server_msg::ERROR, payload);
    } else {
        send_response_to_client(
            client_id,
            sws_protocol::server_msg::ERROR,
            request_id,
            payload,
        );
    }
}

fn send_sgfx_frame_rejected(
    client_id: usize,
    identity: SgfxBufferIdentity,
    commit_serial: u64,
    code: u32,
) {
    let payload = sws_protocol::payload_sgfx_frame_rejected(
        identity.window_id,
        identity.buffer_id,
        identity.generation,
        identity.compositor_epoch,
        commit_serial,
        code,
    );
    send_message_to_client(
        client_id,
        sws_protocol::server_msg::SGFX_FRAME_REJECTED,
        payload.to_vec(),
    );
}

fn send_sgfx_buffer_released(release: SgfxCommitToken) {
    let identity = release.identity;
    let payload = sws_protocol::payload_sgfx_buffer_released(
        identity.window_id,
        identity.buffer_id,
        identity.generation,
        identity.compositor_epoch,
        release.commit_serial,
    );
    super::ipc::send_message_to_window(
        identity.window_id,
        sws_protocol::server_msg::SGFX_BUFFER_RELEASED,
        payload.to_vec(),
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SwsBackend {
    Auto,
    Cpu,
    Sgfx,
}

fn selected_sws_backend() -> Result<SwsBackend, &'static str> {
    match env::var("SWS_BACKEND").ok() {
        None => Ok(SwsBackend::Auto),
        Some(value) if value.eq_ignore_ascii_case("auto") => Ok(SwsBackend::Auto),
        Some(value) if value.eq_ignore_ascii_case("cpu") => Ok(SwsBackend::Cpu),
        Some(value) if value.eq_ignore_ascii_case("sgfx") => Ok(SwsBackend::Sgfx),
        Some(_) => Err("SWS_BACKEND must be one of: auto, cpu, sgfx"),
    }
}

// NOTE: The compositor intentionally opens the modern display surface endpoint,
// not the legacy framebuffer node. The endpoint may internally use mmap.

// Debug: validate that compositor output in VRAM matches what we expect
// from window buffers (helps catch stride/offset/blit bugs).
const LOG_RENDER_VALIDATION: bool = false;

// Feature flag: Enable dirty rect optimization (false = always full redraw)
// Disable this if you suspect partial redraw is causing rendering artifacts
const ENABLE_DIRTY_RECT: bool = true;
const MAX_PENDING_DAMAGE_RECTS: usize = 8;
const DAMAGE_MERGE_AREA_FACTOR: u64 = 2;
const FRAME_BATCH_INTERVAL_NS: u64 = 16_666_667;
const RUNTIME_ERROR_RETRY_DELAY_MS: u64 = 100;
const COMPOSITOR_IDLE_RECHECK_NS: i64 = 250_000_000;
const COMPOSITOR_WAKE_ERROR_DELAY_MS: u64 = 10;
const DEFAULT_OUTPUT_SCALE_MILLI: u32 = 2000;
const DEFAULT_CURSOR_THEME_PATH: &str = "/share/cursors/default";
const INSTALLED_CURSOR_THEME_ROOT: &str = "/share/cursors/";
const OVERVIEW_WINDOW_DRAG_THRESHOLD_LOGICAL: u32 = 10;

#[derive(Debug, Clone)]
struct SwsConfig {
    output_scale_milli: u32,
    cursor: CursorConfig,
    overview_toggle_bindings: Vec<OverviewToggleBinding>,
    shell_action_bindings: Vec<(KeyBinding, ShellAction)>,
    ime_toggle_bindings: Vec<KeyBinding>,
    preferred_input_method_name: Option<String>,
    auto_remove_empty_workspaces: bool,
}

#[derive(Debug, Clone)]
struct CursorConfig {
    theme_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KeyBinding {
    code: u16,
    modifiers: KeyModifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverviewToggleBinding {
    SuperTap,
    Chord(KeyBinding),
}

/// Compositor-resolved shell navigation actions.
///
/// Actions dispatch through one exact-modifier binding table, so a specific
/// chord such as `Super+Shift+Left` can never be shadowed by the broader
/// `Super+Left` binding regardless of declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellAction {
    OverviewToggle,
    Home,
    WorkspaceLeft,
    WorkspaceRight,
    MoveWindowLeft,
    MoveWindowRight,
    OverviewActivate,
    AddWorkspace,
    RemoveWorkspace,
}

/// Configurable chord keys in `[keybindings]` and their default actions.
const ACTION_BINDING_KEYS: &[(&str, ShellAction)] = &[
    ("workspace_left", ShellAction::WorkspaceLeft),
    ("workspace_right", ShellAction::WorkspaceRight),
    ("move_window_left", ShellAction::MoveWindowLeft),
    ("move_window_right", ShellAction::MoveWindowRight),
    ("home", ShellAction::Home),
    ("overview_activate", ShellAction::OverviewActivate),
    ("add_workspace", ShellAction::AddWorkspace),
    ("remove_workspace", ShellAction::RemoveWorkspace),
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct KeyModifiers {
    ctrl: bool,
    shift: bool,
    alt: bool,
    meta: bool,
}

fn laptop_overview_space_opens_home(
    tablet_mode: bool,
    presentation: sws_protocol::workspace::ShellPresentation,
    code: u16,
    modifiers: KeyModifiers,
) -> bool {
    !tablet_mode
        && presentation == sws_protocol::workspace::ShellPresentation::Overview
        && code == key_codes::KEY_SPACE
        && modifiers == KeyModifiers::default()
}

fn shell_workspace_rail_accepts_selection(
    presentation: sws_protocol::workspace::ShellPresentation,
) -> bool {
    matches!(
        presentation,
        sws_protocol::workspace::ShellPresentation::Overview
            | sws_protocol::workspace::ShellPresentation::Home
    )
}

const SUPER_KEY_CODES: [u16; 2] = [key_codes::KEY_LEFTMETA, key_codes::KEY_RIGHTMETA];

fn load_sws_config() -> SwsConfig {
    let mut config = SwsConfig {
        output_scale_milli: DEFAULT_OUTPUT_SCALE_MILLI,
        cursor: default_cursor_config(),
        overview_toggle_bindings: default_overview_toggle_bindings(),
        shell_action_bindings: default_shell_action_bindings(),
        ime_toggle_bindings: default_ime_toggle_bindings(),
        preferred_input_method_name: None,
        auto_remove_empty_workspaces: false,
    };

    match config::read_sws_config() {
        Ok(content) => {
            if let Some(output_scale_milli) = parse_output_scale_milli(&content) {
                config.output_scale_milli = output_scale_milli;
            } else {
                println!(
                    "[Compositor] No output scale in {}; using default {}",
                    config::SWS_CONFIG_PATH,
                    DEFAULT_OUTPUT_SCALE_MILLI
                );
            }

            if let Some(bindings) = parse_keybindings_ime_toggle(&content) {
                if bindings.is_empty() {
                    println!(
                        "[Compositor] Ignoring empty keybindings.ime_toggle in {}; using default",
                        config::SWS_CONFIG_PATH
                    );
                } else {
                    config.ime_toggle_bindings = bindings;
                }
            }

            if let Some(bindings) = parse_keybindings_overview_toggle(&content) {
                if bindings.is_empty() {
                    println!(
                        "[Compositor] Ignoring empty keybindings.overview_toggle in {}; using default",
                        config::SWS_CONFIG_PATH
                    );
                } else {
                    config.overview_toggle_bindings = bindings;
                }
            }

            config.shell_action_bindings = load_shell_action_bindings(&content);

            config.cursor = parse_cursor_config(&content);
            config.preferred_input_method_name = config::parse_active_input_method(&content);
            config.auto_remove_empty_workspaces =
                parse_auto_remove_empty_workspaces(&content).unwrap_or(false);
        }
        Err(_) => {}
    }

    config
}

fn default_cursor_config() -> CursorConfig {
    CursorConfig {
        theme_path: String::from(DEFAULT_CURSOR_THEME_PATH),
    }
}

fn parse_auto_remove_empty_workspaces(content: &str) -> Option<bool> {
    let mut accepts_workspaces = false;
    for raw_line in content.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            accepts_workspaces = line[1..line.len() - 1].trim() == "workspaces";
            continue;
        }
        if !accepts_workspaces {
            continue;
        }
        let Some(eq_pos) = line.find('=') else {
            continue;
        };
        if line[..eq_pos].trim() != "auto_remove_empty" {
            continue;
        }
        return match trim_toml_string(&line[eq_pos + 1..])
            .to_ascii_lowercase()
            .as_str()
        {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        };
    }
    None
}

fn parse_cursor_config(content: &str) -> CursorConfig {
    let mut cursor = default_cursor_config();
    let mut accepts_cursor = false;

    for raw_line in content.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            accepts_cursor = line[1..line.len() - 1].trim() == "cursor";
            continue;
        }
        if !accepts_cursor {
            continue;
        }

        let Some(eq_pos) = line.find('=') else {
            continue;
        };
        let key = line[..eq_pos].trim();
        let value = line[eq_pos + 1..].trim();
        match key {
            "theme" => {
                let theme_path = trim_toml_string(value);
                if !theme_path.is_empty() {
                    cursor.theme_path = String::from(theme_path);
                }
            }
            _ => {}
        }
    }

    cursor
}

fn default_ime_toggle_bindings() -> Vec<KeyBinding> {
    let mut bindings = Vec::new();
    bindings.push(KeyBinding {
        code: key_codes::KEY_BACKSLASH,
        modifiers: KeyModifiers {
            ctrl: true,
            shift: false,
            alt: false,
            meta: false,
        },
    });
    bindings
}

fn default_overview_toggle_bindings() -> Vec<OverviewToggleBinding> {
    vec![OverviewToggleBinding::SuperTap]
}

fn meta_chord(code: u16, shift: bool) -> KeyBinding {
    KeyBinding {
        code,
        modifiers: KeyModifiers {
            meta: true,
            shift,
            ..KeyModifiers::default()
        },
    }
}

fn default_shell_action_bindings() -> Vec<(KeyBinding, ShellAction)> {
    vec![
        (
            meta_chord(key_codes::KEY_LEFT, true),
            ShellAction::MoveWindowLeft,
        ),
        (
            meta_chord(key_codes::KEY_RIGHT, true),
            ShellAction::MoveWindowRight,
        ),
        (
            meta_chord(key_codes::KEY_LEFT, false),
            ShellAction::WorkspaceLeft,
        ),
        (
            meta_chord(key_codes::KEY_RIGHT, false),
            ShellAction::WorkspaceRight,
        ),
        (meta_chord(key_codes::KEY_SPACE, false), ShellAction::Home),
        (
            meta_chord(key_codes::KEY_ENTER, false),
            ShellAction::OverviewActivate,
        ),
        (
            meta_chord(key_codes::KEY_N, true),
            ShellAction::AddWorkspace,
        ),
        (
            meta_chord(key_codes::KEY_DELETE, false),
            ShellAction::RemoveWorkspace,
        ),
    ]
}

/// Replace any same-chord entry, keeping exactly one action per binding.
fn upsert_shell_binding(
    bindings: &mut Vec<(KeyBinding, ShellAction)>,
    binding: KeyBinding,
    action: ShellAction,
) {
    bindings.retain(|(existing, _)| {
        !(existing.code == binding.code && existing.modifiers == binding.modifiers)
    });
    bindings.push((binding, action));
}

fn load_shell_action_bindings(content: &str) -> Vec<(KeyBinding, ShellAction)> {
    let mut bindings = default_shell_action_bindings();
    let entries = parse_keybinding_entries(content);
    for (name, action) in ACTION_BINDING_KEYS {
        let Some((_, raw)) = entries.iter().find(|(key, _)| key == name) else {
            continue;
        };
        let parsed = parse_key_binding_list(raw);
        if parsed.is_empty() {
            println!(
                "[Compositor] Ignoring empty keybindings.{name} in {}; using default",
                config::SWS_CONFIG_PATH
            );
            continue;
        }
        // A configured action replaces that action's defaults, not merely a
        // same-chord entry. Otherwise changing `workspace_left` would leave
        // `Super+Left` active as an undocumented second binding.
        bindings.retain(|(_, existing_action)| existing_action != action);
        for binding in parsed {
            upsert_shell_binding(&mut bindings, binding, *action);
        }
    }
    if let Some(overview) = parse_keybindings_overview_toggle(content) {
        for binding in overview {
            if let OverviewToggleBinding::Chord(binding) = binding {
                upsert_shell_binding(&mut bindings, binding, ShellAction::OverviewToggle);
            }
        }
    }
    bindings
}

fn resolve_shell_action(
    bindings: &[(KeyBinding, ShellAction)],
    code: u16,
    modifiers: KeyModifiers,
) -> Option<ShellAction> {
    bindings
        .iter()
        .find(|(binding, _)| binding.code == code && binding.modifiers == modifiers)
        .map(|(_, action)| *action)
}

fn parse_output_scale_milli(content: &str) -> Option<u32> {
    let mut accepts_output_scale = true;

    for raw_line in content.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            let section = line[1..line.len() - 1].trim();
            accepts_output_scale = section == "output" || section == "display";
            continue;
        }

        let Some(eq_pos) = line.find('=') else {
            continue;
        };
        let key = line[..eq_pos].trim();
        let value = line[eq_pos + 1..].trim();

        if key == "scale_milli" && accepts_output_scale {
            if let Some(scale_milli) = parse_u32_value(value) {
                return Some(normalize_scale_milli(scale_milli));
            }
        }

        if key == "scale" && accepts_output_scale {
            if let Some(scale_milli) = parse_scale_value_milli(value) {
                return Some(normalize_scale_milli(scale_milli));
            }
        }
    }

    None
}

fn parse_keybindings_ime_toggle(content: &str) -> Option<Vec<KeyBinding>> {
    parse_keybinding_entries(content)
        .into_iter()
        .find(|(key, _)| key == "ime_toggle")
        .map(|(_, value)| parse_key_binding_list(&value))
}

fn parse_keybindings_overview_toggle(content: &str) -> Option<Vec<OverviewToggleBinding>> {
    parse_keybinding_entries(content)
        .into_iter()
        .find(|(key, _)| key == "overview_toggle")
        .map(|(_, value)| parse_overview_toggle_binding_list(&value))
}

fn parse_keybinding_entries(content: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    let mut accepts_keybindings = false;

    for raw_line in content.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            let section = line[1..line.len() - 1].trim();
            accepts_keybindings = section == "keybindings";
            continue;
        }

        let Some(eq_pos) = line.find('=') else {
            continue;
        };
        if accepts_keybindings {
            entries.push((
                line[..eq_pos].trim().to_string(),
                line[eq_pos + 1..].trim().to_string(),
            ));
        }
    }

    entries
}

fn parse_overview_toggle_binding_list(value: &str) -> Vec<OverviewToggleBinding> {
    let value = value.trim();
    if value.starts_with('[') && value.ends_with(']') {
        value[1..value.len() - 1]
            .split(',')
            .filter_map(parse_overview_toggle_binding_value)
            .collect()
    } else {
        parse_overview_toggle_binding_value(value)
            .into_iter()
            .collect()
    }
}

fn parse_overview_toggle_binding_value(value: &str) -> Option<OverviewToggleBinding> {
    let value = trim_toml_string(value);
    if value.eq_ignore_ascii_case("meta")
        || value.eq_ignore_ascii_case("super")
        || value.eq_ignore_ascii_case("logo")
        || value.eq_ignore_ascii_case("cmd")
    {
        return Some(OverviewToggleBinding::SuperTap);
    }
    parse_key_binding_value(value).map(OverviewToggleBinding::Chord)
}

fn parse_key_binding_list(value: &str) -> Vec<KeyBinding> {
    let value = value.trim();
    if value.starts_with('[') && value.ends_with(']') {
        value[1..value.len() - 1]
            .split(',')
            .filter_map(parse_key_binding_value)
            .collect()
    } else {
        parse_key_binding_value(value).into_iter().collect()
    }
}

fn parse_key_binding_value(value: &str) -> Option<KeyBinding> {
    let value = trim_toml_string(value);
    let mut modifiers = KeyModifiers::default();
    let mut code = None;

    for token in value.split('+') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }

        if token.eq_ignore_ascii_case("ctrl") || token.eq_ignore_ascii_case("control") {
            modifiers.ctrl = true;
            continue;
        }
        if token.eq_ignore_ascii_case("shift") {
            modifiers.shift = true;
            continue;
        }
        if token.eq_ignore_ascii_case("alt") || token.eq_ignore_ascii_case("option") {
            modifiers.alt = true;
            continue;
        }
        if token.eq_ignore_ascii_case("meta")
            || token.eq_ignore_ascii_case("super")
            || token.eq_ignore_ascii_case("logo")
            || token.eq_ignore_ascii_case("cmd")
        {
            modifiers.meta = true;
            continue;
        }

        code = parse_key_code_name(token);
    }

    code.map(|code| KeyBinding { code, modifiers })
}

fn parse_key_code_name(value: &str) -> Option<u16> {
    let value = trim_toml_string(value);
    if let Some(code) = value.strip_prefix("keycode:") {
        return code.trim().parse::<u16>().ok();
    }
    if let Some(code) = value.strip_prefix("code:") {
        return code.trim().parse::<u16>().ok();
    }
    if let Ok(code) = value.parse::<u16>() {
        return Some(code);
    }

    if value == "-" || value.eq_ignore_ascii_case("minus") {
        return Some(key_codes::KEY_MINUS);
    }
    if value == "=" || value.eq_ignore_ascii_case("equal") || value.eq_ignore_ascii_case("equals") {
        return Some(key_codes::KEY_EQUAL);
    }
    if value == "["
        || value.eq_ignore_ascii_case("leftbrace")
        || value.eq_ignore_ascii_case("lbracket")
    {
        return Some(key_codes::KEY_LEFTBRACE);
    }
    if value == "]"
        || value.eq_ignore_ascii_case("rightbrace")
        || value.eq_ignore_ascii_case("rbracket")
    {
        return Some(key_codes::KEY_RIGHTBRACE);
    }
    if value == ";" || value.eq_ignore_ascii_case("semicolon") {
        return Some(key_codes::KEY_SEMICOLON);
    }
    if value == "'"
        || value.eq_ignore_ascii_case("apostrophe")
        || value.eq_ignore_ascii_case("quote")
    {
        return Some(key_codes::KEY_APOSTROPHE);
    }
    if value == "`" || value.eq_ignore_ascii_case("grave") || value.eq_ignore_ascii_case("backtick")
    {
        return Some(key_codes::KEY_GRAVE);
    }
    if value == "," || value.eq_ignore_ascii_case("comma") {
        return Some(key_codes::KEY_COMMA);
    }
    if value == "." || value.eq_ignore_ascii_case("dot") || value.eq_ignore_ascii_case("period") {
        return Some(key_codes::KEY_DOT);
    }
    if value == "/" || value.eq_ignore_ascii_case("slash") {
        return Some(key_codes::KEY_SLASH);
    }
    if value == "\\" || value.eq_ignore_ascii_case("backslash") {
        return Some(key_codes::KEY_BACKSLASH);
    }
    if value.eq_ignore_ascii_case("space") {
        return Some(key_codes::KEY_SPACE);
    }
    if value.eq_ignore_ascii_case("tab") {
        return Some(key_codes::KEY_TAB);
    }
    if value.eq_ignore_ascii_case("enter") || value.eq_ignore_ascii_case("return") {
        return Some(key_codes::KEY_ENTER);
    }
    if value.eq_ignore_ascii_case("escape") || value.eq_ignore_ascii_case("esc") {
        return Some(key_codes::KEY_ESC);
    }
    if value.eq_ignore_ascii_case("left") {
        return Some(key_codes::KEY_LEFT);
    }
    if value.eq_ignore_ascii_case("right") {
        return Some(key_codes::KEY_RIGHT);
    }
    if value.eq_ignore_ascii_case("h") {
        return Some(key_codes::KEY_H);
    }
    if value.eq_ignore_ascii_case("n") {
        return Some(key_codes::KEY_N);
    }
    if value.eq_ignore_ascii_case("delete") || value.eq_ignore_ascii_case("del") {
        return Some(key_codes::KEY_DELETE);
    }
    if value.eq_ignore_ascii_case("zenkaku_hankaku")
        || value.eq_ignore_ascii_case("zenkakuhankaku")
        || value.eq_ignore_ascii_case("hankaku_zenkaku")
        || value.eq_ignore_ascii_case("hankakuzenkaku")
    {
        return Some(key_codes::KEY_ZENKAKUHANKAKU);
    }
    if value.eq_ignore_ascii_case("henkan") {
        return Some(key_codes::KEY_HENKAN);
    }
    if value.eq_ignore_ascii_case("muhenkan") {
        return Some(key_codes::KEY_MUHENKAN);
    }
    if value.eq_ignore_ascii_case("hangul") || value.eq_ignore_ascii_case("hanguel") {
        return Some(key_codes::KEY_HANGUEL);
    }

    None
}

#[cfg(test)]
mod keybinding_config_tests {
    use super::*;

    #[test]
    fn overview_toggle_accepts_a_super_modifier_tap() {
        let bindings =
            parse_keybindings_overview_toggle("[keybindings]\noverview_toggle = \"Super\"\n")
                .unwrap();

        assert_eq!(bindings, vec![OverviewToggleBinding::SuperTap]);
    }

    #[test]
    fn overview_toggle_accepts_configurable_chord_fallbacks() {
        let bindings = parse_keybindings_overview_toggle(
            "[keybindings]\noverview_toggle = [\"Super+Space\", \"Super+Tab\"]\n",
        )
        .unwrap();
        let meta = KeyModifiers {
            meta: true,
            ..KeyModifiers::default()
        };

        assert_eq!(
            bindings,
            vec![
                OverviewToggleBinding::Chord(KeyBinding {
                    code: key_codes::KEY_SPACE,
                    modifiers: meta,
                }),
                OverviewToggleBinding::Chord(KeyBinding {
                    code: key_codes::KEY_TAB,
                    modifiers: meta,
                }),
            ]
        );
    }

    #[test]
    fn overview_and_ime_bindings_are_parsed_independently() {
        let config = "[keybindings]\noverview_toggle = \"Super\"\nime_toggle = \"Ctrl+Space\"\n";

        assert_eq!(
            parse_keybindings_overview_toggle(config),
            Some(vec![OverviewToggleBinding::SuperTap])
        );
        assert_eq!(parse_keybindings_ime_toggle(config).unwrap().len(), 1);
    }

    #[test]
    fn modifier_specific_chords_never_shadow_each_other() {
        let bindings = load_shell_action_bindings("[keybindings]\noverview_toggle = \"Super\"\n");
        let meta = KeyModifiers {
            meta: true,
            ..KeyModifiers::default()
        };
        let meta_shift = KeyModifiers {
            meta: true,
            shift: true,
            ..KeyModifiers::default()
        };

        assert_eq!(
            resolve_shell_action(&bindings, key_codes::KEY_LEFT, meta),
            Some(ShellAction::WorkspaceLeft)
        );
        assert_eq!(
            resolve_shell_action(&bindings, key_codes::KEY_LEFT, meta_shift),
            Some(ShellAction::MoveWindowLeft)
        );
        assert_eq!(
            resolve_shell_action(&bindings, key_codes::KEY_RIGHT, meta_shift),
            Some(ShellAction::MoveWindowRight)
        );
        assert_eq!(
            resolve_shell_action(&bindings, key_codes::KEY_SPACE, meta),
            Some(ShellAction::Home)
        );
    }

    #[test]
    fn plain_space_opens_home_only_from_laptop_overview() {
        let overview = sws_protocol::workspace::ShellPresentation::Overview;
        assert!(laptop_overview_space_opens_home(
            false,
            overview,
            key_codes::KEY_SPACE,
            KeyModifiers::default()
        ));
        assert!(!laptop_overview_space_opens_home(
            true,
            overview,
            key_codes::KEY_SPACE,
            KeyModifiers::default()
        ));
        assert!(!laptop_overview_space_opens_home(
            false,
            sws_protocol::workspace::ShellPresentation::Home,
            key_codes::KEY_SPACE,
            KeyModifiers::default()
        ));
        assert!(!laptop_overview_space_opens_home(
            false,
            overview,
            key_codes::KEY_SPACE,
            KeyModifiers {
                meta: true,
                ..KeyModifiers::default()
            }
        ));
    }

    #[test]
    fn overview_and_home_both_expose_keyboard_workspace_selection() {
        assert!(shell_workspace_rail_accepts_selection(
            sws_protocol::workspace::ShellPresentation::Overview
        ));
        assert!(shell_workspace_rail_accepts_selection(
            sws_protocol::workspace::ShellPresentation::Home
        ));
        assert!(!shell_workspace_rail_accepts_selection(
            sws_protocol::workspace::ShellPresentation::Workspace
        ));
    }

    #[test]
    fn physical_modifier_sequence_resolves_one_specific_action() {
        let bindings = default_shell_action_bindings();
        let source = KeyboardSource::Local(0);
        let mut held = HeldKeys::default();
        assert!(held.update(source, key_codes::KEY_LEFTMETA, 1));
        assert!(held.update(source, key_codes::KEY_LEFTSHIFT, 1));
        assert!(held.update(source, key_codes::KEY_LEFT, 1));
        let modifiers = KeyModifiers {
            ctrl: held.has_any(&[key_codes::KEY_LEFTCTRL, key_codes::KEY_RIGHTCTRL]),
            shift: held.has_any(&[key_codes::KEY_LEFTSHIFT, key_codes::KEY_RIGHTSHIFT]),
            alt: held.has_any(&[key_codes::KEY_LEFTALT, key_codes::KEY_RIGHTALT]),
            meta: held.has_any(&[key_codes::KEY_LEFTMETA, key_codes::KEY_RIGHTMETA]),
        };

        assert_eq!(
            resolve_shell_action(&bindings, key_codes::KEY_LEFT, modifiers),
            Some(ShellAction::MoveWindowLeft)
        );
        assert_eq!(
            bindings
                .iter()
                .filter(|(binding, _)| {
                    binding.code == key_codes::KEY_LEFT && binding.modifiers == modifiers
                })
                .count(),
            1
        );
    }

    #[test]
    fn configured_action_overrides_replace_only_their_default_chord() {
        let bindings =
            load_shell_action_bindings("[keybindings]\nworkspace_left = \"Ctrl+Alt+Left\"\n");
        let ctrl_alt = KeyModifiers {
            ctrl: true,
            alt: true,
            ..KeyModifiers::default()
        };
        let meta = KeyModifiers {
            meta: true,
            ..KeyModifiers::default()
        };

        assert_eq!(
            resolve_shell_action(&bindings, key_codes::KEY_LEFT, ctrl_alt),
            Some(ShellAction::WorkspaceLeft)
        );
        assert_eq!(
            resolve_shell_action(&bindings, key_codes::KEY_LEFT, meta),
            None
        );
        assert_eq!(
            resolve_shell_action(&bindings, key_codes::KEY_RIGHT, meta),
            Some(ShellAction::WorkspaceRight)
        );
    }

    #[test]
    fn overview_toggle_chords_join_the_shared_action_table() {
        let bindings =
            load_shell_action_bindings("[keybindings]\noverview_toggle = \"Super+Space\"\n");
        let meta = KeyModifiers {
            meta: true,
            ..KeyModifiers::default()
        };

        assert_eq!(
            resolve_shell_action(&bindings, key_codes::KEY_SPACE, meta),
            Some(ShellAction::OverviewToggle)
        );
    }

    #[test]
    fn workspace_lifecycle_bindings_share_the_exact_action_table() {
        let bindings = load_shell_action_bindings(
            "[keybindings]\nadd_workspace = \"Ctrl+Alt+N\"\nremove_workspace = \"Super+Delete\"\n",
        );
        let ctrl_alt = KeyModifiers {
            ctrl: true,
            alt: true,
            ..KeyModifiers::default()
        };
        let meta = KeyModifiers {
            meta: true,
            ..KeyModifiers::default()
        };

        assert_eq!(
            resolve_shell_action(&bindings, key_codes::KEY_N, ctrl_alt),
            Some(ShellAction::AddWorkspace)
        );
        assert_eq!(
            resolve_shell_action(&bindings, key_codes::KEY_DELETE, meta),
            Some(ShellAction::RemoveWorkspace)
        );
        assert_eq!(
            resolve_shell_action(
                &bindings,
                key_codes::KEY_N,
                KeyModifiers {
                    meta: true,
                    shift: true,
                    ..KeyModifiers::default()
                }
            ),
            None
        );
    }

    #[test]
    fn automatic_workspace_removal_is_an_explicit_boolean_policy() {
        let manual = "[workspaces]\nauto_remove_empty = false\n";
        let automatic = "[workspaces]\nauto_remove_empty = true\n";
        let unrelated = "[output]\nauto_remove_empty = true\n";

        assert_eq!(parse_auto_remove_empty_workspaces(manual), Some(false));
        assert_eq!(parse_auto_remove_empty_workspaces(automatic), Some(true));
        assert_eq!(parse_auto_remove_empty_workspaces(unrelated), None);
    }
}

fn strip_toml_comment(line: &str) -> &str {
    let mut in_string = false;
    for (index, ch) in line.char_indices() {
        match ch {
            '"' => in_string = !in_string,
            '#' if !in_string => return &line[..index],
            _ => {}
        }
    }
    line
}

fn trim_toml_string(value: &str) -> &str {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].trim()
    } else {
        value
    }
}

fn parse_u32_value(value: &str) -> Option<u32> {
    trim_toml_string(value).parse::<u32>().ok()
}

fn parse_scale_value_milli(value: &str) -> Option<u32> {
    let value = trim_toml_string(value);
    if value.is_empty() {
        return None;
    }

    let mut parts = value.split('.');
    let whole = parts.next()?.parse::<u32>().ok()?;
    let frac = parts.next();
    if parts.next().is_some() {
        return None;
    }

    let mut frac_milli = 0u32;
    if let Some(frac) = frac {
        let mut factor = 100;
        for ch in frac.chars().take(3) {
            let digit = ch.to_digit(10)?;
            frac_milli = frac_milli.saturating_add(digit.saturating_mul(factor));
            factor /= 10;
        }
    }

    Some(whole.saturating_mul(1000).saturating_add(frac_milli))
}

fn normalize_scale_milli(scale_milli: u32) -> u32 {
    scale_milli.clamp(250, 8000)
}

fn cursor_render_scale_milli(output_scale_milli: u32, image_scale_milli: u32) -> u32 {
    let numerator = u64::from(output_scale_milli).saturating_mul(1000);
    let denominator = u64::from(image_scale_milli.max(1));
    ((numerator.saturating_add(denominator / 2)) / denominator)
        .max(1)
        .min(u64::from(u32::MAX)) as u32
}

fn load_cursor_theme(theme_path: &str, output_scale_milli: u32) -> Result<Cursor, &'static str> {
    let theme = CursorTheme::load(theme_path)?;
    let render_scale_milli =
        cursor_render_scale_milli(output_scale_milli, theme.image_scale_milli());
    let arrow = theme
        .image(sws_protocol::CursorIcon::Arrow)
        .ok_or("Cursor theme does not contain an arrow image")?;
    let mut cursor = Cursor::from_png_file(
        &arrow.image_path,
        render_scale_milli,
        arrow.hotspot_x,
        arrow.hotspot_y,
    )?;
    let mut loaded = 1;
    for image in theme.images() {
        if image.icon == sws_protocol::CursorIcon::Arrow {
            continue;
        }
        match cursor.load_png_icon(
            image.icon,
            &image.image_path,
            render_scale_milli,
            image.hotspot_x,
            image.hotspot_y,
        ) {
            Ok(()) => loaded += 1,
            Err(error) => println!(
                "[Compositor] Cursor theme image {} unavailable: {}",
                image.image_path, error
            ),
        }
    }
    println!(
        "[Compositor] Cursor theme: {} from {} ({} of {} images loaded; asset scale {}.{:03})",
        theme.name(),
        theme_path,
        loaded,
        theme.images().len(),
        theme.image_scale_milli() / 1000,
        theme.image_scale_milli() % 1000
    );
    Ok(cursor)
}

fn is_installed_cursor_theme_path(theme_path: &str) -> bool {
    let Some(theme_name) = theme_path.strip_prefix(INSTALLED_CURSOR_THEME_ROOT) else {
        return false;
    };
    !theme_name.is_empty()
        && theme_name != "."
        && theme_name != ".."
        && !theme_name.contains('/')
        && !theme_name.contains('\0')
}

fn is_shell_app_id(app_id: &[u8]) -> bool {
    app_id == b"org.scarlet-os.desktop.taskbar"
        || app_id == b"org.scarlet-os.desktop.background"
        || app_id == b"org.scarlet-os.desktop.desktop"
        || app_id == b"org.scarlet-os.desktop.launcher"
        || app_id == b"org.scarlet-os.desktop.shell"
        || app_id == b"org.scarlet-os.desktop.shell.home"
}

#[derive(Debug, Clone, Copy)]
struct PendingFrameCallback {
    client_id: usize,
    window_id: u32,
    callback_id: u64,
    /// Presentation counter observed when the request entered the compositor.
    requested_after_present: u64,
}

const fn frame_callback_is_ready(
    is_presented: bool,
    has_submitted_frame: bool,
    presentation_counter: u64,
    requested_after_present: u64,
) -> bool {
    is_presented && (!has_submitted_frame || presentation_counter > requested_after_present)
}

/// Compositor - the main window server with proper layer compositing
pub struct Compositor {
    display: DisplaySurface,
    backend: SwsBackend,
    gpu_compositor: Option<GpuCompositor>,
    window_manager: WindowManager,
    ipc_server: IpcServer,
    remote_server: RemoteServer,
    capture_session: CaptureSession,
    wake_read: Handle,
    cursor: Cursor,
    screen_width: u32,
    screen_height: u32,
    output_scale_milli: u32,
    bg_color: [u8; 4],
    bytes_per_pixel: u32,
    backbuffer: Vec<u8>,
    backbuffer_stride: u32,
    full_redraw_needed: bool,
    pending_damage: Vec<(i32, i32, u32, u32)>,
    presented_damage: Vec<PresentDamage>,
    event_counter: u64,
    pending_frame_callbacks: Vec<PendingFrameCallback>,
    next_frame_deadline_ns: Option<u64>,
    left_button_down: bool,
    overview_pointer_navigation: Option<OverviewPointerNavigation>,
    overview_window_drag: Option<OverviewWindowDrag>,
    overview_add_workspace_selected: bool,
    overview_last_scroll_step_ns: u64,
    last_left_down_cursor: Option<(i32, i32)>,
    /// Window currently owning pointer hover while no implicit button grab is active.
    pointer_focus_window_id: Option<u32>,
    pointer_grab_window_id: Option<u32>,
    /// Explicit client-owned pointer capture, independent of implicit button grabs.
    pointer_lock: Option<PointerLockState>,
    move_drag: Option<MoveDragState>,
    resize_drag: Option<ResizeDragState>,
    resize_outline: Option<(i32, i32, u32, u32)>,
    workarea: Option<(i32, i32, u32, u32)>,
    /// Track the currently active application's app_id to avoid redundant ACTIVE_APP_CHANGED broadcasts
    active_app_id: Option<Vec<u8>>,
    /// Track the last focused window ID to avoid redundant FOCUS_CHANGED broadcasts
    last_focused_window_id: Option<u32>,
    /// Last focused workspace scene, retained across shell-surface focus.
    last_workspace_focus: Option<u32>,
    /// Scene to restore after leaving Home or Overview.
    overview_restore_focus: Option<u32>,
    held_keys: HeldKeys,
    shell_action_bindings: Vec<(KeyBinding, ShellAction)>,
    overview_super_tap: bool,
    super_tap_state: ModifierTapState,
    workspace_shortcut_keys: ConsumedKeys,
    ime_toggle_bindings: Vec<KeyBinding>,
    ime_trigger_keys: ConsumedKeys,
    key_repeat: KeyRepeatState,
    gesture_recognizer: GestureRecognizer,
    direct_touch_grabs: Vec<DirectTouchGrab>,
    system_touch_navigation: Option<SystemTouchNavigation>,
    input_modality: InputModality,
    tablet_mode: bool,
    windowing_mode: sws_protocol::WindowingMode,
    workspace_manager: super::workspace::WorkspaceManager,
    /// Normal surfaces awaiting a definitive top-level/transient relationship.
    ///
    /// Parent assignment is a separate IPC request, so tablet launch policy is
    /// committed on the first submitted frame instead of at window creation.
    pending_workspace_scenes: Vec<u32>,
    /// Apply geometry-changing startup policy only after the initial client
    /// frame has been presented with its original buffer identity.
    window_policy_after_present: bool,
    lid_closed: bool,
    ime_popup_windows: Vec<ImePopupWindow>,
    next_activation_token_serial: u64,
    activation_tokens: Vec<ActivationRecord>,
}

#[derive(Debug, Clone, Copy)]
struct ImePopupWindow {
    context_id: u32,
    window_id: u32,
    offset_x: i32,
    offset_y: i32,
    visible: bool,
}

#[derive(Debug, Clone, Copy)]
struct DirectTouchGrab {
    source: PointerSource,
    tracking_id: i32,
    window_id: u32,
    legacy_primary: bool,
    /// This contact has taken over a client-requested interactive move.
    driving_move_drag: bool,
    screen_x: i32,
    screen_y: i32,
}

#[derive(Debug, Clone, Copy)]
struct SystemTouchNavigation {
    source: PointerSource,
    tracking_id: i32,
    start_time_ns: u64,
    start_x: i32,
    start_y: i32,
    current_x: i32,
    current_y: i32,
    origin: sws_protocol::workspace::ShellPresentation,
    drag_window_id: Option<u32>,
    remove_workspace_id: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
struct OverviewPointerNavigation {
    start_x: i32,
    start_y: i32,
    start_workspace_id: Option<u32>,
    start_add_workspace: bool,
    start_remove_workspace_id: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
struct OverviewWindowDrag {
    window_id: u32,
    source_workspace_id: u32,
    from_workspace_thumbnail: bool,
    start_x: i32,
    start_y: i32,
    current_x: i32,
    current_y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectLegacyEventKind {
    Press,
    Move,
    Release,
    Cancel,
}

type DirectLegacyInputEvent = (u16, u16, i32);
// Window-local coordinates are non-negative.  A fixed sentinel avoids any
// arithmetic on the window geometry and therefore cannot overflow.
const DIRECT_LEGACY_OUTSIDE_COORD: i32 = -1;

fn direct_legacy_event_sequence(
    local_x: i32,
    local_y: i32,
    kind: DirectLegacyEventKind,
) -> Vec<DirectLegacyInputEvent> {
    let abs = super::input::event_types::EV_ABS;
    let key = super::input::event_types::EV_KEY;
    let syn = super::input::event_types::EV_SYN;
    let x = super::input::abs_codes::ABS_X;
    let y = super::input::abs_codes::ABS_Y;
    let button = key_codes::BTN_LEFT;
    let outside = DIRECT_LEGACY_OUTSIDE_COORD;

    match kind {
        DirectLegacyEventKind::Press => vec![
            (abs, x, local_x),
            (abs, y, local_y),
            (key, button, 1),
            (syn, 0, 0),
        ],
        DirectLegacyEventKind::Move => vec![(abs, x, local_x), (abs, y, local_y), (syn, 0, 0)],
        DirectLegacyEventKind::Release => vec![
            (abs, x, local_x),
            (abs, y, local_y),
            (key, button, 0),
            (syn, 0, 0),
            (abs, x, outside),
            (abs, y, outside),
            (syn, 0, 0),
        ],
        DirectLegacyEventKind::Cancel => vec![
            (abs, x, outside),
            (abs, y, outside),
            (key, button, 0),
            (syn, 0, 0),
        ],
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct InputModality {
    cursor_hidden_by_touch: bool,
}

impl InputModality {
    fn direct_touch(&mut self) -> bool {
        core::mem::replace(&mut self.cursor_hidden_by_touch, true) != true
    }

    fn pointer_motion(&mut self) -> bool {
        core::mem::replace(&mut self.cursor_hidden_by_touch, false)
    }
}

fn normalized_touch_to_screen(value: i32, dimension: u32) -> i32 {
    (i64::from(value.clamp(0, TOUCH_COORD_MAX))
        .saturating_mul(i64::from(dimension.saturating_sub(1)))
        / i64::from(TOUCH_COORD_MAX)) as i32
}

fn interactive_move_grab_origin(
    mouse_button_down: bool,
    direct_touch: Option<(i32, i32)>,
    last_mouse_down: Option<(i32, i32)>,
    cursor: (i32, i32),
) -> Option<(i32, i32)> {
    direct_touch.or_else(|| mouse_button_down.then(|| last_mouse_down.unwrap_or(cursor)))
}

#[derive(Debug, Clone, Copy)]
struct MoveDragState {
    window_id: u32,
    grab_cursor_x: i32,
    grab_cursor_y: i32,
    start_window_x: i32,
    start_window_y: i32,
}

#[derive(Debug, Clone, Copy)]
struct ResizeDragState {
    window_id: u32,
    icon: sws_protocol::CursorIcon,
    grab_cursor_x: i32,
    grab_cursor_y: i32,
    start_width: u32,
    start_height: u32,
    last_width: u32,
    last_height: u32,
}

fn resize_outline_for_surface(
    surface_x: i32,
    surface_y: i32,
    surface_width: u32,
    surface_height: u32,
    insets: sws_protocol::WindowGeometryInsets,
) -> (i32, i32, u32, u32) {
    (
        surface_x.saturating_add(insets.left as i32),
        surface_y.saturating_add(insets.top as i32),
        surface_width.saturating_sub(insets.horizontal()).max(1),
        surface_height.saturating_sub(insets.vertical()).max(1),
    )
}

const RESIZE_GRIP_PX: i32 = 8;
const MIN_WINDOW_WIDTH: u32 = 64;
const MIN_WINDOW_HEIGHT: u32 = 64;
const ACTIVATION_TOKEN_TTL_NS: u64 = 10_000_000_000;
const MAX_PENDING_ACTIVATION_TOKENS: usize = 64;
const DEFAULT_WINDOW_INSET: i64 = 32;
const DEFAULT_WINDOW_CASCADE: i64 = 28;
const OVERVIEW_SCROLL_STEP_INTERVAL_NS: u64 = 120_000_000;

#[derive(Debug, Clone)]
struct ActivationRecord {
    token: String,
    source_window_id: u32,
    source_app_id: Vec<u8>,
    target_app_id: Vec<u8>,
    created_at_ns: u64,
}

impl Compositor {
    /// Create a new compositor
    pub fn new() -> Result<Self, &'static str> {
        println!("[Compositor] Starting initialization...");

        // Open the modern display endpoint. The legacy framebuffer node remains
        // a compatibility path for direct framebuffer clients.
        let display = DisplaySurface::open_primary().map_err(|_| "Failed to open display")?;
        println!(
            "[Compositor] Scanout swap: {}",
            if display.has_swapchain() {
                "enabled"
            } else {
                "unavailable"
            }
        );

        // Get screen dimensions
        let var_info = display
            .get_var_screen_info()
            .map_err(|_| "Failed to get screen info")?;

        let fix_info = display
            .get_fix_screen_info()
            .map_err(|_| "Failed to get fixed screen info")?;

        let screen_width = var_info.xres;
        let screen_height = var_info.yres;
        let backend = selected_sws_backend()?;
        let sws_config = load_sws_config();
        let output_scale_milli = sws_config.output_scale_milli;
        let bytes_per_pixel = 4; // BGRA

        println!("[Compositor] Screen: {}x{}", screen_width, screen_height);
        println!(
            "[Compositor] Output scale: {}.{}",
            output_scale_milli / 1000,
            output_scale_milli % 1000
        );
        println!(
            "[Compositor] Shell action bindings: {}",
            sws_config.shell_action_bindings.len()
        );
        println!(
            "[Compositor] IME toggle bindings: {}",
            sws_config.ime_toggle_bindings.len()
        );
        println!(
            "[Compositor] Preferred input method: {}",
            sws_config
                .preferred_input_method_name
                .as_deref()
                .unwrap_or("automatic")
        );
        println!(
            "[Compositor] Framebuffer: bpp={} line_length={} smem_len={}",
            var_info.bits_per_pixel, fix_info.line_length, fix_info.smem_len
        );
        println!("[Compositor] Selected backend: {:?}", backend);

        let (wake_read, wake_write) =
            scarlet_os::ipc::pipe().map_err(|_| "Failed to create compositor wake pipe")?;
        super::ipc::set_compositor_wake_handle(wake_write);
        super::ipc::set_preferred_input_method(sws_config.preferred_input_method_name.clone());

        // Initialize IPC server
        let mut ipc_server = IpcServer::new("/tmp/sws.sock")?;
        let mut remote_server = RemoteServer::new("/tmp/sws-remote.sock");
        // Claim both public endpoints before starting any compositor workers.
        // A stale or live server address must fail initialization without
        // leaving input, GPU, or accept threads behind.
        ipc_server.bind()?;
        remote_server.bind()?;

        // Start input threads only after both server addresses are secured.
        InputManager::start_input_thread(screen_width, screen_height)?;

        // Initialize window manager
        let window_manager = WindowManager::new();

        // Initialize the configured theme at the center of the output. Keep a
        // built-in arrow available so a damaged manifest or image cannot prevent
        // the window server from starting.
        let mut cursor = match load_cursor_theme(&sws_config.cursor.theme_path, output_scale_milli)
        {
            Ok(cursor) => cursor,
            Err(error) => {
                println!(
                    "[Compositor] Failed to load cursor theme {}: {}; using built-in cursor",
                    sws_config.cursor.theme_path, error
                );
                Cursor::fallback(output_scale_milli)
            }
        };
        cursor.x = (screen_width / 2) as i32;
        cursor.y = (screen_height / 2) as i32;
        // Keep prev position consistent to avoid an oversized first dirty region.
        cursor.mark_drawn();

        // Slightly desaturated charcoal background to better fit desktop surfaces.
        let bg_color = [24, 28, 36, 255];

        let backbuffer_stride = screen_width * bytes_per_pixel;
        let buffer_size = (screen_width * screen_height * bytes_per_pixel) as usize;
        let mut backbuffer = Vec::with_capacity(buffer_size);
        backbuffer.resize(buffer_size, 0);

        let gpu_compositor = match backend {
            SwsBackend::Cpu => {
                println!("[Compositor] CPU composition forced by SWS_BACKEND");
                None
            }
            SwsBackend::Auto => match GpuCompositor::new(screen_width, screen_height, &cursor) {
                Ok(compositor) => {
                    println!("[Compositor] GPU composition enabled");
                    Some(compositor)
                }
                Err(error) => {
                    println!("[Compositor] GPU composition unavailable: {}", error);
                    None
                }
            },
            SwsBackend::Sgfx => Some(
                GpuCompositor::new(screen_width, screen_height, &cursor)
                    .map_err(|_| "SWS_BACKEND=sgfx requested but GPU initialization failed")?,
            ),
        };
        super::ipc::set_sgfx_shared_images_available(gpu_compositor.is_some());
        let overview_super_tap = sws_config
            .overview_toggle_bindings
            .iter()
            .any(|binding| matches!(binding, OverviewToggleBinding::SuperTap));
        let auto_remove_empty_workspaces = sws_config.auto_remove_empty_workspaces;
        let input_environment = input_environment::snapshot();
        println!(
            "[Compositor] Input environment: generation={} tablet={} lid_closed={} windowing={:?} tablet_override={} windowing_override={} capabilities={:#x}",
            input_environment.generation,
            input_environment.tablet_mode(),
            input_environment.lid_closed(),
            input_environment.windowing_mode(),
            input_environment.tablet_mode_override_active(),
            input_environment.windowing_mode_override_active(),
            input_environment.capability_flags,
        );
        super::ipc::set_window_creation_environment(
            screen_width,
            screen_height,
            None,
            input_environment.windowing_mode(),
        );
        ipc_server.listen()?;
        remote_server.listen()?;

        let mut gesture_recognizer = GestureRecognizer::new(screen_width, screen_height);
        // No contact can be active during construction, so this only installs
        // the first-frame filtering policy.
        let _ = gesture_recognizer.set_tablet_mode(input_environment.tablet_mode());

        Ok(Self {
            display,
            backend,
            gpu_compositor,
            window_manager,
            ipc_server,
            remote_server,
            capture_session: CaptureSession::new(screen_width, screen_height),
            wake_read,
            cursor,
            screen_width,
            screen_height,
            output_scale_milli,
            bg_color,
            bytes_per_pixel,
            backbuffer,
            backbuffer_stride,
            full_redraw_needed: true,
            pending_damage: Vec::new(),
            presented_damage: Vec::new(),
            event_counter: 0,
            pending_frame_callbacks: Vec::new(),
            next_frame_deadline_ns: None,
            left_button_down: false,
            overview_pointer_navigation: None,
            overview_window_drag: None,
            overview_add_workspace_selected: false,
            overview_last_scroll_step_ns: 0,
            last_left_down_cursor: None,
            pointer_focus_window_id: None,
            pointer_grab_window_id: None,
            pointer_lock: None,
            move_drag: None,
            resize_drag: None,
            resize_outline: None,
            workarea: None,
            active_app_id: None,
            last_focused_window_id: None,
            last_workspace_focus: None,
            overview_restore_focus: None,
            held_keys: HeldKeys::default(),
            shell_action_bindings: sws_config.shell_action_bindings,
            overview_super_tap,
            super_tap_state: ModifierTapState::default(),
            workspace_shortcut_keys: ConsumedKeys::default(),
            ime_toggle_bindings: sws_config.ime_toggle_bindings,
            ime_trigger_keys: ConsumedKeys::default(),
            key_repeat: KeyRepeatState::default(),
            gesture_recognizer,
            direct_touch_grabs: Vec::new(),
            system_touch_navigation: None,
            input_modality: InputModality::default(),
            tablet_mode: input_environment.tablet_mode(),
            windowing_mode: input_environment.windowing_mode(),
            workspace_manager: super::workspace::WorkspaceManager::with_auto_remove_empty(
                auto_remove_empty_workspaces,
            ),
            pending_workspace_scenes: Vec::new(),
            window_policy_after_present: false,
            lid_closed: input_environment.lid_closed(),
            ime_popup_windows: Vec::new(),
            next_activation_token_serial: 1,
            activation_tokens: Vec::new(),
        })
    }

    fn current_key_modifiers(&self) -> KeyModifiers {
        KeyModifiers {
            ctrl: self
                .held_keys
                .has_any(&[key_codes::KEY_LEFTCTRL, key_codes::KEY_RIGHTCTRL]),
            shift: self
                .held_keys
                .has_any(&[key_codes::KEY_LEFTSHIFT, key_codes::KEY_RIGHTSHIFT]),
            alt: self
                .held_keys
                .has_any(&[key_codes::KEY_LEFTALT, key_codes::KEY_RIGHTALT]),
            meta: self
                .held_keys
                .has_any(&[key_codes::KEY_LEFTMETA, key_codes::KEY_RIGHTMETA]),
        }
    }

    fn release_keyboard_source(&mut self, source: KeyboardSource) -> Result<(), &'static str> {
        self.super_tap_state.cancel_source(source);
        let held_codes = self.held_keys.codes_for_source(source);
        for code in held_codes {
            self.handle_input_event(CompositorInputEvent::Keyboard {
                code,
                value: 0,
                source,
                synthetic: false,
            })?;
        }
        self.key_repeat.cancel_source(source);
        self.workspace_shortcut_keys.drain_source(source);
        self.ime_trigger_keys.drain_source(source);
        Ok(())
    }

    fn live_workspace_window_ids(&self) -> Vec<u32> {
        self.window_manager
            .get_windows()
            .iter()
            .filter(|window| {
                window.window_type == WindowType::Normal
                    && window.parent.is_none()
                    && !is_shell_app_id(window.app_id.as_deref().unwrap_or(b""))
            })
            .map(|window| window.id)
            .collect()
    }

    fn publish_workspace_state(&self) {
        super::ipc::publish_workspace_state(sws_protocol::workspace::encode_state(
            &self.workspace_manager.snapshot(),
        ));
    }

    fn is_ime_toggle_key(&self, code: u16) -> bool {
        let modifiers = self.current_key_modifiers();
        self.ime_toggle_bindings
            .iter()
            .any(|binding| binding.code == code && binding.modifiers == modifiers)
    }

    fn uses_super_tap_for_overview(&self) -> bool {
        self.overview_super_tap
    }

    fn shell_action_for(&self, code: u16, modifiers: KeyModifiers) -> Option<ShellAction> {
        resolve_shell_action(&self.shell_action_bindings, code, modifiers)
    }

    fn toggle_overview_presentation(&mut self) -> bool {
        let changed = self.workspace_manager.toggle_overview();
        if changed {
            self.commit_workspace_change();
        }
        changed
    }

    fn commit_workspace_change(&mut self) {
        self.overview_add_workspace_selected = false;
        self.apply_workspace_presentation_policy();
        self.publish_workspace_state();
        self.full_redraw_needed = true;
    }

    fn navigate_workspace(&mut self, direction: i32) -> bool {
        if shell_workspace_rail_accepts_selection(self.workspace_manager.presentation()) {
            return self.move_overview_selection(direction);
        }
        let changed = self.workspace_manager.cycle_workspace(direction);
        if changed {
            self.commit_workspace_change();
        }
        changed
    }

    fn activate_shell_workspace_selection(&mut self) -> bool {
        if !shell_workspace_rail_accepts_selection(self.workspace_manager.presentation()) {
            return false;
        }
        let changed = if self.overview_add_workspace_selected {
            self.workspace_manager.create_workspace().is_some()
        } else {
            let workspace_id = self.workspace_manager.active_workspace();
            self.workspace_manager
                .select_workspace_from_overview(workspace_id)
        };
        if changed {
            self.commit_workspace_change();
        }
        changed
    }

    fn run_shell_action(&mut self, action: ShellAction) {
        match action {
            ShellAction::OverviewToggle => {
                self.toggle_overview_presentation();
            }
            ShellAction::Home => {
                let changed = if self.workspace_manager.presentation()
                    == sws_protocol::workspace::ShellPresentation::Home
                {
                    self.workspace_manager.return_to_workspace()
                } else {
                    self.workspace_manager
                        .set_presentation(sws_protocol::workspace::ShellPresentation::Home)
                };
                if changed {
                    self.commit_workspace_change();
                }
            }
            ShellAction::WorkspaceLeft => {
                self.navigate_workspace(-1);
            }
            ShellAction::WorkspaceRight => {
                self.navigate_workspace(1);
            }
            ShellAction::MoveWindowLeft => {
                self.move_focused_window_to_adjacent_workspace(-1);
            }
            ShellAction::MoveWindowRight => {
                self.move_focused_window_to_adjacent_workspace(1);
            }
            ShellAction::OverviewActivate => {
                self.activate_shell_workspace_selection();
            }
            ShellAction::AddWorkspace => {
                if self.workspace_manager.create_workspace().is_some() {
                    self.commit_workspace_change();
                }
            }
            ShellAction::RemoveWorkspace => {
                let workspace_id = self.workspace_manager.active_workspace();
                let allow_freeform_merge =
                    self.windowing_mode == sws_protocol::WindowingMode::Freeform;
                if self
                    .workspace_manager
                    .remove_workspace(workspace_id, allow_freeform_merge)
                {
                    self.commit_workspace_change();
                }
            }
        }
    }

    fn move_focused_window_to_adjacent_workspace(&mut self, direction: i32) -> bool {
        let Some(focused_id) = self.window_manager.get_focused_window_id() else {
            return false;
        };
        let root_id = self.top_level_window_id(focused_id);
        if root_id == 0 {
            return false;
        }
        let changed = self
            .workspace_manager
            .move_window_to_adjacent_workspace(root_id, direction);
        if changed {
            self.commit_workspace_change();
        }
        changed
    }

    fn draw_outline_rect_to_buffer(
        screen_width: u32,
        screen_height: u32,
        bytes_per_pixel: u32,
        buffer: &mut [u8],
        stride: u32,
        rect: (i32, i32, u32, u32),
        clip_rect: Option<(i32, i32, u32, u32)>,
    ) {
        let (x, y, w, h) = rect;
        if w == 0 || h == 0 {
            return;
        }

        // High-contrast outline (outer black, inner white).
        // This stays visible regardless of the window background.
        let outer = [0u8, 0u8, 0u8, 255u8];
        let inner = [255u8, 255u8, 255u8, 255u8];

        let x0 = x;
        let y0 = y;
        let x1 = x.saturating_add(w as i32).saturating_sub(1);
        let y1 = y.saturating_add(h as i32).saturating_sub(1);

        let mut draw_outline = |rx0: i32, ry0: i32, rx1: i32, ry1: i32, color: [u8; 4]| {
            if rx1 < rx0 || ry1 < ry0 {
                return;
            }

            // Top/bottom
            for sx in rx0..=rx1 {
                for sy in [ry0, ry1] {
                    if sx < 0 || sx >= screen_width as i32 || sy < 0 || sy >= screen_height as i32 {
                        continue;
                    }
                    if let Some((clip_x, clip_y, clip_w, clip_h)) = clip_rect {
                        if sx < clip_x
                            || sx >= clip_x + clip_w as i32
                            || sy < clip_y
                            || sy >= clip_y + clip_h as i32
                        {
                            continue;
                        }
                    }
                    let off = ((sy as u32 * stride) + (sx as u32 * bytes_per_pixel)) as usize;
                    if off + 4 <= buffer.len() {
                        buffer[off] = color[0];
                        buffer[off + 1] = color[1];
                        buffer[off + 2] = color[2];
                        buffer[off + 3] = color[3];
                    }
                }
            }

            // Left/right
            for sy in ry0..=ry1 {
                for sx in [rx0, rx1] {
                    if sx < 0 || sx >= screen_width as i32 || sy < 0 || sy >= screen_height as i32 {
                        continue;
                    }
                    if let Some((clip_x, clip_y, clip_w, clip_h)) = clip_rect {
                        if sx < clip_x
                            || sx >= clip_x + clip_w as i32
                            || sy < clip_y
                            || sy >= clip_y + clip_h as i32
                        {
                            continue;
                        }
                    }
                    let off = ((sy as u32 * stride) + (sx as u32 * bytes_per_pixel)) as usize;
                    if off + 4 <= buffer.len() {
                        buffer[off] = color[0];
                        buffer[off + 1] = color[1];
                        buffer[off + 2] = color[2];
                        buffer[off + 3] = color[3];
                    }
                }
            }
        };

        // Outer black outline.
        draw_outline(x0, y0, x1, y1, outer);

        // Inner white outline (1px inset) when possible.
        if w > 2 && h > 2 {
            draw_outline(x0 + 1, y0 + 1, x1 - 1, y1 - 1, inner);
        }
    }

    fn fill_rounded_rect_to_buffer(
        screen_width: u32,
        screen_height: u32,
        bytes_per_pixel: u32,
        buffer: &mut [u8],
        stride: u32,
        rect: (i32, i32, u32, u32),
        radius: u32,
        color: [u8; 4],
        clip_rect: Option<(i32, i32, u32, u32)>,
    ) {
        let (x, y, width, height) = rect;
        let left = x.max(0);
        let top = y.max(0);
        let mut right = x.saturating_add(width as i32).min(screen_width as i32);
        let mut bottom = y.saturating_add(height as i32).min(screen_height as i32);
        let mut clipped_left = left;
        let mut clipped_top = top;
        if let Some((clip_x, clip_y, clip_width, clip_height)) = clip_rect {
            clipped_left = clipped_left.max(clip_x);
            clipped_top = clipped_top.max(clip_y);
            right = right.min(clip_x.saturating_add(clip_width as i32));
            bottom = bottom.min(clip_y.saturating_add(clip_height as i32));
        }
        if right <= clipped_left || bottom <= clipped_top {
            return;
        }

        for row_y in clipped_top..bottom {
            let Some((rounded_left, rounded_right)) = rounded_rect_row_span(rect, radius, row_y)
            else {
                continue;
            };
            let row_left = clipped_left.max(rounded_left);
            let row_right = right.min(rounded_right);
            if row_right <= row_left {
                continue;
            }
            let row_offset = (row_y as u32 * stride) as usize;
            for screen_x in row_left..row_right {
                let offset = row_offset + (screen_x as u32 * bytes_per_pixel) as usize;
                if offset + 4 <= buffer.len() {
                    let alpha = u32::from(color[3]);
                    let inverse_alpha = 255u32.saturating_sub(alpha);
                    for channel in 0..3 {
                        let source = u32::from(color[channel]);
                        let destination = u32::from(buffer[offset + channel]);
                        buffer[offset + channel] =
                            ((source * alpha + destination * inverse_alpha + 127) / 255) as u8;
                    }
                    buffer[offset + 3] = 255;
                }
            }
        }
    }

    fn draw_overview_backplates_to_buffer(
        screen_width: u32,
        screen_height: u32,
        bytes_per_pixel: u32,
        buffer: &mut [u8],
        stride: u32,
        backplates: &[((i32, i32, u32, u32), bool, bool)],
        clip_rect: Option<(i32, i32, u32, u32)>,
    ) {
        for (rect, selected_or_hovered, add_workspace) in backplates {
            let color = if *add_workspace {
                if *selected_or_hovered {
                    [255, 255, 255, 34]
                } else {
                    [255, 255, 255, 18]
                }
            } else if *selected_or_hovered {
                sws_protocol::workspace::OVERVIEW_CARD_SELECTED_OVERLAY_BGRA
            } else {
                sws_protocol::workspace::OVERVIEW_CARD_INACTIVE_OVERLAY_BGRA
            };
            Self::fill_rounded_rect_to_buffer(
                screen_width,
                screen_height,
                bytes_per_pixel,
                buffer,
                stride,
                *rect,
                sws_protocol::workspace::OVERVIEW_CARD_CORNER_RADIUS,
                color,
                clip_rect,
            );
            if !*add_workspace {
                continue;
            }

            let short_side = rect.2.min(rect.3);
            let arm = (short_side / 5).clamp(14, 52);
            let thickness = (short_side / 48).clamp(2, 6);
            let center_x = rect.0.saturating_add((rect.2 / 2) as i32);
            let center_y = rect.1.saturating_add((rect.3 / 2) as i32);
            let plus_color = [255, 255, 255, if *selected_or_hovered { 220 } else { 170 }];
            Self::fill_rounded_rect_to_buffer(
                screen_width,
                screen_height,
                bytes_per_pixel,
                buffer,
                stride,
                (
                    center_x.saturating_sub((arm / 2) as i32),
                    center_y.saturating_sub((thickness / 2) as i32),
                    arm,
                    thickness,
                ),
                thickness / 2,
                plus_color,
                clip_rect,
            );
            Self::fill_rounded_rect_to_buffer(
                screen_width,
                screen_height,
                bytes_per_pixel,
                buffer,
                stride,
                (
                    center_x.saturating_sub((thickness / 2) as i32),
                    center_y.saturating_sub((arm / 2) as i32),
                    thickness,
                    arm,
                ),
                thickness / 2,
                plus_color,
                clip_rect,
            );
        }
    }

    fn draw_overview_shadows_to_buffer(
        screen_width: u32,
        screen_height: u32,
        bytes_per_pixel: u32,
        buffer: &mut [u8],
        stride: u32,
        shadows: &[OverviewShadowLayer],
        clip_rect: Option<(i32, i32, u32, u32)>,
    ) {
        for (rect, radius, color) in shadows {
            Self::fill_rounded_rect_to_buffer(
                screen_width,
                screen_height,
                bytes_per_pixel,
                buffer,
                stride,
                *rect,
                *radius,
                *color,
                clip_rect,
            );
        }
    }

    fn draw_overview_remove_buttons_to_buffer(
        screen_width: u32,
        screen_height: u32,
        bytes_per_pixel: u32,
        buffer: &mut [u8],
        stride: u32,
        buttons: &[(u32, (i32, i32, u32, u32), bool)],
        clip_rect: Option<(i32, i32, u32, u32)>,
    ) {
        for (_, rect, hovered) in buttons {
            Self::fill_rounded_rect_to_buffer(
                screen_width,
                screen_height,
                bytes_per_pixel,
                buffer,
                stride,
                *rect,
                rect.2 / 2,
                [0, 0, 0, if *hovered { 118 } else { 82 }],
                clip_rect,
            );
            let center_x = rect.0.saturating_add((rect.2 / 2) as i32);
            let center_y = rect.1.saturating_add((rect.3 / 2) as i32);
            let half = (rect.2.min(rect.3) / 5).max(5) as i32;
            let thickness = (rect.2.min(rect.3) / 16).clamp(2, 3) as i32;
            for delta in -half..=half {
                for offset in -(thickness / 2)..=(thickness / 2) {
                    for (x, y) in [
                        (
                            center_x.saturating_add(delta),
                            center_y.saturating_add(delta).saturating_add(offset),
                        ),
                        (
                            center_x.saturating_add(delta),
                            center_y.saturating_sub(delta).saturating_add(offset),
                        ),
                    ] {
                        if x < 0 || y < 0 || x >= screen_width as i32 || y >= screen_height as i32 {
                            continue;
                        }
                        if let Some((clip_x, clip_y, clip_width, clip_height)) = clip_rect
                            && (x < clip_x
                                || y < clip_y
                                || x >= clip_x.saturating_add(clip_width as i32)
                                || y >= clip_y.saturating_add(clip_height as i32))
                        {
                            continue;
                        }
                        let index = ((y as u32 * stride) + x as u32 * bytes_per_pixel) as usize;
                        if index + 4 <= buffer.len() {
                            let alpha = if *hovered { 235u32 } else { 205u32 };
                            let inverse = 255 - alpha;
                            for channel in 0..3 {
                                buffer[index + channel] = ((255 * alpha
                                    + u32::from(buffer[index + channel]) * inverse
                                    + 127)
                                    / 255)
                                    as u8;
                            }
                            buffer[index + 3] = 255;
                        }
                    }
                }
            }
        }
    }

    fn add_resize_replacement_damage(
        &mut self,
        old_rect: (i32, i32, u32, u32),
        new_rect: (i32, i32, u32, u32),
    ) {
        let (ox, oy, ow, oh) = old_rect;
        let (nx, ny, nw, nh) = new_rect;

        let old_x1 = ox.saturating_add(ow as i32);
        let old_y1 = oy.saturating_add(oh as i32);
        let new_x1 = nx.saturating_add(nw as i32);
        let new_y1 = ny.saturating_add(nh as i32);

        let x0 = ox.min(nx);
        let y0 = oy.min(ny);
        let x1 = old_x1.max(new_x1);
        let y1 = old_y1.max(new_y1);

        if x1 > x0 && y1 > y0 {
            self.add_pending_damage((x0, y0, (x1 - x0) as u32, (y1 - y0) as u32));
        }
    }

    /// Initialize display state for the first compositor frame.
    ///
    /// The first present is intentionally deferred to [`Self::run`]. Display
    /// drivers may wait for a page-flip completion, so presenting here would
    /// make the service readiness notification depend on display hardware
    /// latency.
    pub fn init_display(&mut self) -> Result<(), &'static str> {
        println!("[Compositor] Initializing display...");

        println!("[Compositor] No debug windows created (clean desktop startup)");

        self.dump_memory_layout("after init_display (empty)");

        // Let the main loop perform the first composite after SWS has reported
        // readiness. In particular, Apple DCP can spend its full reply timeout
        // waiting for the initial swap completion. stemd must not interpret
        // that transient display delay as a failure to start the service.
        self.full_redraw_needed = true;

        println!("[Compositor] Display initialized; first present deferred");

        Ok(())
    }

    fn check_display_resize(&mut self) -> Result<bool, &'static str> {
        let var_info = self
            .display
            .get_var_screen_info()
            .map_err(|_| "Failed to get screen info")?;
        let new_width = var_info.xres;
        let new_height = var_info.yres;

        if new_width == 0
            || new_height == 0
            || (new_width == self.screen_width && new_height == self.screen_height)
        {
            return Ok(false);
        }

        println!(
            "[Compositor] Display resize: {}x{} -> {}x{}",
            self.screen_width, self.screen_height, new_width, new_height
        );

        self.display
            .refresh_mapping()
            .map_err(|_| "Failed to refresh display mapping")?;

        self.screen_width = new_width;
        self.screen_height = new_height;
        self.backbuffer_stride = new_width.saturating_mul(self.bytes_per_pixel);
        let buffer_size = new_width
            .saturating_mul(new_height)
            .saturating_mul(self.bytes_per_pixel) as usize;
        self.backbuffer.resize(buffer_size, 0);
        super::input::set_screen_size(new_width, new_height);
        self.cursor
            .set_position(self.cursor.x, self.cursor.y, new_width, new_height);

        let gpu_resize_failed = match self.gpu_compositor.as_mut() {
            Some(gpu_compositor) => gpu_compositor.resize_target(new_width, new_height).is_err(),
            None => false,
        };
        if gpu_resize_failed {
            println!("[Compositor] Disabling GPU composition after display resize failure");
            self.disable_gpu_after_runtime_failure("SWS_BACKEND=sgfx target resize failed")?;
        }
        self.capture_session.output_changed(new_width, new_height);

        let payload = sws_protocol::payload_screen_size(new_width, new_height);
        super::ipc::broadcast_message_to_all_clients(
            sws_protocol::server_msg::SCREEN_SIZE_CHANGED,
            payload.to_vec(),
        );

        let windows: Vec<(u32, super::window::WindowType, u32, bool)> = self
            .window_manager
            .get_windows()
            .iter()
            .map(|w| (w.id, w.window_type, w.height, w.fullscreen))
            .collect();
        let mut taskbar_height = 0;

        for (window_id, window_type, height, fullscreen) in windows {
            if fullscreen {
                println!(
                    "[Compositor] Configuring fullscreen window #{} to {}x{}",
                    window_id, new_width, new_height
                );
                self.window_manager
                    .resize_fullscreen_window(window_id, new_width, new_height);
                self.send_current_window_configure(window_id);
                continue;
            }
            match window_type {
                super::window::WindowType::Desktop
                | super::window::WindowType::ShellBackground
                | super::window::WindowType::ShellChrome => {
                    println!(
                        "[Compositor] Configuring DESKTOP window #{} to {}x{}",
                        window_id, new_width, new_height
                    );
                    self.window_manager
                        .resize_window_in_place(window_id, new_width, new_height);
                    let payload =
                        sws_protocol::payload_window_configure(window_id, new_width, new_height);
                    super::ipc::send_message_to_window(
                        window_id,
                        sws_protocol::server_msg::WINDOW_CONFIGURE,
                        payload.to_vec(),
                    );
                }
                super::window::WindowType::Taskbar => {
                    taskbar_height = taskbar_height.max(height);
                    println!(
                        "[Compositor] Configuring TASKBAR window #{} to {}x{}",
                        window_id, new_width, height
                    );
                    self.window_manager
                        .resize_window_in_place(window_id, new_width, height);
                    let payload =
                        sws_protocol::payload_window_configure(window_id, new_width, height);
                    super::ipc::send_message_to_window(
                        window_id,
                        sws_protocol::server_msg::WINDOW_CONFIGURE,
                        payload.to_vec(),
                    );
                }
                _ => {}
            }
        }

        if taskbar_height != 0 {
            let workarea_y = taskbar_height as i32;
            let workarea_height = new_height.saturating_sub(taskbar_height);
            self.workarea = Some((0, workarea_y, new_width, workarea_height));
            self.window_manager
                .set_workarea(0, workarea_y, new_width, workarea_height);
            self.reflow_maximized_windows_to_workarea();
        }
        self.publish_window_creation_environment();
        if self.windowing_mode == sws_protocol::WindowingMode::Focused {
            self.apply_windowing_mode_policy();
        }

        self.full_redraw_needed = true;
        self.pending_damage.clear();
        Ok(true)
    }

    fn dump_memory_layout(&self, reason: &str) {
        if !is_sws_debug_enabled() {
            return;
        }

        println!("[Compositor] === Memory layout dump: {} ===", reason);

        // Backbuffer lives on the heap; log its virtual range and fingerprint.
        // This helps detect accidental aliasing/corruption and confirms it doesn't overlap VRAM.
        let bb_start = self.backbuffer.as_ptr() as usize;
        let bb_len = self.backbuffer.len();
        let bb_end = bb_start.saturating_add(bb_len);
        let bb_fp = Self::buffer_fingerprint(&self.backbuffer);
        println!(
            "[Compositor] backbuffer: 0x{:x}..0x{:x} ({} bytes) stride={} fp=0x{:08x}",
            bb_start, bb_end, bb_len, self.backbuffer_stride, bb_fp
        );

        // Best-effort stack location hint: address of a local variable.
        // We don't know the full stack range here, but if this falls inside VRAM it is a red flag.
        let stack_marker: u8 = 0;
        let sp_hint = (&stack_marker as *const u8) as usize;
        println!("[Compositor] stack marker addr: 0x{:x}", sp_hint);

        if let Some((addr, size)) = self.display.get_mapping_info() {
            let vram_start = addr;
            let vram_end = addr.saturating_add(size);
            println!(
                "[Compositor] display mmap: 0x{:x}..0x{:x} ({} bytes)",
                vram_start, vram_end, size
            );

            let bb_overlap = bb_start < vram_end && vram_start < bb_end;
            if bb_overlap {
                println!("[Compositor] WARNING: backbuffer overlaps display mapping!");
            }

            if sp_hint >= vram_start && sp_hint < vram_end {
                println!("[Compositor] WARNING: stack marker is inside display mapping!");
            }
        } else {
            println!("[Compositor] display mmap: (unavailable)");
        }

        let mut ranges: Vec<(u32, usize, usize, usize)> = Vec::new();
        for w in self.window_manager.get_windows() {
            // Check for SHM-backed window
            if let Some(shm_addr) = w.shm_mapped_addr {
                let buffer_size = (w.width as usize)
                    .saturating_mul(w.height as usize)
                    .saturating_mul(4);
                let end = shm_addr.saturating_add(buffer_size);
                ranges.push((w.id, shm_addr, end, buffer_size));

                println!(
                    "[Compositor] window #{} SHM: 0x{:x}..0x{:x} ({} bytes) [SHM-backed]",
                    w.id, shm_addr, end, buffer_size
                );

                if let Some((vram_start, vram_size)) = self.display.get_mapping_info() {
                    let vram_end = vram_start.saturating_add(vram_size);
                    let overlap = shm_addr < vram_end && vram_start < end;
                    if overlap {
                        println!(
                            "[Compositor] WARNING: window #{} SHM overlaps display mapping!",
                            w.id
                        );
                    }
                }

                let overlap_bb = shm_addr < bb_end && bb_start < end;
                if overlap_bb {
                    println!(
                        "[Compositor] WARNING: window #{} SHM overlaps backbuffer!",
                        w.id
                    );
                }
            } else if let Some(ref buf) = w.buffer {
                // Legacy Vec-backed window
                let start = buf.as_ptr() as usize;
                let len = buf.len();
                let end = start.saturating_add(len);
                ranges.push((w.id, start, end, len));

                let fp = Self::buffer_fingerprint(buf);
                println!(
                    "[Compositor] window #{} buffer: 0x{:x}..0x{:x} ({} bytes) fp=0x{:08x}",
                    w.id, start, end, len, fp
                );

                if let Some((vram_start, vram_size)) = self.display.get_mapping_info() {
                    let vram_end = vram_start.saturating_add(vram_size);
                    let overlap = start < vram_end && vram_start < end;
                    if overlap {
                        println!(
                            "[Compositor] WARNING: window #{} buffer overlaps display mapping!",
                            w.id
                        );
                    }
                }

                let overlap_bb = start < bb_end && bb_start < end;
                if overlap_bb {
                    println!(
                        "[Compositor] WARNING: window #{} buffer overlaps backbuffer!",
                        w.id
                    );
                }
            } else {
                println!("[Compositor] window #{} buffer: (none)", w.id);
            }
        }

        // Check overlap between window buffers themselves (should never happen).
        if ranges.len() >= 2 {
            ranges.sort_by_key(|(_id, start, _end, _len)| *start);
            for i in 1..ranges.len() {
                let (prev_id, prev_start, prev_end, _prev_len) = ranges[i - 1];
                let (id, start, _end, _len) = ranges[i];
                if start < prev_end {
                    println!(
                        "[Compositor] WARNING: window buffers overlap: #{} (0x{:x}..0x{:x}) and #{} (starts 0x{:x})",
                        prev_id, prev_start, prev_end, id, start
                    );
                }
            }
        }

        // Best-effort: print current program break (sbrk(0)).
        // This is useful to see if heap grows towards the VRAM mapping.
        {
            use scarlet_sys::{Syscall, syscall1};
            let brk_now = syscall1(Syscall::Sbrk, 0);
            println!("[Compositor] sbrk(0) -> 0x{:x}", brk_now);
        }
    }

    fn buffer_fingerprint(buf: &[u8]) -> u32 {
        // Cheap fingerprint to detect unexpected buffer mutations.
        // Mix a small prefix + suffix and a stride sample to reduce overhead.
        let mut x: u32 = 0x811c_9dc5;

        let take = core::cmp::min(256, buf.len());
        for &b in &buf[..take] {
            x = x.rotate_left(5) ^ (b as u32);
        }

        if buf.len() > 256 {
            let tail_take = core::cmp::min(256, buf.len());
            for &b in &buf[buf.len() - tail_take..] {
                x = x.rotate_left(5) ^ (b as u32);
            }
        }

        // Sample every ~4KB to catch larger-scale corruption.
        let mut i = 0usize;
        while i < buf.len() {
            x = x.rotate_left(5) ^ (buf[i] as u32);
            i = i.saturating_add(4096);
        }

        x
    }

    /// Fill buffer with gradient (for testing, static method)
    #[allow(dead_code)]
    fn fill_buffer_gradient(buffer: &mut [u8], width: u32, height: u32, base_color: [u8; 4]) {
        for y in 0..height {
            for x in 0..width {
                let offset = ((y * width + x) * 4) as usize;
                if offset + 4 <= buffer.len() {
                    // Create gradient effect
                    let intensity =
                        (x as f32 / width as f32 * 0.5 + y as f32 / height as f32 * 0.5) as u8;
                    buffer[offset] = base_color[0].saturating_sub(intensity); // B
                    buffer[offset + 1] = base_color[1].saturating_sub(intensity); // G
                    buffer[offset + 2] = base_color[2].saturating_sub(intensity); // R
                    buffer[offset + 3] = base_color[3]; // A
                }
            }
        }
    }

    fn clamp_rect_to_screen(&self, rect: (i32, i32, u32, u32)) -> Option<(i32, i32, u32, u32)> {
        let (x, y, w, h) = rect;
        if w == 0 || h == 0 {
            return None;
        }

        let sx0 = x.max(0).min(self.screen_width as i32);
        let sy0 = y.max(0).min(self.screen_height as i32);
        let sx1 = (x.saturating_add(w as i32))
            .max(0)
            .min(self.screen_width as i32);
        let sy1 = (y.saturating_add(h as i32))
            .max(0)
            .min(self.screen_height as i32);

        let cw = (sx1 - sx0).max(0) as u32;
        let ch = (sy1 - sy0).max(0) as u32;
        if cw == 0 || ch == 0 {
            None
        } else {
            Some((sx0, sy0, cw, ch))
        }
    }

    fn rect_area(rect: (i32, i32, u32, u32)) -> u64 {
        u64::from(rect.2).saturating_mul(u64::from(rect.3))
    }

    fn union_damage_rect(a: (i32, i32, u32, u32), b: (i32, i32, u32, u32)) -> (i32, i32, u32, u32) {
        let ax1 = (a.0 as i64).saturating_add(a.2 as i64);
        let ay1 = (a.1 as i64).saturating_add(a.3 as i64);
        let bx1 = (b.0 as i64).saturating_add(b.2 as i64);
        let by1 = (b.1 as i64).saturating_add(b.3 as i64);
        let x0 = core::cmp::min(a.0 as i64, b.0 as i64);
        let y0 = core::cmp::min(a.1 as i64, b.1 as i64);
        let x1 = core::cmp::max(ax1, bx1);
        let y1 = core::cmp::max(ay1, by1);
        (
            x0 as i32,
            y0 as i32,
            (x1 - x0).max(0) as u32,
            (y1 - y0).max(0) as u32,
        )
    }

    fn should_merge_damage(a: (i32, i32, u32, u32), b: (i32, i32, u32, u32)) -> bool {
        let union = Self::union_damage_rect(a, b);
        let separate_area = Self::rect_area(a).saturating_add(Self::rect_area(b));
        let union_area = Self::rect_area(union);
        union_area <= separate_area.saturating_mul(DAMAGE_MERGE_AREA_FACTOR)
    }

    fn push_damage_rect(rects: &mut Vec<DamageRect>, rect: DamageRect) {
        for existing in rects.iter_mut() {
            if Self::should_merge_damage(*existing, rect) {
                *existing = Self::union_damage_rect(*existing, rect);
                return;
            }
        }

        if rects.len() < MAX_PENDING_DAMAGE_RECTS {
            rects.push(rect);
            return;
        }

        let mut best_index = 0;
        let mut best_extra_area = u64::MAX;
        for (idx, existing) in rects.iter().enumerate() {
            let union = Self::union_damage_rect(*existing, rect);
            let extra_area = Self::rect_area(union).saturating_sub(Self::rect_area(*existing));
            if extra_area < best_extra_area {
                best_index = idx;
                best_extra_area = extra_area;
            }
        }
        rects[best_index] = Self::union_damage_rect(rects[best_index], rect);
    }

    fn merge_present_damage(accumulated: &mut PresentDamage, next: PresentDamage) {
        match (accumulated, next) {
            (accum @ Some(_), None) => {
                *accum = None;
            }
            (Some(accumulated_rects), Some(next_rects)) => {
                for rect in next_rects {
                    Self::push_damage_rect(accumulated_rects, rect);
                }
            }
            (None, _) => {}
        }
    }

    fn add_pending_damage(&mut self, rect: (i32, i32, u32, u32)) {
        if !ENABLE_DIRTY_RECT {
            self.full_redraw_needed = true;
            return;
        }

        let Some((sx0, sy0, w, h)) = self.clamp_rect_to_screen(rect) else {
            return;
        };

        Self::push_damage_rect(&mut self.pending_damage, (sx0, sy0, w, h));
    }

    fn window_order(&self) -> Vec<u32> {
        self.window_manager
            .get_windows()
            .iter()
            .map(|window| window.id)
            .collect()
    }

    fn top_level_window_id(&self, mut window_id: u32) -> u32 {
        for _ in 0..32 {
            let parent = self
                .window_manager
                .get_window(window_id)
                .and_then(|window| window.parent);
            match parent {
                Some(parent_id) if parent_id != window_id => window_id = parent_id,
                _ => break,
            }
        }
        window_id
    }

    fn visible_window_group_rects(&self, window_id: u32) -> Vec<DamageRect> {
        let root_id = self.top_level_window_id(window_id);
        self.window_manager
            .get_windows()
            .iter()
            .filter(|window| {
                window.is_presented() && self.top_level_window_id(window.id) == root_id
            })
            .map(|window| window.presentation_geometry())
            .collect()
    }

    fn window_follows_move(&self, mut window_id: u32, ancestor_id: u32) -> bool {
        if window_id == ancestor_id {
            return true;
        }

        for _ in 0..32 {
            let Some(window) = self.window_manager.get_window(window_id) else {
                return false;
            };
            if window.transient_flags & sws_protocol::transient_flags::FOLLOW_PARENT_MOVE == 0 {
                return false;
            }
            let Some(parent_id) = window.parent else {
                return false;
            };
            if parent_id == ancestor_id {
                return true;
            }
            window_id = parent_id;
        }

        false
    }

    fn visible_move_group_geometry(&self, window_id: u32) -> Vec<WindowGeometrySnapshot> {
        self.window_manager
            .get_windows()
            .iter()
            .filter(|window| {
                window.is_presented() && self.window_follows_move(window.id, window_id)
            })
            .map(|window| (window.id, (window.x, window.y, window.width, window.height)))
            .collect()
    }

    fn damage_window(&mut self, window_id: u32) {
        let rect = self
            .window_manager
            .get_window(window_id)
            .filter(|window| window.is_presented())
            .map(|window| window.presentation_geometry());
        if let Some(rect) = rect {
            self.add_pending_damage(rect);
        }
    }

    fn damage_compositor_focus_style(&mut self, window_id: u32) {
        let uses_compositor_placeholder = self
            .window_manager
            .get_window(window_id)
            .is_some_and(|window| window.is_presented() && window.pixels().is_err())
            && !self
                .gpu_compositor
                .as_ref()
                .is_some_and(|gpu| gpu.has_committed_shared_buffer(window_id));
        if uses_compositor_placeholder {
            self.damage_window(window_id);
        }
    }

    fn damage_geometry_changes(
        &mut self,
        before: &[WindowGeometrySnapshot],
        after: &[WindowGeometrySnapshot],
    ) {
        for rect in changed_geometry_damage(before, after) {
            self.add_pending_damage(rect);
        }
    }

    fn raise_window_with_damage(&mut self, window_id: u32) {
        let old_order = self.window_order();
        self.window_manager.raise_to_top_with_type(window_id);
        if self.window_order() == old_order {
            return;
        }

        for rect in self.visible_window_group_rects(window_id) {
            self.add_pending_damage(rect);
        }
    }

    fn set_window_position_with_damage(&mut self, window_id: u32, x: i32, y: i32) {
        if self.window_manager.get_window(window_id).is_none() {
            return;
        }

        let before = self.visible_move_group_geometry(window_id);
        self.window_manager.set_window_position(window_id, x, y);
        let after = self.visible_move_group_geometry(window_id);
        self.damage_geometry_changes(&before, &after);
        self.position_all_ime_popup_windows();
    }

    /// Mark a window's entire area as damaged and request full redraw
    #[allow(dead_code)]
    fn mark_window_damage(&mut self, window_id: u32) {
        if let Some(w) = self.window_manager.get_window(window_id) {
            // println!("[Compositor] Marking window #{} damage: ({},{}) {}x{}",
            //     window_id, w.x, w.y, w.width, w.height);
            self.add_pending_damage(w.presentation_geometry());
            self.full_redraw_needed = true;
            // println!("[Compositor] Full redraw needed: {}", self.full_redraw_needed);
        }
    }

    fn pending_present_damage(&self) -> PresentDamage {
        if !ENABLE_DIRTY_RECT {
            // Force full redraw when dirty rect optimization is disabled
            None
        } else if self.full_redraw_needed {
            None
        } else {
            let mut rects = self.pending_damage.clone();
            let cursor_dirty = if self.cursor.needs_redraw() {
                Some(self.cursor.get_dirty_region())
            } else {
                None
            };
            if let Some(rect) = cursor_dirty {
                Self::push_damage_rect(&mut rects, rect);
            }
            Some(rects)
        }
    }

    fn composite_damage_to_display(
        &mut self,
        dirty_rects: &PresentDamage,
    ) -> Result<(), &'static str> {
        match dirty_rects {
            None => {
                self.composite_via_display(None)?;
            }
            Some(rects) => {
                for rect in rects.iter().copied() {
                    self.composite_via_display(Some(rect))?;
                }
            }
        }

        self.cursor.mark_drawn();
        self.full_redraw_needed = false;
        self.pending_damage.clear();
        Ok(())
    }

    fn present_damage(&mut self, dirty_rects: PresentDamage) -> Result<(), &'static str> {
        if self.display.has_swapchain() {
            let age = self.display.buffer_age().unwrap_or(0) as usize;
            let mut copy_damage = dirty_rects.clone();
            if age == 0 || age > self.presented_damage.len() {
                copy_damage = None;
            } else {
                let first = self.presented_damage.len() - age;
                for damage in &self.presented_damage[first..] {
                    Self::merge_present_damage(&mut copy_damage, damage.clone());
                }
            }

            self.copy_backbuffer_damage_to_scanout(&copy_damage)?;
            match &copy_damage {
                None => self
                    .display
                    .present()
                    .map_err(|_| "Failed to swap display buffer")?,
                Some(rects) => {
                    let mut regions = Vec::with_capacity(rects.len());
                    for rect in rects.iter().copied() {
                        let Some((x, y, width, height)) = self.clamp_rect_to_screen(rect) else {
                            continue;
                        };
                        regions.push(DisplayPresentRegion {
                            x: x as u32,
                            y: y as u32,
                            width,
                            height,
                        });
                    }
                    self.display
                        .present_regions(&regions)
                        .map_err(|_| "Failed to swap damaged display buffer")?;
                }
            }

            self.presented_damage.push(dirty_rects);
            let capacity = self.display.swapchain_buffer_count();
            if self.presented_damage.len() > capacity {
                self.presented_damage.remove(0);
            }
            return Ok(());
        }

        // Present only the damage SWS actually composed. Legacy framebuffer
        // clients keep their own full-frame compatibility path.
        match dirty_rects {
            None => self
                .display
                .present()
                .map_err(|_| "Failed to present display")?,
            Some(rects) => {
                for (x, y, width, height) in rects {
                    let Some((x, y, width, height)) =
                        self.clamp_rect_to_screen((x, y, width, height))
                    else {
                        continue;
                    };
                    self.display
                        .present_region(x as u32, y as u32, width, height)
                        .map_err(|_| "Failed to present display region")?;
                }
            }
        }

        Ok(())
    }

    fn copy_backbuffer_damage_to_scanout(
        &self,
        damage: &PresentDamage,
    ) -> Result<(), &'static str> {
        let (address, length) = self
            .display
            .get_mapping_info()
            .ok_or("Back scanout buffer is not mapped")?;

        match damage {
            None => {
                if self.backbuffer.len() > length {
                    return Err("Full scanout copy exceeds buffer bounds");
                }
                let copy_len = self.backbuffer.len();
                // SAFETY: DisplaySurface owns the writable mmap for the current
                // non-visible scanout buffer and reports its exact size.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        self.backbuffer.as_ptr(),
                        address as *mut u8,
                        copy_len,
                    );
                }
            }
            Some(rects) => {
                let stride = self.backbuffer_stride as usize;
                let bytes_per_pixel = self.bytes_per_pixel as usize;
                for rect in rects.iter().copied() {
                    let Some((x, y, width, height)) = self.clamp_rect_to_screen(rect) else {
                        continue;
                    };
                    let row_bytes = width as usize * bytes_per_pixel;
                    for row in 0..height as usize {
                        let offset = (y as usize + row)
                            .saturating_mul(stride)
                            .saturating_add(x as usize * bytes_per_pixel);
                        if offset.saturating_add(row_bytes) > self.backbuffer.len()
                            || offset.saturating_add(row_bytes) > length
                        {
                            return Err("Scanout damage copy exceeds buffer bounds");
                        }
                        // SAFETY: both ranges were checked against their live
                        // buffers and the scanout mapping does not overlap the
                        // compositor-owned backbuffer.
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                self.backbuffer.as_ptr().add(offset),
                                (address as *mut u8).add(offset),
                                row_bytes,
                            );
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn composite_pending_to_display(&mut self) -> Result<PresentDamage, &'static str> {
        let dirty_rects = self.pending_present_damage();
        self.composite_damage_to_display(&dirty_rects)?;
        Ok(dirty_rects)
    }

    /// Composite all layers directly to the display backing store.
    fn composite_and_present(&mut self) -> Result<(), &'static str> {
        if self.backend == SwsBackend::Sgfx && self.gpu_compositor.is_none() {
            return Err("SWS_BACKEND=sgfx compositor is unavailable");
        }
        if self.composite_and_present_gpu()? {
            return Ok(());
        }
        let dirty_rects = self.composite_pending_to_display()?;
        self.present_damage(dirty_rects)
    }

    /// Disable the failed GPU backend and apply the selected fallback policy.
    fn disable_gpu_after_runtime_failure(
        &mut self,
        strict_error: &'static str,
    ) -> Result<(), &'static str> {
        self.gpu_compositor = None;
        self.full_redraw_needed = true;
        self.pending_damage.clear();
        self.presented_damage.clear();
        super::ipc::notify_sgfx_backend_lost();
        if self.backend == SwsBackend::Sgfx {
            Err(strict_error)
        } else {
            if self.backend == SwsBackend::Auto {
                println!("[Compositor] Using CPU fallback");
            }
            Ok(())
        }
    }

    /// Retire one compositor-owned upload texture before CPU backing changes.
    ///
    /// The modern SGFX resource table is append-only, so the GPU compositor
    /// rebuilds its private session on the next frame instead of leaking the
    /// retired logical texture slot.
    fn release_gpu_window_texture(&mut self, window_id: u32) -> Result<(), &'static str> {
        let release_failed = match self.gpu_compositor.as_mut() {
            Some(gpu_compositor) => gpu_compositor.remove_window_texture(window_id).is_err(),
            None => false,
        };
        if !release_failed {
            return Ok(());
        }
        println!(
            "[Compositor] GPU texture retirement failed for window {}; disabling GPU composition",
            window_id
        );
        self.disable_gpu_after_runtime_failure("SWS_BACKEND=sgfx texture retirement failed")
    }

    /// Retire every GPU resource owned by a window that is being closed.
    fn release_gpu_window(&mut self, window_id: u32) -> Result<(), &'static str> {
        let release_failed = match self.gpu_compositor.as_mut() {
            Some(gpu_compositor) => gpu_compositor.remove_window(window_id).is_err(),
            None => false,
        };
        if !release_failed {
            return Ok(());
        }
        println!(
            "[Compositor] GPU resource release failed for window {}; disabling GPU composition",
            window_id
        );
        self.disable_gpu_after_runtime_failure("SWS_BACKEND=sgfx window release failed")
    }

    fn gpu_present_damage(&self) -> Option<(u32, u32, u32, u32)> {
        let Some(rects) = self.pending_present_damage() else {
            return None;
        };
        let mut rects = rects.into_iter();
        let first = rects.next()?;
        let union = rects.fold(first, Self::union_damage_rect);
        self.clamp_rect_to_screen(union)
            .map(|(x, y, width, height)| (x as u32, y as u32, width, height))
    }

    /// Compose and present the damaged scene through the optional GPU path.
    ///
    /// In `auto` mode a GPU error triggers a full CPU redraw during this same
    /// frame. The strict `sgfx` mode instead propagates a fatal error.
    fn composite_and_present_gpu(&mut self) -> Result<bool, &'static str> {
        let damage = self.gpu_present_damage();
        let overview_shadows = self.overview_render_shadows();
        let overview_cards = self.overview_render_backplates();
        let overview_remove_buttons = self.overview_remove_buttons();
        let Some(gpu_compositor) = self.gpu_compositor.as_mut() else {
            return Ok(false);
        };
        let result = gpu_compositor.compose_and_present(
            &self.display,
            self.window_manager.get_windows(),
            &self.cursor,
            self.bg_color,
            &overview_shadows,
            &overview_cards,
            &overview_remove_buttons,
            self.resize_outline,
            cursor_visible(self.pointer_lock) && !self.input_modality.cursor_hidden_by_touch,
            damage,
        );
        match result {
            Ok(releases) => {
                super::trace::set_compositor_stage(super::trace::STAGE_GPU_NOTIFY_RELEASES);
                for release in releases {
                    send_sgfx_buffer_released(release);
                }
                self.cursor.mark_drawn();
                self.full_redraw_needed = false;
                self.pending_damage.clear();
                self.presented_damage.clear();
                Ok(true)
            }
            Err(error) => {
                println!("[Compositor] GPU composition failed: {}", error);
                if error.invalidates_shared_images() {
                    self.disable_gpu_after_runtime_failure("SWS_BACKEND=sgfx compositor failed")?;
                } else {
                    // IR recording and frame construction failures are local to
                    // this frame. They are not equivalent to Vulkan-style
                    // device loss and must not invalidate every client's shared
                    // image epoch. Keep the compositor session so the next
                    // frame can retry after the CPU fallback has presented a
                    // coherent desktop.
                    self.full_redraw_needed = true;
                    if self.backend == SwsBackend::Sgfx {
                        return Err("SWS_BACKEND=sgfx frame composition failed");
                    }
                    if self.backend == SwsBackend::Auto {
                        println!("[Compositor] Using CPU fallback for this frame");
                    }
                }
                Ok(false)
            }
        }
    }

    fn validate_vram_samples(
        &self,
        vram: &[u8],
        stride: u32,
        dirty: Option<(i32, i32, u32, u32)>,
        reason: &str,
    ) {
        if !LOG_RENDER_VALIDATION {
            return;
        }

        // Pick coordinates that avoid the default cursor position (center) and
        // sample both corners and the center for sanity.
        let mut samples: Vec<(u32, u32, &'static str)> = Vec::new();
        samples.push((0, 0, "bg top-left"));
        samples.push((10, 10, "bg near top-left"));
        samples.push((self.screen_width / 2, self.screen_height / 2, "bg center"));
        samples.push((
            self.screen_width.saturating_sub(20),
            self.screen_height / 2,
            "bg mid-right",
        ));
        samples.push((
            self.screen_width.saturating_sub(20),
            self.screen_height.saturating_sub(20),
            "bg bottom-right",
        ));

        // For incremental redraw, also validate a point inside the dirty region.
        if let Some((dx, dy, dw, dh)) = dirty {
            if dw > 0 && dh > 0 {
                let cx = (dx + (dw as i32 / 2)).max(0) as u32;
                let cy = (dy + (dh as i32 / 2)).max(0) as u32;
                samples.push((cx, cy, "inside dirty region center"));
            }
        }

        match dirty {
            Some((dx, dy, dw, dh)) => {
                println!(
                    "[Compositor] === VRAM sample validation: {} (dirty=({}, {}) {}x{}) ===",
                    reason, dx, dy, dw, dh
                );
            }
            None => {
                println!("[Compositor] === VRAM sample validation: {} ===", reason);
            }
        }

        for (x, y, label) in samples {
            if x >= self.screen_width || y >= self.screen_height {
                continue;
            }

            // Cursor is an overlay; expected_pixel_at_with_source does not account for it.
            // Skip samples that fall within the cursor bounding box to avoid false mismatches.
            let (cx0, cy0) = self.cursor.draw_position();
            let cx1 = cx0.saturating_add(self.cursor.width as i32);
            let cy1 = cy0.saturating_add(self.cursor.height as i32);
            let xi = x as i32;
            let yi = y as i32;
            if xi >= cx0 && xi < cx1 && yi >= cy0 && yi < cy1 {
                println!("[Compositor] skip {} ({},{}) under cursor", label, x, y);
                continue;
            }

            if let Some((dx, dy, dw, dh)) = dirty {
                let inside = xi >= dx && xi < dx + dw as i32 && yi >= dy && yi < dy + dh as i32;
                if !inside {
                    println!(
                        "[Compositor] skip {} ({},{}) outside dirty region",
                        label, x, y
                    );
                    continue;
                }
            }

            let off = (y as usize)
                .saturating_mul(stride as usize)
                .saturating_add((x as usize).saturating_mul(self.bytes_per_pixel as usize));
            if off + 4 > vram.len() {
                println!(
                    "[Compositor] skip {} ({},{}) out of VRAM range off=0x{:x}",
                    label, x, y, off
                );
                continue;
            }

            let actual = [vram[off], vram[off + 1], vram[off + 2], vram[off + 3]];
            let (expected, src) = self.expected_pixel_at_with_source(x, y);

            if actual != expected {
                println!(
                    "[Compositor] MISMATCH {} ({},{}) actual={:?} expected={:?} src={}",
                    label, x, y, actual, expected, src
                );
            } else {
                println!(
                    "[Compositor] ok {} ({},{}) value={:?} src={}",
                    label, x, y, actual, src
                );
            }
        }
    }

    fn expected_pixel_at_with_source(&self, x: u32, y: u32) -> ([u8; 4], std::string::String) {
        let sx = x as i32;
        let sy = y as i32;

        // Top-most window wins.
        if let Some(window) = self
            .window_manager
            .get_windows()
            .iter()
            .rev()
            .find(|w| w.is_presented() && w.contains_presentation_point(sx, sy))
        {
            let (visual_x, visual_y, visual_width, visual_height) = window.presentation_geometry();
            let local_x = ((i64::from(sx - visual_x) * i64::from(window.width))
                / i64::from(visual_width.max(1))) as u32;
            let local_y = ((i64::from(sy - visual_y) * i64::from(window.height))
                / i64::from(visual_height.max(1))) as u32;
            let is_border = local_x == 0
                || local_y == 0
                || local_x + 1 == window.width
                || local_y + 1 == window.height;

            if is_border {
                if window.focused {
                    (
                        [50, 50, 150, 255],
                        std::format!("window#{} border(focused)", window.id),
                    )
                } else {
                    (
                        [100, 100, 100, 255],
                        std::format!("window#{} border", window.id),
                    )
                }
            } else if let Some(shm_addr) = window.shm_mapped_addr {
                // SHM-backed window
                let row_stride = if window.shm_stride != 0 {
                    window.shm_stride as usize
                } else {
                    window.width as usize * 4
                };
                let wo = window
                    .shm_offset
                    .saturating_add(local_y as usize * row_stride)
                    .saturating_add(local_x as usize * 4);

                let buffer_size = if window.shm_size != 0 {
                    window.shm_size
                } else {
                    row_stride.saturating_mul(window.height as usize)
                };
                let available_len = buffer_size.saturating_sub(window.shm_offset);
                let source_width = if row_stride >= 4 { row_stride / 4 } else { 0 };
                let source_height = if row_stride != 0 {
                    available_len / row_stride
                } else {
                    0
                };

                if local_x as usize >= source_width || local_y as usize >= source_height {
                    (
                        self.bg_color,
                        std::format!(
                            "window#{} SHM outside source local=({}, {}) source={}x{}",
                            window.id,
                            local_x,
                            local_y,
                            source_width,
                            source_height
                        ),
                    )
                } else if wo + 4 <= buffer_size {
                    // SAFETY: `shm_addr` is the live mapping recorded for this
                    // window and `wo..wo + 4` was checked against its size.
                    unsafe {
                        let ptr = shm_addr as *const u8;
                        (
                            [
                                *ptr.add(wo),
                                *ptr.add(wo + 1),
                                *ptr.add(wo + 2),
                                *ptr.add(wo + 3),
                            ],
                            std::format!(
                                "window#{} SHM local=({}, {}) off=0x{:x}",
                                window.id,
                                local_x,
                                local_y,
                                wo
                            ),
                        )
                    }
                } else {
                    (self.bg_color, std::format!("window#{} SHM OOB", window.id))
                }
            } else if let Some(ref buf) = window.buffer {
                // Legacy Vec-backed window
                let wo = ((local_y as usize)
                    .saturating_mul(window.width as usize)
                    .saturating_add(local_x as usize))
                .saturating_mul(4);

                if wo + 4 <= buf.len() {
                    (
                        [buf[wo], buf[wo + 1], buf[wo + 2], buf[wo + 3]],
                        std::format!(
                            "window#{} buffer local=({}, {}) off=0x{:x}",
                            window.id,
                            local_x,
                            local_y,
                            wo
                        ),
                    )
                } else {
                    (
                        self.bg_color,
                        std::format!("window#{} buffer OOB", window.id),
                    )
                }
            } else if window.focused {
                (
                    [150, 150, 200, 255],
                    std::format!("window#{} placeholder(focused)", window.id),
                )
            } else {
                (
                    [180, 180, 180, 255],
                    std::format!("window#{} placeholder", window.id),
                )
            }
        } else {
            (self.bg_color, std::string::String::from("bg"))
        }
    }

    /// clip_rect: (x, y, width, height) in screen coordinates
    fn draw_window_to_buffer_clipped(
        screen_width: u32,
        screen_height: u32,
        bytes_per_pixel: u32,
        window: &super::window::Window,
        buffer: &mut [u8],
        stride: u32,
        clip_rect: Option<(i32, i32, u32, u32)>,
    ) {
        Self::draw_window_instance_to_buffer_clipped(
            screen_width,
            screen_height,
            bytes_per_pixel,
            window,
            None,
            buffer,
            stride,
            clip_rect,
        );
    }

    fn draw_window_instance_to_buffer_clipped(
        screen_width: u32,
        screen_height: u32,
        bytes_per_pixel: u32,
        window: &super::window::Window,
        instance: Option<PresentationInstance>,
        buffer: &mut [u8],
        stride: u32,
        clip_rect: Option<(i32, i32, u32, u32)>,
    ) {
        let presentation_clip = instance
            .and_then(|instance| instance.clip)
            .or(window.presentation_clip);
        let clip_radius = instance.map_or(window.presentation_clip_radius, |instance| {
            instance.clip_radius
        });
        let rounded_clip = presentation_clip
            .filter(|_| clip_radius > 0)
            .map(|rect| (rect, clip_radius));
        let clip_rect = match (clip_rect, presentation_clip) {
            (Some(damage), Some(presentation)) => {
                let Some(intersection) = intersect_compositor_rects(damage, presentation) else {
                    return;
                };
                Some(intersection)
            }
            (Some(clip), None) | (None, Some(clip)) => Some(clip),
            (None, None) => None,
        };
        let visual_geometry = instance.map_or_else(
            || window.presentation_geometry(),
            |instance| {
                let transform = instance.transform;
                (transform.x, transform.y, transform.width, transform.height)
            },
        );
        let presentation_opacity = instance.map_or_else(
            || window.presentation_opacity(),
            |instance| (window.opacity * instance.transform.opacity).clamp(0.0, 1.0),
        );
        // Check if window uses SHM or Vec buffer
        if let Some(shm_addr) = window.shm_mapped_addr {
            // SHM-backed window: read from mapped memory
            let row_stride = if window.shm_stride != 0 {
                window.shm_stride as usize
            } else {
                window.width as usize * 4
            };
            let buffer_size = if window.shm_size != 0 {
                window.shm_size
            } else {
                window
                    .shm_offset
                    .saturating_add(row_stride.saturating_mul(window.height as usize))
            };

            // SAFETY: `shm_addr` is the live mapping retained by the window and
            // `buffer_size` is its recorded mapped extent.
            let window_buffer =
                unsafe { core::slice::from_raw_parts(shm_addr as *const u8, buffer_size) };

            Self::draw_window_from_buffer(
                screen_width,
                screen_height,
                bytes_per_pixel,
                window,
                window_buffer,
                buffer,
                stride,
                clip_rect,
                rounded_clip,
                visual_geometry,
                presentation_opacity,
            );
        } else if let Some(ref window_buffer) = window.buffer {
            // Legacy Vec-backed window
            Self::draw_window_from_buffer(
                screen_width,
                screen_height,
                bytes_per_pixel,
                window,
                window_buffer,
                buffer,
                stride,
                clip_rect,
                rounded_clip,
                visual_geometry,
                presentation_opacity,
            );
        } else {
            // No buffer: draw placeholder
            Self::draw_window_placeholder(
                screen_width,
                screen_height,
                bytes_per_pixel,
                window,
                buffer,
                stride,
                clip_rect,
                rounded_clip,
                visual_geometry,
            );
        }
    }

    /// Draw window from its shared memory buffer
    fn draw_window_from_buffer(
        screen_width: u32,
        screen_height: u32,
        bytes_per_pixel: u32,
        window: &super::window::Window,
        window_buffer: &[u8],
        screen_buffer: &mut [u8],
        stride: u32,
        clip_rect: Option<(i32, i32, u32, u32)>,
        rounded_clip: Option<((i32, i32, u32, u32), u32)>,
        visual_geometry: (i32, i32, u32, u32),
        presentation_opacity: f32,
    ) {
        let (win_x0, win_y0, visual_width, visual_height) = visual_geometry;
        if visual_width == 0 || visual_height == 0 {
            return;
        }
        let win_x1 = win_x0.saturating_add(visual_width as i32);
        let win_y1 = win_y0.saturating_add(visual_height as i32);

        let mut x0 = win_x0.max(0);
        let mut y0 = win_y0.max(0);
        let mut x1 = win_x1.min(screen_width as i32);
        let mut y1 = win_y1.min(screen_height as i32);

        if let Some((clip_x, clip_y, clip_w, clip_h)) = clip_rect {
            let clip_x1 = clip_x.saturating_add(clip_w as i32);
            let clip_y1 = clip_y.saturating_add(clip_h as i32);
            x0 = x0.max(clip_x);
            y0 = y0.max(clip_y);
            x1 = x1.min(clip_x1);
            y1 = y1.min(clip_y1);
        }

        if x1 <= x0 || y1 <= y0 {
            return;
        }

        // Check if window has transparency (opacity < 1.0 or content has alpha channel)
        // - presentation opacity < 1.0: window/shell-level transparency
        // - window.has_alpha_content: pixel-level transparency (semi-transparent UI elements)
        let has_transparency = presentation_opacity < 1.0 || window.has_alpha_content;

        let row_stride = if window.shm_mapped_addr.is_some() && window.shm_stride != 0 {
            window.shm_stride as usize
        } else {
            window.width as usize * 4
        };
        let base_offset = if window.shm_mapped_addr.is_some() {
            window.shm_offset
        } else {
            0
        };
        if row_stride == 0 || base_offset >= window_buffer.len() {
            return;
        }

        let available_len = window_buffer.len().saturating_sub(base_offset);
        let source_width = (row_stride / bytes_per_pixel as usize) as u32;
        let source_height = (available_len / row_stride) as u32;
        if source_width == 0 || source_height == 0 {
            return;
        }

        // Fast path for opaque windows: copy row by row
        if !has_transparency {
            for sy in y0..y1 {
                let (row_x0, row_x1) = match rounded_clip {
                    Some((rect, radius)) => {
                        let Some((left, right)) = rounded_rect_row_span(rect, radius, sy) else {
                            continue;
                        };
                        (x0.max(left), x1.min(right))
                    }
                    None => (x0, x1),
                };
                if row_x1 <= row_x0 {
                    continue;
                }
                let wy = ((i64::from(sy - win_y0) * i64::from(source_height))
                    / i64::from(visual_height)) as u32;
                let screen_row_off = (sy as u32 * stride) as usize;
                for sx in row_x0..row_x1 {
                    let wx = ((i64::from(sx - win_x0) * i64::from(source_width))
                        / i64::from(visual_width)) as u32;
                    let window_offset = base_offset
                        .saturating_add(wy as usize * row_stride)
                        .saturating_add(wx as usize * 4);
                    let screen_offset = screen_row_off + (sx as u32 * bytes_per_pixel) as usize;

                    if window_offset + 4 <= window_buffer.len()
                        && screen_offset + 4 <= screen_buffer.len()
                    {
                        screen_buffer[screen_offset..screen_offset + 3]
                            .copy_from_slice(&window_buffer[window_offset..window_offset + 3]);
                        // Opaque windows ignore their source alpha. Keeping a
                        // fractional alpha in the final scanout lets DCP blend
                        // against an older underlay and leaves visible trails.
                        screen_buffer[screen_offset + 3] = 255;
                    }
                }
            }
        } else {
            // Slow path for transparent windows: per-pixel alpha blending
            for sy in y0..y1 {
                let (row_x0, row_x1) = match rounded_clip {
                    Some((rect, radius)) => {
                        let Some((left, right)) = rounded_rect_row_span(rect, radius, sy) else {
                            continue;
                        };
                        (x0.max(left), x1.min(right))
                    }
                    None => (x0, x1),
                };
                if row_x1 <= row_x0 {
                    continue;
                }
                let wy = ((i64::from(sy - win_y0) * i64::from(source_height))
                    / i64::from(visual_height)) as u32;
                let screen_row_off = (sy as u32 * stride) as usize;
                for sx in row_x0..row_x1 {
                    let wx = ((i64::from(sx - win_x0) * i64::from(source_width))
                        / i64::from(visual_width)) as u32;
                    let window_offset = base_offset
                        .saturating_add(wy as usize * row_stride)
                        .saturating_add(wx as usize * 4);
                    let screen_offset = screen_row_off + (sx as u32 * bytes_per_pixel) as usize;

                    if window_offset + 4 <= window_buffer.len()
                        && screen_offset + 4 <= screen_buffer.len()
                    {
                        // Alpha blending: BGRA format
                        let src_b = window_buffer[window_offset] as u32;
                        let src_g = window_buffer[window_offset + 1] as u32;
                        let src_r = window_buffer[window_offset + 2] as u32;
                        let src_a = window_buffer[window_offset + 3] as u32;

                        // Apply window opacity to pixel alpha
                        let effective_alpha =
                            ((src_a as f32 * presentation_opacity) as u32).min(255);

                        let dst_b = screen_buffer[screen_offset] as u32;
                        let dst_g = screen_buffer[screen_offset + 1] as u32;
                        let dst_r = screen_buffer[screen_offset + 2] as u32;

                        // Alpha blending formula: dst = src * alpha + dst * (1 - alpha)
                        let inv_alpha = 255 - effective_alpha;
                        let out_b = ((src_b * effective_alpha + dst_b * inv_alpha) / 255) as u8;
                        let out_g = ((src_g * effective_alpha + dst_g * inv_alpha) / 255) as u8;
                        let out_r = ((src_r * effective_alpha + dst_r * inv_alpha) / 255) as u8;

                        screen_buffer[screen_offset] = out_b;
                        screen_buffer[screen_offset + 1] = out_g;
                        screen_buffer[screen_offset + 2] = out_r;
                        screen_buffer[screen_offset + 3] = 255; // Output is always opaque
                    }
                }
            }
        }
    }

    /// Draw placeholder window (for windows without buffers yet)
    fn draw_window_placeholder(
        screen_width: u32,
        screen_height: u32,
        bytes_per_pixel: u32,
        window: &super::window::Window,
        buffer: &mut [u8],
        stride: u32,
        clip_rect: Option<(i32, i32, u32, u32)>,
        rounded_clip: Option<((i32, i32, u32, u32), u32)>,
        visual_geometry: (i32, i32, u32, u32),
    ) {
        let window_color = if window.focused {
            [150, 150, 200, 255]
        } else {
            [180, 180, 180, 255]
        };

        // Pre-calculate visible area to reduce per-pixel checks
        let (visual_x, visual_y, visual_width, visual_height) = visual_geometry;
        let win_x0 = visual_x.max(0);
        let win_y0 = visual_y.max(0);
        let win_x1 = visual_x
            .saturating_add(visual_width as i32)
            .min(screen_width as i32);
        let win_y1 = visual_y
            .saturating_add(visual_height as i32)
            .min(screen_height as i32);

        let (x0, y0, x1, y1) = if let Some((clip_x, clip_y, clip_w, clip_h)) = clip_rect {
            let clip_x1 = clip_x.saturating_add(clip_w as i32);
            let clip_y1 = clip_y.saturating_add(clip_h as i32);
            (
                win_x0.max(clip_x),
                win_y0.max(clip_y),
                win_x1.min(clip_x1),
                win_y1.min(clip_y1),
            )
        } else {
            (win_x0, win_y0, win_x1, win_y1)
        };

        if x1 <= x0 || y1 <= y0 {
            return;
        }

        // Process row by row for better cache locality
        for sy in y0..y1 {
            let (row_x0, row_x1) = match rounded_clip {
                Some((rect, radius)) => {
                    let Some((left, right)) = rounded_rect_row_span(rect, radius, sy) else {
                        continue;
                    };
                    (x0.max(left), x1.min(right))
                }
                None => (x0, x1),
            };
            if row_x1 <= row_x0 {
                continue;
            }
            let screen_row_off = (sy as u32 * stride + row_x0 as u32 * bytes_per_pixel) as usize;
            for sx in row_x0..row_x1 {
                let offset = screen_row_off + ((sx - row_x0) as u32 * bytes_per_pixel) as usize;
                if offset + 4 <= buffer.len() {
                    buffer[offset..offset + 4].copy_from_slice(&window_color);
                }
            }
        }
    }

    /// Composite into the persistent backbuffer, then present the affected region.
    fn composite_via_display(
        &mut self,
        dirty: Option<(i32, i32, u32, u32)>,
    ) -> Result<(), &'static str> {
        let backbuffer_len = self.backbuffer.len();
        let stride = self.backbuffer_stride;
        let overview_shadows = self.overview_render_shadows();
        let overview_cards = self.overview_render_backplates();
        let overview_remove_buttons = self.overview_remove_buttons();

        // Clip dirty region to screen bounds.
        let (x0, y0, w, h) = match dirty {
            None => (0i32, 0i32, self.screen_width, self.screen_height),
            Some((dx, dy, dw, dh)) => {
                let sx0 = dx.max(0).min(self.screen_width as i32);
                let sy0 = dy.max(0).min(self.screen_height as i32);
                let sx1 = (dx.saturating_add(dw as i32))
                    .max(0)
                    .min(self.screen_width as i32);
                let sy1 = (dy.saturating_add(dh as i32))
                    .max(0)
                    .min(self.screen_height as i32);
                let cw = (sx1 - sx0).max(0) as u32;
                let ch = (sy1 - sy0).max(0) as u32;
                (sx0, sy0, cw, ch)
            }
        };

        if w == 0 || h == 0 {
            // Nothing to redraw.
            return Ok(());
        }

        // Mutate backbuffer within a limited scope so we can immutably borrow `self`
        // afterwards for validation/present.
        {
            let backbuffer = &mut self.backbuffer;

            // Layer 1: Fill background (only within dirty region).
            // Pre-calculate values outside the loop
            let x0_usize = x0 as usize;
            let y0_u32 = y0 as u32;
            let w_usize = w as usize;
            let h_usize = h as usize;
            let stride_usize = stride as usize;
            let bytes_per_pixel_usize = self.bytes_per_pixel as usize;
            let row_len = w_usize.saturating_mul(bytes_per_pixel_usize);

            for yy in 0..h_usize {
                let sy = y0_u32.saturating_add(yy as u32);
                let row_off = sy as usize * stride_usize + x0_usize * bytes_per_pixel_usize;
                if row_off.saturating_add(row_len) > backbuffer_len {
                    continue;
                }
                let row = &mut backbuffer[row_off..row_off + row_len];
                for px in row.chunks_exact_mut(4) {
                    px.copy_from_slice(&self.bg_color);
                }
            }

            // Layer 2: Draw shell background, then independent rounded
            // workspace cards and their live application actors.
            let clip = if dirty.is_some() {
                Some((x0, y0, w, h))
            } else {
                None
            };
            let screen_width = self.screen_width;
            let screen_height = self.screen_height;
            let bytes_per_pixel = self.bytes_per_pixel;
            let mut overview_backplates_drawn = false;
            for window in self.window_manager.get_windows() {
                if !window.is_presented() {
                    continue;
                }
                if !overview_backplates_drawn
                    && !matches!(
                        window.window_type,
                        WindowType::Desktop | WindowType::ShellBackground
                    )
                {
                    Self::draw_overview_shadows_to_buffer(
                        screen_width,
                        screen_height,
                        bytes_per_pixel,
                        backbuffer,
                        stride,
                        &overview_shadows,
                        clip,
                    );
                    Self::draw_overview_backplates_to_buffer(
                        screen_width,
                        screen_height,
                        bytes_per_pixel,
                        backbuffer,
                        stride,
                        &overview_cards,
                        clip,
                    );
                    overview_backplates_drawn = true;
                }
                Self::draw_window_to_buffer_clipped(
                    screen_width,
                    screen_height,
                    bytes_per_pixel,
                    window,
                    backbuffer,
                    stride,
                    clip,
                );
                for instance in &window.presentation_instances {
                    Self::draw_window_instance_to_buffer_clipped(
                        screen_width,
                        screen_height,
                        bytes_per_pixel,
                        window,
                        Some(*instance),
                        backbuffer,
                        stride,
                        clip,
                    );
                }
            }
            if !overview_backplates_drawn {
                Self::draw_overview_shadows_to_buffer(
                    screen_width,
                    screen_height,
                    bytes_per_pixel,
                    backbuffer,
                    stride,
                    &overview_shadows,
                    clip,
                );
                Self::draw_overview_backplates_to_buffer(
                    screen_width,
                    screen_height,
                    bytes_per_pixel,
                    backbuffer,
                    stride,
                    &overview_cards,
                    clip,
                );
            }
            Self::draw_overview_remove_buttons_to_buffer(
                screen_width,
                screen_height,
                bytes_per_pixel,
                backbuffer,
                stride,
                &overview_remove_buttons,
                clip,
            );

            // Layer 2.5: Draw interactive resize outline (if any)
            if let Some(rect) = self.resize_outline {
                Self::draw_outline_rect_to_buffer(
                    screen_width,
                    screen_height,
                    bytes_per_pixel,
                    backbuffer,
                    stride,
                    rect,
                    clip,
                );
            }

            // Layer 3: Draw cursor unless an application owns pointer lock.
            if cursor_visible(self.pointer_lock) && !self.input_modality.cursor_hidden_by_touch {
                let cursor = &self.cursor;
                cursor.draw_to_buffer_direct_clipped(
                    backbuffer,
                    screen_width,
                    screen_height,
                    bytes_per_pixel,
                    stride,
                    clip,
                );
            }
        }

        // Validate composition against expected pixels before presenting.
        self.validate_vram_samples(&self.backbuffer, stride, dirty, "after display composite");

        if !self.display.has_swapchain() {
            // Present only the dirty region when direct scanout is unavailable.
            let src_off = (y0 as usize)
                .saturating_mul(stride as usize)
                .saturating_add((x0 as usize).saturating_mul(self.bytes_per_pixel as usize));
            if src_off >= backbuffer_len {
                return Err("Backbuffer offset out of range");
            }
            let src = &self.backbuffer[src_off..];

            self.display
                .write_bgra_strided(x0 as u32, y0 as u32, w, h, src, stride as usize)
                .map_err(|_| "Failed to write backbuffer")?;
        }

        Ok(())
    }

    /// Process input events
    /// Handle mouse click (for window focus)
    fn handle_click(&mut self) -> Result<(), &'static str> {
        let click_x = self.cursor.x;
        let click_y = self.cursor.y;

        // Find topmost window at click position
        if let Some(win_id) = self.window_manager.window_at_point(click_x, click_y) {
            self.activate_window_from_input(win_id);
        }

        Ok(())
    }

    /// Focus/raise the window selected by an input hit-test.
    ///
    /// Mouse clicks and direct touchscreen contacts share activation policy,
    /// but touchscreen input must not borrow the mouse cursor coordinates.
    fn activate_window_from_input(&mut self, win_id: u32) {
        sws_debug!("[Compositor] Input activated window #{}", win_id);

        // Taskbar and Desktop windows are global UI elements that don't steal
        // keyboard focus, but they still participate in their normal stacking
        // policy when directly touched.
        if !self.window_manager.window_accepts_focus(win_id) {
            self.raise_window_with_damage(win_id);
            return;
        }

        let previous_focus = self.window_manager.get_focused_window_id();
        self.raise_window_with_damage(win_id);
        self.window_manager.set_focus(win_id);
        self.broadcast_focus_change(win_id);

        if previous_focus != Some(win_id) {
            if let Some(previous_focus) = previous_focus {
                self.damage_compositor_focus_style(previous_focus);
            }
            self.damage_compositor_focus_style(win_id);
        }
    }

    /// Broadcast focus change event to all connected clients
    fn broadcast_focus_change(&mut self, window_id: u32) {
        if self.workspace_manager.activate_window(
            window_id,
            self.windowing_mode == sws_protocol::WindowingMode::Focused,
        ) {
            self.apply_workspace_presentation_policy();
            self.publish_workspace_state();
            self.full_redraw_needed = true;
        }
        if self
            .pointer_lock
            .is_some_and(|state| state.window_id != window_id)
        {
            self.release_pointer_lock();
        }
        // Only broadcast if the focused window actually changed
        if self.last_focused_window_id == Some(window_id) {
            sws_debug!(
                "[Compositor] Window #{} already focused, skipping FOCUS_CHANGED broadcast",
                window_id
            );
            return;
        }

        let root_id = self.top_level_window_id(window_id);
        if self
            .window_manager
            .get_window(window_id)
            .is_some_and(|window| window.window_type == WindowType::Normal)
            && self
                .workspace_manager
                .workspace_for_window(root_id)
                .is_some()
        {
            self.last_workspace_focus = Some(window_id);
        }

        if let Some(window) = self.window_manager.get_window(window_id) {
            let app_id_bytes = window.app_id.as_deref().unwrap_or(b"");
            let title_bytes = window.title.as_deref().unwrap_or(b"");

            // Get app_name and menu_titles from AppSession
            let (app_name, menu_titles) = super::ipc::get_app_session_info(window_id);
            let app_name_bytes = app_name.as_bytes();
            let menu_titles_bytes = menu_titles.as_bytes();

            // Update last focused window ID
            self.last_focused_window_id = Some(window_id);
            super::ipc::set_focused_window(window_id);

            // Broadcast FOCUS_CHANGED for all windows
            let payload = sws_protocol::payload_focus_changed(
                window_id,
                app_id_bytes,
                app_name_bytes,
                title_bytes,
                menu_titles_bytes,
            );

            sws_debug!(
                "[Compositor] ABOUT TO broadcast focus change: window_id={}, app_id_len={}, app_name_len={}, title_len={}, menu_titles_len={}, app_name={}, menu_titles={}",
                window_id,
                app_id_bytes.len(),
                app_name_bytes.len(),
                title_bytes.len(),
                menu_titles_bytes.len(),
                core::str::from_utf8(app_name_bytes).unwrap_or(""),
                core::str::from_utf8(menu_titles_bytes).unwrap_or("")
            );

            super::ipc::broadcast_message_to_all_clients(
                sws_protocol::server_msg::FOCUS_CHANGED,
                payload,
            );

            sws_debug!(
                "[Compositor] Broadcast focus change: window_id={}, app_id_len={}, app_name_len={}, title_len={}, menu_titles_len={}",
                window_id,
                app_id_bytes.len(),
                app_name_bytes.len(),
                title_bytes.len(),
                menu_titles_bytes.len()
            );

            // For active-app windows only, check if app_id changed and broadcast ACTIVE_APP_CHANGED.
            sws_debug!(
                "[Compositor] Checking active-on-focus: window_id={}, active_on_focus={}",
                window_id,
                window.active_on_focus
            );
            if window.active_on_focus {
                let app_id_changed = match &self.active_app_id {
                    Some(current_app_id) => current_app_id != app_id_bytes,
                    None => true,
                };

                if app_id_changed {
                    sws_debug!(
                        "[Compositor] Active app changed: {:?} -> {:?}, broadcasting ACTIVE_APP_CHANGED",
                        self.active_app_id
                            .as_ref()
                            .map(|id| core::str::from_utf8(id).unwrap_or("")),
                        core::str::from_utf8(app_id_bytes).unwrap_or("")
                    );

                    // Update active_app_id
                    self.active_app_id = Some(app_id_bytes.to_vec());

                    let active_app_payload = sws_protocol::payload_active_app_changed(
                        window_id,
                        app_id_bytes,
                        app_name_bytes,
                        title_bytes,
                        menu_titles_bytes,
                    );

                    super::ipc::broadcast_message_to_all_clients(
                        sws_protocol::server_msg::ACTIVE_APP_CHANGED,
                        active_app_payload,
                    );
                } else {
                    sws_debug!(
                        "[Compositor] Active app unchanged ({}), skipping ACTIVE_APP_CHANGED",
                        core::str::from_utf8(app_id_bytes).unwrap_or("")
                    );
                }
            }
        } else {
            println!(
                "[Compositor] Warning: Failed to broadcast focus change for non-existent window #{}",
                window_id
            );
        }
    }

    /// Check if cursor is within the bounds of a window
    /// Returns window-local coordinates if inside, None if outside
    fn cursor_position_in_window(&self, window: &super::window::Window) -> Option<(i32, i32)> {
        let window_x = self.cursor.x - window.x;
        let window_y = self.cursor.y - window.y;

        // println!("[Boundary Check] Window #{}: cursor=({}, {}), window pos=({}, {}), size={}x{}, window_local=({}, {})",
        //     window_id, self.cursor.x, self.cursor.y, window.x, window.y, window.width, window.height, window_x, window_y);

        if window.contains_point(self.cursor.x, self.cursor.y) {
            // println!("[Boundary Check] -> INSIDE");
            Some((window_x, window_y))
        } else {
            // println!("[Boundary Check] -> OUTSIDE");
            None
        }
    }

    fn send_mouse_position_to_window_coords(
        &self,
        window_id: u32,
        window: &super::window::Window,
        window_x: i32,
        window_y: i32,
    ) {
        // Check if this is an extension-owned window
        if let Some((extension_id, external_client_id)) = window.extension_owner {
            // Send EXTENSION_INPUT_EVENT for extension windows
            super::ipc::send_extension_input_event(
                extension_id,
                external_client_id,
                window_id,
                0,
                super::input::event_types::EV_ABS,
                super::input::abs_codes::ABS_X,
                window_x,
            );
            super::ipc::send_extension_input_event(
                extension_id,
                external_client_id,
                window_id,
                0,
                super::input::event_types::EV_ABS,
                super::input::abs_codes::ABS_Y,
                window_y,
            );
            super::ipc::send_extension_input_event(
                extension_id,
                external_client_id,
                window_id,
                0,
                super::input::event_types::EV_SYN,
                0,
                0,
            );
        } else {
            // Send regular INPUT_EVENT for normal windows
            super::ipc::send_input_to_window(
                window_id,
                0,
                super::input::event_types::EV_ABS,
                super::input::abs_codes::ABS_X,
                window_x,
            );
            super::ipc::send_input_to_window(
                window_id,
                0,
                super::input::event_types::EV_ABS,
                super::input::abs_codes::ABS_Y,
                window_y,
            );
            super::ipc::send_input_to_window(window_id, 0, super::input::event_types::EV_SYN, 0, 0);
        }
    }

    /// Send mouse position event to a window
    fn send_mouse_position_to_window(&self, window_id: u32, window: &super::window::Window) {
        if let Some((window_x, window_y)) = self.cursor_position_in_window(window) {
            self.send_mouse_position_to_window_coords(window_id, window, window_x, window_y);
        }
    }

    fn send_mouse_position_to_window_unclipped(
        &self,
        window_id: u32,
        window: &super::window::Window,
    ) {
        let window_x = self.cursor.x - window.x;
        let window_y = self.cursor.y - window.y;
        self.send_mouse_position_to_window_coords(window_id, window, window_x, window_y);
    }

    fn clear_pointer_focus_for_shell_navigation(&mut self) {
        if let Some(previous_window_id) = self.pointer_focus_window_id
            && let Some(previous_window) = self.window_manager.get_window(previous_window_id)
        {
            self.send_mouse_position_to_window_coords(previous_window_id, previous_window, -1, -1);
        }
        self.pointer_focus_window_id = None;
    }

    fn set_cursor_hidden_by_touch(&mut self, hidden: bool) -> bool {
        let changed = if hidden {
            self.input_modality.direct_touch()
        } else {
            self.input_modality.pointer_motion()
        };
        if !changed {
            return false;
        }
        self.add_pending_damage(self.cursor.get_dirty_region());
        true
    }

    fn send_direct_legacy_event(&self, grab: DirectTouchGrab, kind: DirectLegacyEventKind) {
        let Some(window) = self.window_manager.get_window(grab.window_id) else {
            return;
        };
        let local_x = grab.screen_x - window.x;
        let local_y = grab.screen_y - window.y;
        if let Some((extension_id, external_client_id)) = window.extension_owner {
            for (type_, code, value) in direct_legacy_event_sequence(local_x, local_y, kind) {
                super::ipc::send_extension_input_event(
                    extension_id,
                    external_client_id,
                    grab.window_id,
                    0,
                    type_,
                    code,
                    value,
                );
            }
        } else {
            for (type_, code, value) in direct_legacy_event_sequence(local_x, local_y, kind) {
                super::ipc::send_input_to_window(grab.window_id, 0, type_, code, value);
            }
        }
    }

    fn workspace_card_at_point(&self, x: i32, y: i32) -> Option<u32> {
        self.overview_card_rects()
            .into_iter()
            .rev()
            .find_map(|(workspace_id, rect)| {
                rounded_rect_contains_point(
                    rect,
                    sws_protocol::workspace::OVERVIEW_CARD_CORNER_RADIUS,
                    x,
                    y,
                )
                .then_some(workspace_id)
            })
    }

    fn point_in_overview_add_workspace(&self, x: i32, y: i32) -> bool {
        self.overview_add_workspace_rect().is_some_and(|rect| {
            rounded_rect_contains_point(
                rect,
                sws_protocol::workspace::OVERVIEW_CARD_CORNER_RADIUS,
                x,
                y,
            )
        })
    }

    fn overview_window_at_point(&self, x: i32, y: i32) -> Option<u32> {
        self.window_manager
            .get_windows()
            .iter()
            .rev()
            .find(|window| {
                window.window_type == WindowType::Normal
                    && window.is_presented()
                    && (window.presentation_transform.is_some()
                        && window.contains_presentation_point(x, y)
                        || window
                            .presentation_instances
                            .iter()
                            .rev()
                            .any(|instance| presentation_instance_contains_point(instance, x, y)))
            })
            .map(|window| self.top_level_window_id(window.id))
            .filter(|window_id| {
                self.workspace_manager
                    .workspace_for_window(*window_id)
                    .is_some()
            })
    }

    fn overview_window_drag_threshold(&self) -> u32 {
        ((u64::from(OVERVIEW_WINDOW_DRAG_THRESHOLD_LOGICAL)
            * u64::from(self.output_scale_milli.max(1)))
            / 1000)
            .clamp(8, 40) as u32
    }

    fn begin_overview_window_drag(&mut self, x: i32, y: i32) -> bool {
        let Some(window_id) = self.overview_window_at_point(x, y) else {
            return false;
        };
        let Some(source_workspace_id) = self.workspace_manager.workspace_for_window(window_id)
        else {
            return false;
        };
        self.raise_window_with_damage(window_id);
        self.overview_window_drag = Some(OverviewWindowDrag {
            window_id,
            source_workspace_id,
            from_workspace_thumbnail: self.point_in_overview_workspace_region(x, y),
            start_x: x,
            start_y: y,
            current_x: x,
            current_y: y,
        });
        self.clear_pointer_focus_for_shell_navigation();
        self.update_overview_transforms();
        self.full_redraw_needed = true;
        true
    }

    fn update_overview_window_drag(&mut self, x: i32, y: i32) -> bool {
        let Some(mut drag) = self.overview_window_drag else {
            return false;
        };
        if drag.current_x == x && drag.current_y == y {
            return true;
        }
        drag.current_x = x;
        drag.current_y = y;
        self.overview_window_drag = Some(drag);
        self.update_overview_transforms();
        self.full_redraw_needed = true;
        true
    }

    fn overview_drop_position_for_window(
        &self,
        window_id: u32,
        target_card: (i32, i32, u32, u32),
    ) -> Option<(i32, i32)> {
        let window = self.window_manager.get_window(window_id)?;
        let preview = window.presentation_transform?;
        let workarea = self
            .workarea
            .unwrap_or((0, 0, self.screen_width, self.screen_height));
        Some(map_overview_drop_position(
            (preview.x, preview.y),
            target_card,
            workarea,
            (window.width, window.height),
        ))
    }

    fn finish_overview_window_drag(&mut self, drag: OverviewWindowDrag) -> bool {
        let dx = drag.current_x.saturating_sub(drag.start_x);
        let dy = drag.current_y.saturating_sub(drag.start_y);
        let threshold = self.overview_window_drag_threshold();
        let crossed_threshold = dx.unsigned_abs().max(dy.unsigned_abs()) >= threshold;
        let target_workspace_id = self.workspace_card_at_point(drag.current_x, drag.current_y);
        let add_workspace_target =
            self.point_in_overview_add_workspace(drag.current_x, drag.current_y);
        let target_card = if add_workspace_target {
            self.overview_add_workspace_rect()
        } else {
            target_workspace_id.and_then(|target| {
                self.overview_card_rects()
                    .into_iter()
                    .find_map(|(workspace_id, rect)| (workspace_id == target).then_some(rect))
            })
        };
        let freeform_drop_position = (crossed_threshold
            && self.windowing_mode == sws_protocol::WindowingMode::Freeform)
            .then(|| {
                target_card
                    .and_then(|card| self.overview_drop_position_for_window(drag.window_id, card))
            })
            .flatten();

        let changed = if crossed_threshold {
            if add_workspace_target {
                self.workspace_manager
                    .move_window_to_new_workspace(drag.window_id)
                    .is_some()
            } else {
                target_workspace_id
                    .filter(|target| *target != drag.source_workspace_id)
                    .is_some_and(|target| {
                        if self.windowing_mode == sws_protocol::WindowingMode::Focused {
                            match self.workspace_manager.tablet_layout(target) {
                                sws_protocol::workspace::TabletLayout::Empty => self
                                    .workspace_manager
                                    .move_window_to_workspace(drag.window_id, target),
                                sws_protocol::workspace::TabletLayout::Single { .. } => self
                                    .workspace_manager
                                    .move_window_to_workspace_as_split(drag.window_id, target),
                                sws_protocol::workspace::TabletLayout::Split { .. } => false,
                            }
                        } else {
                            self.workspace_manager
                                .move_window_to_workspace(drag.window_id, target)
                        }
                    })
            }
        } else if !self.tablet_mode
            && !self.point_in_overview_workspace_region(drag.start_x, drag.start_y)
        {
            self.overview_restore_focus = Some(drag.window_id);
            self.workspace_manager
                .select_workspace_from_overview(drag.source_workspace_id)
        } else {
            target_workspace_id
                .filter(|target| *target == drag.source_workspace_id)
                .is_some_and(|workspace_id| {
                    self.workspace_manager
                        .select_workspace_from_overview(workspace_id)
                })
        };

        if changed && let Some((x, y)) = freeform_drop_position {
            self.set_window_position_with_damage(drag.window_id, x, y);
        }

        // Clearing the drag before recomputing presentation restores the
        // actor's card-relative transform or applies its destination card.
        self.overview_window_drag = None;
        self.overview_add_workspace_selected = false;
        self.apply_workspace_presentation_policy();
        if changed {
            self.publish_workspace_state();
        }
        self.full_redraw_needed = true;
        changed
    }

    fn cancel_overview_window_drag(&mut self) -> bool {
        if self.overview_window_drag.take().is_none() {
            return false;
        }
        self.apply_workspace_presentation_policy();
        self.full_redraw_needed = true;
        true
    }

    fn point_in_overview_workspace_region(&self, x: i32, y: i32) -> bool {
        let (region_x, region_y, width, height) = self.overview_workspace_region();
        x >= region_x
            && x < region_x.saturating_add(width as i32)
            && y >= region_y
            && y < region_y.saturating_add(height as i32)
    }

    fn shell_navigation_captures_pointer_at(&self, x: i32, y: i32) -> bool {
        if self.workspace_manager.presentation()
            == sws_protocol::workspace::ShellPresentation::Workspace
        {
            return false;
        }
        if self.point_in_overview_workspace_region(x, y) {
            return true;
        }
        self.window_manager
            .window_at_point(x, y)
            .and_then(|window_id| self.window_manager.get_window(window_id))
            .is_some_and(|window| {
                window.window_type == WindowType::Normal && window.presentation_transform.is_some()
            })
    }

    fn apply_overview_navigation_change(&mut self, changed: bool) -> bool {
        if !changed {
            return false;
        }
        self.overview_add_workspace_selected = false;
        self.apply_workspace_presentation_policy();
        self.publish_workspace_state();
        self.full_redraw_needed = true;
        true
    }

    fn move_overview_selection(&mut self, direction: i32) -> bool {
        if direction == 0
            || self.workspace_manager.presentation()
                == sws_protocol::workspace::ShellPresentation::Workspace
        {
            return false;
        }
        if self.overview_add_workspace_selected {
            if direction > 0 {
                return false;
            }
            self.overview_add_workspace_selected = false;
            self.update_overview_transforms();
            self.full_redraw_needed = true;
            return true;
        }

        let state = self.workspace_manager.snapshot();
        let can_select_add = state.workspaces.len() < sws_protocol::workspace::MAX_WORKSPACES
            && state
                .workspaces
                .last()
                .is_some_and(|workspace| workspace.id == state.active_workspace);
        if direction > 0 && can_select_add {
            self.overview_add_workspace_selected = true;
            self.update_overview_transforms();
            self.full_redraw_needed = true;
            return true;
        }

        let changed = self.workspace_manager.move_overview_selection(direction);
        self.apply_overview_navigation_change(changed)
    }

    fn handle_overview_horizontal_scroll(&mut self, dx: i32) -> bool {
        if dx == 0
            || self.workspace_manager.presentation()
                == sws_protocol::workspace::ShellPresentation::Workspace
        {
            return false;
        }
        let now = monotonic_time_ns();
        if self.overview_last_scroll_step_ns != 0
            && now.saturating_sub(self.overview_last_scroll_step_ns)
                < OVERVIEW_SCROLL_STEP_INTERVAL_NS
        {
            return true;
        }
        let changed = self.move_overview_selection(if dx < 0 { 1 } else { -1 });
        if changed {
            self.overview_last_scroll_step_ns = now;
        }
        true
    }

    fn finish_overview_pointer_navigation(
        &mut self,
        navigation: OverviewPointerNavigation,
        end_x: i32,
        end_y: i32,
    ) -> bool {
        let dx = end_x.saturating_sub(navigation.start_x);
        let dy = end_y.saturating_sub(navigation.start_y);
        let drag_threshold = (self.screen_width / 14).clamp(48, 160);
        if dx.unsigned_abs() >= drag_threshold && dx.unsigned_abs() > dy.unsigned_abs() {
            return self.move_overview_selection(if dx < 0 { 1 } else { -1 });
        }
        let tap_threshold = 16;
        if dx.unsigned_abs() > tap_threshold || dy.unsigned_abs() > tap_threshold {
            return false;
        }
        if let Some(workspace_id) = navigation.start_remove_workspace_id
            && self.overview_remove_workspace_at_point(end_x, end_y) == Some(workspace_id)
        {
            let changed = self.workspace_manager.remove_workspace(
                workspace_id,
                self.windowing_mode == sws_protocol::WindowingMode::Freeform,
            );
            return self.apply_overview_navigation_change(changed);
        }
        if navigation.start_add_workspace && self.point_in_overview_add_workspace(end_x, end_y) {
            let changed = self.workspace_manager.create_workspace().is_some();
            return self.apply_overview_navigation_change(changed);
        }
        let end_workspace_id = self.workspace_card_at_point(end_x, end_y);
        let changed = navigation
            .start_workspace_id
            .filter(|workspace_id| Some(*workspace_id) == end_workspace_id)
            .is_some_and(|workspace_id| {
                self.workspace_manager
                    .select_workspace_from_overview(workspace_id)
            });
        self.apply_overview_navigation_change(changed)
    }

    fn finish_system_touch_navigation(&mut self, navigation: SystemTouchNavigation) -> bool {
        let dx = navigation.current_x.saturating_sub(navigation.start_x);
        let dy = navigation.current_y.saturating_sub(navigation.start_y);
        let duration_ns = monotonic_time_ns().saturating_sub(navigation.start_time_ns);
        let mut changed = false;

        if matches!(
            navigation.origin,
            sws_protocol::workspace::ShellPresentation::Home
                | sws_protocol::workspace::ShellPresentation::Overview
        ) {
            if dx.unsigned_abs() >= self.screen_width / 10 && dx.unsigned_abs() > dy.unsigned_abs()
            {
                changed =
                    self.workspace_manager
                        .move_overview_selection(if dx < 0 { 1 } else { -1 });
            } else if dx.unsigned_abs() <= self.screen_width / 12
                && dy.unsigned_abs() <= self.screen_height / 12
            {
                changed = if let Some(workspace_id) = navigation.remove_workspace_id
                    && self.overview_remove_workspace_at_point(
                        navigation.current_x,
                        navigation.current_y,
                    ) == Some(workspace_id)
                {
                    self.workspace_manager.remove_workspace(
                        workspace_id,
                        self.windowing_mode == sws_protocol::WindowingMode::Freeform,
                    )
                } else if self
                    .point_in_overview_add_workspace(navigation.current_x, navigation.current_y)
                {
                    self.workspace_manager.create_workspace().is_some()
                } else {
                    match self.workspace_card_at_point(navigation.current_x, navigation.current_y) {
                        Some(workspace_id) => self
                            .workspace_manager
                            .select_workspace_from_overview(workspace_id),
                        None => false,
                    }
                };
            }
        } else if dx.unsigned_abs() >= self.screen_width / 7
            && dx.unsigned_abs() > dy.unsigned_abs()
        {
            changed = self
                .workspace_manager
                .cycle_workspace(if dx < 0 { 1 } else { -1 });
        } else if dy <= -((self.screen_height / 9) as i32) {
            if duration_ns >= 280_000_000 {
                changed = self
                    .workspace_manager
                    .set_presentation(sws_protocol::workspace::ShellPresentation::Overview);
            } else {
                changed = self
                    .workspace_manager
                    .set_presentation(sws_protocol::workspace::ShellPresentation::Home);
            }
        }

        if changed {
            self.apply_workspace_presentation_policy();
            self.publish_workspace_state();
            self.full_redraw_needed = true;
        }
        changed
    }

    /// Claim direct-touch contacts reserved for outer shell navigation.
    ///
    /// Returning `Some` means the complete frame belongs to the system gesture
    /// arena and must not be translated into legacy application pointer events.
    fn handle_system_touch_navigation(&mut self, frame: &TouchFrame) -> Option<bool> {
        if let Some(mut navigation) = self.system_touch_navigation {
            if navigation.source != frame.source {
                return None;
            }
            if !frame.cancelled
                && let Some(contact) = frame
                    .contacts
                    .iter()
                    .find(|contact| contact.tracking_id == navigation.tracking_id)
            {
                navigation.current_x = normalized_touch_to_screen(contact.x, self.screen_width);
                navigation.current_y = normalized_touch_to_screen(contact.y, self.screen_height);
                self.system_touch_navigation = Some(navigation);
                if navigation.drag_window_id.is_some() {
                    self.update_overview_window_drag(navigation.current_x, navigation.current_y);
                }
                return Some(true);
            }
            self.system_touch_navigation = None;
            return Some(if navigation.drag_window_id.is_some() {
                if frame.cancelled {
                    self.cancel_overview_window_drag()
                } else if let Some(drag) = self.overview_window_drag.take() {
                    self.finish_overview_window_drag(OverviewWindowDrag {
                        current_x: navigation.current_x,
                        current_y: navigation.current_y,
                        ..drag
                    })
                } else {
                    false
                }
            } else if frame.cancelled {
                self.apply_workspace_presentation_policy()
            } else {
                self.finish_system_touch_navigation(navigation)
            });
        }

        let presentation = self.workspace_manager.presentation();
        let overview = presentation == sws_protocol::workspace::ShellPresentation::Overview;
        let shell_navigation = matches!(
            presentation,
            sws_protocol::workspace::ShellPresentation::Home
                | sws_protocol::workspace::ShellPresentation::Overview
        );
        if frame.cancelled || !self.tablet_mode && !shell_navigation {
            return None;
        }
        if self
            .direct_touch_grabs
            .iter()
            .any(|grab| grab.source == frame.source)
        {
            return None;
        }
        let contact = frame.contacts.first()?;
        let screen_x = normalized_touch_to_screen(contact.x, self.screen_width);
        let screen_y = normalized_touch_to_screen(contact.y, self.screen_height);
        let edge_height = (self.screen_height / 24).clamp(24, 72);
        let in_workspace_region =
            shell_navigation && self.point_in_overview_workspace_region(screen_x, screen_y);
        let over_laptop_spread = overview
            && !self.tablet_mode
            && self.overview_window_at_point(screen_x, screen_y).is_some();
        if shell_navigation && !in_workspace_region && !over_laptop_spread {
            return None;
        }
        if !shell_navigation && screen_y < self.screen_height.saturating_sub(edge_height) as i32 {
            return None;
        }
        let remove_workspace_id = overview
            .then(|| self.overview_remove_workspace_at_point(screen_x, screen_y))
            .flatten();
        let drag_window_id = if shell_navigation
            && remove_workspace_id.is_none()
            && self.begin_overview_window_drag(screen_x, screen_y)
        {
            self.overview_window_drag.map(|drag| drag.window_id)
        } else {
            None
        };
        self.system_touch_navigation = Some(SystemTouchNavigation {
            source: frame.source,
            tracking_id: contact.tracking_id,
            start_time_ns: frame.time_ns,
            start_x: screen_x,
            start_y: screen_y,
            current_x: screen_x,
            current_y: screen_y,
            origin: self.workspace_manager.presentation(),
            drag_window_id,
            remove_workspace_id,
        });
        Some(true)
    }

    fn handle_direct_touch_frame(&mut self, frame: TouchFrame) -> bool {
        if let Some(redraw) = self.handle_system_touch_navigation(&frame) {
            return redraw;
        }
        // An empty/cancel frame can be generated during device discovery,
        // disconnect, or SYN_DROPPED recovery.  It must not hide an otherwise
        // active mouse cursor.  Once a real direct contact occurs, keep the
        // cursor hidden until the next indirect pointer action.
        let mut redraw = if !frame.cancelled && !frame.contacts.is_empty() {
            self.set_cursor_hidden_by_touch(true)
        } else {
            false
        };

        let mut index = 0;
        while index < self.direct_touch_grabs.len() {
            let grab = self.direct_touch_grabs[index];
            let ended = grab.source == frame.source
                && (frame.cancelled
                    || !frame
                        .contacts
                        .iter()
                        .any(|contact| contact.tracking_id == grab.tracking_id));
            if ended {
                let grab = self.direct_touch_grabs.remove(index);
                if grab.legacy_primary {
                    let kind = if frame.cancelled {
                        DirectLegacyEventKind::Cancel
                    } else {
                        DirectLegacyEventKind::Release
                    };
                    self.send_direct_legacy_event(grab, kind);
                    if grab.driving_move_drag
                        && self
                            .move_drag
                            .is_some_and(|state| state.window_id == grab.window_id)
                    {
                        self.move_drag = None;
                    }
                }
            } else {
                index += 1;
            }
        }

        if frame.cancelled {
            return redraw;
        }
        for contact in frame.contacts {
            let screen_x = normalized_touch_to_screen(contact.x, self.screen_width);
            let screen_y = normalized_touch_to_screen(contact.y, self.screen_height);
            if let Some(index) = self.direct_touch_grabs.iter().position(|grab| {
                grab.source == frame.source && grab.tracking_id == contact.tracking_id
            }) {
                let previous = self.direct_touch_grabs[index];
                self.direct_touch_grabs[index].screen_x = screen_x;
                self.direct_touch_grabs[index].screen_y = screen_y;
                if previous.legacy_primary
                    && let Some(mut state) = self.move_drag
                    && state.window_id == previous.window_id
                {
                    // request_move_window is shared with mouse input and is
                    // initially anchored to the cursor.  A direct contact
                    // deliberately never moves that cursor, so rebase the
                    // first touch-driven update to the finger's previous
                    // position and keep using finger deltas thereafter.
                    if !previous.driving_move_drag {
                        state.grab_cursor_x = previous.screen_x;
                        state.grab_cursor_y = previous.screen_y;
                        self.direct_touch_grabs[index].driving_move_drag = true;
                    }
                    let new_x = state.start_window_x + (screen_x - state.grab_cursor_x);
                    let new_y = state.start_window_y + (screen_y - state.grab_cursor_y);
                    self.move_drag = Some(state);
                    self.set_window_position_with_damage(state.window_id, new_x, new_y);
                    redraw = true;
                }
                let grab = self.direct_touch_grabs[index];
                if grab.legacy_primary {
                    self.send_direct_legacy_event(grab, DirectLegacyEventKind::Move);
                }
                continue;
            }

            let Some(window_id) = self.window_manager.window_at_point(screen_x, screen_y) else {
                continue;
            };
            let legacy_primary = !self
                .direct_touch_grabs
                .iter()
                .any(|grab| grab.source == frame.source);
            if legacy_primary {
                self.activate_window_from_input(window_id);
                redraw = true;
            }
            let grab = DirectTouchGrab {
                source: frame.source,
                tracking_id: contact.tracking_id,
                window_id,
                legacy_primary,
                driving_move_drag: false,
                screen_x,
                screen_y,
            };
            self.direct_touch_grabs.push(grab);
            if legacy_primary {
                self.send_direct_legacy_event(grab, DirectLegacyEventKind::Press);
            }
        }
        redraw
    }

    fn handle_gesture_event(&self, event: GestureEvent) {
        // Pinch/swipe have no public SWS ABI yet. Keep them visible to debug
        // builds without translating them into unrelated pointer packets.
        sws_debug!("[Compositor] gesture: {:?}", event);
    }

    /// Update the surface that owns pointer hover.
    ///
    /// SWS input packets carry absolute coordinates rather than a separate
    /// leave message. Before changing targets, send one final unclipped motion
    /// to the previous window so clients can derive a pointer-exit transition.
    fn update_pointer_focus(&mut self, next_window_id: Option<u32>) {
        if self.pointer_focus_window_id == next_window_id {
            return;
        }

        if let Some(previous_window_id) = self.pointer_focus_window_id
            && let Some(previous_window) = self.window_manager.get_window(previous_window_id)
        {
            self.send_mouse_position_to_window_unclipped(previous_window_id, previous_window);
        }

        self.pointer_focus_window_id = next_window_id;
    }

    /// Route the current pointer position to the topmost window under it.
    fn route_pointer_motion_at_cursor(&mut self) {
        if self.shell_navigation_captures_pointer_at(self.cursor.x, self.cursor.y) {
            self.clear_pointer_focus_for_shell_navigation();
            self.refresh_cursor_icon();
            return;
        }
        let target_id = self
            .window_manager
            .window_at_point(self.cursor.x, self.cursor.y);
        self.update_pointer_focus(target_id);
        if let Some(target_id) = target_id
            && let Some(window) = self.window_manager.get_window(target_id)
        {
            self.send_mouse_position_to_window(target_id, window);
        }
        self.refresh_cursor_icon();
    }

    fn desired_cursor_icon(&self) -> sws_protocol::CursorIcon {
        if let Some(state) = self.resize_drag {
            return state.icon;
        }
        if self.move_drag.is_some() {
            return sws_protocol::CursorIcon::Move;
        }

        let target_id = self.pointer_grab_window_id.or_else(|| {
            self.window_manager
                .window_at_point(self.cursor.x, self.cursor.y)
        });
        let Some(window) = target_id.and_then(|id| self.window_manager.get_window(id)) else {
            return sws_protocol::CursorIcon::Arrow;
        };

        if window.resizable && !window.fullscreen {
            let (geometry_x, geometry_y, geometry_width, geometry_height) =
                window.window_geometry();
            let inside = window.contains_point(self.cursor.x, self.cursor.y);
            let near_right = inside
                && self.cursor.x
                    >= geometry_x.saturating_add(geometry_width as i32 - RESIZE_GRIP_PX);
            let near_bottom = inside
                && self.cursor.y
                    >= geometry_y.saturating_add(geometry_height as i32 - RESIZE_GRIP_PX);
            match (near_right, near_bottom) {
                (true, true) => return sws_protocol::CursorIcon::ResizeNwse,
                (true, false) => return sws_protocol::CursorIcon::ResizeEw,
                (false, true) => return sws_protocol::CursorIcon::ResizeNs,
                (false, false) => {}
            }
        }

        window.cursor_icon
    }

    fn refresh_cursor_icon(&mut self) -> bool {
        let icon = self.desired_cursor_icon();
        self.cursor.set_icon(icon)
    }

    fn activate_cursor_theme(&mut self, theme_path: &str) -> Result<(), u32> {
        if !is_installed_cursor_theme_path(theme_path) {
            return Err(sws_protocol::error_codes::INVALID_CURSOR_THEME);
        }

        let mut cursor =
            load_cursor_theme(theme_path, self.output_scale_milli).map_err(|error| {
                println!(
                    "[Compositor] Rejecting cursor theme {}: {}",
                    theme_path, error
                );
                sws_protocol::error_codes::INVALID_CURSOR_THEME
            })?;
        config::persist_cursor_theme(theme_path).map_err(|error| {
            println!(
                "[Compositor] Failed to persist cursor theme {}: {}",
                theme_path, error
            );
            sws_protocol::error_codes::CURSOR_THEME_PERSIST_FAILED
        })?;

        cursor.x = self.cursor.x;
        cursor.y = self.cursor.y;
        cursor.mark_drawn();
        self.cursor = cursor;
        self.refresh_cursor_icon();
        if let Some(gpu_compositor) = self.gpu_compositor.as_mut() {
            gpu_compositor.invalidate_cursor_texture();
        }
        self.full_redraw_needed = true;
        println!("[Compositor] Activated cursor theme: {}", theme_path);
        Ok(())
    }

    fn cursor_rect(&self) -> (i32, i32, u32, u32) {
        let (x, y) = self.cursor.draw_position();
        (x, y, self.cursor.width, self.cursor.height)
    }

    fn send_pointer_lock_changed(&self, state: PointerLockState, locked: bool) {
        super::ipc::send_message_to_client(
            state.client_id,
            sws_protocol::server_msg::POINTER_LOCK_CHANGED,
            sws_protocol::payload_pointer_lock_changed(state.window_id, locked).to_vec(),
        );
    }

    /// Release explicit capture and damage the cursor layer for redisplay.
    fn release_pointer_lock(&mut self) -> bool {
        let Some(state) = self.pointer_lock.take() else {
            return false;
        };
        self.add_pending_damage(self.cursor_rect());
        self.send_pointer_lock_changed(state, false);
        true
    }

    fn pointer_lock_is_valid(&self, state: PointerLockState) -> bool {
        self.window_manager
            .get_window(state.window_id)
            .is_some_and(|window| {
                !state.must_release(
                    window.owner_client_id,
                    window.is_presented(),
                    window.minimized,
                    window.focused,
                    self.window_manager.get_focused_window_id() == Some(state.window_id),
                )
            })
    }

    fn release_invalid_pointer_lock(&mut self) -> bool {
        let Some(state) = self.pointer_lock else {
            return false;
        };
        if !self.pointer_lock_is_valid(state) {
            return self.release_pointer_lock();
        }
        if let Some((x, y, width, height)) = self
            .window_manager
            .get_window(state.window_id)
            .map(|window| window.window_geometry())
        {
            let max_x = x.saturating_add(width.saturating_sub(1) as i32);
            let max_y = y.saturating_add(height.saturating_sub(1) as i32);
            self.cursor.set_position(
                self.cursor.x.clamp(x, max_x),
                self.cursor.y.clamp(y, max_y),
                self.screen_width,
                self.screen_height,
            );
        }
        false
    }

    fn set_pointer_lock(
        &mut self,
        client_id: usize,
        window_id: u32,
        locked: bool,
    ) -> Result<bool, u32> {
        if !locked {
            if self
                .pointer_lock
                .is_some_and(|state| state.client_id == client_id && state.window_id == window_id)
            {
                return Ok(self.release_pointer_lock());
            }
            return Ok(false);
        }

        let Some(window) = self.window_manager.get_window(window_id) else {
            return Err(sws_protocol::error_codes::POINTER_LOCK_NOT_OWNED);
        };
        let mut interaction = PointerInteractionState {
            focused_window_id: self.window_manager.get_focused_window_id(),
            implicit_grab_window_id: self.pointer_grab_window_id,
            locked_window_id: self.pointer_lock.map(|state| state.window_id),
        };
        let interaction_allows_lock = interaction.request_lock(window_id);
        validate_request(
            window.owner_client_id,
            client_id,
            window.is_presented(),
            window.minimized,
            window.focused,
            self.window_manager.get_focused_window_id() == Some(window_id),
            !interaction_allows_lock || self.move_drag.is_some() || self.resize_drag.is_some(),
        )
        .map_err(|denial| match denial {
            PointerLockDenial::NotOwned => sws_protocol::error_codes::POINTER_LOCK_NOT_OWNED,
            PointerLockDenial::Denied => sws_protocol::error_codes::POINTER_LOCK_DENIED,
        })?;
        if let Some(existing) = self.pointer_lock {
            if existing.client_id == client_id && existing.window_id == window_id {
                return Ok(false);
            }
            return Err(sws_protocol::error_codes::POINTER_LOCK_DENIED);
        }

        let cursor_rect = self.cursor_rect();
        let (geometry_x, geometry_y, geometry_width, geometry_height) = window.window_geometry();
        let max_x = geometry_x.saturating_add(geometry_width.saturating_sub(1) as i32);
        let max_y = geometry_y.saturating_add(geometry_height.saturating_sub(1) as i32);
        let locked_x = self.cursor.x.clamp(geometry_x, max_x);
        let locked_y = self.cursor.y.clamp(geometry_y, max_y);
        self.add_pending_damage(cursor_rect);
        self.cursor
            .set_position(locked_x, locked_y, self.screen_width, self.screen_height);
        if self.pointer_grab_window_id != interaction.implicit_grab_window_id {
            self.pointer_grab_window_id = interaction.implicit_grab_window_id;
            self.last_left_down_cursor = None;
        }
        self.pointer_focus_window_id = Some(window_id);
        let state = PointerLockState::new(client_id, window_id);
        self.pointer_lock = Some(state);
        self.send_pointer_lock_changed(state, true);
        Ok(true)
    }

    fn send_locked_relative_motion(&self, window_id: u32, dx: i32, dy: i32) {
        self.send_locked_input_event(
            window_id,
            0,
            super::input::event_types::EV_REL,
            super::input::rel_codes::REL_X,
            dx,
        );
        self.send_locked_input_event(
            window_id,
            0,
            super::input::event_types::EV_REL,
            super::input::rel_codes::REL_Y,
            dy,
        );
        self.send_locked_input_event(window_id, 0, super::input::event_types::EV_SYN, 0, 0);
    }

    fn send_locked_input_event(
        &self,
        window_id: u32,
        time: u64,
        type_: u16,
        code: u16,
        value: i32,
    ) {
        let extension_owner = self
            .window_manager
            .get_window(window_id)
            .and_then(|window| window.extension_owner);
        super::ipc::send_pointer_lock_input_event(
            input_route(window_id, extension_owner),
            time,
            type_,
            code,
            value,
        );
    }

    fn event_wait_timeout_ns(&self) -> i64 {
        let now = monotonic_time_ns();
        let mut timeout = COMPOSITOR_IDLE_RECHECK_NS as u64;
        if cursor_visible(self.pointer_lock)
            && !self.input_modality.cursor_hidden_by_touch
            && let Some(deadline) = self.cursor.next_animation_deadline_ns()
        {
            timeout = timeout.min(deadline.saturating_sub(now));
        }
        if let Some(deadline) = self.key_repeat.next_deadline_ns() {
            timeout = timeout.min(deadline.saturating_sub(now));
        }
        timeout as i64
    }

    fn wait_for_event_signal(&mut self) {
        let mut handles = [PollHandle::new(self.wake_read.as_raw() as u32, POLLIN)];
        let ready = match poll(&mut handles, self.event_wait_timeout_ns()) {
            Ok(ready) => ready,
            Err(_) => {
                super::ipc::consume_compositor_wake();
                thread::sleep(Duration::from_millis(COMPOSITOR_WAKE_ERROR_DELAY_MS));
                return;
            }
        };
        if ready == 0 {
            // A timeout is also a repair point for a stale coalescing flag.
            // Any byte racing this reset remains readable on the next poll.
            super::ipc::consume_compositor_wake();
            return;
        }
        if (handles[0].revents & POLLIN) == 0 {
            super::ipc::consume_compositor_wake();
            thread::sleep(Duration::from_millis(COMPOSITOR_WAKE_ERROR_DELAY_MS));
            return;
        }

        if let Ok(stream) = self.wake_read.as_stream() {
            let mut buf = [0u8; 1];
            let _ = stream.read(&mut buf);
        }
        // Once poll reported the byte as readable, let a producer publish a
        // fresh wake even if the subsequent read raced an endpoint failure.
        super::ipc::consume_compositor_wake();
    }

    fn consume_event_signal_if_ready(&mut self) {
        let mut handles = [PollHandle::new(self.wake_read.as_raw() as u32, POLLIN)];
        let Ok(ready) = poll(&mut handles, 0) else {
            return;
        };
        if ready == 0 || (handles[0].revents & POLLIN) == 0 {
            return;
        }

        let Ok(stream) = self.wake_read.as_stream() else {
            return;
        };

        let mut buf = [0u8; 1];
        let _ = stream.read(&mut buf);
        super::ipc::consume_compositor_wake();
    }

    fn handle_remote_event(&mut self, event: RemoteEvent) -> Result<(), &'static str> {
        match event {
            RemoteEvent::CreateCapture {
                client_id,
                output_id,
            } => {
                if !self.capture_session.create(client_id, output_id) {
                    println!(
                        "[SwsRemote] Rejected capture session from client {} for output {}",
                        client_id, output_id
                    );
                }
            }
            RemoteEvent::RegisterBuffer {
                client_id,
                buffer_id,
                width,
                height,
                stride,
                format,
                handle,
            } => {
                if let Err(error) = self
                    .capture_session
                    .register_buffer(client_id, buffer_id, width, height, stride, format, handle)
                {
                    println!(
                        "[SwsRemote] Rejected capture buffer {} from client {}: {}",
                        buffer_id, client_id, error
                    );
                }
            }
            RemoteEvent::RequestFrame {
                client_id,
                buffer_id,
            } => {
                let result = match self.gpu_compositor.as_ref() {
                    Some(gpu) => self.capture_session.capture_gpu(client_id, buffer_id, gpu),
                    None => self.capture_session.capture_cpu(
                        client_id,
                        buffer_id,
                        &self.backbuffer,
                        self.backbuffer_stride,
                    ),
                };
                if let Err(error) = result {
                    println!(
                        "[SwsRemote] Failed capture request {} from client {}: {}",
                        buffer_id, client_id, error
                    );
                }
            }
            RemoteEvent::Input { client_id, message } => {
                if self.capture_session.is_owner(client_id)
                    && let Some(event) = super::remote::input::compositor_event(client_id, &message)
                {
                    super::input::push_input_event(event);
                }
            }
            RemoteEvent::Disconnected { client_id } => {
                self.capture_session.disconnect(client_id);
                super::input::release_pointer_source(super::input::PointerSource::Remote(
                    client_id,
                ));
                self.release_keyboard_source(KeyboardSource::Remote(client_id))?;
            }
        }
        Ok(())
    }

    fn process_pending_events(&mut self) -> Result<(), &'static str> {
        if cursor_visible(self.pointer_lock) && !self.input_modality.cursor_hidden_by_touch {
            self.cursor.advance_animation(monotonic_time_ns());
        }

        self.check_display_resize()?;

        // Process IPC events from global queue (non-blocking)
        let ipc_events = self.ipc_server.process_messages()?;
        // if !ipc_events.is_empty() {
        //     println!("[Compositor] Processing {} IPC events", ipc_events.len());
        // }
        for event in ipc_events {
            self.handle_ipc_event(event)?;
        }
        // Parent assignment is a separate request. Resolve first-frame scene
        // candidates only after the complete IPC batch so a trailing parent
        // request can classify the surface as a transient before publication.
        let ready_workspace_scenes = self
            .pending_workspace_scenes
            .iter()
            .copied()
            .filter(|window_id| {
                self.window_manager
                    .get_window(*window_id)
                    .is_some_and(|window| window.has_presented_frame)
            })
            .collect::<Vec<_>>();
        for window_id in ready_workspace_scenes {
            self.finalize_pending_workspace_scene(window_id);
        }

        // Remote capture and virtual input use a separate privileged protocol,
        // but converge on the same compositor thread and input pipeline.
        let remote_events = self.remote_server.process_messages();
        for event in remote_events {
            self.handle_remote_event(event)?;
        }

        // Process input events from global queue (non-blocking)
        let input_events = super::input::pop_all_input_events();
        if !input_events.is_empty() {
            for event in input_events {
                self.handle_input_event(event)?;
            }
        }

        let focused_id = self.window_manager.get_focused_window_id();
        self.key_repeat.cancel_if_focus_changed(focused_id);
        if let Some((source, code)) = self.key_repeat.take_due(monotonic_time_ns(), focused_id) {
            self.handle_input_event(CompositorInputEvent::Keyboard {
                code,
                value: 2,
                source,
                synthetic: true,
            })?;
        }

        if cursor_visible(self.pointer_lock) && !self.input_modality.cursor_hidden_by_touch {
            self.cursor.advance_animation(monotonic_time_ns());
        }

        Ok(())
    }

    fn has_pending_redraw(&self) -> bool {
        self.full_redraw_needed || !self.pending_damage.is_empty() || self.cursor.needs_redraw()
    }

    fn wait_for_frame_batch(&mut self) -> Result<(), &'static str> {
        // Synchronous DCP presentation already waits for the next completed
        // page flip. Sleeping for another frame here would double-pace the
        // compositor and reduce interactive updates to roughly 30 Hz.
        if !self.display.has_swapchain() {
            let now = monotonic_time_ns();
            let deadline = self.next_frame_deadline_ns.unwrap_or(now);
            if deadline > now {
                thread::sleep(Duration::from_nanos(deadline - now));
            }
        }
        self.consume_event_signal_if_ready();
        self.process_pending_events()
    }

    fn note_frame_presented(&mut self) {
        if self.display.has_swapchain() {
            self.next_frame_deadline_ns = None;
            return;
        }

        let now = monotonic_time_ns();
        let next = self
            .next_frame_deadline_ns
            .unwrap_or(now)
            .saturating_add(FRAME_BATCH_INTERVAL_NS);
        let next = if next > now {
            next
        } else {
            let skipped = now
                .saturating_sub(next)
                .checked_div(FRAME_BATCH_INTERVAL_NS)
                .unwrap_or(0)
                .saturating_add(1);
            next.saturating_add(skipped.saturating_mul(FRAME_BATCH_INTERVAL_NS))
        };
        let next = if next > now {
            next
        } else {
            now.saturating_add(FRAME_BATCH_INTERVAL_NS)
        };
        self.next_frame_deadline_ns = Some(next);
    }

    fn has_queued_event_work(&self) -> bool {
        super::ipc::has_pending_ipc_events()
            || super::remote::server::has_pending_events()
            || super::input::has_pending_input_events()
    }

    /// Main event loop
    pub fn run(&mut self) -> Result<(), &'static str> {
        println!("[Compositor] Starting main loop (multithreaded)");

        loop {
            if let Err(error) = self.run_iteration() {
                // Explicit Sgfx mode is strict by contract. In auto/CPU mode,
                // however, one transient display or DCP error must not tear
                // down the process that owns every desktop connection.
                if self.backend == SwsBackend::Sgfx {
                    return Err(error);
                }
                println!(
                    "[Compositor] Runtime error: {}; retrying full redraw in {} ms",
                    error, RUNTIME_ERROR_RETRY_DELAY_MS
                );
                self.full_redraw_needed = true;
                self.presented_damage.clear();
                self.next_frame_deadline_ns = None;
                super::trace::set_compositor_stage(super::trace::STAGE_ACTIVE);
                thread::sleep(Duration::from_millis(RUNTIME_ERROR_RETRY_DELAY_MS));
            }
        }
    }

    fn run_iteration(&mut self) -> Result<(), &'static str> {
        super::trace::compositor_loop();
        super::trace::set_compositor_stage(super::trace::STAGE_PROCESS_EVENTS);
        self.process_pending_events()?;
        super::trace::set_compositor_stage(super::trace::STAGE_ACTIVE);
        if self.backend == SwsBackend::Sgfx && self.gpu_compositor.is_none() {
            return Err("SWS_BACKEND=sgfx compositor is unavailable");
        }

        // Re-composite and present if needed.
        let mut presented = false;
        if self.has_pending_redraw() {
            if self.gpu_compositor.is_some() {
                super::trace::set_compositor_stage(super::trace::STAGE_FRAME_BATCH);
                self.wait_for_frame_batch()?;
                if self.full_redraw_needed {
                    sws_debug!("[Compositor] Full redraw triggered");
                }
                super::trace::set_compositor_stage(super::trace::STAGE_GPU_COMPOSITE);
                let present_damage = self.pending_present_damage();
                self.composite_and_present()?;
                if self.gpu_compositor.is_some() {
                    self.capture_session.frame_presented(&present_damage);
                } else {
                    // Automatic GPU fallback reconstructs the CPU backbuffer
                    // from scratch, so a capture previously sourced from the
                    // GPU target must refresh the complete output.
                    self.capture_session.frame_presented(&None);
                }
            } else {
                super::trace::set_compositor_stage(super::trace::STAGE_CPU_COMPOSITE);
                let mut present_damage = self.composite_pending_to_display()?;
                super::trace::set_compositor_stage(super::trace::STAGE_FRAME_BATCH);
                self.wait_for_frame_batch()?;
                if self.full_redraw_needed {
                    sws_debug!("[Compositor] Full redraw triggered");
                }
                if self.has_pending_redraw() {
                    super::trace::set_compositor_stage(super::trace::STAGE_CPU_COMPOSITE);
                    let next_damage = self.composite_pending_to_display()?;
                    Self::merge_present_damage(&mut present_damage, next_damage);
                }
                super::trace::set_compositor_stage(super::trace::STAGE_PRESENT);
                self.present_damage(present_damage.clone())?;
                self.capture_session.frame_presented(&present_damage);
            }
            super::trace::compositor_present();
            super::trace::set_compositor_stage(super::trace::STAGE_ACTIVE);
            self.note_frame_presented();
            self.event_counter += 1;
            presented = true;
            // Grant only after the display accepted the frame. Presentation
            // policy applied below may expose a different scene, which must
            // first reach its own redraw before receiving callbacks.
            self.grant_pending_frame_callbacks();

            if self.window_policy_after_present {
                self.window_policy_after_present = false;
                let _ = self.apply_post_present_window_policy();
            }
        }
        if !presented {
            // A never-submitted surface needs one bootstrap callback before it
            // can produce its first frame. Already-rendered surfaces remain
            // pending until a later presentation advances `event_counter`.
            self.grant_pending_frame_callbacks();
        }

        // Sleep until IPC/input explicitly signals that new work is queued.
        // Signal writes are coalesced so producers cannot fill the pipe while
        // the compositor is busy processing a batch.
        if self.has_pending_redraw() || self.has_queued_event_work() {
            self.consume_event_signal_if_ready();
        } else {
            super::trace::set_compositor_stage(super::trace::STAGE_WAIT_SIGNAL);
            self.wait_for_event_signal();
            super::trace::set_compositor_stage(super::trace::STAGE_ACTIVE);
        }

        Ok(())
    }

    fn apply_input_environment_snapshot(
        &mut self,
        snapshot: input_environment::Snapshot,
    ) -> Result<bool, &'static str> {
        self.lid_closed = snapshot.lid_closed();
        let windowing_mode = snapshot.windowing_mode();
        let windowing_mode_changed = self.windowing_mode != windowing_mode;
        let previous_windowing_mode = self.windowing_mode;
        let preferred_window_id = self
            .window_manager
            .get_focused_window_id()
            .map(|window_id| self.top_level_window_id(window_id))
            .filter(|window_id| {
                self.workspace_manager
                    .workspace_for_window(*window_id)
                    .is_some()
            });
        self.windowing_mode = windowing_mode;
        self.publish_window_creation_environment();
        let tablet_mode = snapshot.tablet_mode();
        let tablet_mode_changed = self.tablet_mode != tablet_mode;
        let mut redraw = false;
        if tablet_mode_changed {
            self.tablet_mode = tablet_mode;
            for policy_event in self.gesture_recognizer.set_tablet_mode(tablet_mode) {
                match policy_event {
                    TouchPolicyEvent::Pointer(CompositorInputEvent::MouseButton {
                        button,
                        pressed,
                    }) => super::input::push_pointer_button(
                        PointerSource::Local(u8::MAX),
                        button,
                        pressed,
                    ),
                    TouchPolicyEvent::Pointer(event) => {
                        redraw |= self.handle_input_event(event)?;
                    }
                    TouchPolicyEvent::Gesture(event) => self.handle_gesture_event(event),
                    TouchPolicyEvent::DirectTouch(frame) => {
                        redraw |= self.handle_direct_touch_frame(frame);
                    }
                    TouchPolicyEvent::SourceButton {
                        source,
                        button,
                        pressed,
                    } => super::input::push_pointer_button(source, button, pressed),
                    TouchPolicyEvent::ReleaseSource { source } => {
                        super::input::release_pointer_source(source)
                    }
                }
            }
        }
        let workspace_state_changed = match (previous_windowing_mode, windowing_mode) {
            (sws_protocol::WindowingMode::Freeform, sws_protocol::WindowingMode::Focused) => self
                .workspace_manager
                .enter_tablet_experience(preferred_window_id),
            (sws_protocol::WindowingMode::Focused, sws_protocol::WindowingMode::Freeform) => {
                self.workspace_manager.leave_tablet_experience()
            }
            _ => false,
        };
        if windowing_mode_changed {
            redraw |= self.apply_windowing_mode_policy();
        }
        if workspace_state_changed {
            self.publish_workspace_state();
            redraw = true;
        }
        println!(
            "[Compositor] Input environment changed: generation={} tablet={} lid_closed={} windowing={:?} tablet_override={} windowing_override={} capabilities={:#x}",
            snapshot.generation,
            tablet_mode,
            self.lid_closed,
            windowing_mode,
            snapshot.tablet_mode_override_active(),
            snapshot.windowing_mode_override_active(),
            snapshot.capability_flags,
        );
        super::ipc::broadcast_input_environment_changed(snapshot);
        super::input_environment_sbus::queue_state_changed(snapshot);
        Ok(redraw)
    }

    /// Handle input event from input thread
    fn handle_input_event(&mut self, event: CompositorInputEvent) -> Result<bool, &'static str> {
        match event {
            CompositorInputEvent::MouseMove { dx, dy } => {
                self.set_cursor_hidden_by_touch(false);
                self.release_invalid_pointer_lock();
                if let Some(window_id) = captured_window(self.pointer_lock) {
                    self.send_locked_relative_motion(window_id, dx, dy);
                    return Ok(true);
                }
                self.cursor
                    .update_position(dx, dy, self.screen_width, self.screen_height);
                self.refresh_cursor_icon();
                if self.workspace_manager.presentation()
                    == sws_protocol::workspace::ShellPresentation::Overview
                {
                    self.full_redraw_needed = true;
                }

                if self.overview_window_drag.is_some() {
                    self.update_overview_window_drag(self.cursor.x, self.cursor.y);
                    return Ok(true);
                }
                if self.overview_pointer_navigation.is_some() {
                    return Ok(true);
                }

                if self.left_button_down {
                    if let Some(mut state) = self.resize_drag {
                        let old_outline = self.resize_outline;
                        let delta_x = self.cursor.x - state.grab_cursor_x;
                        let delta_y = self.cursor.y - state.grab_cursor_y;

                        let new_w = (state.start_width as i32 + delta_x)
                            .max(MIN_WINDOW_WIDTH as i32)
                            as u32;
                        let new_h = (state.start_height as i32 + delta_y)
                            .max(MIN_WINDOW_HEIGHT as i32)
                            as u32;
                        let (new_w, new_h) = self.window_manager.clamp_size_for_window(
                            state.window_id,
                            new_w,
                            new_h,
                        );
                        state.last_width = new_w;
                        state.last_height = new_h;
                        self.resize_drag = Some(state);

                        if let Some(window) = self.window_manager.get_window(state.window_id) {
                            self.resize_outline = Some(resize_outline_for_surface(
                                window.x,
                                window.y,
                                new_w,
                                new_h,
                                window.window_geometry_insets,
                            ));
                        }

                        if let Some(r) = old_outline {
                            self.add_pending_damage(r);
                        }
                        if let Some(r) = self.resize_outline {
                            self.add_pending_damage(r);
                        }
                        // While resizing, compositor grabs the pointer.
                        return Ok(true);
                    }
                }

                // If a window move is in progress, update the window position before
                // converting cursor coordinates into window-local space.
                if let Some(state) = self.move_drag {
                    let new_x = state.start_window_x + (self.cursor.x - state.grab_cursor_x);
                    let new_y = state.start_window_y + (self.cursor.y - state.grab_cursor_y);
                    sws_debug!(
                        "[Compositor] Move drag: window #{} start=({}, {}) grab=({}, {}) cursor=({}, {}) new=({}, {})",
                        state.window_id,
                        state.start_window_x,
                        state.start_window_y,
                        state.grab_cursor_x,
                        state.grab_cursor_y,
                        self.cursor.x,
                        self.cursor.y,
                        new_x,
                        new_y
                    );
                    self.set_window_position_with_damage(state.window_id, new_x, new_y);

                    // While moving a window, the compositor "grabs" the pointer.
                    // Avoid routing mouse moves to the currently focused client.
                    return Ok(true);
                }

                if let Some(grab_id) = self.pointer_grab_window_id {
                    if let Some(window) = self.window_manager.get_window(grab_id) {
                        self.pointer_focus_window_id = Some(grab_id);
                        self.send_mouse_position_to_window_unclipped(grab_id, window);
                        return Ok(true);
                    }
                }

                // Pointer focus follows geometry, independently from keyboard focus.
                self.route_pointer_motion_at_cursor();

                Ok(true)
            }
            CompositorInputEvent::MouseAbsolute { x, y } => {
                self.set_cursor_hidden_by_touch(false);
                self.release_invalid_pointer_lock();
                if let Some(state) = self.pointer_lock.as_mut() {
                    let delta = state.absolute_delta(x, y);
                    let window_id = state.window_id;
                    if let Some((dx, dy)) = delta {
                        self.send_locked_relative_motion(window_id, dx, dy);
                    }
                    return Ok(true);
                }
                self.cursor
                    .set_position(x, y, self.screen_width, self.screen_height);
                self.refresh_cursor_icon();
                if self.workspace_manager.presentation()
                    == sws_protocol::workspace::ShellPresentation::Overview
                {
                    self.full_redraw_needed = true;
                }

                if self.overview_window_drag.is_some() {
                    self.update_overview_window_drag(self.cursor.x, self.cursor.y);
                    return Ok(true);
                }
                if self.overview_pointer_navigation.is_some() {
                    return Ok(true);
                }

                if self.left_button_down {
                    if let Some(mut state) = self.resize_drag {
                        let old_outline = self.resize_outline;
                        let delta_x = self.cursor.x - state.grab_cursor_x;
                        let delta_y = self.cursor.y - state.grab_cursor_y;

                        let new_w = (state.start_width as i32 + delta_x)
                            .max(MIN_WINDOW_WIDTH as i32)
                            as u32;
                        let new_h = (state.start_height as i32 + delta_y)
                            .max(MIN_WINDOW_HEIGHT as i32)
                            as u32;
                        let (new_w, new_h) = self.window_manager.clamp_size_for_window(
                            state.window_id,
                            new_w,
                            new_h,
                        );
                        state.last_width = new_w;
                        state.last_height = new_h;
                        self.resize_drag = Some(state);

                        if let Some(window) = self.window_manager.get_window(state.window_id) {
                            self.resize_outline = Some(resize_outline_for_surface(
                                window.x,
                                window.y,
                                new_w,
                                new_h,
                                window.window_geometry_insets,
                            ));
                        }

                        if let Some(r) = old_outline {
                            self.add_pending_damage(r);
                        }
                        if let Some(r) = self.resize_outline {
                            self.add_pending_damage(r);
                        }
                        return Ok(true);
                    }
                }

                if let Some(state) = self.move_drag {
                    let new_x = state.start_window_x + (self.cursor.x - state.grab_cursor_x);
                    let new_y = state.start_window_y + (self.cursor.y - state.grab_cursor_y);
                    self.set_window_position_with_damage(state.window_id, new_x, new_y);

                    // While moving a window, the compositor "grabs" the pointer.
                    // Avoid routing mouse moves to the currently focused client.
                    return Ok(true);
                }

                if let Some(grab_id) = self.pointer_grab_window_id {
                    if let Some(window) = self.window_manager.get_window(grab_id) {
                        self.pointer_focus_window_id = Some(grab_id);
                        self.send_mouse_position_to_window_unclipped(grab_id, window);
                        return Ok(true);
                    }
                }

                self.route_pointer_motion_at_cursor();

                Ok(true)
            }
            CompositorInputEvent::MouseWheel { dx, dy } => {
                self.set_cursor_hidden_by_touch(false);
                if self.workspace_manager.presentation()
                    == sws_protocol::workspace::ShellPresentation::Overview
                    && dx != 0
                {
                    return Ok(self.handle_overview_horizontal_scroll(dx));
                }
                if self.shell_navigation_captures_pointer_at(self.cursor.x, self.cursor.y) {
                    self.clear_pointer_focus_for_shell_navigation();
                    return Ok(true);
                }
                let px_dy = dy.saturating_mul(super::input::WHEEL_PIXELS_PER_NOTCH);
                let px_dx = dx.saturating_mul(super::input::WHEEL_PIXELS_PER_NOTCH);
                let hi_dy = dy.saturating_mul(120);
                let hi_dx = dx.saturating_mul(120);

                let lock_target = captured_window(self.pointer_lock);
                if let Some(win_id) = lock_target.or_else(|| {
                    self.window_manager
                        .window_at_point(self.cursor.x, self.cursor.y)
                }) {
                    if let Some(window) = self.window_manager.get_window(win_id) {
                        if self.cursor_position_in_window(window).is_some()
                            || window.extension_owner.is_some()
                        {
                            let time = 0u64;
                            let ev_rel = super::input::event_types::EV_REL;
                            let ev_syn = super::input::event_types::EV_SYN;
                            use super::input::rel_codes as rel;

                            if let Some((extension_id, external_client_id)) = window.extension_owner
                            {
                                let send_ext = |code: u16, value: i32| {
                                    super::ipc::send_extension_input_event(
                                        extension_id,
                                        external_client_id,
                                        win_id,
                                        time,
                                        ev_rel,
                                        code,
                                        value,
                                    );
                                };
                                if px_dy != 0 {
                                    send_ext(rel::REL_WHEEL, px_dy);
                                    send_ext(rel::REL_WHEEL_HI_RES, hi_dy);
                                }
                                if px_dx != 0 {
                                    send_ext(rel::REL_HWHEEL, px_dx);
                                    send_ext(rel::REL_HWHEEL_HI_RES, hi_dx);
                                }
                                super::ipc::send_extension_input_event(
                                    extension_id,
                                    external_client_id,
                                    win_id,
                                    time,
                                    ev_syn,
                                    0,
                                    0,
                                );
                            } else {
                                let send = |code: u16, value: i32| {
                                    super::ipc::send_input_to_window(
                                        win_id, time, ev_rel, code, value,
                                    );
                                };
                                if px_dy != 0 {
                                    send(rel::REL_WHEEL, px_dy);
                                    send(rel::REL_WHEEL_HI_RES, hi_dy);
                                }
                                if px_dx != 0 {
                                    send(rel::REL_HWHEEL, px_dx);
                                    send(rel::REL_HWHEEL_HI_RES, hi_dx);
                                }
                                super::ipc::send_input_to_window(win_id, time, ev_syn, 0, 0);
                            }
                        }
                    }
                }

                Ok(true)
            }
            CompositorInputEvent::MouseButton { button, pressed } => {
                self.set_cursor_hidden_by_touch(false);
                if button == key_codes::BTN_LEFT {
                    self.left_button_down = pressed;
                    if !pressed && let Some(drag) = self.overview_window_drag.take() {
                        self.finish_overview_window_drag(OverviewWindowDrag {
                            current_x: self.cursor.x,
                            current_y: self.cursor.y,
                            ..drag
                        });
                        return Ok(true);
                    }
                    if !pressed && let Some(navigation) = self.overview_pointer_navigation.take() {
                        self.finish_overview_pointer_navigation(
                            navigation,
                            self.cursor.x,
                            self.cursor.y,
                        );
                        return Ok(true);
                    }
                    let presentation = self.workspace_manager.presentation();
                    let overview =
                        presentation == sws_protocol::workspace::ShellPresentation::Overview;
                    let shell_navigation =
                        presentation != sws_protocol::workspace::ShellPresentation::Workspace;
                    let in_workspace_region = shell_navigation
                        && self.point_in_overview_workspace_region(self.cursor.x, self.cursor.y);
                    let over_laptop_spread = overview
                        && !self.tablet_mode
                        && self
                            .overview_window_at_point(self.cursor.x, self.cursor.y)
                            .is_some();
                    if pressed && (in_workspace_region || over_laptop_spread) {
                        let remove_workspace_id = in_workspace_region
                            .then(|| {
                                self.overview_remove_workspace_at_point(
                                    self.cursor.x,
                                    self.cursor.y,
                                )
                            })
                            .flatten();
                        if overview
                            && remove_workspace_id.is_none()
                            && self.begin_overview_window_drag(self.cursor.x, self.cursor.y)
                        {
                            return Ok(true);
                        }
                        self.overview_pointer_navigation = Some(OverviewPointerNavigation {
                            start_x: self.cursor.x,
                            start_y: self.cursor.y,
                            start_workspace_id: self
                                .workspace_card_at_point(self.cursor.x, self.cursor.y),
                            start_add_workspace: self
                                .point_in_overview_add_workspace(self.cursor.x, self.cursor.y),
                            start_remove_workspace_id: remove_workspace_id,
                        });
                        self.update_pointer_focus(None);
                        return Ok(true);
                    }
                }
                if self.shell_navigation_captures_pointer_at(self.cursor.x, self.cursor.y) {
                    self.pointer_grab_window_id = None;
                    self.clear_pointer_focus_for_shell_navigation();
                    self.refresh_cursor_icon();
                    return Ok(true);
                }
                self.release_invalid_pointer_lock();
                if let Some(state) = self.pointer_lock {
                    if button == key_codes::BTN_LEFT {
                        self.left_button_down = pressed;
                    }
                    self.send_locked_input_event(
                        state.window_id,
                        0,
                        super::input::event_types::EV_KEY,
                        button,
                        if pressed { 1 } else { 0 },
                    );
                    self.send_locked_input_event(
                        state.window_id,
                        0,
                        super::input::event_types::EV_SYN,
                        0,
                        0,
                    );
                    return Ok(true);
                }
                let mut grab_target = None;

                if button == key_codes::BTN_LEFT {
                    self.left_button_down = pressed;
                    sws_debug!(
                        "[Compositor] Left button {} at cursor=({}, {})",
                        if pressed { "down" } else { "up" },
                        self.cursor.x,
                        self.cursor.y
                    );
                    if !pressed {
                        grab_target = self.pointer_grab_window_id;
                        self.pointer_grab_window_id = None;
                        self.last_left_down_cursor = None;
                        // Always exit move mode on left button release.
                        if self.move_drag.take().is_some() {
                            // No special redraw needed: the last drag motion already queued damage.
                        }

                        // Finalize resize on left button release.
                        if let Some(state) = self.resize_drag.take() {
                            let old_outline = self.resize_outline;
                            self.resize_outline = None;
                            if let Some(r) = old_outline {
                                self.add_pending_damage(r);
                            }

                            // Ask client to resize once (outline-only during drag).
                            let (width, height) = self.window_manager.clamp_size_for_window(
                                state.window_id,
                                state.last_width,
                                state.last_height,
                            );
                            let payload = sws_protocol::payload_window_configure(
                                state.window_id,
                                width,
                                height,
                            );
                            super::ipc::send_message_to_window(
                                state.window_id,
                                sws_protocol::server_msg::WINDOW_CONFIGURE,
                                payload.to_vec(),
                            );
                        }
                    }
                }

                if button == key_codes::BTN_LEFT && pressed {
                    self.last_left_down_cursor = Some((self.cursor.x, self.cursor.y));
                    // Determine target window under cursor.
                    let win_id_opt = self
                        .window_manager
                        .window_at_point(self.cursor.x, self.cursor.y);
                    if let Some(win_id) = win_id_opt {
                        let accepts_focus = self.window_manager.window_accepts_focus(win_id);
                        let mut interaction = PointerInteractionState {
                            focused_window_id: self.window_manager.get_focused_window_id(),
                            implicit_grab_window_id: self.pointer_grab_window_id,
                            locked_window_id: self.pointer_lock.map(|state| state.window_id),
                        };
                        interaction.button_pressed(win_id, accepts_focus);
                        self.pointer_grab_window_id = interaction.implicit_grab_window_id;
                        self.update_pointer_focus(Some(win_id));

                        // Apply focus and stacking before an edge press can
                        // return early into interactive resize mode.
                        self.handle_click()?;

                        // Start interactive resize if we're near the bottom/right edge.
                        if let Some(window) = self.window_manager.get_window(win_id) {
                            if self.cursor_position_in_window(window).is_some() {
                                let (geometry_x, geometry_y, geometry_width, geometry_height) =
                                    window.window_geometry();
                                let near_right = self.cursor.x
                                    >= geometry_x
                                        .saturating_add(geometry_width as i32 - RESIZE_GRIP_PX);
                                let near_bottom = self.cursor.y
                                    >= geometry_y
                                        .saturating_add(geometry_height as i32 - RESIZE_GRIP_PX);
                                // Only allow resize if window is marked as resizable
                                if (near_right || near_bottom)
                                    && window.resizable
                                    && !window.fullscreen
                                {
                                    let icon = match (near_right, near_bottom) {
                                        (true, true) => sws_protocol::CursorIcon::ResizeNwse,
                                        (true, false) => sws_protocol::CursorIcon::ResizeEw,
                                        (false, true) => sws_protocol::CursorIcon::ResizeNs,
                                        (false, false) => sws_protocol::CursorIcon::Arrow,
                                    };
                                    self.move_drag = None;
                                    self.resize_drag = Some(ResizeDragState {
                                        window_id: win_id,
                                        icon,
                                        grab_cursor_x: self.cursor.x,
                                        grab_cursor_y: self.cursor.y,
                                        start_width: window.width,
                                        start_height: window.height,
                                        last_width: window.width,
                                        last_height: window.height,
                                    });
                                    self.resize_outline = Some(resize_outline_for_surface(
                                        window.x,
                                        window.y,
                                        window.width,
                                        window.height,
                                        window.window_geometry_insets,
                                    ));
                                    if let Some(outline) = self.resize_outline {
                                        self.add_pending_damage(outline);
                                    }
                                    self.refresh_cursor_icon();
                                    return Ok(true);
                                }
                            }
                        }
                    } else {
                        self.pointer_grab_window_id = None;
                        self.update_pointer_focus(None);
                    }
                }

                // Route button event to the window under the cursor (even if it can't take focus).
                let target_id = if button == key_codes::BTN_LEFT {
                    if pressed {
                        self.pointer_grab_window_id.or_else(|| {
                            self.window_manager
                                .window_at_point(self.cursor.x, self.cursor.y)
                        })
                    } else {
                        grab_target.or_else(|| {
                            self.window_manager
                                .window_at_point(self.cursor.x, self.cursor.y)
                        })
                    }
                } else {
                    self.window_manager
                        .window_at_point(self.cursor.x, self.cursor.y)
                };

                if let Some(target_id) = target_id {
                    let window = self
                        .window_manager
                        .get_window(target_id)
                        .ok_or("Target window not found")?;
                    let allow_outside =
                        button == key_codes::BTN_LEFT && !pressed && grab_target == Some(target_id);
                    if allow_outside || self.cursor_position_in_window(window).is_some() {
                        // Ensure clients see the current pointer position before the button event.
                        self.send_mouse_position_to_window_unclipped(target_id, window);
                        // Check if this is an extension-owned window
                        if let Some((extension_id, external_client_id)) = window.extension_owner {
                            super::ipc::send_extension_input_event(
                                extension_id,
                                external_client_id,
                                target_id,
                                0,
                                super::input::event_types::EV_KEY,
                                button,
                                if pressed { 1 } else { 0 },
                            );
                            super::ipc::send_extension_input_event(
                                extension_id,
                                external_client_id,
                                target_id,
                                0,
                                super::input::event_types::EV_SYN,
                                0,
                                0,
                            );
                        } else {
                            super::ipc::send_input_to_window(
                                target_id,
                                0,
                                super::input::event_types::EV_KEY,
                                button,
                                if pressed { 1 } else { 0 },
                            );
                            super::ipc::send_input_to_window(
                                target_id,
                                0,
                                super::input::event_types::EV_SYN,
                                0,
                                0,
                            );
                        }
                    }
                }

                // Ending the implicit grab transfers pointer hover to the
                // actual window under the cursor, even when the mouse stays
                // stationary after release.
                if button == key_codes::BTN_LEFT && !pressed {
                    self.route_pointer_motion_at_cursor();
                }

                Ok(true)
            }
            CompositorInputEvent::TouchFrame(frame) => {
                let source = frame.source;
                let mut redraw = false;
                for policy_event in self.gesture_recognizer.process(frame) {
                    match policy_event {
                        TouchPolicyEvent::Pointer(CompositorInputEvent::MouseButton {
                            button,
                            pressed,
                        }) => super::input::push_pointer_button(source, button, pressed),
                        TouchPolicyEvent::Pointer(event) => {
                            redraw |= self.handle_input_event(event)?;
                        }
                        TouchPolicyEvent::Gesture(event) => self.handle_gesture_event(event),
                        TouchPolicyEvent::DirectTouch(frame) => {
                            redraw |= self.handle_direct_touch_frame(frame);
                        }
                        TouchPolicyEvent::SourceButton {
                            source,
                            button,
                            pressed,
                        } => super::input::push_pointer_button(source, button, pressed),
                        TouchPolicyEvent::ReleaseSource { source } => {
                            super::input::release_pointer_source(source)
                        }
                    }
                }
                Ok(redraw)
            }
            CompositorInputEvent::PostureChanged {
                tablet_mode,
                lid_closed,
            } => {
                let updated = input_environment::update_posture(tablet_mode, lid_closed);
                if let Some(snapshot) = updated {
                    return self.apply_input_environment_snapshot(snapshot);
                }
                Ok(false)
            }
            CompositorInputEvent::Keyboard {
                code,
                value,
                source,
                synthetic,
            } => {
                // Every current SWS keyboard source explicitly delegates
                // repeat timing to the compositor. Ignore raw value 2 so a
                // late device repeat cannot cross a focus boundary.
                if !synthetic && !is_physical_key_value(value) {
                    return Ok(false);
                }
                let pressed = value != 0;
                let focused_id = self.window_manager.get_focused_window_id();
                let super_tap_completed = if !synthetic && self.uses_super_tap_for_overview() {
                    let blocked_at_start = self.held_keys.has_any_other_than(&SUPER_KEY_CODES);
                    self.super_tap_state.observe(
                        source,
                        code,
                        value,
                        &SUPER_KEY_CODES,
                        blocked_at_start,
                    )
                } else {
                    false
                };
                if !synthetic {
                    let logical_transition = self.held_keys.update(source, code, value);
                    if !logical_transition {
                        if self.workspace_shortcut_keys.contains_code(code) {
                            self.workspace_shortcut_keys
                                .update_duplicate(source, code, value);
                            return Ok(false);
                        }
                        if self.ime_trigger_keys.contains_code(code) {
                            self.ime_trigger_keys.update_duplicate(source, code, value);
                            return Ok(false);
                        }
                        if value == 0
                            && let Some(replacement) = self.held_keys.source_for_code(code)
                        {
                            self.key_repeat.transfer_source(source, replacement, code);
                        }
                        return Ok(false);
                    }
                    self.key_repeat.handle_key_event(
                        code,
                        value,
                        source,
                        focused_id,
                        monotonic_time_ns(),
                    );
                }
                if !pressed && self.workspace_shortcut_keys.release(source, code) {
                    return Ok(false);
                }
                let modifiers = self.current_key_modifiers();
                // One exact-modifier table resolves every configurable shell
                // chord, so specific combinations cannot be shadowed by the
                // broader bindings they extend.
                if is_initial_press(value)
                    && let Some(action) = self.shell_action_for(code, modifiers)
                {
                    sws_debug!(
                        "[Compositor] shell shortcut: code={} modifiers={:?} action={:?}",
                        code,
                        modifiers,
                        action
                    );
                    self.key_repeat.cancel_key(source, code);
                    self.workspace_shortcut_keys.press(source, code);
                    self.run_shell_action(action);
                    return Ok(true);
                }
                if is_initial_press(value)
                    && laptop_overview_space_opens_home(
                        self.tablet_mode,
                        self.workspace_manager.presentation(),
                        code,
                        modifiers,
                    )
                {
                    self.key_repeat.cancel_key(source, code);
                    self.workspace_shortcut_keys.press(source, code);
                    if self
                        .workspace_manager
                        .set_presentation(sws_protocol::workspace::ShellPresentation::Home)
                    {
                        self.commit_workspace_change();
                    }
                    return Ok(true);
                }
                if is_initial_press(value)
                    && code == key_codes::KEY_ENTER
                    && modifiers == KeyModifiers::default()
                    && self.overview_add_workspace_selected
                    && self.workspace_manager.presentation()
                        == sws_protocol::workspace::ShellPresentation::Overview
                {
                    self.key_repeat.cancel_key(source, code);
                    self.workspace_shortcut_keys.press(source, code);
                    self.activate_shell_workspace_selection();
                    return Ok(true);
                }
                if is_initial_press(value)
                    && self.workspace_manager.presentation()
                        == sws_protocol::workspace::ShellPresentation::Overview
                    && modifiers == KeyModifiers::default()
                {
                    let handled = match code {
                        key_codes::KEY_LEFT => {
                            self.move_overview_selection(-1);
                            true
                        }
                        key_codes::KEY_RIGHT => {
                            self.move_overview_selection(1);
                            true
                        }
                        key_codes::KEY_ENTER => {
                            self.activate_shell_workspace_selection();
                            true
                        }
                        key_codes::KEY_ESC => {
                            if self.workspace_manager.return_to_workspace() {
                                self.commit_workspace_change();
                            }
                            true
                        }
                        _ => false,
                    };
                    if handled {
                        self.key_repeat.cancel_key(source, code);
                        self.workspace_shortcut_keys.press(source, code);
                        return Ok(true);
                    }
                }
                // A consumed IME trigger release must remain consumed even if
                // focus or the focused window's extension route changed while
                // the key was held.
                if !pressed && self.ime_trigger_keys.release(source, code) {
                    return Ok(false);
                }

                // Overview is a compositor-owned modal keyboard surface. Its
                // navigation keys and global shell chords were handled above;
                // no remaining key may leak into the hidden application
                // drawer (or an application underneath it). Home deliberately
                // falls through because that is the drawer's input depth.
                if self.workspace_manager.presentation()
                    == sws_protocol::workspace::ShellPresentation::Overview
                {
                    if super_tap_completed {
                        self.toggle_overview_presentation();
                        return Ok(true);
                    }
                    self.key_repeat.cancel_key(source, code);
                    return Ok(false);
                }

                // Route keyboard events to focused window
                if let Some(focused_id) = self.window_manager.get_focused_window_id() {
                    if let Some(window) = self.window_manager.get_window(focused_id) {
                        // Check if this is an extension-owned window
                        if let Some((extension_id, external_client_id)) = window.extension_owner {
                            if forward_to_binary_key_protocol(synthetic) {
                                super::ipc::send_extension_input_event(
                                    extension_id,
                                    external_client_id,
                                    focused_id,
                                    0,
                                    super::input::event_types::EV_KEY,
                                    code,
                                    value,
                                );
                                super::ipc::send_extension_input_event(
                                    extension_id,
                                    external_client_id,
                                    focused_id,
                                    0,
                                    super::input::event_types::EV_SYN,
                                    0,
                                    0,
                                );
                            }
                        } else {
                            if is_initial_press(value) && self.is_ime_toggle_key(code) {
                                if super::ipc::send_input_method_trigger(focused_id, 0, code) {
                                    self.key_repeat.cancel_key(source, code);
                                    self.ime_trigger_keys.press(source, code);
                                    return Ok(false);
                                }
                            }

                            if !super::ipc::send_key_to_input_method(
                                focused_id,
                                0,
                                super::input::event_types::EV_KEY,
                                code,
                                value,
                            ) {
                                super::ipc::send_input_to_window(
                                    focused_id,
                                    0,
                                    super::input::event_types::EV_KEY,
                                    code,
                                    value,
                                );
                                super::ipc::send_input_to_window(
                                    focused_id,
                                    0,
                                    super::input::event_types::EV_SYN,
                                    0,
                                    0,
                                );
                            }
                        }
                    }
                }
                if super_tap_completed {
                    self.toggle_overview_presentation();
                    return Ok(true);
                }
                Ok(false) // Keyboard events don't trigger redraws
            }
            CompositorInputEvent::KeyboardReset { source } => {
                self.release_keyboard_source(source)?;
                Ok(false)
            }
        }
    }

    fn window_id_in(window_ids: &[u32], window_id: u32) -> bool {
        window_ids.iter().any(|&id| id == window_id)
    }

    fn broadcast_empty_active_app_changed(&mut self) {
        let empty_payload = sws_protocol::payload_active_app_changed(
            0,   // dummy window_id
            b"", // empty app_id
            b"", // empty app_name
            b"", // empty title
            b"", // empty menu_titles
        );
        println!("[Compositor] Broadcasting empty ACTIVE_APP_CHANGED to clear TaskBar menu");
        super::ipc::broadcast_message_to_all_clients(
            sws_protocol::server_msg::ACTIVE_APP_CHANGED,
            empty_payload,
        );
    }

    fn clear_interaction_state_for_removed_windows(&mut self, window_ids: &[u32]) {
        self.pending_workspace_scenes
            .retain(|window_id| !Self::window_id_in(window_ids, *window_id));
        if self
            .pointer_lock
            .is_some_and(|state| Self::window_id_in(window_ids, state.window_id))
        {
            self.release_pointer_lock();
        }
        if let Some(window_id) = self.pointer_focus_window_id
            && Self::window_id_in(window_ids, window_id)
        {
            self.pointer_focus_window_id = None;
        }

        if let Some(window_id) = self.pointer_grab_window_id {
            if Self::window_id_in(window_ids, window_id) {
                self.pointer_grab_window_id = None;
                self.last_left_down_cursor = None;
            }
        }

        if let Some(state) = self.move_drag {
            if Self::window_id_in(window_ids, state.window_id) {
                self.move_drag = None;
            }
        }
        if let Some(state) = self.overview_window_drag {
            if Self::window_id_in(window_ids, state.window_id) {
                self.overview_window_drag = None;
            }
        }

        if let Some(state) = self.resize_drag {
            if Self::window_id_in(window_ids, state.window_id) {
                if let Some(outline) = self.resize_outline.take() {
                    self.add_pending_damage(outline);
                }
                self.resize_drag = None;
            }
        }
    }

    fn close_client_windows(
        &mut self,
        client_id: usize,
        window_ids: &[u32],
        notify_client: bool,
    ) -> Result<bool, &'static str> {
        self.pending_frame_callbacks.retain(|callback| {
            callback.client_id != client_id || !window_ids.contains(&callback.window_id)
        });
        let mut removed_windows: Vec<(u32, (i32, i32, u32, u32), Vec<u8>)> = Vec::new();
        for &window_id in window_ids {
            if let Some(window) = self.window_manager.get_window(window_id) {
                removed_windows.push((
                    window_id,
                    (window.x, window.y, window.width, window.height),
                    window.app_id.clone().unwrap_or_default(),
                ));
            }
        }

        if removed_windows.is_empty() {
            return Ok(false);
        }

        let removed_ids: Vec<u32> = removed_windows
            .iter()
            .map(|(window_id, _, _)| *window_id)
            .collect();
        let mut workspace_changed = false;
        for window_id in &removed_ids {
            workspace_changed |= self.workspace_manager.remove_window(*window_id);
        }
        workspace_changed |= self
            .workspace_manager
            .settle_after_close(self.windowing_mode == sws_protocol::WindowingMode::Focused);
        self.clear_interaction_state_for_removed_windows(&removed_ids);
        self.remove_ime_popup_windows(&removed_ids);
        for window_id in &removed_ids {
            self.release_gpu_window(*window_id)?;
        }

        let mut active_app_removed = false;
        for (window_id, rect, app_id) in &removed_windows {
            if notify_client {
                let payload = sws_protocol::payload_window_destroyed(*window_id);
                send_message_to_client(
                    client_id,
                    sws_protocol::server_msg::WINDOW_DESTROYED,
                    payload.to_vec(),
                );
                println!(
                    "[Compositor] Sent WINDOW_DESTROYED for window #{} to client {}",
                    window_id, client_id
                );
            }

            if self.last_focused_window_id == Some(*window_id) {
                println!(
                    "[Compositor] Focused window destroyed, resetting last_focused_window_id (was={})",
                    window_id
                );
                self.last_focused_window_id = None;
            }

            if self.active_app_id.as_ref().map_or(false, |current_app_id| {
                current_app_id.as_slice() == app_id.as_slice()
            }) {
                println!(
                    "[Compositor] Active app window destroyed, resetting active_app_id (was={})",
                    core::str::from_utf8(app_id).unwrap_or("")
                );
                active_app_removed = true;
            }

            self.window_manager.close_window(*window_id);
            self.add_pending_damage(*rect);
        }

        if active_app_removed {
            self.active_app_id = None;
        }

        if let Some(new_focus) = self.window_manager.get_focused_window_id() {
            if active_app_removed || self.last_focused_window_id != Some(new_focus) {
                self.last_focused_window_id = None;
                self.broadcast_focus_change(new_focus);
            }
        }

        if active_app_removed && self.active_app_id.is_none() {
            self.broadcast_empty_active_app_changed();
        }

        if workspace_changed {
            self.apply_workspace_presentation_policy();
            self.publish_workspace_state();
        }

        self.refresh_cursor_icon();
        self.full_redraw_needed = true;
        Ok(true)
    }

    fn set_ime_popup_window(
        &mut self,
        context_id: u32,
        window_id: u32,
        offset_x: i32,
        offset_y: i32,
        visible: bool,
    ) -> bool {
        if self.window_manager.get_window(window_id).is_none() {
            return false;
        }

        if let Some(popup) = self
            .ime_popup_windows
            .iter_mut()
            .find(|popup| popup.window_id == window_id)
        {
            popup.context_id = context_id;
            popup.offset_x = offset_x;
            popup.offset_y = offset_y;
            popup.visible = visible;
        } else {
            self.ime_popup_windows.push(ImePopupWindow {
                context_id,
                window_id,
                offset_x,
                offset_y,
                visible,
            });
        }

        self.position_ime_popup_window(window_id)
    }

    fn position_ime_popups_for_context(&mut self, context_id: u32) -> bool {
        let popup_ids: Vec<u32> = self
            .ime_popup_windows
            .iter()
            .filter(|popup| popup.context_id == context_id)
            .map(|popup| popup.window_id)
            .collect();
        let mut changed = false;
        for window_id in popup_ids {
            changed |= self.position_ime_popup_window(window_id);
        }
        changed
    }

    fn position_all_ime_popup_windows(&mut self) -> bool {
        let popup_ids: Vec<u32> = self
            .ime_popup_windows
            .iter()
            .map(|popup| popup.window_id)
            .collect();
        let mut changed = false;
        for window_id in popup_ids {
            changed |= self.position_ime_popup_window(window_id);
        }
        changed
    }

    fn remove_ime_popup_windows(&mut self, window_ids: &[u32]) {
        self.ime_popup_windows
            .retain(|popup| !window_ids.iter().any(|id| *id == popup.window_id));
    }

    fn position_ime_popup_window(&mut self, window_id: u32) -> bool {
        let Some(popup) = self
            .ime_popup_windows
            .iter()
            .find(|popup| popup.window_id == window_id)
            .copied()
        else {
            return false;
        };

        let Some(cursor) = super::ipc::text_input_cursor_rect(popup.context_id) else {
            if let Some(window) = self.window_manager.get_window_mut(window_id) {
                window.visible = false;
            }
            return true;
        };
        let Some(anchor_window) = self.window_manager.get_window(cursor.window_id) else {
            if let Some(window) = self.window_manager.get_window_mut(window_id) {
                window.visible = false;
            }
            return true;
        };
        let anchor_x = anchor_window.x;
        let anchor_y = anchor_window.y;

        let Some(old_window) = self.window_manager.get_window(window_id) else {
            return false;
        };
        let old_rect = (
            old_window.x,
            old_window.y,
            old_window.width,
            old_window.height,
        );

        if let Some(window) = self.window_manager.get_window_mut(window_id) {
            window.visible = popup.visible;
        }

        let mut x = anchor_x
            .saturating_add(cursor.x)
            .saturating_add(popup.offset_x);
        let mut y = anchor_y
            .saturating_add(cursor.y)
            .saturating_add(cursor.height as i32)
            .saturating_add(popup.offset_y);

        let max_x = (self.screen_width as i32).saturating_sub(old_rect.2 as i32);
        let max_y = (self.screen_height as i32).saturating_sub(old_rect.3 as i32);
        if y > max_y {
            let cursor_top = anchor_y.saturating_add(cursor.y);
            let cursor_bottom = cursor_top.saturating_add(cursor.height as i32);
            let below_space = (self.screen_height as i32).saturating_sub(cursor_bottom);
            let above_space = cursor_top.max(0);
            let above_y = anchor_y
                .saturating_add(cursor.y)
                .saturating_sub(old_rect.3 as i32)
                .saturating_sub(popup.offset_y);
            if above_space >= below_space {
                y = above_y;
            }
        }
        if x > max_x {
            x = max_x;
        }
        x = x.max(0).min(max_x.max(0));
        y = y.max(0).min(max_y.max(0));

        self.window_manager.set_window_position(window_id, x, y);
        self.window_manager.raise_to_top_with_type(window_id);

        self.add_pending_damage(old_rect);
        if let Some(new_window) = self.window_manager.get_window(window_id) {
            self.add_pending_damage((
                new_window.x,
                new_window.y,
                new_window.width,
                new_window.height,
            ));
        }
        true
    }

    /// Return whether a normal surface is currently a top-level app scene.
    fn is_workspace_scene_root(&self, window_id: u32) -> bool {
        self.window_manager
            .get_window(window_id)
            .is_some_and(|window| {
                window.window_type == WindowType::Normal
                    && window.parent.is_none()
                    && !is_shell_app_id(window.app_id.as_deref().unwrap_or(b""))
            })
    }

    fn register_new_workspace_scene(&mut self, window_id: u32) -> bool {
        if !self.is_workspace_scene_root(window_id) {
            return false;
        }
        if self.tablet_mode {
            if !self.pending_workspace_scenes.contains(&window_id) {
                self.pending_workspace_scenes.push(window_id);
            }
            return false;
        }
        self.workspace_manager.add_scene_root(
            window_id,
            false,
            self.windowing_mode == sws_protocol::WindowingMode::Focused,
        );
        true
    }

    fn discard_pending_workspace_scene(&mut self, window_id: u32) -> bool {
        let previous_len = self.pending_workspace_scenes.len();
        self.pending_workspace_scenes
            .retain(|candidate| *candidate != window_id);
        previous_len != self.pending_workspace_scenes.len()
    }

    fn finalize_pending_workspace_scene(&mut self, window_id: u32) -> bool {
        if !self.discard_pending_workspace_scene(window_id)
            || !self.is_workspace_scene_root(window_id)
            || self
                .workspace_manager
                .workspace_for_window(window_id)
                .is_some()
        {
            return false;
        }

        self.workspace_manager.add_scene_root(
            window_id,
            self.tablet_mode,
            self.windowing_mode == sws_protocol::WindowingMode::Focused,
        );
        if let Some(focused_window_id) = self.window_manager.get_focused_window_id()
            && self.top_level_window_id(focused_window_id) == window_id
        {
            self.last_workspace_focus = Some(focused_window_id);
        }
        self.apply_workspace_presentation_policy();
        self.publish_workspace_state();
        self.full_redraw_needed = true;
        true
    }

    fn client_owns_window(&self, client_id: usize, window_id: u32) -> bool {
        self.window_manager
            .get_window(window_id)
            .is_some_and(|window| window.owner_client_id == Some(client_id))
    }

    fn queue_frame_callback(&mut self, client_id: usize, window_id: u32, callback_id: u64) {
        if !self.client_owns_window(client_id, window_id) {
            send_message_to_client(
                client_id,
                sws_protocol::server_msg::ERROR,
                sws_protocol::payload_error(sws_protocol::error_codes::WINDOW_NOT_OWNED).to_vec(),
            );
            return;
        }
        if self
            .pending_frame_callbacks
            .iter()
            .any(|callback| callback.client_id == client_id && callback.window_id == window_id)
        {
            send_message_to_client(
                client_id,
                sws_protocol::server_msg::ERROR,
                sws_protocol::payload_error(sws_protocol::error_codes::INVALID_FRAME_REQUEST)
                    .to_vec(),
            );
            return;
        }
        self.pending_frame_callbacks.push(PendingFrameCallback {
            client_id,
            window_id,
            callback_id,
            requested_after_present: self.event_counter,
        });
    }

    fn grant_pending_frame_callbacks(&mut self) {
        if self.pending_frame_callbacks.is_empty() {
            return;
        }
        let now_ns = monotonic_time_ns();
        let mut waiting = Vec::new();
        for callback in core::mem::take(&mut self.pending_frame_callbacks) {
            let Some(window) = self.window_manager.get_window(callback.window_id) else {
                continue;
            };
            if window.owner_client_id != Some(callback.client_id) {
                continue;
            }
            let ready = frame_callback_is_ready(
                window.is_presented(),
                window.has_presented_frame,
                self.event_counter,
                callback.requested_after_present,
            );
            if !ready {
                waiting.push(callback);
                continue;
            }
            let payload =
                sws_protocol::payload_frame_done(callback.window_id, callback.callback_id, now_ns);
            send_message_to_client(
                callback.client_id,
                sws_protocol::server_msg::FRAME_DONE,
                payload.to_vec(),
            );
        }
        self.pending_frame_callbacks = waiting;
    }

    fn send_window_state_changed(&self, window_id: u32) {
        let Some(window) = self.window_manager.get_window(window_id) else {
            return;
        };
        let payload = sws_protocol::payload_window_state_changed(window_id, window.state_flags());
        super::ipc::send_message_to_window(
            window_id,
            sws_protocol::server_msg::WINDOW_STATE_CHANGED,
            payload.to_vec(),
        );
    }

    fn send_current_window_configure(&self, window_id: u32) {
        let Some(window) = self.window_manager.get_window(window_id) else {
            return;
        };
        let payload =
            sws_protocol::payload_window_configure(window_id, window.width, window.height);
        super::ipc::send_message_to_window(
            window_id,
            sws_protocol::server_msg::WINDOW_CONFIGURE,
            payload.to_vec(),
        );
    }

    fn maximized_geometry(&self, window_id: u32) -> Option<(i32, i32, u32, u32)> {
        let window = self.window_manager.get_window(window_id)?;
        Some(maximized_geometry_for(
            window.window_type,
            self.workarea,
            self.screen_width,
            self.screen_height,
        ))
    }

    fn reflow_maximized_windows_to_workarea(&mut self) -> bool {
        let window_ids: Vec<u32> = self
            .window_manager
            .get_windows()
            .iter()
            .filter(|window| {
                window.maximized
                    && !window.fullscreen
                    && window.window_type == super::window::WindowType::Normal
            })
            .map(|window| window.id)
            .collect();
        let mut changed = false;

        for window_id in window_ids {
            let Some((old_surface, old_geometry, visible)) =
                self.window_manager.get_window(window_id).map(|window| {
                    (
                        window.surface_geometry(),
                        window.window_geometry(),
                        window.visible,
                    )
                })
            else {
                continue;
            };
            let Some((x, y, width, height)) = self.maximized_geometry(window_id) else {
                continue;
            };
            if old_geometry == (x, y, width, height) {
                continue;
            }

            self.window_manager.set_window_position(window_id, x, y);
            self.window_manager
                .resize_window_geometry_in_place(window_id, width, height);
            if visible {
                self.add_pending_damage(old_surface);
                if let Some(new_surface) = self
                    .window_manager
                    .get_window(window_id)
                    .map(|window| window.surface_geometry())
                {
                    self.add_pending_damage(new_surface);
                }
            }
            self.send_current_window_configure(window_id);
            changed = true;
        }

        changed
    }

    fn finish_policy_geometry_change(
        &mut self,
        window_id: u32,
        old_surface: (i32, i32, u32, u32),
        old_state_flags: u32,
        old_visible: bool,
    ) -> bool {
        let Some((new_surface, new_state_flags, new_visible)) =
            self.window_manager.get_window(window_id).map(|window| {
                (
                    window.surface_geometry(),
                    window.state_flags(),
                    window.visible,
                )
            })
        else {
            return false;
        };
        let geometry_changed = old_surface != new_surface;
        let state_changed = old_state_flags != new_state_flags;
        let surface_resized = old_surface.2 != new_surface.2 || old_surface.3 != new_surface.3;
        if !geometry_changed && !state_changed {
            return false;
        }

        if old_visible {
            self.add_pending_damage(old_surface);
        }
        if new_visible {
            self.add_pending_damage(new_surface);
        }
        if state_changed {
            self.send_window_state_changed(window_id);
        }
        if surface_resized {
            self.send_current_window_configure(window_id);
        }
        self.full_redraw_needed = true;
        true
    }

    fn note_window_frame_submitted(&mut self, window_id: u32) {
        let focused = self.windowing_mode == sws_protocol::WindowingMode::Focused;
        let first_frame_needs_policy =
            self.window_manager
                .get_window_mut(window_id)
                .is_some_and(|window| {
                    let first = window.mark_presented_frame();
                    first && (window.pending_maximize || focused)
                });
        if first_frame_needs_policy {
            self.window_policy_after_present = true;
        }
    }

    fn maximize_window_from_client(&mut self, window_id: u32) -> bool {
        let Some((old_surface, old_state_flags, old_visible)) =
            self.window_manager.get_window(window_id).map(|window| {
                (
                    window.surface_geometry(),
                    window.state_flags(),
                    window.visible,
                )
            })
        else {
            return false;
        };
        let Some((max_x, max_y, max_width, max_height)) = self.maximized_geometry(window_id) else {
            return false;
        };
        if !self
            .window_manager
            .maximize_window(window_id, max_width, max_height)
        {
            return false;
        }
        self.window_manager
            .set_window_position(window_id, max_x, max_y);
        self.finish_policy_geometry_change(window_id, old_surface, old_state_flags, old_visible)
    }

    fn apply_post_present_window_policy(&mut self) -> bool {
        let pending_maximize: Vec<u32> = self
            .window_manager
            .get_windows()
            .iter()
            .filter(|window| window.has_presented_frame && window.pending_maximize)
            .map(|window| window.id)
            .collect();
        let mut changed = false;
        for window_id in pending_maximize {
            if let Some(window) = self.window_manager.get_window_mut(window_id) {
                window.pending_maximize = false;
            }
            changed |= self.maximize_window_from_client(window_id);
        }
        if self.windowing_mode == sws_protocol::WindowingMode::Focused {
            changed |= self.apply_windowing_mode_policy();
        }
        changed
    }

    fn restore_workspace_managed_window(&mut self, window_id: u32) -> bool {
        let Some((old_surface, old_visible, managed, restore_geometry)) =
            self.window_manager.get_window(window_id).map(|window| {
                (
                    window.surface_geometry(),
                    window.is_presented(),
                    window.workspace_layout_managed,
                    window.workspace_restore_geometry,
                )
            })
        else {
            return false;
        };
        if !managed {
            return false;
        }
        if let Some((x, y, width, height)) = restore_geometry {
            self.window_manager.set_window_position(window_id, x, y);
            self.window_manager
                .resize_window_in_place(window_id, width, height);
        }
        if let Some(window) = self.window_manager.get_window_mut(window_id) {
            window.workspace_layout_managed = false;
            window.workspace_restore_geometry = None;
        }
        if old_visible {
            self.add_pending_damage(old_surface);
        }
        if let Some((new_surface, new_visible)) = self
            .window_manager
            .get_window(window_id)
            .map(|window| (window.surface_geometry(), window.is_presented()))
        {
            if new_visible {
                self.add_pending_damage(new_surface);
            }
            if old_surface.2 != new_surface.2 || old_surface.3 != new_surface.3 {
                self.send_current_window_configure(window_id);
            }
        }
        true
    }

    fn tablet_slot_for_window(&self, window_id: u32) -> Option<(i32, i32, u32, u32)> {
        let workspace_id = self
            .workspace_manager
            .workspace_for_window(self.top_level_window_id(window_id))?;
        let (x, y, width, height) =
            self.workarea
                .unwrap_or((0, 0, self.screen_width, self.screen_height));
        let divider = 8u32.min(width.saturating_sub(2));
        match self.workspace_manager.tablet_layout(workspace_id) {
            sws_protocol::workspace::TabletLayout::Empty => None,
            sws_protocol::workspace::TabletLayout::Single {
                window_id: presented,
            } => (presented == window_id).then_some((x, y, width.max(1), height.max(1))),
            sws_protocol::workspace::TabletLayout::Split {
                axis,
                first_window_id,
                second_window_id,
                ratio_milli,
            } => match axis {
                sws_protocol::workspace::SplitAxis::Horizontal => {
                    let available = width.saturating_sub(divider).max(2);
                    let first_width = ((u64::from(available) * u64::from(ratio_milli)) / 1000)
                        .clamp(1, u64::from(available.saturating_sub(1)))
                        as u32;
                    let second_width = available.saturating_sub(first_width).max(1);
                    if first_window_id == window_id {
                        Some((x, y, first_width, height.max(1)))
                    } else if second_window_id == window_id {
                        Some((
                            x.saturating_add(first_width.saturating_add(divider) as i32),
                            y,
                            second_width,
                            height.max(1),
                        ))
                    } else {
                        None
                    }
                }
                sws_protocol::workspace::SplitAxis::Vertical => {
                    let available = height.saturating_sub(divider).max(2);
                    let first_height = ((u64::from(available) * u64::from(ratio_milli)) / 1000)
                        .clamp(1, u64::from(available.saturating_sub(1)))
                        as u32;
                    let second_height = available.saturating_sub(first_height).max(1);
                    if first_window_id == window_id {
                        Some((x, y, width.max(1), first_height))
                    } else if second_window_id == window_id {
                        Some((
                            x,
                            y.saturating_add(first_height.saturating_add(divider) as i32),
                            width.max(1),
                            second_height,
                        ))
                    } else {
                        None
                    }
                }
            },
        }
    }

    fn tablet_surface_geometry_for_window(
        &self,
        window_id: u32,
        slot: (i32, i32, u32, u32),
    ) -> Option<(i32, i32, u32, u32)> {
        let window = self.window_manager.get_window(window_id)?;
        let geometry = window.window_geometry();
        let target = if window.supports_focused_windowing() {
            slot
        } else {
            (
                slot.0.saturating_add(
                    (slot.2.saturating_sub(geometry.2) / 2).min(i32::MAX as u32) as i32
                ),
                slot.1.saturating_add(
                    (slot.3.saturating_sub(geometry.3) / 2).min(i32::MAX as u32) as i32
                ),
                geometry.2.min(slot.2).max(1),
                geometry.3.min(slot.3).max(1),
            )
        };
        Some(window.surface_geometry_for_window_geometry(target.0, target.1, target.2, target.3))
    }

    fn apply_tablet_geometry(&mut self, window_id: u32, slot: (i32, i32, u32, u32)) -> bool {
        let Some((old_surface, old_visible, ready, negotiated)) =
            self.window_manager.get_window(window_id).map(|window| {
                (
                    window.surface_geometry(),
                    window.is_presented(),
                    window.has_presented_frame,
                    window.initial_size_negotiated,
                )
            })
        else {
            return false;
        };
        if !ready && !negotiated {
            return false;
        }

        let target_surface = self
            .tablet_surface_geometry_for_window(window_id, slot)
            .unwrap_or(old_surface);
        if old_surface == target_surface {
            return false;
        }
        if let Some(window) = self.window_manager.get_window_mut(window_id) {
            if !window.workspace_layout_managed {
                window.workspace_restore_geometry = Some(old_surface);
            }
            window.workspace_layout_managed = true;
            window.focused_mode_managed = false;
            window.center_on_first_geometry = false;
            window.x = target_surface.0;
            window.y = target_surface.1;
            window.width = target_surface.2;
            window.height = target_surface.3;
        }
        if old_visible {
            self.add_pending_damage(old_surface);
        }
        self.add_pending_damage(target_surface);
        if old_surface.2 != target_surface.2 || old_surface.3 != target_surface.3 {
            self.send_current_window_configure(window_id);
        }
        true
    }

    fn desired_workspace_visibility(&self, window_id: u32) -> bool {
        let Some(window) = self.window_manager.get_window(window_id) else {
            return false;
        };
        if matches!(
            window.window_type,
            WindowType::ShellBackground | WindowType::ShellChrome
        ) {
            return matches!(
                self.workspace_manager.presentation(),
                sws_protocol::workspace::ShellPresentation::Home
                    | sws_protocol::workspace::ShellPresentation::Overview
            );
        }
        if matches!(
            window.window_type,
            WindowType::Desktop | WindowType::Taskbar
        ) {
            return true;
        }
        if window.app_id.as_deref() == Some(b"org.scarlet-os.desktop.shell.home") {
            return matches!(
                self.workspace_manager.presentation(),
                sws_protocol::workspace::ShellPresentation::Home
                    | sws_protocol::workspace::ShellPresentation::Overview
            );
        }
        let root_id = self.top_level_window_id(window_id);
        let Some(workspace_id) = self.workspace_manager.workspace_for_window(root_id) else {
            // System overlays such as the launcher are deliberately not
            // workspace members. Normal application roots are hidden until
            // registration assigns them to a workspace.
            return window.window_type != WindowType::Normal;
        };
        match self.workspace_manager.presentation() {
            sws_protocol::workspace::ShellPresentation::Workspace => {
                if workspace_id != self.workspace_manager.active_workspace() {
                    return false;
                }
                self.windowing_mode == sws_protocol::WindowingMode::Freeform
                    || self.tablet_slot_for_window(root_id).is_some()
            }
            sws_protocol::workspace::ShellPresentation::Home
            | sws_protocol::workspace::ShellPresentation::Overview => {
                self.overview_card_rects().iter().any(|(candidate, rect)| {
                    *candidate == workspace_id
                        && intersect_compositor_rects(
                            *rect,
                            (0, 0, self.screen_width, self.screen_height),
                        )
                        .is_some()
                }) && (self.windowing_mode == sws_protocol::WindowingMode::Freeform
                    || self.tablet_slot_for_window(root_id).is_some())
            }
        }
    }

    fn overview_workspace_region(&self) -> (i32, i32, u32, u32) {
        let workarea = self
            .workarea
            .unwrap_or((0, 0, self.screen_width, self.screen_height));
        overview_workspace_region_for(
            workarea,
            self.tablet_mode,
            self.output_scale_milli,
            self.workspace_manager.presentation(),
        )
    }

    fn overview_layout_rects(&self) -> Vec<(u32, (i32, i32, u32, u32))> {
        let state = self.workspace_manager.snapshot();
        let workarea = self
            .workarea
            .unwrap_or((0, 0, self.screen_width, self.screen_height));
        let mut workspace_ids = state
            .workspaces
            .iter()
            .map(|workspace| workspace.id)
            .collect::<Vec<_>>();
        if workspace_ids.len() < sws_protocol::workspace::MAX_WORKSPACES {
            // Zero is never a valid WorkspaceId and therefore safely denotes
            // the visual-only `+` creation tile.
            workspace_ids.push(0);
        }
        layout_overview_cards(
            workarea,
            self.tablet_mode,
            self.output_scale_milli,
            state.presentation,
            &workspace_ids,
            state.active_workspace,
        )
    }

    fn overview_card_rects(&self) -> Vec<(u32, (i32, i32, u32, u32))> {
        self.overview_layout_rects()
            .into_iter()
            .filter(|(workspace_id, _)| *workspace_id != 0)
            .collect()
    }

    fn overview_add_workspace_rect(&self) -> Option<(i32, i32, u32, u32)> {
        self.overview_layout_rects()
            .into_iter()
            .find_map(|(workspace_id, rect)| (workspace_id == 0).then_some(rect))
    }

    fn overview_remove_buttons(&self) -> Vec<(u32, (i32, i32, u32, u32), bool)> {
        if self.workspace_manager.presentation()
            != sws_protocol::workspace::ShellPresentation::Overview
        {
            return Vec::new();
        }
        let state = self.workspace_manager.snapshot();
        if state.workspaces.len() <= 1 {
            return Vec::new();
        }
        let (pointer_x, pointer_y) = self
            .overview_window_drag
            .map_or((self.cursor.x, self.cursor.y), |drag| {
                (drag.current_x, drag.current_y)
            });
        self.overview_card_rects()
            .into_iter()
            .filter_map(|(workspace_id, card)| {
                if !self.workspace_manager.can_remove_workspace(
                    workspace_id,
                    self.windowing_mode == sws_protocol::WindowingMode::Freeform,
                ) {
                    return None;
                }
                let hovered = rounded_rect_contains_point(
                    card,
                    sws_protocol::workspace::OVERVIEW_CARD_CORNER_RADIUS,
                    pointer_x,
                    pointer_y,
                );
                let visible = hovered
                    || self.tablet_mode
                        && !self.overview_add_workspace_selected
                        && workspace_id == self.workspace_manager.active_workspace();
                if !visible {
                    return None;
                }
                let short_side = card.2.min(card.3);
                let size = (short_side / 7).clamp(28, 44);
                let inset = (size / 4).max(6);
                let rect = (
                    card.0
                        .saturating_add(card.2 as i32)
                        .saturating_sub(size as i32)
                        .saturating_sub(inset as i32),
                    card.1.saturating_add(inset as i32),
                    size,
                    size,
                );
                let button_hovered =
                    rounded_rect_contains_point(rect, size / 2, pointer_x, pointer_y);
                Some((workspace_id, rect, button_hovered))
            })
            .collect()
    }

    fn overview_remove_workspace_at_point(&self, x: i32, y: i32) -> Option<u32> {
        self.overview_remove_buttons()
            .into_iter()
            .find_map(|(workspace_id, rect, _)| {
                rounded_rect_contains_point(rect, rect.2 / 2, x, y).then_some(workspace_id)
            })
    }

    fn overview_render_backplates(&self) -> Vec<((i32, i32, u32, u32), bool, bool)> {
        if !matches!(
            self.workspace_manager.presentation(),
            sws_protocol::workspace::ShellPresentation::Home
                | sws_protocol::workspace::ShellPresentation::Overview
        ) {
            return Vec::new();
        }
        let active = self.workspace_manager.active_workspace();
        let mut backplates = self
            .overview_card_rects()
            .into_iter()
            .map(|(workspace_id, rect)| {
                (
                    rect,
                    !self.overview_add_workspace_selected && workspace_id == active,
                    false,
                )
            })
            .collect::<Vec<_>>();
        if let Some(rect) = self.overview_add_workspace_rect() {
            let (pointer_x, pointer_y) = self
                .overview_window_drag
                .map_or((self.cursor.x, self.cursor.y), |drag| {
                    (drag.current_x, drag.current_y)
                });
            let hovered = rounded_rect_contains_point(
                rect,
                sws_protocol::workspace::OVERVIEW_CARD_CORNER_RADIUS,
                pointer_x,
                pointer_y,
            );
            backplates.push((rect, hovered || self.overview_add_workspace_selected, true));
        }
        backplates
    }

    fn overview_render_shadows(&self) -> Vec<OverviewShadowLayer> {
        if !matches!(
            self.workspace_manager.presentation(),
            sws_protocol::workspace::ShellPresentation::Home
                | sws_protocol::workspace::ShellPresentation::Overview
        ) {
            return Vec::new();
        }
        let mut layers = Vec::new();
        for (_, rect) in self.overview_layout_rects() {
            push_overview_shadow_layers(
                &mut layers,
                rect,
                sws_protocol::workspace::OVERVIEW_CARD_CORNER_RADIUS,
                false,
            );
        }
        if !self.tablet_mode
            && self.workspace_manager.presentation()
                == sws_protocol::workspace::ShellPresentation::Overview
        {
            for slot in self.laptop_overview_spread_slots() {
                let Some(window) = self.window_manager.get_window(slot.window_id) else {
                    continue;
                };
                // ScarletUI reserves transparent surface outsets for its own
                // standard window shadow. That shadow is already part of the
                // retained buffer and scales cleanly with the Overview actor;
                // adding another compositor shadow would create a muddy,
                // oversized double halo.
                if !needs_overview_fallback_shadow(window.window_geometry_insets) {
                    continue;
                }
                let rect = window
                    .presentation_transform
                    .map_or(slot.rect, |transform| {
                        (transform.x, transform.y, transform.width, transform.height)
                    });
                push_overview_shadow_layers(
                    &mut layers,
                    rect,
                    sws_protocol::workspace::OVERVIEW_CARD_CORNER_RADIUS,
                    true,
                );
            }
        }
        layers
    }

    fn overview_card_transform_for_window(
        &self,
        window_id: u32,
        apply_drag: bool,
    ) -> Option<PresentationTransform> {
        let root_id = self.top_level_window_id(window_id);
        let workspace_id = self.workspace_manager.workspace_for_window(root_id)?;
        let (_, card) = self
            .overview_card_rects()
            .into_iter()
            .find(|(candidate, _)| *candidate == workspace_id)?;
        let (work_x, work_y, work_width, work_height) =
            self.workarea
                .unwrap_or((0, 0, self.screen_width, self.screen_height));
        let surface = if self.windowing_mode == sws_protocol::WindowingMode::Focused
            && window_id == root_id
        {
            self.tablet_slot_for_window(root_id)
                .and_then(|slot| self.tablet_surface_geometry_for_window(window_id, slot))
        } else {
            None
        }
        .or_else(|| {
            self.window_manager
                .get_window(window_id)
                .map(|window| window.surface_geometry())
        })?;
        let map_offset = |value: i32, origin: i32, source_extent: u32, target_extent: u32| {
            let relative = i64::from(value).saturating_sub(i64::from(origin));
            let scaled =
                relative.saturating_mul(i64::from(target_extent)) / i64::from(source_extent.max(1));
            scaled.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
        };
        let mut x = card
            .0
            .saturating_add(map_offset(surface.0, work_x, work_width, card.2));
        let mut y = card
            .1
            .saturating_add(map_offset(surface.1, work_y, work_height, card.3));
        let width =
            ((u64::from(surface.2) * u64::from(card.2)) / u64::from(work_width.max(1))) as u32;
        let height =
            ((u64::from(surface.3) * u64::from(card.3)) / u64::from(work_height.max(1))) as u32;
        let dragging = apply_drag
            .then_some(self.overview_window_drag)
            .flatten()
            .filter(|drag| drag.window_id == root_id);
        if let Some(drag) = dragging {
            x = x.saturating_add(drag.current_x.saturating_sub(drag.start_x));
            y = y.saturating_add(drag.current_y.saturating_sub(drag.start_y));
        }
        Some(PresentationTransform {
            x,
            y,
            width: width.max(1),
            height: height.max(1),
            opacity: if dragging.is_some() { 0.94 } else { 1.0 },
        })
    }

    fn laptop_overview_spread_slots(&self) -> Vec<OverviewSpreadSlot> {
        if self.tablet_mode
            || self.workspace_manager.presentation()
                != sws_protocol::workspace::ShellPresentation::Overview
        {
            return Vec::new();
        }
        let state = self.workspace_manager.snapshot();
        let Some(workspace) = state
            .workspaces
            .iter()
            .find(|workspace| workspace.id == state.active_workspace)
        else {
            return Vec::new();
        };
        let windows = workspace
            .window_ids
            .iter()
            .filter_map(|window_id| {
                self.window_manager
                    .get_window(*window_id)
                    .and_then(|window| {
                        (window.visible && !window.minimized).then_some((
                            *window_id,
                            window.width.max(1),
                            window.height.max(1),
                        ))
                    })
            })
            .collect::<Vec<_>>();
        let workarea = self
            .workarea
            .unwrap_or((0, 0, self.screen_width, self.screen_height));
        let stage = laptop_overview_window_stage_for(
            workarea,
            self.overview_workspace_region(),
            self.output_scale_milli,
        );
        layout_overview_window_spread(stage, self.output_scale_milli, &windows)
    }

    fn laptop_overview_spread_transform_for_window(
        &self,
        window_id: u32,
    ) -> Option<PresentationTransform> {
        let root_id = self.top_level_window_id(window_id);
        let root = self.window_manager.get_window(root_id)?;
        let window = self.window_manager.get_window(window_id)?;
        let slot = self
            .laptop_overview_spread_slots()
            .into_iter()
            .find(|slot| slot.window_id == root_id)?;
        let root_geometry = root.surface_geometry();
        let geometry = window.surface_geometry();
        let map_offset = |value: i32, origin: i32, source_extent: u32, target_extent: u32| {
            let relative = i64::from(value).saturating_sub(i64::from(origin));
            let scaled =
                relative.saturating_mul(i64::from(target_extent)) / i64::from(source_extent.max(1));
            scaled.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
        };
        let x = slot.rect.0.saturating_add(map_offset(
            geometry.0,
            root_geometry.0,
            root_geometry.2,
            slot.rect.2,
        ));
        let y = slot.rect.1.saturating_add(map_offset(
            geometry.1,
            root_geometry.1,
            root_geometry.3,
            slot.rect.3,
        ));
        let width = ((u64::from(geometry.2) * u64::from(slot.rect.2))
            / u64::from(root_geometry.2.max(1))) as u32;
        let height = ((u64::from(geometry.3) * u64::from(slot.rect.3))
            / u64::from(root_geometry.3.max(1))) as u32;
        let dragging = self
            .overview_window_drag
            .filter(|drag| drag.window_id == root_id);
        let rect = if let Some(drag) = dragging {
            let thumbnail = self.overview_card_transform_for_window(root_id, false)?;
            let workspace_region = self.overview_workspace_region();
            let rail_bottom = workspace_region.1.saturating_add(workspace_region.3 as i32);
            let progress = overview_drag_progress_milli(drag.start_y, drag.current_y, rail_bottom);
            let dragged_root = overview_dragged_root_rect(
                slot.rect,
                (thumbnail.width, thumbnail.height),
                (drag.start_x, drag.start_y),
                (drag.current_x, drag.current_y),
                progress,
            );
            project_overview_rect_between_roots(
                (x, y, width.max(1), height.max(1)),
                slot.rect,
                dragged_root,
            )
        } else {
            (x, y, width.max(1), height.max(1))
        };
        Some(PresentationTransform {
            x: rect.0,
            y: rect.1,
            width: rect.2,
            height: rect.3,
            opacity: if dragging.is_some() { 0.94 } else { 1.0 },
        })
    }

    fn uses_laptop_overview_spread(&self, window_id: u32) -> bool {
        let root_id = self.top_level_window_id(window_id);
        !self.tablet_mode
            && self.workspace_manager.presentation()
                == sws_protocol::workspace::ShellPresentation::Overview
            && self.workspace_manager.workspace_for_window(root_id)
                == Some(self.workspace_manager.active_workspace())
            && !self
                .overview_window_drag
                .is_some_and(|drag| drag.window_id == root_id && drag.from_workspace_thumbnail)
    }

    fn overview_transform_for_window(&self, window_id: u32) -> Option<PresentationTransform> {
        if self.uses_laptop_overview_spread(window_id) {
            self.laptop_overview_spread_transform_for_window(window_id)
        } else {
            self.overview_card_transform_for_window(window_id, true)
        }
    }

    fn overview_thumbnail_instance_for_window(
        &self,
        window_id: u32,
    ) -> Option<PresentationInstance> {
        if !self.uses_laptop_overview_spread(window_id) {
            return None;
        }
        let root_id = self.top_level_window_id(window_id);
        let workspace_id = self.workspace_manager.workspace_for_window(root_id)?;
        let clip = self
            .overview_card_rects()
            .into_iter()
            .find_map(|(candidate, rect)| (candidate == workspace_id).then_some(rect))?;
        Some(PresentationInstance {
            transform: self.overview_card_transform_for_window(window_id, false)?,
            clip: Some(clip),
            clip_radius: sws_protocol::workspace::OVERVIEW_CARD_CORNER_RADIUS,
        })
    }

    fn update_overview_transforms(&mut self) -> bool {
        let shell_presentation = matches!(
            self.workspace_manager.presentation(),
            sws_protocol::workspace::ShellPresentation::Home
                | sws_protocol::workspace::ShellPresentation::Overview
        );
        let cards = shell_presentation
            .then(|| self.overview_card_rects())
            .unwrap_or_default();
        let transforms = self
            .window_manager
            .get_windows()
            .iter()
            .map(|window| {
                let root_id = self.top_level_window_id(window.id);
                let workspace_id = self.workspace_manager.workspace_for_window(root_id);
                let transform = if shell_presentation
                    && workspace_id.is_some()
                    && self.desired_workspace_visibility(window.id)
                {
                    self.overview_transform_for_window(window.id)
                } else {
                    None
                };
                let dragging = self
                    .overview_window_drag
                    .is_some_and(|drag| drag.window_id == root_id);
                let clip = (!dragging)
                    .then_some(transform)
                    .flatten()
                    .and_then(|transform| {
                        if self.uses_laptop_overview_spread(window.id) {
                            Some((transform.x, transform.y, transform.width, transform.height))
                        } else {
                            workspace_id.and_then(|workspace_id| {
                                cards.iter().find_map(|(candidate, rect)| {
                                    (*candidate == workspace_id).then_some(*rect)
                                })
                            })
                        }
                    });
                let clip_radius = if clip.is_some() {
                    sws_protocol::workspace::OVERVIEW_CARD_CORNER_RADIUS
                } else {
                    0
                };
                let instances = self
                    .overview_thumbnail_instance_for_window(window.id)
                    .into_iter()
                    .collect::<Vec<_>>();
                (window.id, transform, instances, clip, clip_radius)
            })
            .collect::<Vec<_>>();
        let mut changed = false;
        for (window_id, transform, instances, clip, clip_radius) in transforms {
            if let Some(window) = self.window_manager.get_window_mut(window_id)
                && (window.presentation_transform != transform
                    || window.presentation_instances != instances
                    || window.presentation_clip != clip
                    || window.presentation_clip_radius != clip_radius)
            {
                window.presentation_transform = transform;
                window.presentation_instances = instances;
                window.presentation_clip = clip;
                window.presentation_clip_radius = clip_radius;
                changed = true;
            }
        }
        changed
    }

    fn sync_shell_presentation_focus(&mut self) -> bool {
        let shell_presentation = matches!(
            self.workspace_manager.presentation(),
            sws_protocol::workspace::ShellPresentation::Home
                | sws_protocol::workspace::ShellPresentation::Overview
        );
        let current_focus = self.window_manager.get_focused_window_id();

        if shell_presentation {
            if self.overview_restore_focus.is_none() {
                self.overview_restore_focus = current_focus
                    .filter(|window_id| {
                        let root_id = self.top_level_window_id(*window_id);
                        self.window_manager
                            .get_window(*window_id)
                            .is_some_and(|window| window.window_type == WindowType::Normal)
                            && self
                                .workspace_manager
                                .workspace_for_window(root_id)
                                .is_some()
                    })
                    .or(self.last_workspace_focus);
            }
            let shell_focus = self
                .window_manager
                .get_windows()
                .iter()
                .rev()
                .find(|window| {
                    window.window_type == WindowType::ShellBackground && window.is_presented()
                })
                .map(|window| window.id);
            let Some(shell_focus) = shell_focus else {
                return false;
            };
            if current_focus == Some(shell_focus) {
                return false;
            }
            if let Some(previous) = current_focus {
                self.damage_compositor_focus_style(previous);
            }
            self.window_manager.set_focus(shell_focus);
            self.broadcast_focus_change(shell_focus);
            self.damage_compositor_focus_style(shell_focus);
            return true;
        }

        let preferred = self.overview_restore_focus.take();
        let restore_focus = preferred
            .into_iter()
            .chain(self.last_workspace_focus)
            .find(|window_id| {
                self.window_manager
                    .get_window(*window_id)
                    .is_some_and(|window| {
                        window.window_type == WindowType::Normal && window.is_presented()
                    })
            })
            .or_else(|| {
                self.window_manager
                    .get_windows()
                    .iter()
                    .rev()
                    .find(|window| {
                        window.window_type == WindowType::Normal && window.is_presented()
                    })
                    .map(|window| window.id)
            });
        let Some(restore_focus) = restore_focus else {
            return false;
        };
        if current_focus == Some(restore_focus) {
            return false;
        }
        if let Some(previous) = current_focus {
            self.damage_compositor_focus_style(previous);
        }
        self.window_manager.set_focus(restore_focus);
        self.broadcast_focus_change(restore_focus);
        self.damage_compositor_focus_style(restore_focus);
        true
    }

    fn apply_workspace_presentation_policy(&mut self) -> bool {
        if self.workspace_manager.presentation()
            != sws_protocol::workspace::ShellPresentation::Overview
        {
            self.overview_add_workspace_selected = false;
        }
        let window_ids = self
            .window_manager
            .get_windows()
            .iter()
            .map(|window| window.id)
            .collect::<Vec<_>>();
        let mut changed = false;
        let mut visibility_changed = Vec::new();

        if self.windowing_mode == sws_protocol::WindowingMode::Freeform {
            for window_id in &window_ids {
                changed |= self.restore_workspace_managed_window(*window_id);
            }
        }

        for window_id in &window_ids {
            let desired = self.desired_workspace_visibility(*window_id);
            if let Some(window) = self.window_manager.get_window_mut(*window_id)
                && window.workspace_visible != desired
            {
                window.workspace_visible = desired;
                visibility_changed.push(*window_id);
                changed = true;
            }
        }

        changed |= self.update_overview_transforms();
        changed |= self.sync_shell_presentation_focus();
        for window_id in visibility_changed {
            self.send_window_state_changed(window_id);
        }

        if self.windowing_mode == sws_protocol::WindowingMode::Focused
            && self.workspace_manager.presentation()
                == sws_protocol::workspace::ShellPresentation::Workspace
        {
            for window_id in window_ids {
                let is_root = self.top_level_window_id(window_id) == window_id;
                if !is_root || !self.desired_workspace_visibility(window_id) {
                    continue;
                }
                if let Some(slot) = self.tablet_slot_for_window(window_id) {
                    changed |= self.apply_tablet_geometry(window_id, slot);
                }
            }
        }

        if changed {
            self.full_redraw_needed = true;
            self.route_pointer_motion_at_cursor();
        }
        changed
    }

    fn apply_focused_policy_to_window(&mut self, _window_id: u32) -> bool {
        self.apply_workspace_presentation_policy()
    }

    fn apply_windowing_mode_policy(&mut self) -> bool {
        self.apply_workspace_presentation_policy()
    }

    fn prune_expired_activation_tokens(&mut self, now_ns: u64) {
        self.activation_tokens.retain(|record| {
            now_ns.saturating_sub(record.created_at_ns) <= ACTIVATION_TOKEN_TTL_NS
        });
    }

    fn issue_activation_token(
        &mut self,
        source_window_id: u32,
        target_app_id: Vec<u8>,
    ) -> Option<String> {
        if target_app_id.is_empty()
            || self.window_manager.get_focused_window_id() != Some(source_window_id)
        {
            return None;
        }

        let source_app_id = {
            let source = self.window_manager.get_window(source_window_id)?;
            if !source.is_presented() {
                return None;
            }
            source.app_id.clone().unwrap_or_default()
        };

        let now_ns = monotonic_time_ns();
        self.prune_expired_activation_tokens(now_ns);
        if self.activation_tokens.len() >= MAX_PENDING_ACTIVATION_TOKENS {
            self.activation_tokens.remove(0);
        }

        let serial = self.next_activation_token_serial;
        self.next_activation_token_serial = self.next_activation_token_serial.wrapping_add(1);
        if self.next_activation_token_serial == 0 {
            self.next_activation_token_serial = 1;
        }
        let token = std::format!("sws-{:016x}-{:016x}", now_ns, serial);
        self.activation_tokens.push(ActivationRecord {
            token: token.clone(),
            source_window_id,
            source_app_id,
            target_app_id,
            created_at_ns: now_ns,
        });
        Some(token)
    }

    fn consume_activation_token(
        &mut self,
        token: Option<&[u8]>,
        app_id: &[u8],
        window_type: u32,
        requested_placement: sws_protocol::WindowPlacement,
    ) -> (sws_protocol::WindowPlacement, bool) {
        if window_type != sws_protocol::window_types::NORMAL {
            return (requested_placement, false);
        }
        let Some(token) = token else {
            return (requested_placement, false);
        };

        self.prune_expired_activation_tokens(monotonic_time_ns());
        let Some(index) = self
            .activation_tokens
            .iter()
            .position(|record| record.token.as_bytes() == token)
        else {
            return (requested_placement, false);
        };
        let record = self.activation_tokens.remove(index);
        if record.target_app_id != app_id {
            return (requested_placement, false);
        }

        sws_debug!(
            "[Compositor] Consumed activation from window #{} ({}) for {}",
            record.source_window_id,
            String::from_utf8_lossy(&record.source_app_id),
            String::from_utf8_lossy(app_id)
        );
        let placement = if matches!(requested_placement, sws_protocol::WindowPlacement::Default) {
            sws_protocol::WindowPlacement::Centered
        } else {
            requested_placement
        };
        (placement, true)
    }

    /// Choose a safe compositor-managed position for a regular window.
    ///
    /// A newly created window may arrive before the shell has advertised a
    /// workarea, and launching from shell UI temporarily moves focus away from
    /// the previous application. Prefer the focused normal window as the
    /// cascade anchor, then the topmost visible normal window. The first window
    /// uses a small workarea inset. Centering is reserved for an explicit
    /// placement or a validated activation token.
    fn default_window_position(&self, width: u32, height: u32) -> (i32, i32) {
        let focused_anchor = self
            .window_manager
            .get_focused_window_id()
            .and_then(|id| self.window_manager.get_window(id))
            .filter(|window| {
                matches!(window.window_type, super::window::WindowType::Normal)
                    && window.is_presented()
            });
        let anchor = focused_anchor.or_else(|| {
            self.window_manager
                .get_windows()
                .iter()
                .rev()
                .find(|window| {
                    matches!(window.window_type, super::window::WindowType::Normal)
                        && window.is_presented()
                })
        });
        let anchor_position = anchor.map(|window| {
            let (x, y, _, _) = window.window_geometry();
            (x, y)
        });

        let (bounds_x, bounds_y, bounds_width, bounds_height) =
            self.workarea
                .unwrap_or((0, 0, self.screen_width, self.screen_height));
        let min_x = bounds_x as i64;
        let min_y = bounds_y as i64;
        let max_x = (min_x + bounds_width as i64 - width as i64).max(min_x);
        let max_y = (min_y + bounds_height as i64 - height as i64).max(min_y);
        let initial_x = (min_x + DEFAULT_WINDOW_INSET).min(max_x);
        let initial_y = (min_y + DEFAULT_WINDOW_INSET).min(max_y);

        let (mut desired_x, mut desired_y) = match anchor_position {
            Some((x, y)) => (
                x as i64 + DEFAULT_WINDOW_CASCADE,
                y as i64 + DEFAULT_WINDOW_CASCADE,
            ),
            None => (initial_x, initial_y),
        };
        if desired_x > max_x {
            desired_x = initial_x;
        }
        if desired_y > max_y {
            desired_y = initial_y;
        }

        (
            desired_x
                .clamp(min_x, max_x)
                .clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            desired_y
                .clamp(min_y, max_y)
                .clamp(i32::MIN as i64, i32::MAX as i64) as i32,
        )
    }

    fn centered_window_position(&self, width: u32, height: u32) -> (i32, i32) {
        let (work_x, work_y, work_width, work_height) =
            self.workarea
                .unwrap_or((0, 0, self.screen_width, self.screen_height));
        (
            work_x + (work_width as i32 - width as i32).max(0) / 2,
            work_y + (work_height as i32 - height as i32).max(0) / 2,
        )
    }

    fn publish_window_creation_environment(&self) {
        super::ipc::set_window_creation_environment(
            self.screen_width,
            self.screen_height,
            self.workarea,
            self.windowing_mode,
        );
    }

    fn handle_ipc_event(&mut self, event: IpcEvent) -> Result<bool, &'static str> {
        match event {
            IpcEvent::CreateWindow {
                client_id,
                app_id,
                window_id,
                requested_width,
                requested_height,
                width,
                height,
                window_type,
                resizable,
                mut focus_on_create,
                mut active_on_focus,
                initial_position,
                activation_token,
                initial_configuration,
                shm,
                shm_mapped_addr,
                shm_size,
            } => {
                println!(
                    "[Compositor] Client {} creating window #{} ({}x{}, type={})",
                    client_id, window_id, width, height, window_type
                );

                use sws_protocol::window_types;

                let (initial_position, activated) = self.consume_activation_token(
                    activation_token.as_deref(),
                    &app_id,
                    window_type,
                    initial_position,
                );
                if activated {
                    focus_on_create = true;
                    active_on_focus = true;
                }
                let center_on_first_geometry =
                    matches!(initial_position, sws_protocol::WindowPlacement::Centered);

                // Calculate initial position based on window type
                let (x, y) = match initial_position {
                    sws_protocol::WindowPlacement::Absolute { x, y } => {
                        println!(
                            "[Compositor] Using requested position for window #{}: ({}, {})",
                            window_id, x, y
                        );
                        (x, y)
                    }
                    sws_protocol::WindowPlacement::Centered => {
                        let (x, y) = self.centered_window_position(width, height);
                        println!(
                            "[Compositor] Centering window #{} in workarea at ({}, {})",
                            window_id, x, y
                        );
                        (x, y)
                    }
                    sws_protocol::WindowPlacement::Default => {
                        match window_type {
                            window_types::NORMAL | window_types::ALWAYS_ON_TOP => {
                                let position = self.default_window_position(width, height);
                                println!(
                                    "[Compositor] Default placement for window #{}: ({}, {})",
                                    window_id, position.0, position.1
                                );
                                position
                            }
                            // Shell-owned surfaces intentionally use the output origin.
                            window_types::TASKBAR
                            | window_types::DESKTOP
                            | window_types::SHELL_BACKGROUND
                            | window_types::SHELL_CHROME => {
                                println!("[Compositor] Positioning shell window at output origin");
                                (0, 0)
                            }
                            // IME popups are positioned later from the text-input cursor.
                            _ => (0, 0),
                        }
                    }
                };

                let wtype = match window_type {
                    window_types::NORMAL => super::window::WindowType::Normal,
                    window_types::ALWAYS_ON_TOP => super::window::WindowType::AlwaysOnTop,
                    window_types::TASKBAR => super::window::WindowType::Taskbar,
                    window_types::DESKTOP => super::window::WindowType::Desktop,
                    window_types::IME_POPUP => super::window::WindowType::ImePopup,
                    window_types::SHELL_BACKGROUND => super::window::WindowType::ShellBackground,
                    window_types::SHELL_CHROME => super::window::WindowType::ShellChrome,
                    _ => super::window::WindowType::Normal,
                };

                // Check if SHM was provided (modern path)
                if let Some(shm_obj) = shm {
                    println!(
                        "[Compositor] Window #{} uses SHM at 0x{:x?}",
                        window_id, shm_mapped_addr
                    );

                    // Create window with SHM ownership
                    match self.window_manager.create_window_with_shm_from_event(
                        window_id,
                        x,
                        y,
                        width,
                        height,
                        shm_obj,
                        shm_mapped_addr,
                        shm_size,
                    ) {
                        Ok(_) => {
                            println!("[Compositor] Window #{} with SHM created", window_id);
                        }
                        Err(e) => {
                            println!("[Compositor] Failed to create SHM window: {}", e);
                        }
                    }
                } else {
                    // Fallback: legacy Vec-backed window (for test windows)
                    println!("[Compositor] Window #{} uses legacy Vec buffer", window_id);
                    self.window_manager
                        .create_window_with_id(window_id, x, y, width, height);
                }

                if let Some(window) = self.window_manager.get_window_mut(window_id) {
                    window.app_id = Some(app_id);
                    window.owner_client_id = Some(client_id);
                    window.center_on_first_geometry = center_on_first_geometry;
                }
                if self.window_manager.set_window_type(window_id, wtype) {
                    println!("[Compositor] Set window #{} type to {:?}", window_id, wtype);
                }
                self.window_manager
                    .set_window_resizable(window_id, resizable);
                if let Some(window) = self.window_manager.get_window_mut(window_id) {
                    window.active_on_focus = active_on_focus;
                }

                if self.register_new_workspace_scene(window_id) {
                    self.publish_workspace_state();
                }

                let mut negotiated_restore_geometry = None;
                if let Some(configuration) = initial_configuration {
                    let limits = configuration.size_limits;
                    self.window_manager.set_window_size_limits(
                        window_id,
                        limits.min_width,
                        limits.min_height,
                        limits.max_width,
                        limits.max_height,
                    );
                    let insets = configuration.geometry_insets;
                    let managed_width = width.saturating_sub(insets.horizontal()).max(1);
                    let managed_height = height.saturating_sub(insets.vertical()).max(1);
                    if let Err(error) = self.window_manager.set_window_geometry(
                        window_id,
                        insets.left as i32,
                        insets.top as i32,
                        managed_width,
                        managed_height,
                    ) {
                        println!(
                            "[Compositor] Invalid negotiated geometry for window #{}: {}",
                            window_id, error
                        );
                    }
                    if center_on_first_geometry {
                        let (x, y) = self.centered_window_position(managed_width, managed_height);
                        self.window_manager.set_window_position(window_id, x, y);
                    }
                    let (restore_width, restore_height) =
                        limits.clamp(requested_width, requested_height);
                    if let Some(window) = self.window_manager.get_window_mut(window_id) {
                        window.initial_size_negotiated = true;
                        window.center_on_first_geometry = false;
                        negotiated_restore_geometry =
                            Some((window.x, window.y, restore_width, restore_height));
                    }
                }

                if self.windowing_mode == sws_protocol::WindowingMode::Focused {
                    self.apply_focused_policy_to_window(window_id);
                    if let Some(restore_geometry) = negotiated_restore_geometry
                        && let Some(window) = self.window_manager.get_window_mut(window_id)
                        && window.workspace_layout_managed
                    {
                        window.workspace_restore_geometry = Some(restore_geometry);
                    }
                }

                // Focus is for input routing only; give focus to newly created windows.
                if focus_on_create && self.window_manager.window_accepts_focus(window_id) {
                    self.window_manager.set_focus(window_id);
                    self.broadcast_focus_change(window_id);
                }

                // Auto-configure DESKTOP and TASKBAR windows to match screen dimensions.
                // The compositor is the authoritative source for screen size, so it enforces
                // the correct dimensions even if the client requested a different size.
                match wtype {
                    super::window::WindowType::Desktop
                    | super::window::WindowType::ShellBackground
                    | super::window::WindowType::ShellChrome => {
                        let needs_resize =
                            width != self.screen_width || height != self.screen_height;
                        if needs_resize {
                            println!(
                                "[Compositor] Auto-resizing DESKTOP window #{} from {}x{} to {}x{}",
                                window_id, width, height, self.screen_width, self.screen_height
                            );
                            self.window_manager.resize_window_in_place(
                                window_id,
                                self.screen_width,
                                self.screen_height,
                            );
                            let payload = sws_protocol::payload_window_configure(
                                window_id,
                                self.screen_width,
                                self.screen_height,
                            );
                            super::ipc::send_message_to_window(
                                window_id,
                                sws_protocol::server_msg::WINDOW_CONFIGURE,
                                payload.to_vec(),
                            );
                        }
                    }
                    super::window::WindowType::Taskbar => {
                        if width != self.screen_width {
                            println!(
                                "[Compositor] Auto-resizing TASKBAR window #{} width from {} to {}",
                                window_id, width, self.screen_width
                            );
                            self.window_manager.resize_window_in_place(
                                window_id,
                                self.screen_width,
                                height,
                            );
                            let payload = sws_protocol::payload_window_configure(
                                window_id,
                                self.screen_width,
                                height,
                            );
                            super::ipc::send_message_to_window(
                                window_id,
                                sws_protocol::server_msg::WINDOW_CONFIGURE,
                                payload.to_vec(),
                            );
                        }

                        let workarea_y = height as i32;
                        let workarea_height = self.screen_height.saturating_sub(height);
                        self.workarea = Some((0, workarea_y, self.screen_width, workarea_height));
                        self.window_manager.set_workarea(
                            0,
                            workarea_y,
                            self.screen_width,
                            workarea_height,
                        );
                        self.publish_window_creation_environment();
                        println!(
                            "[Compositor] Updated workarea for taskbar #{}: y={}, height={}",
                            window_id, workarea_y, workarea_height
                        );
                    }
                    _ => {}
                }

                if matches!(
                    wtype,
                    super::window::WindowType::ShellBackground
                        | super::window::WindowType::ShellChrome
                ) {
                    // Shell scenes may be created after the presentation was
                    // already selected. Apply visibility and focus
                    // immediately instead of waiting for another workspace
                    // transaction to make the new surface participate.
                    self.apply_workspace_presentation_policy();
                }

                // Don't trigger redraw yet - wait for client to draw and send UPDATE_BUFFER
                // self.full_redraw_needed = true;

                self.dump_memory_layout("after IPC CreateWindow");
            }
            IpcEvent::DestroyWindow {
                client_id,
                window_id,
            } => {
                println!(
                    "[Compositor] Client {} destroying window #{}",
                    client_id, window_id
                );

                if self.close_client_windows(client_id, &[window_id], true)? {
                    self.dump_memory_layout("after IPC DestroyWindow");
                    return Ok(true);
                }
            }
            IpcEvent::ClientDisconnected {
                client_id,
                window_ids,
            } => {
                println!(
                    "[Compositor] Client {} disconnected; removing {} windows",
                    client_id,
                    window_ids.len()
                );

                if self.close_client_windows(client_id, &window_ids, false)? {
                    self.dump_memory_layout("after IPC ClientDisconnected");
                    return Ok(true);
                }
            }
            IpcEvent::RequestFrame {
                client_id,
                window_id,
                callback_id,
            } => {
                self.queue_frame_callback(client_id, window_id, callback_id);
            }
            IpcEvent::BufferUpdated {
                window_id,
                damage_x,
                damage_y,
                damage_width,
                damage_height,
            } => {
                if damage_width == 0 || damage_height == 0 {
                    // Ignore empty damage to avoid pointless redraws and potential edge-case bugs.
                    println!(
                        "[Compositor] Window #{} buffer updated with empty damage: ({},{}) {}x{} (ignored)",
                        window_id, damage_x, damage_y, damage_width, damage_height
                    );
                    return Ok(false);
                }

                let (win_x, win_y, presented, presentation_transform, presentation_instances) =
                    match self.window_manager.get_window(window_id) {
                        Some(w) => (
                            w.x,
                            w.y,
                            w.is_presented(),
                            w.presentation_transform,
                            w.presentation_instances
                                .iter()
                                .map(|instance| instance.transform)
                                .collect::<Vec<_>>(),
                        ),
                        None => {
                            println!(
                                "[Compositor] Window #{} buffer updated but window not found (ignored)",
                                window_id
                            );
                            return Ok(false);
                        }
                    };
                if let Some(gpu_compositor) = self.gpu_compositor.as_mut() {
                    gpu_compositor.mark_window_damage(
                        window_id,
                        damage_x,
                        damage_y,
                        damage_width,
                        damage_height,
                    );
                }
                self.note_window_frame_submitted(window_id);
                if !presented {
                    return Ok(false);
                }

                // Convert window-local damage -> screen-space rect and clamp to screen.
                let rx0 = win_x.saturating_add(damage_x);
                let ry0 = win_y.saturating_add(damage_y);
                let rx1 = rx0.saturating_add(damage_width as i32);
                let ry1 = ry0.saturating_add(damage_height as i32);

                let sx0 = rx0.max(0).min(self.screen_width as i32);
                let sy0 = ry0.max(0).min(self.screen_height as i32);
                let sx1 = rx1.max(0).min(self.screen_width as i32);
                let sy1 = ry1.max(0).min(self.screen_height as i32);
                let w = (sx1 - sx0).max(0) as u32;
                let h = (sy1 - sy0).max(0) as u32;
                if (w == 0 || h == 0)
                    && presentation_transform.is_none()
                    && presentation_instances.is_empty()
                {
                    println!(
                        "[Compositor] Window #{} buffer updated but damage out of bounds: ({},{}) {}x{} (ignored)",
                        window_id, damage_x, damage_y, damage_width, damage_height
                    );
                    return Ok(false);
                }

                // println!(
                //     "[Compositor] Window #{} buffer updated: ({},{}) {}x{} -> screen ({},{}) {}x{}",
                //     window_id, damage_x, damage_y, damage_width, damage_height, sx0, sy0, w, h
                // );

                if let Some(transform) = presentation_transform {
                    self.add_pending_damage((
                        transform.x,
                        transform.y,
                        transform.width,
                        transform.height,
                    ));
                } else {
                    self.add_pending_damage((sx0, sy0, w, h));
                }
                for transform in presentation_instances {
                    self.add_pending_damage((
                        transform.x,
                        transform.y,
                        transform.width,
                        transform.height,
                    ));
                }
            }
            IpcEvent::RegisterSgfxBuffer {
                client_id,
                request_id,
                window_id,
                buffer_id,
                generation,
                compositor_epoch,
                width,
                height,
                handle,
            } => {
                if !self.client_owns_window(client_id, window_id) {
                    send_sgfx_protocol_error(
                        client_id,
                        request_id,
                        sws_protocol::error_codes::WINDOW_NOT_OWNED,
                    );
                    return Ok(false);
                }
                let identity = SgfxBufferIdentity {
                    window_id,
                    buffer_id,
                    generation,
                    compositor_epoch,
                };
                let extent_matches = self
                    .window_manager
                    .get_window(window_id)
                    .is_some_and(|window| window.backing_extent() == (width, height));
                let result = if !extent_matches {
                    Err(SgfxBufferError::InvalidBuffer)
                } else {
                    match self.gpu_compositor.as_mut() {
                        Some(gpu) => gpu.register_shared_buffer(identity, width, height, handle),
                        None => Err(SgfxBufferError::Unavailable),
                    }
                };
                match result {
                    Ok(()) => {
                        let payload = sws_protocol::payload_sgfx_buffer_identity(
                            window_id,
                            buffer_id,
                            generation,
                            compositor_epoch,
                        );
                        super::ipc::send_response_to_client(
                            client_id,
                            sws_protocol::server_msg::SGFX_BUFFER_REGISTERED,
                            request_id,
                            payload.to_vec(),
                        );
                    }
                    Err(error) => {
                        println!(
                            "[Compositor] Failed to register shared SGFX buffer for window {}: {:?}",
                            window_id, error
                        );
                        let code = sgfx_error_code(error);
                        super::ipc::send_response_to_client(
                            client_id,
                            sws_protocol::server_msg::ERROR,
                            request_id,
                            sws_protocol::payload_error(code).to_vec(),
                        );
                        if error == SgfxBufferError::Unavailable {
                            self.disable_gpu_after_runtime_failure(
                                "SWS_BACKEND=sgfx shared-buffer registration failed",
                            )?;
                        }
                    }
                }
            }
            IpcEvent::CommitSgfxFrame {
                client_id,
                window_id,
                buffer_id,
                generation,
                compositor_epoch,
                commit_serial,
                damage_rects,
            } => {
                let identity = SgfxBufferIdentity {
                    window_id,
                    buffer_id,
                    generation,
                    compositor_epoch,
                };
                if !self.client_owns_window(client_id, window_id) {
                    send_sgfx_frame_rejected(
                        client_id,
                        identity,
                        commit_serial,
                        sws_protocol::error_codes::WINDOW_NOT_OWNED,
                    );
                    return Ok(false);
                }
                let result = match self.gpu_compositor.as_mut() {
                    Some(gpu) => gpu.commit_shared_buffer(identity, commit_serial, &damage_rects),
                    None => Err(SgfxBufferError::Unavailable),
                };
                match result {
                    Ok(damage) => {
                        if let Some((window_x, window_y, presented, transform, instances)) =
                            self.window_manager.get_window(window_id).map(|window| {
                                (
                                    window.x,
                                    window.y,
                                    window.is_presented(),
                                    window.presentation_transform,
                                    window
                                        .presentation_instances
                                        .iter()
                                        .map(|instance| instance.transform)
                                        .collect::<Vec<_>>(),
                                )
                            })
                            && presented
                        {
                            if let Some(transform) = transform {
                                self.add_pending_damage((
                                    transform.x,
                                    transform.y,
                                    transform.width,
                                    transform.height,
                                ));
                            } else {
                                for (x, y, width, height) in &damage {
                                    self.add_pending_damage((
                                        window_x.saturating_add(*x as i32),
                                        window_y.saturating_add(*y as i32),
                                        *width,
                                        *height,
                                    ));
                                }
                            }
                            for instance in instances {
                                self.add_pending_damage((
                                    instance.x,
                                    instance.y,
                                    instance.width,
                                    instance.height,
                                ));
                            }
                        }
                        self.note_window_frame_submitted(window_id);
                    }
                    Err(error) => {
                        println!(
                            "[Compositor] Failed to commit shared SGFX buffer for window {}: {:?}",
                            window_id, error
                        );
                        send_sgfx_frame_rejected(
                            client_id,
                            identity,
                            commit_serial,
                            sgfx_error_code(error),
                        );
                        if error == SgfxBufferError::Unavailable {
                            self.disable_gpu_after_runtime_failure(
                                "SWS_BACKEND=sgfx shared-buffer commit failed",
                            )?;
                        }
                    }
                }
            }
            IpcEvent::DestroySgfxBuffer {
                client_id,
                request_id,
                window_id,
                buffer_id,
                generation,
                compositor_epoch,
            } => {
                if !self.client_owns_window(client_id, window_id) {
                    send_sgfx_protocol_error(
                        client_id,
                        request_id,
                        sws_protocol::error_codes::WINDOW_NOT_OWNED,
                    );
                    return Ok(false);
                }
                let identity = SgfxBufferIdentity {
                    window_id,
                    buffer_id,
                    generation,
                    compositor_epoch,
                };
                let result = match self.gpu_compositor.as_mut() {
                    Some(gpu) => gpu.destroy_shared_buffer(identity),
                    None => Err(SgfxBufferError::Unavailable),
                };
                let backend_failed = result == Err(SgfxBufferError::Unavailable);
                let (msg_type, payload) = match result {
                    Ok(()) => (
                        sws_protocol::server_msg::SGFX_BUFFER_DESTROYED,
                        sws_protocol::payload_sgfx_buffer_identity(
                            window_id,
                            buffer_id,
                            generation,
                            compositor_epoch,
                        )
                        .to_vec(),
                    ),
                    Err(error) => (
                        sws_protocol::server_msg::ERROR,
                        sws_protocol::payload_error(sgfx_error_code(error)).to_vec(),
                    ),
                };
                super::ipc::send_response_to_client(client_id, msg_type, request_id, payload);
                if backend_failed {
                    self.disable_gpu_after_runtime_failure(
                        "SWS_BACKEND=sgfx shared-buffer destruction failed",
                    )?;
                }
            }
            IpcEvent::RequestMove { window_id } => {
                sws_debug!("[Compositor] Window #{} requested move", window_id);
                if self.window_manager.is_fullscreen(window_id)
                    || self
                        .window_manager
                        .get_window(window_id)
                        .is_some_and(|window| window.focused_mode_managed)
                {
                    sws_debug!(
                        "[Compositor] Ignoring move request for fullscreen window #{}",
                        window_id
                    );
                    return Ok(false);
                }
                let direct_grab_position = self
                    .direct_touch_grabs
                    .iter()
                    .find(|grab| grab.window_id == window_id && grab.legacy_primary)
                    .map(|grab| (grab.screen_x, grab.screen_y));
                let move_grab_origin = interactive_move_grab_origin(
                    self.left_button_down,
                    direct_grab_position,
                    self.last_left_down_cursor,
                    (self.cursor.x, self.cursor.y),
                );
                sws_debug!(
                    "[Compositor] RequestMove state: left_down={} direct_grab={:?} last_left_down={:?} cursor=({}, {})",
                    self.left_button_down,
                    direct_grab_position,
                    self.last_left_down_cursor,
                    self.cursor.x,
                    self.cursor.y
                );
                let Some((grab_cursor_x, grab_cursor_y)) = move_grab_origin else {
                    sws_debug!(
                        "[Compositor] Ignoring move request for window #{} (no mouse or touch grab)",
                        window_id
                    );
                    return Ok(false);
                };

                let (start_window_x, start_window_y) =
                    match self.window_manager.get_window(window_id) {
                        Some(w) => {
                            let (x, y, _, _) = w.window_geometry();
                            (x, y)
                        }
                        None => return Ok(false),
                    };

                sws_debug!(
                    "[Compositor] Move start: window #{} grab=({}, {}) cursor=({}, {})",
                    window_id,
                    grab_cursor_x,
                    grab_cursor_y,
                    self.cursor.x,
                    self.cursor.y
                );

                // Bring the window to front for the drag (focus is handled by click routing).
                self.raise_window_with_damage(window_id);

                self.move_drag = Some(MoveDragState {
                    window_id,
                    grab_cursor_x,
                    grab_cursor_y,
                    start_window_x,
                    start_window_y,
                });
                if direct_grab_position.is_some()
                    && let Some(grab) = self
                        .direct_touch_grabs
                        .iter_mut()
                        .find(|grab| grab.window_id == window_id && grab.legacy_primary)
                {
                    grab.driving_move_drag = true;
                }
                self.refresh_cursor_icon();
            }
            IpcEvent::MoveWindow { window_id, x, y } => {
                if self.window_manager.is_fullscreen(window_id)
                    || self
                        .window_manager
                        .get_window(window_id)
                        .is_some_and(|window| window.focused_mode_managed)
                {
                    println!(
                        "[Compositor] Ignoring explicit move for fullscreen window #{}",
                        window_id
                    );
                    return Ok(false);
                }
                println!(
                    "[Compositor] Moving window #{} to ({}, {})",
                    window_id, x, y
                );
                self.set_window_position_with_damage(window_id, x, y);
            }
            IpcEvent::SetWindowParent {
                window_id,
                parent_id,
            } => {
                let parent = if parent_id == 0 {
                    None
                } else {
                    Some(parent_id)
                };
                println!(
                    "[Compositor] Setting parent of window #{} to {:?}",
                    window_id, parent
                );

                if self.window_manager.set_window_parent(window_id, parent) {
                    let workspace_changed = if parent.is_some() {
                        self.discard_pending_workspace_scene(window_id);
                        self.workspace_manager.remove_window(window_id)
                    } else if self.is_workspace_scene_root(window_id)
                        && self
                            .workspace_manager
                            .workspace_for_window(window_id)
                            .is_none()
                    {
                        self.discard_pending_workspace_scene(window_id);
                        self.workspace_manager.add_scene_root(
                            window_id,
                            self.tablet_mode,
                            self.windowing_mode == sws_protocol::WindowingMode::Focused,
                        );
                        true
                    } else {
                        self.discard_pending_workspace_scene(window_id);
                        false
                    };
                    self.apply_workspace_presentation_policy();
                    if workspace_changed {
                        self.publish_workspace_state();
                    }
                    // Keep transient children above their parent by raising the group.
                    self.window_manager.raise_to_top_with_type(window_id);
                    self.full_redraw_needed = true;
                }
            }
            IpcEvent::SetWindowTransientFlags { window_id, flags } => {
                println!(
                    "[Compositor] Setting transient flags of window #{} to 0x{:x}",
                    window_id, flags
                );
                if self
                    .window_manager
                    .set_window_transient_flags(window_id, flags)
                {
                    // If raise policy is enabled, re-raise the group.
                    if (flags & sws_protocol::transient_flags::RAISE_WITH_PARENT) != 0 {
                        self.window_manager.raise_to_top_with_type(window_id);
                    }
                    self.full_redraw_needed = true;
                }
            }
            IpcEvent::SetWindowSizeLimits {
                window_id,
                min_width,
                min_height,
                max_width,
                max_height,
            } => {
                println!(
                    "[Compositor] Setting size limits of window #{} to min={}x{} max={}x{}",
                    window_id, min_width, min_height, max_width, max_height
                );
                if self
                    .window_manager
                    .set_window_size_limits(window_id, min_width, min_height, max_width, max_height)
                {
                    if self.windowing_mode == sws_protocol::WindowingMode::Focused {
                        self.apply_focused_policy_to_window(window_id);
                    }
                    self.full_redraw_needed = true;
                }
            }
            IpcEvent::ResizeWindow {
                window_id,
                width,
                height,
                shm,
                shm_mapped_addr,
                shm_size,
            } => {
                println!(
                    "[Compositor] Resizing window #{} to {}x{} (shm_mapped=0x{:x?})",
                    window_id, width, height, shm_mapped_addr
                );
                if let Some(window) = self.window_manager.get_window(window_id)
                    && window.fullscreen
                    && (width != window.width || height != window.height)
                {
                    println!(
                        "[Compositor] Rejecting {}x{} backing for fullscreen window #{}; expected {}x{}",
                        width, height, window_id, window.width, window.height
                    );
                    if let Some(address) = shm_mapped_addr
                        && shm_size != 0
                    {
                        let _ = munmap(address, shm_size);
                    }
                    let payload = sws_protocol::payload_window_configure(
                        window_id,
                        window.width,
                        window.height,
                    );
                    super::ipc::send_message_to_window(
                        window_id,
                        sws_protocol::server_msg::WINDOW_CONFIGURE,
                        payload.to_vec(),
                    );
                    return Ok(false);
                }
                let old_rect = self
                    .window_manager
                    .get_window(window_id)
                    .map(|w| (w.x, w.y, w.width, w.height));
                if let Some(shm) = shm {
                    self.release_gpu_window_texture(window_id)?;
                    if self.window_manager.resize_window_with_shm(
                        window_id,
                        width,
                        height,
                        shm,
                        shm_mapped_addr,
                        shm_size,
                    ) {
                        // Keep the last presented shared frame alive while the
                        // client renders the replacement extent. The normal
                        // shared-frame promotion path releases it only after a
                        // new generation has reached the display, so the empty
                        // resized SHM backing is never exposed between frames.
                        if let Some(w) = self.window_manager.get_window(window_id) {
                            let rect = (w.x, w.y, w.width, w.height);
                            if let Some(old_rect) = old_rect {
                                self.add_resize_replacement_damage(old_rect, rect);
                            }
                        }
                    }
                }
            }
            IpcEvent::MinimizeWindow { window_id } => {
                println!("[Compositor] Minimizing window #{}", window_id);
                if let Some(window) = self.window_manager.get_window_mut(window_id) {
                    window.pending_maximize = false;
                }
                if self
                    .pointer_lock
                    .is_some_and(|state| state.window_id == window_id)
                {
                    self.release_pointer_lock();
                }
                let old_rect = self
                    .window_manager
                    .get_window(window_id)
                    .map(|w| (w.x, w.y, w.width, w.height));
                let left_fullscreen = self.window_manager.is_fullscreen(window_id);
                if left_fullscreen {
                    self.window_manager.unset_fullscreen_window(window_id);
                }
                if self.window_manager.minimize_window(window_id) {
                    if let Some(r) = old_rect {
                        self.add_pending_damage(r);
                    }
                    self.send_window_state_changed(window_id);
                    if left_fullscreen
                        && let Some((width, height)) = self
                            .window_manager
                            .get_window(window_id)
                            .map(|window| (window.width, window.height))
                    {
                        let payload =
                            sws_protocol::payload_window_configure(window_id, width, height);
                        super::ipc::send_message_to_window(
                            window_id,
                            sws_protocol::server_msg::WINDOW_CONFIGURE,
                            payload.to_vec(),
                        );
                    }
                    self.full_redraw_needed = true;
                }
            }
            IpcEvent::MaximizeWindow { window_id } => {
                println!("[Compositor] Maximizing window #{}", window_id);
                let ready = self
                    .window_manager
                    .get_window(window_id)
                    .is_some_and(|window| window.has_presented_frame);
                if !ready {
                    if let Some(window) = self.window_manager.get_window_mut(window_id) {
                        window.pending_maximize = true;
                    }
                    println!(
                        "[Compositor] Deferring maximize for window #{} until its first frame",
                        window_id
                    );
                } else {
                    let _ = self.maximize_window_from_client(window_id);
                }
            }
            IpcEvent::RestoreWindow { window_id } => {
                println!("[Compositor] Restoring window #{}", window_id);
                if let Some(window) = self.window_manager.get_window_mut(window_id) {
                    window.pending_maximize = false;
                }
                if self.windowing_mode == sws_protocol::WindowingMode::Focused
                    && self
                        .window_manager
                        .get_window(window_id)
                        .is_some_and(|window| window.supports_focused_windowing())
                {
                    println!(
                        "[Compositor] Keeping window #{} maximized in focused windowing mode",
                        window_id
                    );
                    return Ok(false);
                }
                let old_rect = self
                    .window_manager
                    .get_window(window_id)
                    .map(|w| (w.x, w.y, w.width, w.height));
                if self.window_manager.restore_window(window_id) {
                    if let Some(r) = old_rect {
                        self.add_pending_damage(r);
                    }
                    self.send_window_state_changed(window_id);
                    if let Some(w) = self.window_manager.get_window(window_id) {
                        let (x, y, width, height) = (w.x, w.y, w.width, w.height);
                        self.add_pending_damage((x, y, width, height));

                        // If geometry changed (e.g. restored from maximized), ask the client
                        // to resize its buffer.
                        if let Some((_ox, _oy, ow, oh)) = old_rect {
                            if ow != width || oh != height {
                                let payload = sws_protocol::payload_window_configure(
                                    window_id, width, height,
                                );
                                super::ipc::send_message_to_window(
                                    window_id,
                                    sws_protocol::server_msg::WINDOW_CONFIGURE,
                                    payload.to_vec(),
                                );
                            }
                        }
                    }
                    self.full_redraw_needed = true;
                }
            }
            IpcEvent::SetFullscreen {
                client_id,
                window_id,
            } => {
                if !self.client_owns_window(client_id, window_id) {
                    super::ipc::send_message_to_client(
                        client_id,
                        sws_protocol::server_msg::ERROR,
                        sws_protocol::payload_error(sws_protocol::error_codes::WINDOW_NOT_OWNED)
                            .to_vec(),
                    );
                    return Ok(false);
                }
                if self.window_manager.is_fullscreen(window_id) {
                    self.send_window_state_changed(window_id);
                    return Ok(false);
                }
                if self
                    .window_manager
                    .get_windows()
                    .iter()
                    .any(|window| window.fullscreen && window.id != window_id)
                {
                    self.send_window_state_changed(window_id);
                    self.send_current_window_configure(window_id);
                    super::ipc::send_message_to_client(
                        client_id,
                        sws_protocol::server_msg::ERROR,
                        sws_protocol::payload_error(sws_protocol::error_codes::FULLSCREEN_OCCUPIED)
                            .to_vec(),
                    );
                    return Ok(false);
                }
                if self.window_manager.is_minimized(window_id) {
                    self.window_manager.restore_window(window_id);
                }

                let old_rect = self
                    .window_manager
                    .get_window(window_id)
                    .map(|window| (window.x, window.y, window.width, window.height));
                if !self.window_manager.set_fullscreen_window(
                    window_id,
                    self.screen_width,
                    self.screen_height,
                ) {
                    self.send_window_state_changed(window_id);
                    self.send_current_window_configure(window_id);
                    super::ipc::send_message_to_client(
                        client_id,
                        sws_protocol::server_msg::ERROR,
                        sws_protocol::payload_error(sws_protocol::error_codes::FULLSCREEN_OCCUPIED)
                            .to_vec(),
                    );
                    return Ok(false);
                }

                if self
                    .move_drag
                    .is_some_and(|state| state.window_id == window_id)
                {
                    self.move_drag = None;
                }
                if self
                    .resize_drag
                    .is_some_and(|state| state.window_id == window_id)
                {
                    self.resize_drag = None;
                    self.resize_outline = None;
                }
                self.window_manager.focus_window(window_id);
                self.window_manager.raise_to_top_with_type(window_id);
                self.broadcast_focus_change(window_id);

                if let Some(rect) = old_rect {
                    self.add_pending_damage(rect);
                }
                self.send_window_state_changed(window_id);
                if let Some((x, y, width, height)) = self
                    .window_manager
                    .get_window(window_id)
                    .map(|window| (window.x, window.y, window.width, window.height))
                {
                    let rect = (x, y, width, height);
                    self.add_pending_damage(rect);
                    let payload = sws_protocol::payload_window_configure(window_id, width, height);
                    super::ipc::send_message_to_window(
                        window_id,
                        sws_protocol::server_msg::WINDOW_CONFIGURE,
                        payload.to_vec(),
                    );
                }
                self.full_redraw_needed = true;
            }
            IpcEvent::UnsetFullscreen {
                client_id,
                window_id,
            } => {
                if !self.client_owns_window(client_id, window_id) {
                    super::ipc::send_message_to_client(
                        client_id,
                        sws_protocol::server_msg::ERROR,
                        sws_protocol::payload_error(sws_protocol::error_codes::WINDOW_NOT_OWNED)
                            .to_vec(),
                    );
                    return Ok(false);
                }

                let old_rect = self
                    .window_manager
                    .get_window(window_id)
                    .map(|window| (window.x, window.y, window.width, window.height));
                let restore_maximized = self
                    .window_manager
                    .get_window(window_id)
                    .is_some_and(|window| window.maximized);
                if !self.window_manager.unset_fullscreen_window(window_id) {
                    return Ok(false);
                }

                if restore_maximized
                    && let Some((x, y, width, height)) = self.maximized_geometry(window_id)
                {
                    self.window_manager.set_window_position(window_id, x, y);
                    self.window_manager
                        .resize_window_geometry_in_place(window_id, width, height);
                }
                if let Some(rect) = old_rect {
                    self.add_pending_damage(rect);
                }
                self.send_window_state_changed(window_id);
                if let Some((x, y, width, height)) = self
                    .window_manager
                    .get_window(window_id)
                    .map(|window| (window.x, window.y, window.width, window.height))
                {
                    let rect = (x, y, width, height);
                    self.add_pending_damage(rect);
                    let payload = sws_protocol::payload_window_configure(window_id, width, height);
                    super::ipc::send_message_to_window(
                        window_id,
                        sws_protocol::server_msg::WINDOW_CONFIGURE,
                        payload.to_vec(),
                    );
                }
                self.full_redraw_needed = true;
            }
            IpcEvent::SetPointerLock {
                client_id,
                request_id,
                window_id,
                locked,
            } => match self.set_pointer_lock(client_id, window_id, locked) {
                Ok(changed) => {
                    if !changed {
                        self.send_pointer_lock_changed(
                            PointerLockState::new(client_id, window_id),
                            locked,
                        );
                    }
                    if let CorrelatedReply::State { request_id, locked } =
                        correlated_reply(request_id, locked, true)
                    {
                        send_response_to_client(
                            client_id,
                            sws_protocol::server_msg::POINTER_LOCK_CHANGED,
                            request_id,
                            sws_protocol::payload_pointer_lock_changed(window_id, locked).to_vec(),
                        );
                    }
                }
                Err(code) => {
                    let denial = if code == sws_protocol::error_codes::POINTER_LOCK_NOT_OWNED {
                        PointerLockDenial::NotOwned
                    } else {
                        PointerLockDenial::Denied
                    };
                    self.send_pointer_lock_changed(
                        PointerLockState::new(client_id, window_id),
                        confirmed_lock_state(locked, &Err(denial)),
                    );
                    match correlated_reply(request_id, locked, false) {
                        CorrelatedReply::None => super::ipc::send_message_to_client(
                            client_id,
                            sws_protocol::server_msg::ERROR,
                            sws_protocol::payload_error(code).to_vec(),
                        ),
                        CorrelatedReply::Error { request_id } => send_response_to_client(
                            client_id,
                            sws_protocol::server_msg::ERROR,
                            request_id,
                            sws_protocol::payload_error(code).to_vec(),
                        ),
                        CorrelatedReply::State { .. } => {}
                    }
                }
            },
            IpcEvent::SetCursorIcon { window_id, icon } => {
                if let Some(window) = self.window_manager.get_window_mut(window_id) {
                    window.cursor_icon = icon;
                    self.refresh_cursor_icon();
                }
            }
            IpcEvent::SetCursorTheme {
                client_id,
                request_id,
                theme_path,
            } => match self.activate_cursor_theme(&theme_path) {
                Ok(()) => send_response_to_client(
                    client_id,
                    sws_protocol::server_msg::CURSOR_THEME_CHANGED,
                    request_id,
                    Vec::new(),
                ),
                Err(code) => send_response_to_client(
                    client_id,
                    sws_protocol::server_msg::ERROR,
                    request_id,
                    sws_protocol::payload_error(code).to_vec(),
                ),
            },
            IpcEvent::FocusWindow { window_id } => {
                if let Some(fullscreen_id) = self
                    .window_manager
                    .get_windows()
                    .iter()
                    .find(|window| window.fullscreen)
                    .map(|window| window.id)
                    && !self.window_manager.is_in_fullscreen_group(window_id)
                {
                    println!(
                        "[Compositor] Ignoring focus request for window #{} while #{} is fullscreen",
                        window_id, fullscreen_id
                    );
                    return Ok(false);
                }
                sws_debug!("[Compositor] Focusing window #{}", window_id);
                // Restore if minimized
                if self.window_manager.is_minimized(window_id) {
                    let old_rect = self
                        .window_manager
                        .get_window(window_id)
                        .map(|w| (w.x, w.y, w.width, w.height));
                    if self.window_manager.restore_window(window_id) {
                        if let Some(r) = old_rect {
                            self.add_pending_damage(r);
                        }
                        self.send_window_state_changed(window_id);
                    }
                }

                let previous_focus = self.window_manager.get_focused_window_id();

                // Focus and raise the window
                self.window_manager.focus_window(window_id);
                self.raise_window_with_damage(window_id);

                if previous_focus != Some(window_id) {
                    if let Some(previous_focus) = previous_focus {
                        self.damage_compositor_focus_style(previous_focus);
                    }
                    self.damage_compositor_focus_style(window_id);
                }

                // Broadcast focus change event to all clients
                self.broadcast_focus_change(window_id);
            }
            IpcEvent::SetWindowType {
                window_id,
                window_type,
            } => {
                println!(
                    "[Compositor] Setting window #{} type to {}",
                    window_id, window_type
                );
                use sws_protocol::window_types;
                let wtype = match window_type {
                    window_types::NORMAL => super::window::WindowType::Normal,
                    window_types::ALWAYS_ON_TOP => super::window::WindowType::AlwaysOnTop,
                    window_types::TASKBAR => super::window::WindowType::Taskbar,
                    window_types::DESKTOP => super::window::WindowType::Desktop,
                    window_types::IME_POPUP => super::window::WindowType::ImePopup,
                    window_types::SHELL_BACKGROUND => super::window::WindowType::ShellBackground,
                    window_types::SHELL_CHROME => super::window::WindowType::ShellChrome,
                    _ => {
                        println!("[Compositor] Invalid window type {}, ignoring", window_type);
                        return Ok(false);
                    }
                };
                if self.window_manager.set_window_type(window_id, wtype) {
                    if self.windowing_mode == sws_protocol::WindowingMode::Focused {
                        self.apply_focused_policy_to_window(window_id);
                    }
                    // Re-raise to update Z-order based on window type
                    self.window_manager.raise_to_top_with_type(window_id);
                    self.full_redraw_needed = true;
                }
            }
            IpcEvent::SetWindowOpacity { window_id, opacity } => {
                println!(
                    "[Compositor] Setting window #{} opacity to {}",
                    window_id, opacity
                );
                let opacity_f = (opacity as f32) / 255.0;
                if self.window_manager.set_window_opacity(window_id, opacity_f) {
                    if let Some(w) = self.window_manager.get_window(window_id) {
                        self.add_pending_damage((w.x, w.y, w.width, w.height));
                    }
                }
            }
            IpcEvent::TextInputContextUpdated { context_id } => {
                if self.position_ime_popups_for_context(context_id) {
                    self.full_redraw_needed = true;
                }
            }
            IpcEvent::ImeSetPopupWindow {
                context_id,
                window_id,
                offset_x,
                offset_y,
                visible,
            } => {
                if self.set_ime_popup_window(context_id, window_id, offset_x, offset_y, visible) {
                    self.full_redraw_needed = true;
                }
            }
            IpcEvent::ExtensionRegistered {
                client_id,
                extension_id,
                extension_name,
            } => {
                println!(
                    "[Compositor] IPC: ExtensionRegistered client={} ext_id={} name={}",
                    client_id, extension_id, extension_name
                );
                // Extension is now registered and can create windows
            }
            IpcEvent::ExtensionCreateWindow {
                client_id,
                extension_id,
                external_client_id,
                window_id,
                width,
                height,
                shm,
                shm_mapped_addr,
                shm_size,
            } => {
                println!(
                    "[Compositor] IPC: ExtensionCreateWindow ext_id={} ext_client={} window={} {}x{}",
                    extension_id, external_client_id, window_id, width, height
                );

                // Create window using window manager
                if let Some(shm_handle) = shm {
                    match self.window_manager.create_extension_window(
                        window_id,
                        100, // x position
                        100, // y position
                        width,
                        height,
                        shm_handle,
                        shm_mapped_addr,
                        shm_size,
                        client_id,
                        extension_id,
                        external_client_id,
                    ) {
                        Ok(wid) => {
                            println!(
                                "[Compositor] Created extension window: {} (ext_id={}, ext_client_id={})",
                                wid, extension_id, external_client_id
                            );
                            self.full_redraw_needed = true;
                        }
                        Err(e) => {
                            println!("[Compositor] Failed to create extension window: {}", e);
                        }
                    }
                } else {
                    println!("[Compositor] ExtensionCreateWindow: no SHM provided");
                }
            }
            IpcEvent::ExtensionUpdateBuffer {
                external_client_id,
                window_id,
                damage_x,
                damage_y,
                damage_width,
                damage_height,
            } => {
                sws_debug!(
                    "[Compositor] IPC: ExtensionUpdateBuffer ext_client={} window={} damage=[{},{} {}x{}]",
                    external_client_id,
                    window_id,
                    damage_x,
                    damage_y,
                    damage_width,
                    damage_height
                );

                // Mark window as damaged and trigger redraw
                if let Some((window_x, window_y, presented)) = self
                    .window_manager
                    .get_window(window_id)
                    .map(|window| (window.x, window.y, window.is_presented()))
                {
                    if presented {
                        self.add_pending_damage((
                            window_x + damage_x,
                            window_y + damage_y,
                            damage_width,
                            damage_height,
                        ));
                    }
                    if let Some(gpu_compositor) = self.gpu_compositor.as_mut() {
                        gpu_compositor.mark_window_damage(
                            window_id,
                            damage_x,
                            damage_y,
                            damage_width,
                            damage_height,
                        );
                    }
                    self.note_window_frame_submitted(window_id);
                }
            }
            IpcEvent::ExtensionAttachBuffer {
                external_client_id: _,
                window_id,
                width,
                height,
                offset,
                stride,
                format,
                shm,
                shm_mapped_addr,
                shm_size,
            } => {
                // println!(
                //     "[Compositor] === EXTENSION_ATTACH_BUFFER === ext_client={} window={} ===",
                //     external_client_id, window_id
                // );
                // println!(
                //     "[Compositor] geometry={}x{} stride={} format={} shm_size={} shm={:?} addr={:?}",
                //     width, height, stride, format, shm_size, shm.is_some(), shm_mapped_addr
                // );

                // For zero-copy external buffers, we only need the mapped address
                if let Some(addr) = shm_mapped_addr {
                    // println!("[Compositor] Attaching external buffer at address 0x{:x}", addr);
                    if let Some(shm_handle) = shm {
                        // We have both handle and address (normal case)
                        self.release_gpu_window_texture(window_id)?;
                        if let Err(e) = self.window_manager.replace_window_shm_from_event(
                            window_id,
                            width,
                            height,
                            offset,
                            stride,
                            format,
                            shm_handle,
                            Some(addr),
                            shm_size,
                        ) {
                            println!(
                                "[Compositor] Failed to attach SHM buffer for window {}: {}",
                                window_id, e
                            );
                        } else {
                            if let Some(rect) = self
                                .window_manager
                                .get_window(window_id)
                                .filter(|window| window.is_presented())
                                .map(|window| window.presentation_geometry())
                            {
                                self.add_pending_damage(rect);
                            }
                        }
                    } else {
                        // We have address but no SharedMemory wrapper (e.g., File handle from Linux compat)
                        // This is zero-copy mode - just update the mapped address
                        // println!("[Compositor] Zero-copy mode: updating mapped address without SharedMemory wrapper");
                        self.release_gpu_window_texture(window_id)?;
                        let rect = if let Some(w) = self.window_manager.get_window_mut(window_id) {
                            if let (Some(old_addr), old_size) =
                                (w.shm_mapped_addr.take(), w.shm_size)
                                && old_size != 0
                            {
                                let _ = scarlet_os::handle::capability::memory_mapping::munmap(
                                    old_addr, old_size,
                                );
                            }
                            w.shm = None;
                            w.width = width;
                            w.height = height;
                            w.set_backing_extent(width, height);
                            w.reconcile_window_geometry_after_resize();
                            w.shm_mapped_addr = Some(addr);
                            w.shm_size = shm_size;
                            w.shm_offset = offset.max(0) as usize;
                            w.shm_stride = if stride > 0 {
                                stride as u32
                            } else {
                                width.saturating_mul(4)
                            };
                            w.shm_format = format;
                            w.has_alpha_content = format == 0;
                            w.buffer = None; // Clear Vec buffer if present
                            w.is_presented().then_some(w.presentation_geometry())
                        } else {
                            println!("[Compositor] Window {} not found", window_id);
                            None
                        };

                        if let Some(rect) = rect {
                            self.add_pending_damage(rect);
                        }
                    }
                } else {
                    println!(
                        "[Compositor] No mapped address provided for window {}",
                        window_id
                    );
                }
                // println!("[Compositor] === EXTENSION_ATTACH_BUFFER COMPLETE ===");
            }
            IpcEvent::SetWindowHasAlphaContent {
                window_id,
                has_alpha,
            } => {
                println!(
                    "[Compositor] Setting window #{} has_alpha_content to {}",
                    window_id, has_alpha
                );
                if self
                    .window_manager
                    .set_window_has_alpha_content(window_id, has_alpha)
                {
                    if let Some(w) = self.window_manager.get_window(window_id) {
                        self.add_pending_damage((w.x, w.y, w.width, w.height));
                    }
                }
            }
            IpcEvent::SetWindowGeometry {
                client_id,
                request_id,
                window_id,
                geometry,
            } => {
                if !self.client_owns_window(client_id, window_id) {
                    send_response_to_client(
                        client_id,
                        sws_protocol::server_msg::ERROR,
                        request_id,
                        sws_protocol::payload_error(sws_protocol::error_codes::WINDOW_NOT_OWNED)
                            .to_vec(),
                    );
                    return Ok(false);
                }
                let old_surface = self
                    .window_manager
                    .get_window(window_id)
                    .map(|window| window.surface_geometry());
                let center_after_geometry = self
                    .window_manager
                    .get_window(window_id)
                    .is_some_and(|window| window.center_on_first_geometry);
                match self.window_manager.set_window_geometry(
                    window_id,
                    geometry.x,
                    geometry.y,
                    geometry.width,
                    geometry.height,
                ) {
                    Ok(changed) => {
                        if center_after_geometry {
                            if let Some((width, height)) =
                                self.window_manager.get_window(window_id).map(|window| {
                                    let (_, _, width, height) = window.window_geometry();
                                    (width, height)
                                })
                            {
                                let (x, y) = self.centered_window_position(width, height);
                                self.window_manager.set_window_position(window_id, x, y);
                            }
                            if let Some(window) = self.window_manager.get_window_mut(window_id) {
                                window.center_on_first_geometry = false;
                            }
                        }
                        if changed || center_after_geometry {
                            if let Some(rect) = old_surface {
                                self.add_pending_damage(rect);
                            }
                            if let Some(rect) = self
                                .window_manager
                                .get_window(window_id)
                                .map(|window| window.surface_geometry())
                            {
                                self.add_pending_damage(rect);
                            }
                            self.route_pointer_motion_at_cursor();
                        }
                    }
                    Err(error) => {
                        println!(
                            "[Compositor] Rejecting window geometry for #{}: {}",
                            window_id, error
                        );
                        send_response_to_client(
                            client_id,
                            sws_protocol::server_msg::ERROR,
                            request_id,
                            sws_protocol::payload_error(
                                sws_protocol::error_codes::INVALID_WINDOW_GEOMETRY,
                            )
                            .to_vec(),
                        );
                    }
                }
            }
            IpcEvent::SetWindowMenuTitles {
                window_id,
                menu_titles,
            } => {
                println!(
                    "[Compositor] Updating menu titles for window #{} (len={})",
                    window_id,
                    menu_titles.len()
                );
                let menu_titles_bytes = menu_titles.as_bytes().to_vec();
                if super::ipc::set_app_session_menu_titles(window_id, menu_titles) {
                    let is_focused = self.window_manager.get_focused_window_id() == Some(window_id);
                    if is_focused {
                        self.broadcast_focus_change(window_id);
                    }

                    if let Some(window) = self.window_manager.get_window(window_id) {
                        let window_app_id = window.app_id.as_deref().unwrap_or(b"");
                        let is_active_app = self
                            .active_app_id
                            .as_ref()
                            .map_or(false, |id| id.as_slice() == window_app_id);

                        if is_active_app || is_focused {
                            let (app_name, _) = super::ipc::get_app_session_info(window_id);
                            let app_name_bytes = app_name.as_bytes();
                            let title_bytes = window.title.as_deref().unwrap_or(b"");
                            let payload = sws_protocol::payload_active_app_changed(
                                window_id,
                                window_app_id,
                                app_name_bytes,
                                title_bytes,
                                &menu_titles_bytes,
                            );
                            super::ipc::broadcast_message_to_all_clients(
                                sws_protocol::server_msg::ACTIVE_APP_CHANGED,
                                payload,
                            );
                        }
                    }
                }
            }
            IpcEvent::ActivateMenuItem {
                window_id,
                menu_item_id,
            } => {
                println!(
                    "[Compositor] Activating menu item for window #{} (len={})",
                    window_id,
                    menu_item_id.len()
                );
                let payload =
                    sws_protocol::payload_menu_item_activated(window_id, menu_item_id.as_bytes());
                super::ipc::send_message_to_window(
                    window_id,
                    sws_protocol::server_msg::MENU_ITEM_ACTIVATED,
                    payload,
                );
            }
            IpcEvent::SetWorkarea {
                x,
                y,
                width,
                height,
            } => {
                println!(
                    "[Compositor] Workarea set: x={}, y={}, width={}, height={}",
                    x, y, width, height
                );
                self.workarea = Some((x, y, width, height));
                // Notify window manager about workarea change
                self.window_manager.set_workarea(x, y, width, height);
                self.publish_window_creation_environment();
                self.reflow_maximized_windows_to_workarea();
                if self.windowing_mode == sws_protocol::WindowingMode::Focused {
                    self.apply_windowing_mode_policy();
                }
                self.full_redraw_needed = true;
            }
            IpcEvent::SetWindowResizable {
                window_id,
                resizable,
            } => {
                println!(
                    "[Compositor] Setting window #{} resizable to {}",
                    window_id, resizable
                );
                if self
                    .window_manager
                    .set_window_resizable(window_id, resizable)
                {
                    if self.windowing_mode == sws_protocol::WindowingMode::Focused {
                        self.apply_focused_policy_to_window(window_id);
                    }
                }
            }
            IpcEvent::GetScreenSize {
                client_id,
                request_id,
            } => {
                println!(
                    "[Compositor] GetScreenSize request from client {}",
                    client_id
                );
                let payload =
                    sws_protocol::payload_screen_size(self.screen_width, self.screen_height);
                super::ipc::send_response_to_client(
                    client_id,
                    sws_protocol::server_msg::SCREEN_SIZE,
                    request_id,
                    payload.to_vec(),
                );
                println!(
                    "[Compositor] Sent SCREEN_SIZE: {}x{} to client {}",
                    self.screen_width, self.screen_height, client_id
                );
            }
            IpcEvent::GetOutputScale {
                client_id,
                request_id,
            } => {
                println!(
                    "[Compositor] GetOutputScale request from client {}",
                    client_id
                );
                let payload = sws_protocol::payload_output_scale(self.output_scale_milli);
                super::ipc::send_response_to_client(
                    client_id,
                    sws_protocol::server_msg::OUTPUT_SCALE,
                    request_id,
                    payload.to_vec(),
                );
                println!(
                    "[Compositor] Sent OUTPUT_SCALE: {} to client {}",
                    self.output_scale_milli, client_id
                );
            }
            IpcEvent::GetWindowList {
                client_id,
                request_id,
            } => {
                println!(
                    "[Compositor] GetWindowList request from client {}",
                    client_id
                );
                // Get window list from window manager
                let windows = self.window_manager.get_window_list();

                // Convert to WindowListEntry and use protocol library serialization
                let entries: std::vec::Vec<sws_protocol::WindowListEntry> = windows
                    .into_iter()
                    .map(
                        |(window_id, app_id, title, window_type, visible, focused, minimized)| {
                            sws_protocol::WindowListEntry {
                                window_id,
                                app_id,
                                title,
                                window_type,
                                visible,
                                focused,
                                minimized,
                            }
                        },
                    )
                    .collect();

                let payload = sws_protocol::payload_window_list(&entries);

                // Send WINDOW_LIST response directly to the client (not via window)
                // This works for clients with or without windows (like stemd)
                send_response_to_client(
                    client_id,
                    sws_protocol::server_msg::WINDOW_LIST,
                    request_id,
                    payload,
                );
                println!(
                    "[Compositor] Sent WINDOW_LIST: {} windows to client {}",
                    entries.len(),
                    client_id
                );
            }
            IpcEvent::RequestActivationToken {
                client_id,
                request_id,
                source_window_id,
                target_app_id,
            } => {
                if let Some(token) = self.issue_activation_token(source_window_id, target_app_id) {
                    send_response_to_client(
                        client_id,
                        sws_protocol::server_msg::ACTIVATION_TOKEN,
                        request_id,
                        sws_protocol::payload_activation_token(token.as_bytes()),
                    );
                } else {
                    send_response_to_client(
                        client_id,
                        sws_protocol::server_msg::ERROR,
                        request_id,
                        sws_protocol::payload_error(sws_protocol::error_codes::ACTIVATION_DENIED)
                            .to_vec(),
                    );
                }
            }
            IpcEvent::SetTabletModeOverride {
                client_id,
                request_id,
                tablet_mode,
            } => {
                if let Some(snapshot) = input_environment::set_tablet_mode_override(tablet_mode)
                    && self.apply_input_environment_snapshot(snapshot)?
                {
                    self.full_redraw_needed = true;
                }
                let snapshot = input_environment::snapshot();
                send_response_to_client(
                    client_id,
                    sws_protocol::server_msg::INPUT_ENVIRONMENT_CHANGED,
                    request_id,
                    input_environment::protocol_payload(snapshot).to_vec(),
                );
            }
            IpcEvent::SetWindowingModeOverride {
                client_id,
                request_id,
                windowing_mode,
            } => {
                if let Some(snapshot) =
                    input_environment::set_windowing_mode_override(windowing_mode)
                    && self.apply_input_environment_snapshot(snapshot)?
                {
                    self.full_redraw_needed = true;
                }
                let snapshot = input_environment::snapshot();
                send_response_to_client(
                    client_id,
                    sws_protocol::server_msg::INPUT_ENVIRONMENT_CHANGED,
                    request_id,
                    input_environment::protocol_payload(snapshot).to_vec(),
                );
            }
            IpcEvent::RegisterSystemShell {
                client_id,
                request_id,
            } => {
                if !super::ipc::is_system_shell_client(client_id) {
                    send_response_to_client(
                        client_id,
                        sws_protocol::server_msg::ERROR,
                        request_id,
                        sws_protocol::payload_error(
                            sws_protocol::error_codes::SYSTEM_SHELL_REQUIRED,
                        )
                        .to_vec(),
                    );
                } else {
                    send_response_to_client(
                        client_id,
                        sws_protocol::server_msg::SYSTEM_SHELL_REGISTERED,
                        request_id,
                        sws_protocol::workspace::encode_state(&self.workspace_manager.snapshot()),
                    );
                }
            }
            IpcEvent::GetWorkspaceState {
                client_id,
                request_id,
            } => {
                send_response_to_client(
                    client_id,
                    sws_protocol::server_msg::WORKSPACE_STATE,
                    request_id,
                    sws_protocol::workspace::encode_state(&self.workspace_manager.snapshot()),
                );
            }
            IpcEvent::ApplyWorkspaceTransaction {
                client_id,
                request_id,
                transaction,
            } => {
                if !super::ipc::is_system_shell_client(client_id) {
                    send_response_to_client(
                        client_id,
                        sws_protocol::server_msg::ERROR,
                        request_id,
                        sws_protocol::payload_error(
                            sws_protocol::error_codes::SYSTEM_SHELL_REQUIRED,
                        )
                        .to_vec(),
                    );
                    return Ok(false);
                }
                let live_window_ids = self.live_workspace_window_ids();
                match self
                    .workspace_manager
                    .apply_transaction(transaction, &live_window_ids)
                {
                    Ok(applied) => {
                        self.apply_workspace_presentation_policy();
                        let payload = sws_protocol::workspace::encode_state(&applied.state);
                        send_response_to_client(
                            client_id,
                            sws_protocol::server_msg::WORKSPACE_STATE,
                            request_id,
                            payload,
                        );
                        self.full_redraw_needed = true;
                    }
                    Err(super::workspace::ApplyError::StaleGeneration) => {
                        send_response_to_client(
                            client_id,
                            sws_protocol::server_msg::ERROR,
                            request_id,
                            sws_protocol::payload_error(
                                sws_protocol::error_codes::STALE_WORKSPACE_GENERATION,
                            )
                            .to_vec(),
                        );
                    }
                    Err(super::workspace::ApplyError::InvalidState) => {
                        send_response_to_client(
                            client_id,
                            sws_protocol::server_msg::ERROR,
                            request_id,
                            sws_protocol::payload_error(
                                sws_protocol::error_codes::INVALID_WORKSPACE_STATE,
                            )
                            .to_vec(),
                        );
                    }
                }
            }
        }
        self.refresh_cursor_icon();
        self.release_invalid_pointer_lock();
        Ok(false)
    }
}
