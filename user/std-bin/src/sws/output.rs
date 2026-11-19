//! Display outputs owned by SWS.
//!
//! SWS drives connected outputs through an [`OutputSet`]. Scanout surfaces,
//! backing stores, present history and presentation clocks live on each
//! [`Output`]; the primary retains legacy screen-size and shell policy APIs.
//! See `docs/graphics/multi-display.md`.
//!
//! Output rectangles are expressed in the compositor's global coordinate
//! space. With one output that space is exactly the primary output's pixel
//! grid at `(0, 0)`, so existing window and cursor coordinates are unchanged.
//!
//! This module is generic over the per-output display target so its geometry
//! and bookkeeping can be tested on the host without a display device.

use sws_protocol::outputs::{self as protocol, OutputConfiguration, OutputDescriptor, OutputExperience, OutputMode, OutputSnapshot, OutputTransaction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConfigurationError { Stale, Invalid }

/// Compositor-assigned output identifier, stable while the output exists.
pub(super) type OutputId = u32;

/// Identifier given to the output opened at startup.
pub(super) const PRIMARY_OUTPUT_ID: OutputId = 0;

/// Axis-aligned output rectangle in global compositor coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OutputRect {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: u32,
    pub(super) height: u32,
}

// Point queries are consumed once SWS places more than one output; until then
// they are exercised by the host tests below.
#[cfg_attr(not(test), allow(dead_code))]
impl OutputRect {
    /// Create a rectangle.
    ///
    /// # Arguments
    ///
    /// * `x` - Left edge in global coordinates.
    /// * `y` - Top edge in global coordinates.
    /// * `width` - Width in pixels.
    /// * `height` - Height in pixels.
    ///
    /// # Returns
    ///
    /// The rectangle.
    pub(super) const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn right(self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    fn bottom(self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }

    /// Whether a global point lies inside this rectangle.
    ///
    /// # Arguments
    ///
    /// * `x` - Global x coordinate.
    /// * `y` - Global y coordinate.
    ///
    /// # Returns
    ///
    /// `true` when the point is inside; empty rectangles contain nothing.
    pub(super) fn contains(self, x: i32, y: i32) -> bool {
        let (x, y) = (i64::from(x), i64::from(y));
        x >= i64::from(self.x) && y >= i64::from(self.y) && x < self.right() && y < self.bottom()
    }

    /// Intersection with another rectangle.
    ///
    /// # Arguments
    ///
    /// * `other` - Rectangle to intersect with.
    ///
    /// # Returns
    ///
    /// The overlapping rectangle, or `None` when they do not overlap.
    pub(super) fn intersect(self, other: OutputRect) -> Option<OutputRect> {
        let left = i64::from(self.x).max(i64::from(other.x));
        let top = i64::from(self.y).max(i64::from(other.y));
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (right > left && bottom > top).then(|| {
            OutputRect::new(
                left as i32,
                top as i32,
                (right - left) as u32,
                (bottom - top) as u32,
            )
        })
    }

    /// Nearest point inside this rectangle.
    ///
    /// # Arguments
    ///
    /// * `x` - Global x coordinate.
    /// * `y` - Global y coordinate.
    ///
    /// # Returns
    ///
    /// The clamped point, or `None` for an empty rectangle.
    pub(super) fn clamp_point(self, x: i32, y: i32) -> Option<(i32, i32)> {
        if self.width == 0 || self.height == 0 {
            return None;
        }
        let max_x = (self.right() - 1) as i32;
        let max_y = (self.bottom() - 1) as i32;
        Some((x.clamp(self.x, max_x), y.clamp(self.y, max_y)))
    }
}

/// A projection from the shared logical/subpixel grid to one native target.
/// The identity case retains the established composition path.
#[derive(Clone, Copy)]
pub(super) struct OutputProjection {
    pub origin: (i32, i32),
    pub scene: (u32, u32),
    pub native: (u32, u32),
}
impl OutputProjection {
    pub fn rect(self, rect: (i32, i32, u32, u32)) -> (i32, i32, u32, u32) {
        if self.scene == self.native { return (rect.0.saturating_sub(self.origin.0), rect.1.saturating_sub(self.origin.1), rect.2, rect.3); }
        let axis = |start: i32, length: u32, origin: i32, scene: u32, native: u32| {
            let denominator = i64::from(scene.max(1));
            let left = (i64::from(start) - i64::from(origin)) * i64::from(native);
            let right = left + i64::from(length) * i64::from(native);
            let left = left.div_euclid(denominator);
            let right = (right + denominator - 1).div_euclid(denominator);
            (left.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32, (right - left).clamp(0, i64::from(u32::MAX)) as u32)
        };
        let (x, width) = axis(rect.0, rect.2, self.origin.0, self.scene.0, self.native.0);
        let (y, height) = axis(rect.1, rect.3, self.origin.1, self.scene.1, self.native.1);
        (x, y, width, height)
    }
    pub fn radius(self, radius: u32) -> u32 { (u64::from(radius) * u64::from(self.native.0)).div_ceil(u64::from(self.scene.0.max(1))).min(u64::from(u32::MAX)) as u32 }
    pub fn clipped(self, rect: (i32, i32, u32, u32)) -> Option<(u32, u32, u32, u32)> {
        let (x, y, width, height) = self.rect(rect);
        let clipped = OutputRect::new(x, y, width, height).intersect(OutputRect::new(0, 0, self.native.0, self.native.1))?;
        Some((clipped.x as u32, clipped.y as u32, clipped.width, clipped.height))
    }
}

/// One display output and the compositor state that belongs to it.
pub(super) struct Output<T> {
    pub(super) id: OutputId,
    /// Firmware/bus device and connector identity. A `session:` prefix marks
    /// old drivers without a persistent hardware identity.
    pub(super) metadata: OutputDescriptor,
    pub(super) logical_rect: protocol::LogicalRect,
    pub(super) experience: OutputExperience,
    pub(super) workarea: Option<(i32, i32, u32, u32)>,
    /// Placement in global compositor coordinates. `width`/`height` are the
    /// output's current pixel size.
    pub(super) rect: OutputRect,
    /// Preferred client buffer scale in thousandths.
    pub(super) scale_milli: u32,
    /// Number of frames this output has accepted. Frame callbacks are paced
    /// against this counter.
    pub(super) present_counter: u64,
    pub(super) restore_focus: Option<u32>,
    pub(super) last_presentation: sws_protocol::workspace::ShellPresentation,
    pub(super) overview_add_selected: bool,
    pub(super) overview_scroll_step_ns: u64,
    /// Display device, backing store and present history for this output.
    pub(super) target: T,
}

impl<T> Output<T> {
    /// Create an output at a global position.
    ///
    /// # Arguments
    ///
    /// * `id` - Compositor-assigned identifier.
    /// * `rect` - Placement and size in global coordinates.
    /// * `scale_milli` - Preferred client buffer scale in thousandths.
    /// * `target` - Display target state.
    ///
    /// # Returns
    ///
    /// The output, with no presented frames.
    pub(super) fn new(id: OutputId, rect: OutputRect, scale_milli: u32, target: T) -> Self {
        Self {
            id,
            metadata: OutputDescriptor { id, connector_key: format!("session:output{}", id), monitor_key: String::new(), identity_is_unique: false, name: format!("Display {}", id),
                mode: OutputMode { pixel_width: rect.width, pixel_height: rect.height, refresh_millihz: 0 }, primary: id == 0, internal: id == 0,
                physical_width_mm: 0, physical_height_mm: 0 },
            logical_rect: protocol::LogicalRect { x: rect.x / 2, y: rect.y / 2,
                width: (u64::from(rect.width) * 1000).div_ceil(u64::from(scale_milli.max(1))) as u32,
                height: (u64::from(rect.height) * 1000).div_ceil(u64::from(scale_milli.max(1))) as u32 },
            experience: OutputExperience::Auto,
            workarea: None,
            rect: OutputRect::new(rect.x, rect.y,
                ((u64::from(rect.width) * 1000).div_ceil(u64::from(scale_milli.max(1))) as u32).saturating_mul(2),
                ((u64::from(rect.height) * 1000).div_ceil(u64::from(scale_milli.max(1))) as u32).saturating_mul(2)),
            scale_milli,
            present_counter: 0,
            restore_focus: None,
            last_presentation: sws_protocol::workspace::ShellPresentation::Workspace,
            overview_add_selected: false,
            overview_scroll_step_ns: 0,
            target,
        }
    }

