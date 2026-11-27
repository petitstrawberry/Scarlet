//! In-memory display preferences and arrangement preview.
use std::{format, string::String, vec, vec::Vec};
use sws_protocol::outputs::{
    self as outputs, OutputConfiguration, OutputDescriptor, OutputExperience, OutputRole,
    OutputSnapshot,
};

pub(crate) const SCALES: &[u32] = &[1000, 1250, 1500, 1750, 2000, 2500, 3000];
pub(crate) const WIDTH: f32 = 544.0;
pub(crate) const HEIGHT: f32 = 220.0;

// Thresholds are preview points, independent of monitor resolution and zoom.
const SNAP_DISTANCE: f32 = 7.0;
const SNAP_RELEASE: f32 = 11.0;
#[derive(Clone, Copy)]
pub(crate) struct AxisSnap {
    position: i32,
    pub(crate) guide: i32,
}

fn snap_axis(
    value: i32,
    extent: u32,
    neighbours: impl Iterator<Item = (i32, u32)>,
    zoom: f32,
    held: Option<AxisSnap>,
) -> Option<AxisSnap> {
    let mut best: Option<(bool, f32, AxisSnap)> = None;
    for (origin, length) in neighbours {
        let start = i64::from(origin);
        let end = start + i64::from(length);
        for (position, guide) in [
            (start - i64::from(extent), start),
            (end, end),
            (start, start),
            (end - i64::from(extent), end),
            (
                start + (i64::from(length) - i64::from(extent)) / 2,
                start + i64::from(length) / 2,
            ),
        ] {
            let (Ok(position), Ok(guide)) = (i32::try_from(position), i32::try_from(guide)) else {
                continue;
            };
            let retained =
                held.is_some_and(|snap| snap.position == position && snap.guide == guide);
            let distance = ((i64::from(value) - i64::from(position)).abs() as f32) * zoom;
            if distance
                > if retained {
                    SNAP_RELEASE
                } else {
                    SNAP_DISTANCE
                }
            {
                continue;
            }
            if best.as_ref().is_none_or(|(old_retained, old_distance, _)| {
                (retained && !old_retained) || retained == *old_retained && distance < *old_distance
            }) {
                best = Some((retained, distance, AxisSnap { position, guide }));
            }
        }
    }
    best.map(|(_, _, snap)| snap)
}

pub(crate) struct PendingChanges {
    pub original: OutputSnapshot,
    pub configuration: Vec<OutputConfiguration>,
    pub modes: Vec<(u32, outputs::OutputMode)>,
}

#[derive(Clone)]
pub(crate) struct Editor {
    pub snapshot: Option<OutputSnapshot>,
    pub draft: Vec<OutputConfiguration>,
    pub selected: usize,
    pub status: String,
    pub dirty: bool,
    pub modes: Vec<(u32, Vec<outputs::OutputMode>)>,
    mode_draft: Vec<(u32, outputs::OutputMode)>,
    pub applying: bool,
    pub supports_roles: bool,
    // View callbacks are replaced on rebuild; contact ownership must outlive them.
    pub(crate) touch_contact: Option<(u32, u64)>,
    pub(crate) drag_snap: (Option<AxisSnap>, Option<AxisSnap>),
    pub(crate) drag_before: Option<Vec<OutputConfiguration>>,
    pub(crate) drag: Option<(u32, i32, i32, i32, i32, (f32, f32, f32))>,
}
impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}
impl Editor {
    pub fn new() -> Self {
        Self {
            snapshot: None,
            draft: Vec::new(),
            selected: 0,
            status: String::from("Loading displays…"),
            dirty: false,
            modes: Vec::new(),
            mode_draft: Vec::new(),
            applying: false,
            supports_roles: false,
            drag: None,
            drag_before: None,
            drag_snap: (None, None),
            touch_contact: None,
        }
    }
    pub(crate) fn adopt(&mut self, snapshot: OutputSnapshot) {
        self.selected = self
            .selected_output()
            .and_then(|old| {
                snapshot
                    .outputs
                    .iter()
                    .position(|output| {
                        old.identity_is_unique
                            && output.identity_is_unique
                            && old.monitor_key == output.monitor_key
                    })
                    .or_else(|| {
                        snapshot
                            .outputs
                            .iter()
                            .position(|output| output.connector_key == old.connector_key)
                    })
            })
            .unwrap_or(0);
        self.mode_draft.clear();
        self.draft = snapshot.configuration.clone();
        self.snapshot = Some(snapshot);
        self.dirty = false;
        self.drag = None;
        self.drag_before = None;
        self.drag_snap = (None, None);
        self.touch_contact = None;
    }
    pub fn observe(&mut self, snapshot: OutputSnapshot) {
        if let Some(old) = &self.snapshot {
            if old.generation >= snapshot.generation {
                return;
            }
            // Sensor events advance the generation without invalidating pending edits.
            if !self.applying
                && old.outputs == snapshot.outputs
                && old.configuration == snapshot.configuration
            {
                self.snapshot = Some(snapshot);
                return;
            }
        }
        self.adopt(snapshot);
        self.status.clear();
    }

