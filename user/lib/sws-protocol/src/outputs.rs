//! Output identity, logical desktop geometry and atomic arrangement validation.
//!
//! Scanout modes are physical pixels. Positions and desktop rectangles are
//! logical pixels; scaling never changes the native scanout allocation.
use std::{string::String, vec::Vec};

pub type OutputId = u32;
pub const MAX_OUTPUTS: usize = 16;
pub const MIN_SCALE_MILLI: u32 = 250;
pub const MAX_SCALE_MILLI: u32 = 8000;
pub const MAX_CONNECTOR_KEY_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum OutputExperience {
    #[default]
    Auto = 0,
    Desktop = 1,
    Tablet = 2,
}

impl OutputExperience {
    /// Chassis posture applies to the built-in display. External screens keep
    /// their desktop projection unless explicitly configured for tablet use.
    pub const fn tablet_experience(self, internal: bool, tablet_posture: bool) -> bool {
        match self {
            Self::Auto => internal && tablet_posture,
            Self::Desktop => false,
            Self::Tablet => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputMode {
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub refresh_millihz: u32,
}
impl OutputMode {
    pub fn logical_size(self, scale_milli: u32) -> Result<(u32, u32), LayoutError> {
        if !(MIN_SCALE_MILLI..=MAX_SCALE_MILLI).contains(&scale_milli) {
            return Err(LayoutError::InvalidScale);
        }
        if self.pixel_width == 0 || self.pixel_height == 0 {
            return Err(LayoutError::InvalidMode);
        }
        let convert = |pixels: u32| {
            u32::try_from((u64::from(pixels) * 1000).div_ceil(u64::from(scale_milli)))
                .map_err(|_| LayoutError::CoordinatesOverflow)
        };
        Ok((convert(self.pixel_width)?, convert(self.pixel_height)?))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
impl LogicalRect {
    pub fn contains(self, x: i32, y: i32) -> bool {
        i64::from(x) >= i64::from(self.x)
            && i64::from(y) >= i64::from(self.y)
            && i64::from(x) < self.right()
            && i64::from(y) < self.bottom()
    }
    pub const fn right(self) -> i64 {
        self.x as i64 + self.width as i64
    }
    pub const fn bottom(self) -> i64 {
        self.y as i64 + self.height as i64
    }
    pub fn overlaps(self, other: Self) -> bool {
        i64::from(self.x) < other.right()
            && i64::from(other.x) < self.right()
            && i64::from(self.y) < other.bottom()
            && i64::from(other.y) < self.bottom()
    }
    /// Includes corner contact; overlapping interiors are rejected separately.
    pub fn touches(self, other: Self) -> bool {
        ((self.right() == i64::from(other.x) || other.right() == i64::from(self.x))
            && i64::from(self.y) <= other.bottom()
            && i64::from(other.y) <= self.bottom())
            || ((self.bottom() == i64::from(other.y) || other.bottom() == i64::from(self.y))
                && i64::from(self.x) <= other.right()
                && i64::from(other.x) <= self.right())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDescriptor {
    pub id: OutputId,
    /// Stable hardware connector identity, used for persistent profiles.
    pub connector_key: String,
    pub name: String,
    pub mode: OutputMode,
    pub primary: bool,
    pub internal: bool,
    pub physical_width_mm: u32,
    pub physical_height_mm: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputConfiguration {
    pub id: OutputId,
    pub x: i32,
    pub y: i32,
    pub scale_milli: u32,
    pub experience: OutputExperience,
}

/// A clipped rectangle in an output's native scanout image. These values must
/// never be used as global window coordinates or as another output's damage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Conversion at the output boundary. Window placement and hit testing remain
/// logical; backing images and scanout allocations retain their native size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputTransform {
    logical: LogicalRect,
    mode: OutputMode,
    scale_milli: u32,
}

impl OutputTransform {
    pub const fn logical_rect(self) -> LogicalRect {
        self.logical
    }
    pub const fn native_mode(self) -> OutputMode {
        self.mode
    }
    pub const fn scale_milli(self) -> u32 {
        self.scale_milli
    }
    pub fn new(mode: OutputMode, config: OutputConfiguration) -> Result<Self, LayoutError> {
        let (width, height) = mode.logical_size(config.scale_milli)?;
        let logical = LogicalRect {
            x: config.x,
            y: config.y,
            width,
            height,
        };
        if logical.right() > i64::from(i32::MAX) || logical.bottom() > i64::from(i32::MAX) {
            return Err(LayoutError::CoordinatesOverflow);
        }
        Ok(Self {
            logical,
            mode,
            scale_milli: config.scale_milli,
        })
    }

    /// Conservative pixel coverage for damage/scissors: round outward, then
    /// clip to this output's physical extent. Negative global origins are valid.
    pub fn native_rect(self, rect: LogicalRect) -> Option<NativeRect> {
        let left = i64::from(rect.x).max(i64::from(self.logical.x));
        let top = i64::from(rect.y).max(i64::from(self.logical.y));
        let right = rect.right().min(self.logical.right());
        let bottom = rect.bottom().min(self.logical.bottom());
        if right <= left || bottom <= top {
            return None;
        }
        let scale = u64::from(self.scale_milli);
        let x = ((left - i64::from(self.logical.x)) as u64 * scale / 1000)
            .min(u64::from(self.mode.pixel_width));
        let y = ((top - i64::from(self.logical.y)) as u64 * scale / 1000)
            .min(u64::from(self.mode.pixel_height));
        let r = (((right - i64::from(self.logical.x)) as u64 * scale).div_ceil(1000))
            .min(u64::from(self.mode.pixel_width));
        let b = (((bottom - i64::from(self.logical.y)) as u64 * scale).div_ceil(1000))
            .min(u64::from(self.mode.pixel_height));
        (r > x && b > y).then_some(NativeRect {
            x: x as u32,
            y: y as u32,
            width: (r - x) as u32,
            height: (b - y) as u32,
        })
    }

    /// Map an absolute device point on this monitor into the logical desktop.
    /// Touch contacts stay attached to their source output throughout a gesture.
    pub fn logical_point(self, x: u32, y: u32) -> (i32, i32) {
        let x = u64::from(x.min(self.mode.pixel_width - 1)) * 1000 / u64::from(self.scale_milli);
        let y = u64::from(y.min(self.mode.pixel_height - 1)) * 1000 / u64::from(self.scale_milli);
        (
            (i64::from(self.logical.x) + x as i64) as i32,
            (i64::from(self.logical.y) + y as i64) as i32,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSnapshot {
    pub generation: u32,
    pub outputs: Vec<OutputDescriptor>,
    pub configuration: Vec<OutputConfiguration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    NoOutputs,
    TooManyOutputs,
    InvalidConnector,
    MissingPrimary,
    MultiplePrimaries,
    MissingOutput,
    DuplicateOutput,
    InvalidScale,
    InvalidMode,
    CoordinatesOverflow,
    Overlap,
    Disconnected,
}

fn rectangles(
    outputs: &[OutputDescriptor],
    config: &[OutputConfiguration],
) -> Result<(Vec<LogicalRect>, usize), LayoutError> {
    if outputs.is_empty() {
        return Err(LayoutError::NoOutputs);
    }
    if outputs.len() > MAX_OUTPUTS {
        return Err(LayoutError::TooManyOutputs);
    }
    if config.len() != outputs.len() {
        return Err(LayoutError::MissingOutput);
    }
    let mut primary = None;
    let mut result = Vec::with_capacity(outputs.len());
    for (index, output) in outputs.iter().enumerate() {
        if output.connector_key.is_empty()
            || output.connector_key.len() > MAX_CONNECTOR_KEY_BYTES
            || output.connector_key.chars().any(char::is_control)
        {
            return Err(LayoutError::InvalidConnector);
        }
        if outputs[..index]
            .iter()
            .any(|o| o.id == output.id || o.connector_key == output.connector_key)
        {
            return Err(LayoutError::DuplicateOutput);
        }
        if output.primary && primary.replace(index).is_some() {
            return Err(LayoutError::MultiplePrimaries);
        }
        let mut matching = config.iter().filter(|c| c.id == output.id);
        let current = matching.next().ok_or(LayoutError::MissingOutput)?;
        if matching.next().is_some() {
            return Err(LayoutError::DuplicateOutput);
        }
        let (width, height) = output.mode.logical_size(current.scale_milli)?;
        let rect = LogicalRect {
            x: current.x,
            y: current.y,
            width,
            height,
        };
        if rect.right() > i64::from(i32::MAX) || rect.bottom() > i64::from(i32::MAX) {
            return Err(LayoutError::CoordinatesOverflow);
        }
        result.push(rect);
    }
    Ok((result, primary.ok_or(LayoutError::MissingPrimary)?))
}

fn validate_rectangles(rects: &[LogicalRect], primary: usize) -> Result<(), LayoutError> {
    for (i, rect) in rects.iter().enumerate() {
        if rects[..i].iter().any(|other| rect.overlaps(*other)) {
            return Err(LayoutError::Overlap);
        }
    }
    // A single connected component is stronger than requiring every monitor
    // to touch some monitor, which would permit two disconnected islands.
    let mut reached = [false; MAX_OUTPUTS];
    reached[primary] = true;
    loop {
        let mut changed = false;
        for i in 0..rects.len() {
            if reached[i] {
                continue;
            }
            if (0..rects.len()).any(|j| reached[j] && rects[i].touches(rects[j])) {
                reached[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    if reached[..rects.len()].iter().any(|r| !r) {
        return Err(LayoutError::Disconnected);
    }
    Ok(())
}

/// Validate one complete replacement before saving or applying any output.
/// Returned rectangles follow descriptor order, independent of request order.
pub fn validate_layout(
    outputs: &[OutputDescriptor],
    config: &[OutputConfiguration],
) -> Result<Vec<LogicalRect>, LayoutError> {
    let (rects, primary) = rectangles(outputs, config)?;
    validate_rectangles(&rects, primary)?;
    Ok(rects)
}

/// Normalize a complete valid arrangement around its primary without changing
/// any relative placement. Overflow rejects the whole update before mutation.
pub fn canonical_layout(
    outputs: &[OutputDescriptor],
    config: &[OutputConfiguration],
) -> Result<Vec<OutputConfiguration>, LayoutError> {
    let (_, primary) = rectangles(outputs, config)?;
    validate_layout(outputs, config)?;
    let origin = config
        .iter()
        .find(|c| c.id == outputs[primary].id)
        .ok_or(LayoutError::MissingOutput)?;
    let normalized = config
        .iter()
        .map(|entry| {
            Ok(OutputConfiguration {
                x: i32::try_from(i64::from(entry.x) - i64::from(origin.x))
                    .map_err(|_| LayoutError::CoordinatesOverflow)?,
                y: i32::try_from(i64::from(entry.y) - i64::from(origin.y))
                    .map_err(|_| LayoutError::CoordinatesOverflow)?,
                ..*entry
            })
        })
        .collect::<Result<Vec<_>, LayoutError>>()?;
    validate_layout(outputs, &normalized)?;
    Ok(normalized)
}

/// Find the nearest connected non-overlapping placement for a dragged monitor.
/// The caller can preview a free position during dragging and snap on release.
/// Candidate validation uses fixed stack storage and performs no allocation.
pub fn snapped_position(
    outputs: &[OutputDescriptor],
    config: &[OutputConfiguration],
    moving: OutputId,
    desired: (i32, i32),
) -> Result<(i32, i32), LayoutError> {
    let (mut rects, primary) = rectangles(outputs, config)?;
    let index = outputs
        .iter()
        .position(|o| o.id == moving)
        .ok_or(LayoutError::MissingOutput)?;
    if rects.len() == 1 {
        return Ok((
            i64::from(desired.0).min(i64::from(i32::MAX) - i64::from(rects[0].width)) as i32,
            i64::from(desired.1).min(i64::from(i32::MAX) - i64::from(rects[0].height)) as i32,
        ));
    }
    let original = rects[index];
    let mut best: Option<(i128, i32, i32)> = None;
    let mut evaluate = |x: i64, y: i64| {
        let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else {
            return;
        };
        rects[index] = LogicalRect { x, y, ..original };
        if rects[index].right() > i64::from(i32::MAX)
            || rects[index].bottom() > i64::from(i32::MAX)
            || validate_rectangles(&rects, primary).is_err()
        {
            return;
        }
        let dx = i128::from(x) - i128::from(desired.0);
        let dy = i128::from(y) - i128::from(desired.1);
        let candidate = (dx * dx + dy * dy, x, y);
        if best.is_none_or(|old| candidate < old) {
            best = Some(candidate);
        }
    };
    evaluate(i64::from(original.x), i64::from(original.y));
    for (other_index, other) in outputs.iter().enumerate() {
        if other_index == index {
            continue;
        }
        let c = config
            .iter()
            .find(|c| c.id == other.id)
            .ok_or(LayoutError::MissingOutput)?;
        let (w, h) = other.mode.logical_size(c.scale_milli)?;
        let r = LogicalRect {
            x: c.x,
            y: c.y,
            width: w,
            height: h,
        };
        let left = i64::from(r.x) - i64::from(original.width);
        let right = r.right();
        let top = i64::from(r.y) - i64::from(original.height);
        let bottom = r.bottom();
        let x = i64::from(desired.0).clamp(left, right);
        let y = i64::from(desired.1).clamp(top, bottom);
        for edge_x in [left, right] {
            for edge_y in [top, y, bottom] {
                evaluate(edge_x, edge_y);
            }
        }
        for edge_y in [top, bottom] {
            evaluate(x, edge_y);
        }
    }
    best.map(|(_, x, y)| (x, y))
        .ok_or(LayoutError::Disconnected)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn output(id: u32, primary: bool, width: u32, height: u32) -> OutputDescriptor {
        OutputDescriptor {
            id,
            connector_key: std::format!("connector-{id}"),
            name: std::format!("Display {id}"),
            mode: OutputMode {
                pixel_width: width,
                pixel_height: height,
                refresh_millihz: 60000,
            },
            primary,
            internal: primary,
            physical_width_mm: 0,
            physical_height_mm: 0,
        }
    }
    fn at(id: u32, x: i32, y: i32, scale: u32) -> OutputConfiguration {
        OutputConfiguration {
            id,
            x,
            y,
            scale_milli: scale,
            experience: OutputExperience::Auto,
        }
    }
    #[test]
    fn posture_only_changes_the_automatic_internal_display() {
        for posture in [false, true] {
            assert_eq!(
                OutputExperience::Auto.tablet_experience(true, posture),
                posture
            );
            assert!(!OutputExperience::Auto.tablet_experience(false, posture));
            assert!(!OutputExperience::Desktop.tablet_experience(true, posture));
            assert!(OutputExperience::Tablet.tablet_experience(false, posture));
        }
    }

    #[test]
    fn native_damage_is_clipped_and_rounded_outward_at_fractional_scale() {
        let mode = OutputMode {
            pixel_width: 1920,
            pixel_height: 1080,
            refresh_millihz: 60000,
        };
        let transform = OutputTransform::new(mode, at(1, -1280, -720, 1500)).unwrap();
        assert_eq!(
            transform.native_rect(LogicalRect {
                x: -1279,
                y: -719,
                width: 1,
                height: 1
            }),
            Some(NativeRect {
                x: 1,
                y: 1,
                width: 2,
                height: 2
            })
        );
        assert_eq!(
            transform.native_rect(LogicalRect {
                x: -2000,
                y: -2000,
                width: 4000,
                height: 4000
            }),
            Some(NativeRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080
            })
        );
        assert_eq!(
            transform.native_rect(LogicalRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1
            }),
            None
        );
        assert_eq!(transform.logical_point(0, 0), (-1280, -720));
        assert_eq!(transform.logical_point(u32::MAX, u32::MAX), (-1, -1));
    }

    #[test]
    fn rounded_logical_edge_cannot_overrun_the_native_image() {
        let mode = OutputMode {
            pixel_width: 2160,
            pixel_height: 1440,
            refresh_millihz: 60000,
        };
        let transform = OutputTransform::new(mode, at(0, 0, 0, 1750)).unwrap();
        assert_eq!(transform.logical_rect().width, 1235);
        assert_eq!(
            transform.native_rect(transform.logical_rect()),
            Some(NativeRect {
                x: 0,
                y: 0,
                width: 2160,
                height: 1440
            })
        );
        let integer = OutputTransform::new(mode, at(0, 0, 0, 2000)).unwrap();
        assert_eq!(
            integer.native_rect(LogicalRect {
                x: 10,
                y: 20,
                width: 30,
                height: 40
            }),
            Some(NativeRect {
                x: 20,
                y: 40,
                width: 60,
                height: 80
            })
        );
    }

    #[test]
    fn dragging_primary_normalizes_all_positions_without_changing_adjacency() {
        let outputs = [output(0, true, 2160, 1440), output(1, false, 1920, 1080)];
        let normalized =
            canonical_layout(&outputs, &[at(1, -420, 300, 1000), at(0, 1500, 300, 2000)]).unwrap();
        assert_eq!(normalized, [at(1, -1920, 0, 1000), at(0, 0, 0, 2000)]);
    }
    #[test]
    fn topology_uses_logical_size_at_each_monitors_scale() {
        let outputs = [output(0, true, 2160, 1440), output(1, false, 1920, 1080)];
        let rects = validate_layout(&outputs, &[at(0, 0, 0, 2000), at(1, 1080, 0, 1000)]).unwrap();
        assert_eq!((rects[0].width, rects[0].height), (1080, 720));
        assert_eq!((rects[1].width, rects[1].height), (1920, 1080));
        assert_eq!(
            validate_layout(&outputs, &[at(0, 0, 0, 2000), at(1, 2160, 0, 1000)]),
            Err(LayoutError::Disconnected)
        );
    }
    #[test]
    fn left_above_and_corner_contact_are_valid_but_overlap_is_not() {
        let outputs = [output(0, true, 100, 100), output(1, false, 100, 100)];
        for (x, y) in [(-100, 0), (0, -100), (-100, -100), (100, 100)] {
            assert!(validate_layout(&outputs, &[at(0, 0, 0, 1000), at(1, x, y, 1000)]).is_ok());
        }
        assert_eq!(
            validate_layout(&outputs, &[at(0, 0, 0, 1000), at(1, 99, 99, 1000)]),
            Err(LayoutError::Overlap)
        );
    }
    #[test]
    fn two_locally_connected_islands_are_rejected() {
        let outputs = [
            output(0, true, 100, 100),
            output(1, false, 100, 100),
            output(2, false, 100, 100),
            output(3, false, 100, 100),
        ];
        let configs = [
            at(0, 0, 0, 1000),
            at(1, 100, 0, 1000),
            at(2, 500, 0, 1000),
            at(3, 600, 0, 1000),
        ];
        assert_eq!(
            validate_layout(&outputs, &configs),
            Err(LayoutError::Disconnected)
        );
    }
    #[test]
    fn snap_finds_edges_and_corners_with_negative_coordinates() {
        let outputs = [output(0, true, 100, 100), output(1, false, 100, 100)];
        let configs = [at(0, 0, 0, 1000), at(1, 100, 0, 1000)];
        for (desired, expected) in [
            ((-130, 15), (-100, 15)),
            ((130, 150), (100, 100)),
            ((20, -160), (20, -100)),
        ] {
            assert_eq!(
                snapped_position(&outputs, &configs, 1, desired),
                Ok(expected)
            );
        }
    }
    #[test]
    fn moving_a_bridge_cannot_disconnect_the_remaining_outputs() {
        let outputs = [
            output(0, true, 100, 100),
            output(1, false, 100, 100),
            output(2, false, 100, 100),
        ];
        let mut configs = [at(0, 0, 0, 1000), at(1, 100, 0, 1000), at(2, 200, 0, 1000)];
        let (x, y) = snapped_position(&outputs, &configs, 1, (100, -500)).unwrap();
        configs[1].x = x;
        configs[1].y = y;
        assert!(validate_layout(&outputs, &configs).is_ok());
    }
    #[test]
    fn invalid_requests_are_rejected_before_geometry_changes() {
        let outputs = [output(0, true, 100, 100), output(1, false, 100, 100)];
        assert_eq!(
            validate_layout(&outputs, &[at(0, 0, 0, 1000), at(0, 100, 0, 1000)]),
            Err(LayoutError::DuplicateOutput)
        );
        assert_eq!(
            validate_layout(&outputs, &[at(0, 0, 0, 1000), at(1, 100, 0, 0)]),
            Err(LayoutError::InvalidScale)
        );
        assert_eq!(
            validate_layout(&outputs, &[at(0, 0, 0, 1000), at(1, i32::MAX, 0, 1000)]),
            Err(LayoutError::CoordinatesOverflow)
        );
        assert_eq!(outputs[0].mode.logical_size(1500), Ok((67, 67)));
    }
}