    pub(super) fn with_connector_key(mut self, connector_key: String) -> Self {
        self.metadata.connector_key = connector_key;
        self
    }

    pub(super) fn with_metadata(mut self, metadata: OutputDescriptor) -> Self {
        self.metadata = metadata;
        self
    }

    pub(super) fn projection(&self) -> OutputProjection { OutputProjection { origin: (self.rect.x, self.rect.y), scene: (self.rect.width, self.rect.height), native: (self.metadata.mode.pixel_width, self.metadata.mode.pixel_height) } }

    pub(super) fn configuration(&self) -> OutputConfiguration {
        OutputConfiguration { id: self.id, x: self.logical_rect.x, y: self.logical_rect.y, scale_milli: self.scale_milli, experience: self.experience }
    }

    /// Output width in pixels.
    pub(super) const fn width(&self) -> u32 {
        self.rect.width
    }

    /// Output height in pixels.
    pub(super) const fn height(&self) -> u32 {
        self.rect.height
    }
}

/// All outputs driven by the compositor. Always contains a primary output.
pub(super) struct OutputSet<T> {
    outputs: Vec<Output<T>>,
    primary: OutputId,
    generation: u32,
}

#[cfg_attr(not(test), allow(dead_code))]
impl<T> OutputSet<T> {
    /// Create a set containing only `primary`.
    ///
    /// # Arguments
    ///
    /// * `primary` - The primary output.
    ///
    /// # Returns
    ///
    /// A one-output set.
    pub(super) fn single(primary: Output<T>) -> Self {
        let id = primary.id;
        Self {
            outputs: vec![primary],
            primary: id,
            generation: 1,
        }
    }