    pub fn selected_output(&self) -> Option<&OutputDescriptor> {
        self.snapshot.as_ref()?.outputs.get(self.selected)
    }

    fn config(&self, id: u32) -> Option<OutputConfiguration> {
        self.draft.iter().find(|c| c.id == id).copied()
    }

    pub fn selected_config(&self) -> Option<OutputConfiguration> {
        self.config(self.selected_output()?.id)
    }

    pub fn editable(&self) -> bool {
        !self.applying && self.drag.is_none()
    }

    pub fn has_other_desktop(&self, id: u32) -> bool {
        self.draft
            .iter()
            .any(|c| c.id != id && c.role == OutputRole::Extend)
    }

    pub fn can_change_role(&self) -> bool {
        self.supports_roles
            && self.editable()
            && self
                .selected_config()
                .is_some_and(|c| c.role != OutputRole::Extend || self.has_other_desktop(c.id))
    }

    pub fn mirror_source(&self) -> Option<&OutputDescriptor> {
        let snapshot = self.snapshot.as_ref()?;
        let id = outputs::extended_primary(&snapshot.outputs, &self.draft)?;
        snapshot.outputs.iter().find(|output| output.id == id)
    }

    pub fn mirror_option(&self) -> String {
        let selected = self.selected_config().map(|c| c.id);
        let internal_source = self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.outputs.iter().any(|o| {
                o.internal
                    && Some(o.id) != selected
                    && self
                        .config(o.id)
                        .is_some_and(|c| c.role == OutputRole::Extend)
            })
        });
        if internal_source {
            "Mirror built-in display".into()
        } else {
            "Mirror primary display".into()
        }
    }

    pub fn selected_desktop(&self) -> Option<u32> {
        let config = self.selected_config()?;
        match config.role {
            OutputRole::Extend => Some(config.id),
            OutputRole::Mirror => Some(self.mirror_source()?.id),
            OutputRole::Disabled => None,
        }
    }

    pub fn role_hint(&self) -> String {
        let Some(config) = self.selected_config() else {
            return String::new();
        };
        if !self.supports_roles {
            return "This window server supports extended displays only.".into();
        }
        match config.role {
            OutputRole::Mirror => self.mirror_source().map_or_else(String::new, |source| {
                format!("Showing the desktop from {}.", source.name)
            }),
            OutputRole::Disabled => {
                "Choose Extend desktop or Mirror to turn this display on.".into()
            }
            OutputRole::Extend if !self.has_other_desktop(config.id) => {
                "Another display must extend the desktop before you can mirror or disable this one."
                    .into()
            }
            OutputRole::Extend => String::new(),
        }
    }

    pub fn output_labels(&self) -> Vec<String> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        snapshot
            .outputs
            .iter()
            .enumerate()
            .map(|(i, output)| {
                let suffix = match self.config(output.id).map(|c| c.role) {
                    Some(OutputRole::Mirror) => " · Mirror",
                    Some(OutputRole::Disabled) => " · Disabled",
                    _ => "",
                };
                let built_in = if output.internal { " (built-in)" } else { "" };
                format!("{}. {}{built_in}{suffix}", i + 1, output.name)
            })
            .collect()
    }

    pub fn card_label(&self, index: usize) -> String {
        let mut label = format!("{}", index + 1);
        if let Some(snapshot) = &self.snapshot
            && snapshot.outputs.get(index) == self.mirror_source()
        {
            for (i, mirror) in snapshot.outputs.iter().enumerate() {
                if self
                    .config(mirror.id)
                    .is_some_and(|c| c.role == OutputRole::Mirror)
                {
                    label.push_str(&format!(" + {}", i + 1));
                }
            }
        }
        label
    }

    pub fn set_role(&mut self, id: u32, role: OutputRole) {
        let Some(current) = self.selected_config().filter(|c| c.id == id) else {
            return;
        };
        if !self.can_change_role()
            || current.role == role
            || (role != OutputRole::Extend && !self.has_other_desktop(id))
        {
            return;
        }
        self.draft.iter_mut().find(|c| c.id == id).unwrap().role = role;
        if role == OutputRole::Disabled {
            self.mode_draft.retain(|(output, _)| *output != id);
        }
        self.settle(id);
    }

    pub fn revert(&mut self) {
        if self.editable()
            && let Some(snapshot) = self.snapshot.clone()
        {
            self.adopt(snapshot);
            self.status.clear();
        }
    }

    pub fn selected_modes(&self) -> Vec<outputs::OutputMode> {
        let Some(output) = self.selected_output() else {
            return Vec::new();
        };
        self.modes
            .iter()
            .find(|(id, _)| *id == output.id)
            .map(|(_, modes)| modes.clone())
            .unwrap_or_else(|| vec![output.mode])
    }
    pub fn selected_mode_index(&self) -> usize {
        let Some(output) = self.selected_output() else {
            return 0;
        };
        let mode = self.preview_mode(output);
        self.selected_modes()
            .iter()
            .position(|candidate| *candidate == mode)
            .unwrap_or(usize::MAX)
    }
    fn preview_mode(&self, output: &outputs::OutputDescriptor) -> outputs::OutputMode {
        self.mode_draft
            .iter()
            .find(|(id, _)| *id == output.id)
            .map_or(output.mode, |(_, mode)| *mode)
    }
    pub fn logical_size_label(&self) -> String {
        let size = self.selected_desktop().and_then(|id| {
            let config = self.config(id)?;
            let output = self
                .snapshot
                .as_ref()?
                .outputs
                .iter()
                .find(|output| output.id == id)?;
            self.preview_mode(output)
                .logical_size(config.scale_milli)
                .ok()
        });
        size.map_or_else(|| "—".into(), |(w, h)| format!("{w} × {h}"))
    }
    pub(crate) fn geometry(&self, viewport_width: f32) -> Option<(f32, f32, f32)> {
        // Keep the preview coordinate system fixed while the pointer moves.
        // Re-fitting to the moving card would move it away from the drag anchor.
        if let Some((_, _, _, _, _, geometry)) = self.drag {
            return Some(geometry);
        }
        let snapshot = self.snapshot.as_ref()?;
        let mut left = i64::MAX;
        let mut top = i64::MAX;
        let mut right = i64::MIN;
        let mut bottom = i64::MIN;
        for config in self.draft.iter().filter(|c| c.role == OutputRole::Extend) {
            let output = snapshot
                .outputs
                .iter()
                .find(|output| output.id == config.id)?;
            let (width, height) = self
                .preview_mode(output)
                .logical_size(config.scale_milli)
                .ok()?;
            left = left.min(i64::from(config.x));
            top = top.min(i64::from(config.y));
            right = right.max(i64::from(config.x) + i64::from(width));
            bottom = bottom.max(i64::from(config.y) + i64::from(height));
        }
        if left == i64::MAX {
            return None;
        }
        let zoom = ((viewport_width - 48.0) / (right - left) as f32)
            .min((HEIGHT - 48.0) / (bottom - top) as f32)
            .max(0.001);
        Some((
            (viewport_width - (right - left) as f32 * zoom) * 0.5 - left as f32 * zoom,
            (HEIGHT - (bottom - top) as f32 * zoom) * 0.5 - top as f32 * zoom,
            zoom,
        ))
    }
    pub(crate) fn card(&self, index: usize, viewport_width: f32) -> Option<(f32, f32, f32, f32)> {
        let snapshot = self.snapshot.as_ref()?;
        let output = snapshot.outputs.get(index)?;
        let config = self
            .config(output.id)
            .filter(|c| c.role == OutputRole::Extend)?;
        let (width, height) = self
            .preview_mode(output)
            .logical_size(config.scale_milli)
            .ok()?;
        let (x, y, zoom) = self.geometry(viewport_width)?;
        Some((
            x + config.x as f32 * zoom,
            y + config.y as f32 * zoom,
            width as f32 * zoom,
            height as f32 * zoom,
        ))
    }
    pub(crate) fn move_preview(&mut self, id: u32, desired: (i32, i32), zoom: f32) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(moving) = self.config(id).filter(|c| c.role == OutputRole::Extend) else {
            return;
        };
        let Some(output) = snapshot.outputs.iter().find(|output| output.id == id) else {
            return;
        };
        let Ok((width, height)) = self.preview_mode(output).logical_size(moving.scale_milli) else {
            return;
        };
        let neighbours: Vec<_> = self
            .draft
            .iter()
            .filter(|config| config.id != id && config.role == OutputRole::Extend)
            .filter_map(|config| {
                let output = snapshot
                    .outputs
                    .iter()
                    .find(|output| output.id == config.id)?;
                let (w, h) = self
                    .preview_mode(output)
                    .logical_size(config.scale_milli)
                    .ok()?;
                Some((config.x, config.y, w, h))
            })
            .collect();
        let nearby = |a: i32, length: u32, b: i32, other_length: u32| {
            let gap = (i64::from(a) - i64::from(b) - i64::from(other_length))
                .max(i64::from(b) - i64::from(a) - i64::from(length))
                .max(0);
            gap as f32 * zoom <= SNAP_RELEASE
        };
        let x_snap = snap_axis(
            desired.0,
            width,
            neighbours
                .iter()
                .filter(|(_, y, _, h)| nearby(desired.1, height, *y, *h))
                .map(|(x, _, w, _)| (*x, *w)),
            zoom,
            self.drag_snap.0,
        );
        let y_snap = snap_axis(
            desired.1,
            height,
            neighbours
                .iter()
                .filter(|(x, _, w, _)| nearby(desired.0, width, *x, *w))
                .map(|(_, y, _, h)| (*y, *h)),
            zoom,
            self.drag_snap.1,
        );
        // Do not pull the preview into another monitor. Keep free dragging
        // possible through occupied space; final topology validation is on drop.
        let nonoverlapping = |x: i32, y: i32| {
            neighbours.iter().all(|&(ox, oy, w, h)| {
                i64::from(x) >= i64::from(ox) + i64::from(w)
                    || i64::from(ox) >= i64::from(x) + i64::from(width)
                    || i64::from(y) >= i64::from(oy) + i64::from(h)
                    || i64::from(oy) >= i64::from(y) + i64::from(height)
            })
        };
        let mut position = desired;
        self.drag_snap = (None, None);
        for (sx, sy) in [(x_snap, y_snap), (x_snap, None), (None, y_snap)] {
            let x = sx.map_or(desired.0, |snap| snap.position);
            let y = sy.map_or(desired.1, |snap| snap.position);
            if (sx.is_some() || sy.is_some()) && nonoverlapping(x, y) {
                position = (x, y);
                self.drag_snap = (sx, sy);
                break;
            }
        }
        if let Some(config) = self.draft.iter_mut().find(|config| config.id == id) {
            config.x = position.0;
            config.y = position.1;
        }
    }
    fn projected_outputs(&self) -> Vec<OutputDescriptor> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        let mut projected = snapshot.outputs.clone();
        for output in &mut projected {
            output.mode = self.preview_mode(output);
        }
        projected
    }

    pub fn can_apply(&self) -> bool {
        self.dirty
            && self.editable()
            && outputs::canonical_layout(&self.projected_outputs(), &self.draft).is_ok()
    }

    pub fn begin_apply(&mut self) -> Option<PendingChanges> {
        if !self.can_apply() {
            return None;
        }
        let pending = PendingChanges {
            original: self.snapshot.clone()?,
            configuration: self.draft.clone(),
            modes: self.mode_draft.clone(),
        };
        self.applying = true;
        self.status.clear();
        Some(pending)
    }

    pub fn finish_apply(&mut self, snapshot: Option<OutputSnapshot>, status: String) {
        if let Some(snapshot) = snapshot
            && self
                .snapshot
                .as_ref()
                .is_none_or(|old| old.generation <= snapshot.generation)
        {
            self.adopt(snapshot);
        }
        self.applying = false;
        self.status = status;
    }

    pub(crate) fn settle(&mut self, id: u32) {
        let projected = self.projected_outputs();
        // Keep the old extend position, even behind a mirror or disabled panel.
        // Snap only the edited desktop, and only if its saved position no longer fits.
        if outputs::validate_layout(&projected, &self.draft).is_err()
            && let Some(c) = self.config(id).filter(|c| c.role == OutputRole::Extend)
            && let Ok((x, y)) = outputs::snapped_position(&projected, &self.draft, id, (c.x, c.y))
        {
            let current = self.draft.iter_mut().find(|c| c.id == id).unwrap();
            (current.x, current.y) = (x, y);
        }
        self.status = if outputs::validate_layout(&projected, &self.draft).is_ok() {
            String::new()
        } else if let Some(before) = self.drag_before.take()
            && outputs::validate_layout(&projected, &before).is_ok()
        {
            self.draft = before;
            "Displays must touch without overlapping. The previous arrangement was restored.".into()
        } else {
            "Move the extended displays so they touch without overlapping, or revert the changes."
                .into()
        };
        self.update_dirty();
    }

    pub fn set_resolution(&mut self, id: u32, mode: outputs::OutputMode) {
        let Some(config) = self.selected_config().filter(|c| c.id == id) else {
            return;
        };
        if !self.editable()
            || config.role == OutputRole::Disabled
            || !self.selected_modes().contains(&mode)
        {
            return;
        }
        let current = self.selected_output().unwrap().mode;
        self.mode_draft.retain(|(output, _)| *output != id);
        if mode != current {
            self.mode_draft.push((id, mode));
        }
        self.settle(id);
    }

    fn editable_desktop(&mut self, id: u32) -> Option<&mut OutputConfiguration> {
        if !self.editable() || self.selected_output()?.id != id {
            return None;
        }
        self.draft
            .iter_mut()
            .find(|c| c.id == id && c.role == OutputRole::Extend)
    }

    pub fn set_scale(&mut self, id: u32, scale: u32) {
        if let Some(c) = self.editable_desktop(id) {
            c.scale_milli = scale;
            self.settle(id);
        }
    }

    pub fn set_experience(&mut self, id: u32, experience: OutputExperience) {
        if let Some(c) = self.editable_desktop(id) {
            c.experience = experience;
            self.update_dirty();
        }
    }

    fn update_dirty(&mut self) {
        self.dirty = self
            .snapshot
            .as_ref()
            .is_some_and(|s| self.draft != s.configuration)
            || !self.mode_draft.is_empty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use OutputRole::{Disabled, Extend, Mirror};

    const PANEL: u32 = 10;
    const DOCK: u32 = 20;
    const TV: u32 = 30;
    const MODE: outputs::OutputMode = outputs::OutputMode {
        pixel_width: 1200,
        pixel_height: 800,
        refresh_millihz: 60_000,
    };

    fn snapshot(generation: u32, entries: &[(u32, OutputRole, i32, i32)]) -> OutputSnapshot {
        let outputs = entries
            .iter()
            .map(|&(id, _, _, _)| OutputDescriptor {
                id,
                connector_key: format!("port-{id}"),
                monitor_key: format!("screen-{id}"),
                identity_is_unique: true,
                name: format!("Screen {id}"),
                mode: MODE,
                primary: id == PANEL,
                internal: id == PANEL,
                physical_width_mm: 0,
                physical_height_mm: 0,
            })
            .collect();
        // Configuration order is independent of connector enumeration.
        let configuration = entries
            .iter()
            .rev()
            .map(|&(id, role, x, y)| OutputConfiguration {
                id,
                role,
                x,
                y,
                scale_milli: 1000,
                experience: OutputExperience::Auto,
            })
            .collect();
        OutputSnapshot {
            generation,
            outputs,
            configuration,
        }
    }

    fn editor(entries: &[(u32, OutputRole, i32, i32)]) -> Editor {
        let mut editor = Editor::new();
        editor.supports_roles = true;
        editor.observe(snapshot(1, entries));
        editor
    }

    #[test]
    fn mirror_tracks_internal_then_extended_primary_and_shares_the_preview_card() {
        let mut editor = editor(&[
            (DOCK, Extend, -1200, 100),
            (TV, Extend, 1200, 20),
            (PANEL, Extend, 0, 0),
        ]);
        // Hardware primary can differ from the preferred built-in mirror source.
        for output in &mut editor.snapshot.as_mut().unwrap().outputs {
            output.primary = output.id == DOCK;
        }
        editor.selected = 1;
        assert_eq!(editor.mirror_option(), "Mirror built-in display");
        editor.set_role(TV, Mirror);
        assert_eq!(editor.selected_desktop(), Some(PANEL));
        assert_eq!(editor.card_label(2), "3 + 2");
        assert!(editor.card(1, WIDTH).is_none());
        assert!(editor.can_apply());
        editor.selected = 2;
        editor.set_role(PANEL, Disabled);
        assert_eq!(editor.selected_desktop(), None);
        assert!(editor.card(2, WIDTH).is_none());
        let (x, y, w, h) = editor.card(0, WIDTH).unwrap();
        assert!((x + w / 2.0 - WIDTH / 2.0).abs() < 0.001);
        assert!((y + h / 2.0 - HEIGHT / 2.0).abs() < 0.001);
        editor.selected = 1;
        assert_eq!(editor.selected_desktop(), Some(DOCK));
        assert_eq!(editor.card_label(0), "1 + 2");
        assert_eq!(editor.mirror_option(), "Mirror primary display");
        assert!(editor.role_hint().contains("Screen 20"));
        assert_eq!(editor.output_labels().len(), 3);
        assert!(editor.output_labels()[2].contains("Disabled"));
        assert!(editor.can_apply());
        assert_eq!(editor.config(TV).map(|c| (c.x, c.y)), Some((1200, 20)));
        assert_eq!(editor.config(DOCK).map(|c| (c.x, c.y)), Some((-1200, 100)));
    }

    #[test]
    fn last_source_stays_extended_and_unplug_supersedes_an_inflight_apply() {
        let mut editor = editor(&[(PANEL, Extend, 0, 0), (DOCK, Extend, 1200, 180)]);
        editor.set_role(PANEL, Disabled);
        editor.selected = 1;
        editor.set_role(DOCK, Mirror);
        editor.set_role(DOCK, Disabled);
        assert_eq!(editor.config(DOCK).unwrap().role, Extend);
        let pending = editor.begin_apply().unwrap();
        let docked = OutputSnapshot {
            generation: 2,
            configuration: pending.configuration,
            ..pending.original
        };
        assert_eq!(docked.configuration.len(), 2);
        assert!(
            docked
                .configuration
                .iter()
                .any(|c| c.id == PANEL && c.role == Disabled)
        );
        let recovered = snapshot(3, &[(PANEL, Extend, 0, 0)]);
        editor.observe(recovered.clone());
        editor.finish_apply(Some(docked), String::new());
        assert_eq!(editor.snapshot.as_ref(), Some(&recovered));
        assert_eq!(editor.selected_desktop(), Some(PANEL));
        assert_eq!(editor.output_labels().len(), 1);
        assert!(editor.card(0, WIDTH).is_some());
        editor.set_role(PANEL, Mirror);
        editor.set_role(PANEL, Disabled);
        assert!(!editor.dirty);
        assert!(!editor.can_apply());
    }

    #[test]
    fn newer_live_preferences_win_even_if_they_match_the_pre_apply_layout() {
        let mut editor = editor(&[(PANEL, Extend, 0, 0), (DOCK, Extend, 1200, 0)]);
        editor.set_role(PANEL, Disabled);
        let pending = editor.begin_apply().unwrap();
        let applied = OutputSnapshot {
            generation: 2,
            configuration: pending.configuration,
            ..pending.original.clone()
        };
        // Another client restored the original preferences before the reply arrived.
        let newer = OutputSnapshot {
            generation: 3,
            ..pending.original
        };
        editor.observe(newer.clone());
        editor.finish_apply(Some(applied), String::new());
        assert_eq!(editor.draft, newer.configuration);
        assert_eq!(editor.selected_desktop(), Some(PANEL));
        assert!(!editor.can_apply());
    }

    #[test]
    fn reverting_and_switching_back_keep_extended_coordinates_and_preferences() {
        let mut editor = editor(&[(PANEL, Extend, 0, 0), (DOCK, Extend, -1200, -248)]);
        let original = editor.draft.clone();
        editor.selected = 1;
        editor.set_experience(DOCK, OutputExperience::Tablet);
        for role in [Mirror, Disabled, Extend] {
            editor.set_role(DOCK, role);
            let config = editor.selected_config().unwrap();
            assert_eq!((config.x, config.y), (-1200, -248));
            assert_eq!(config.experience, OutputExperience::Tablet);
            assert!(editor.can_apply());
        }
        editor.set_experience(DOCK, OutputExperience::Auto);
        assert!(!editor.can_apply());
        editor.set_role(DOCK, Mirror);
        let mut same = editor.snapshot.clone().unwrap();
        same.generation += 1;
        editor.observe(same);
        assert_eq!(editor.config(DOCK).unwrap().role, Mirror);
        editor.revert();
        assert_eq!(editor.draft, original);
        assert_eq!(editor.selected, 1);
        assert!(!editor.dirty);
    }

    #[test]
    fn internal_panel_can_mirror_the_external_desktop_and_restore_its_saved_position() {
        let mut editor = editor(&[(PANEL, Extend, 0, 0), (DOCK, Extend, 1200, -96)]);
        editor.selected = 1;
        editor.set_scale(DOCK, SCALES[4]);
        editor.selected = 0;
        let panel = editor.selected_config().unwrap();
        editor.set_role(PANEL, Mirror);
        assert_eq!(editor.selected_desktop(), Some(DOCK));
        assert_eq!(editor.logical_size_label(), "600 × 400");
        assert!(editor.card(0, WIDTH).is_none());
        assert_eq!(editor.card_label(1), "2 + 1");
        editor.set_role(PANEL, Extend);
        assert_eq!(editor.selected_config(), Some(panel));
    }

    #[test]
    fn legacy_servers_keep_scale_editing_but_disable_role_changes() {
        let mut editor = editor(&[(PANEL, Extend, 0, 0), (DOCK, Extend, 1200, 0)]);
        editor.supports_roles = false;
        assert!(!editor.can_change_role());
        editor.set_role(PANEL, Disabled);
        assert_eq!(editor.selected_desktop(), Some(PANEL));
        editor.set_scale(PANEL, 1250);
        assert_eq!(editor.selected_config().unwrap().scale_milli, 1250);
        assert!(editor.can_apply());
    }

    #[test]
    fn removing_a_bridge_requires_reconnecting_the_remaining_desktops_before_apply() {
        let mut editor = editor(&[
            (PANEL, Extend, 0, 0),
            (DOCK, Extend, 1200, 0),
            (TV, Extend, 2400, 200),
        ]);
        editor.selected = 1;
        editor.set_role(DOCK, Disabled);
        assert!(editor.dirty);
        assert!(editor.begin_apply().is_none());
        assert_eq!(editor.config(TV).unwrap().x, 2400);
        // Disabled card positions cannot attract or obstruct a drag.
        editor.move_preview(TV, (1204, 203), 1.0);
        editor.settle(TV);
        assert!(editor.can_apply());
        assert_eq!(editor.config(TV).unwrap().x, 1200);
        editor.set_role(DOCK, Extend);
        assert!(editor.can_apply());
        assert_eq!(editor.config(TV).unwrap().x, 1200);
        assert_ne!(
            (
                editor.config(DOCK).unwrap().x,
                editor.config(DOCK).unwrap().y
            ),
            (1200, 0)
        );
    }

    #[test]
    fn mirror_and_disabled_controls_ignore_inapplicable_input_and_discard_disabled_mode_edits() {
        let mut editor = editor(&[(PANEL, Extend, 0, 0), (DOCK, Mirror, 1200, 120)]);
        let higher = outputs::OutputMode {
            pixel_width: 1600,
            ..MODE
        };
        editor.modes.push((DOCK, vec![MODE, higher]));
        editor.selected = 1;
        editor.set_scale(DOCK, 2000);
        editor.set_experience(DOCK, OutputExperience::Tablet);
        editor.move_preview(DOCK, (25, 30), 1.0);
        editor.set_role(PANEL, Disabled); // stale control for another selection
        assert_eq!(editor.config(DOCK).unwrap().scale_milli, 1000);
        assert_eq!(
            editor.config(DOCK).unwrap().experience,
            OutputExperience::Auto
        );
        assert_eq!(editor.config(DOCK).unwrap().x, 1200);
        assert_eq!(editor.config(PANEL).unwrap().role, Extend);
        editor.set_resolution(DOCK, higher);
        assert_eq!(editor.selected_mode_index(), 1);
        assert_eq!(editor.logical_size_label(), "1200 × 800");
        assert_eq!(editor.mode_draft, vec![(DOCK, higher)]);
        editor.set_role(DOCK, Disabled);
        editor.set_resolution(DOCK, higher);
        assert!(editor.mode_draft.is_empty());
        assert_eq!(editor.logical_size_label(), "—");
        let pending = editor.begin_apply().unwrap();
        assert!(pending.modes.is_empty());
        assert!(editor.begin_apply().is_none());
        editor.set_role(DOCK, Extend);
        editor.revert();
        assert_eq!(editor.draft, pending.configuration);
        editor.finish_apply(Some(pending.original), "Apply failed".into());
        assert_eq!(editor.config(DOCK).unwrap().role, Mirror);
        assert_eq!(editor.status, "Apply failed");
        assert!(!editor.dirty);
    }
}