    pub(super) fn snapshot(&self) -> OutputSnapshot {
        OutputSnapshot { generation: self.generation,
            outputs: self.outputs.iter().map(|output| { let mut metadata = output.metadata.clone(); metadata.primary = output.id == self.primary; metadata }).collect(),
            configuration: self.outputs.iter().map(Output::configuration).collect() }
    }
    pub(super) fn validated_configuration(&self, transaction: &OutputTransaction) -> Result<Vec<OutputConfiguration>, ConfigurationError> {
        if transaction.base_generation != self.generation { return Err(ConfigurationError::Stale); }
        protocol::canonical_layout(&self.snapshot().outputs, &transaction.configuration).map_err(|_| ConfigurationError::Invalid)
    }
    /// Recompute a connected layout for a hardware mode change. Keep the
    /// placement where possible, then try snapping the resized output.
    pub(super) fn layout_for_mode(
        &self,
        id: OutputId,
        mode: OutputMode,
    ) -> Result<Vec<OutputConfiguration>, protocol::LayoutError> {
        let mut snapshot = self.snapshot();
        let descriptor = snapshot.outputs.iter_mut().find(|output| output.id == id)
            .ok_or(protocol::LayoutError::MissingOutput)?;
        descriptor.mode = mode;
        let mut configuration = snapshot.configuration;
        if protocol::validate_layout(&snapshot.outputs, &configuration).is_ok() {
            return Ok(configuration);
        }
        let entry = configuration.iter_mut().find(|entry| entry.id == id)
            .ok_or(protocol::LayoutError::MissingOutput)?;
        let desired = (entry.x, entry.y);
        if let Ok((x, y)) = protocol::snapped_position(&snapshot.outputs, &configuration, id, desired) {
            let entry = configuration.iter_mut().find(|entry| entry.id == id).unwrap();
            entry.x = x;
            entry.y = y;
            if let Ok(layout) = protocol::canonical_layout(&snapshot.outputs, &configuration) {
                return Ok(layout);
            }
        }
        // Reconnect a layout that can no longer fit by putting the primary at
        // the origin and arranging the remaining outputs to its right.
        let mut x = 0i32;
        for output in snapshot.outputs.iter().filter(|o| o.primary)
            .chain(snapshot.outputs.iter().filter(|o| !o.primary))
        {
            let entry = configuration.iter_mut().find(|entry| entry.id == output.id).unwrap();
            entry.x = x;
            entry.y = 0;
            let (width, _) = output.mode.logical_size(entry.scale_milli)?;
            let width = i32::try_from(width).map_err(|_| protocol::LayoutError::CoordinatesOverflow)?;
            x = x.checked_add(width).ok_or(protocol::LayoutError::CoordinatesOverflow)?;
        }
        protocol::canonical_layout(&snapshot.outputs, &configuration)
    }

    pub(super) fn apply_configuration(&mut self, configuration: &[OutputConfiguration]) {
        for entry in configuration {
            let output = self.get_mut(entry.id).expect("validated complete output configuration");
            let (width, height) = output.metadata.mode.logical_size(entry.scale_milli).expect("validated scale");
            let previous = output.rect;
            if let Some(area) = output.workarea {
                let left = area.0.saturating_sub(previous.x).max(0) as u32;
                let top = area.1.saturating_sub(previous.y).max(0) as u32;
                let right = previous.width.saturating_sub(left.saturating_add(area.2));
                let bottom = previous.height.saturating_sub(top.saturating_add(area.3));
                output.workarea = Some((entry.x.saturating_mul(2).saturating_add(left as i32),
                    entry.y.saturating_mul(2).saturating_add(top as i32), width.saturating_mul(2).saturating_sub(left.saturating_add(right)).max(1),
                    height.saturating_mul(2).saturating_sub(top.saturating_add(bottom)).max(1)));
            }
            output.logical_rect = protocol::LogicalRect { x: entry.x, y: entry.y, width, height };
            output.rect = OutputRect::new(entry.x.saturating_mul(2), entry.y.saturating_mul(2), width.saturating_mul(2), height.saturating_mul(2));
            output.scale_milli = entry.scale_milli;
            output.experience = entry.experience;
        }
        self.bump_generation();
    }
    pub(super) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Output<T>> { self.outputs.iter_mut() }
    fn bump_generation(&mut self) { self.generation = self.generation.wrapping_add(1).max(1); }

    fn primary_index(&self) -> usize {
        self.outputs
            .iter()
            .position(|output| output.id == self.primary)
            .expect("output set always contains its primary output")
    }

    /// The primary output: the target of legacy whole-screen semantics such
    /// as `SCREEN_SIZE` and unqualified fullscreen.
    pub(super) fn primary(&self) -> &Output<T> {
        &self.outputs[self.primary_index()]
    }

    /// Mutable access to the primary output.
    pub(super) fn primary_mut(&mut self) -> &mut Output<T> {
        let index = self.primary_index();
        &mut self.outputs[index]
    }

    /// Look up an output by identifier.
    ///
    /// # Arguments
    ///
    /// * `id` - Output identifier.
    ///
    /// # Returns
    ///
    /// The output, if present.
    pub(super) fn get(&self, id: OutputId) -> Option<&Output<T>> {
        self.outputs.iter().find(|output| output.id == id)
    }

    /// Mutable lookup by identifier.
    ///
    /// # Arguments
    ///
    /// * `id` - Output identifier.
    ///
    /// # Returns
    ///
    /// The output, if present.
    pub(super) fn get_mut(&mut self, id: OutputId) -> Option<&mut Output<T>> {
        self.outputs.iter_mut().find(|output| output.id == id)
    }

    /// Iterate over all outputs in placement order.
    pub(super) fn iter(&self) -> impl Iterator<Item = &Output<T>> {
        self.outputs.iter()
    }

    /// Identifier of the primary output.
    pub(super) fn primary_id(&self) -> OutputId {
        self.primary
    }

    /// Add an output.
    ///
    /// # Arguments
    ///
    /// * `output` - Output to add; its identifier must be unused.
    ///
    /// # Returns
    ///
    /// `false`, leaving the set unchanged, when the identifier is in use.
    pub(super) fn push(&mut self, output: Output<T>) -> bool {
        if self.get(output.id).is_some() {
            return false;
        }
        self.outputs.push(output);
        self.bump_generation();
        true
    }

    /// Remove a non-primary output.
    ///
    /// # Arguments
    ///
    /// * `id` - Output identifier.
    ///
    /// # Returns
    ///
    /// The removed output, or `None` for the primary or an unknown id.
    pub(super) fn remove(&mut self, id: OutputId) -> Option<Output<T>> {
        if id == self.primary {
            return None;
        }
        let index = self.outputs.iter().position(|output| output.id == id)?;
        self.bump_generation();
        Some(self.outputs.remove(index))
    }

    /// Where to place a new output: to the right of the current layout,
    /// top-aligned with it.
    pub(super) fn next_placement(&self, width: u32, height: u32) -> OutputRect {
        let bounds = self.bounds();
        let right = i64::from(bounds.x) + i64::from(bounds.width);
        OutputRect::new(right as i32, bounds.y, width, height)
    }

    /// Number of outputs.
    pub(super) fn len(&self) -> usize {
        self.outputs.len()
    }

    /// Bounding box of every output rectangle.
    ///
    /// The box may include space no output covers; use
    /// [`OutputSet::clamp_point`] to confine a pointer.
    pub(super) fn bounds(&self) -> OutputRect {
        let mut iter = self.outputs.iter().map(|output| output.rect);
        let first = iter.next().expect("output set is never empty");
        let (mut left, mut top) = (i64::from(first.x), i64::from(first.y));
        let (mut right, mut bottom) = (first.right(), first.bottom());
        for rect in iter {
            left = left.min(i64::from(rect.x));
            top = top.min(i64::from(rect.y));
            right = right.max(rect.right());
            bottom = bottom.max(rect.bottom());
        }
        OutputRect::new(
            left as i32,
            top as i32,
            (right - left) as u32,
            (bottom - top) as u32,
        )
    }

    /// Output containing a global point.
    ///
    /// # Arguments
    ///
    /// * `x` - Global x coordinate.
    /// * `y` - Global y coordinate.
    ///
    /// # Returns
    ///
    /// The first output whose rectangle contains the point.
    pub(super) fn output_at(&self, x: i32, y: i32) -> Option<&Output<T>> {
        self.outputs
            .iter()
            .find(|output| output.rect.contains(x, y))
    }

    /// Confine a global point to the union of output rectangles.
    ///
    /// Points already on an output are returned unchanged. Otherwise the
    /// point moves to the nearest edge of the nearest output, so a pointer can
    /// never rest in space between outputs of an irregular layout.
    ///
    /// # Arguments
    ///
    /// * `x` - Global x coordinate.
    /// * `y` - Global y coordinate.
    ///
    /// # Returns
    ///
    /// The confined point, or the input point when every output is empty.
    pub(super) fn clamp_point(&self, x: i32, y: i32) -> (i32, i32) {
        if self.output_at(x, y).is_some() {
            return (x, y);
        }
        self.outputs
            .iter()
            .filter_map(|output| output.rect.clamp_point(x, y))
            .min_by_key(|&(cx, cy)| {
                let dx = i64::from(cx) - i64::from(x);
                let dy = i64::from(cy) - i64::from(y);
                dx * dx + dy * dy
            })
            .unwrap_or((x, y))
    }
}

#[cfg(test)]
mod tests {
    use super::{Output, OutputRect, OutputSet, PRIMARY_OUTPUT_ID};

    fn set(rects: &[OutputRect]) -> OutputSet<()> {
        let mut outputs = rects
            .iter()
            .enumerate()
            .map(|(index, rect)| Output::new(index as u32, *rect, 1000, ()));
        let mut set = OutputSet::single(outputs.next().unwrap());
        set.outputs.extend(outputs);
        set
    }

    #[test]
    fn primary_hardware_resize_updates_native_and_shell_extents_at_fractional_scale() {
        use sws_protocol::outputs::OutputMode;

        let mut outputs = OutputSet::single(Output::new(
            PRIMARY_OUTPUT_ID,
            OutputRect::new(0, 0, 1280, 800),
            1500,
            (),
        ));
        let before = outputs.snapshot();
        let mode = OutputMode {
            pixel_width: 1024,
            pixel_height: 768,
            refresh_millihz: 60_000,
        };
        let configuration = outputs.layout_for_mode(PRIMARY_OUTPUT_ID, mode).unwrap();
        outputs.primary_mut().metadata.mode = mode;
        outputs.apply_configuration(&configuration);

        let after = outputs.snapshot();
        assert_ne!(after.generation, before.generation);
        assert_eq!(after.outputs[0].mode, mode);
        assert_eq!(outputs.primary().logical_rect.width, 683);
        assert_eq!(outputs.primary().logical_rect.height, 512);
        assert_eq!(outputs.primary().projection().scene, (1366, 1024));
        assert_eq!(outputs.primary().projection().native, (1024, 768));
    }

    #[test]
    fn single_output_bounds_match_its_rect() {
        let set = set(&[OutputRect::new(0, 0, 1920, 1080)]);
        assert_eq!(set.len(), 1);
        assert_eq!(set.primary().id, PRIMARY_OUTPUT_ID);
        assert_eq!(set.bounds(), OutputRect::new(0, 0, 1920, 1080));
    }

    #[test]
    fn single_output_clamp_matches_screen_clamp() {
        let set = set(&[OutputRect::new(0, 0, 1920, 1080)]);
        assert_eq!(set.clamp_point(-5, 500), (0, 500));
        assert_eq!(set.clamp_point(5000, 5000), (1919, 1079));
        assert_eq!(set.clamp_point(10, 20), (10, 20));
    }

    #[test]
    fn output_at_selects_containing_output() {
        let set = set(&[
            OutputRect::new(0, 0, 1920, 1080),
            OutputRect::new(1920, 0, 2560, 1440),
        ]);
        assert_eq!(set.output_at(1919, 0).map(|output| output.id), Some(0));
        assert_eq!(set.output_at(1920, 0).map(|output| output.id), Some(1));
        assert!(set.output_at(100, 1200).is_none());
        assert!(set.get(1).is_some());
        assert!(set.get(2).is_none());
    }

    #[test]
    fn bounds_cover_irregular_layouts() {
        let set = set(&[
            OutputRect::new(0, 360, 1920, 1080),
            OutputRect::new(1920, 0, 2560, 1440),
        ]);
        assert_eq!(set.bounds(), OutputRect::new(0, 0, 4480, 1440));
    }

    #[test]
    fn clamp_moves_points_out_of_dead_space() {
        // The area above the left output is inside the bounding box but on no
        // output; the pointer must land on the nearest real edge.
        let set = set(&[
            OutputRect::new(0, 360, 1920, 1080),
            OutputRect::new(1920, 0, 2560, 1440),
        ]);
        assert_eq!(set.clamp_point(100, 10), (100, 360));
        assert_eq!(set.clamp_point(1900, 10), (1920, 10));
    }

    #[test]
    fn outputs_are_added_to_the_right_and_removed_except_primary() {
        let mut set = set(&[OutputRect::new(0, 0, 1280, 800)]);
        let placement = set.next_placement(1024, 768);
        assert_eq!(placement, OutputRect::new(1280, 0, 1024, 768));
        assert!(set.push(Output::new(7, placement, 1000, ())));
        assert!(!set.push(Output::new(7, placement, 1000, ())));
        assert_eq!(set.len(), 2);
        assert_eq!(set.bounds(), OutputRect::new(0, 0, 2304, 800));
        // The shorter right output leaves dead space below it.
        assert_eq!(set.clamp_point(2000, 790), (2000, 767));
        assert!(set.remove(PRIMARY_OUTPUT_ID).is_none());
        assert_eq!(set.remove(7).map(|output| output.id), Some(7));
        assert_eq!(set.len(), 1);
        assert_eq!(set.clamp_point(2000, 790), (1279, 790));
    }

    #[test]
    fn damage_is_split_by_output_rectangles() {
        let primary = OutputRect::new(0, 0, 1280, 800);
        let secondary = OutputRect::new(1280, 0, 1024, 768);
        let damage = OutputRect::new(1200, 700, 200, 200);
        assert_eq!(
            damage.intersect(primary),
            Some(OutputRect::new(1200, 700, 80, 100))
        );
        assert_eq!(
            damage.intersect(secondary),
            Some(OutputRect::new(1280, 700, 120, 68))
        );
        assert_eq!(OutputRect::new(0, 0, 10, 10).intersect(secondary), None);
    }

    #[test]
    fn empty_rects_contain_nothing() {
        let rect = OutputRect::new(0, 0, 0, 10);
        assert!(!rect.contains(0, 0));
        assert_eq!(rect.clamp_point(0, 0), None);
    }
}
