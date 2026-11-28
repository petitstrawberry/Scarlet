//! Output identity, logical desktop geometry and atomic arrangement validation.
//!
//! Scanout modes are physical pixels. Positions and desktop rectangles are
//! logical pixels; scaling never changes the native scanout allocation.
//!
//! With `OUTPUT_ROLES`, Get (11) and Apply (12) opt a connection into full
//! connector snapshots for both replies and notifications. Snapshot event 0
//! and transaction bodies start with generation and count. Bit 31 of count
//! selects six little-endian words per configuration (id, x, y, scale_milli,
//! experience, role). Snapshot descriptors still precede the configurations.
//! Legacy Get (0) and Apply (1) use the original five-word records. Send
//! [`encode_legacy_event`] until opt-in, and merge an [`OutputRequest::ApplyLegacy`]
//! against the same generation before validation.
//!
//! Each monitor's TOML profile entry stores its last extend x/y and a `role`
//! string (`extend`, `mirror`, `disabled`). A missing role means `extend`.
use std::{string::String, vec::Vec};

pub type OutputId = u32;
/// SWS stores logical positions at half-pixel precision. Wire arrangements
/// remain ordinary logical pixels; scanout and client images are physical.
pub const COMPOSITOR_SCALE_MILLI: u32 = 2000;
pub const MAX_OUTPUTS: usize = 16;
pub const MIN_SCALE_MILLI: u32 = 250;
pub const MAX_SCALE_MILLI: u32 = 8000;
pub const MAX_CONNECTOR_KEY_BYTES: usize = 128;

/// Stable numeric role values on the wire.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum OutputRole {
    #[default]
    Extend = 0,
    /// Reproduce the internal extended desktop, or the active extended primary.
    Mirror = 1,
    Disabled = 2,
}

impl TryFrom<u32> for OutputRole {
    type Error = crate::ProtocolError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Extend),
            1 => Ok(Self::Mirror),
            2 => Ok(Self::Disabled),
            _ => Err(crate::ProtocolError::MalformedPayload),
        }
    }
}

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
    /// Display identity is independent of its transport/connector.
    pub monitor_key: String,
    pub identity_is_unique: bool,
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
    /// Saved extend position, including while mirroring or disabled.
    pub x: i32,
    pub y: i32,
    pub scale_milli: u32,
    pub experience: OutputExperience,
    pub role: OutputRole,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputTransaction {
    pub base_generation: u32,
    pub configuration: Vec<OutputConfiguration>,
}

impl OutputTransaction {
    /// Reconstruct a complete transaction from a legacy extended-desktop view.
    /// The server checks the base generation against this snapshot before merging,
    /// then applies normal layout validation to the result.
    pub fn merge_legacy(&self, current: &OutputSnapshot) -> Result<Self, LayoutError> {
        let extended = current
            .configuration
            .iter()
            .filter(|c| c.role == OutputRole::Extend);
        if self.configuration.len() != extended.count() {
            return Err(LayoutError::MissingOutput);
        }
        // Legacy clients may normalize around the primary advertised in their
        // snapshot. Anchor the visible group at that connector's live position
        // so its geometry stays in the same coordinate system as parked outputs.
        let primary = extended_primary(&current.outputs, &current.configuration)
            .ok_or(LayoutError::NoExtendedOutput)?;
        let live = current
            .configuration
            .iter()
            .find(|c| c.id == primary)
            .ok_or(LayoutError::MissingOutput)?;
        let submitted = self
            .configuration
            .iter()
            .find(|c| c.id == primary)
            .ok_or(LayoutError::MissingOutput)?;
        let dx = i64::from(live.x) - i64::from(submitted.x);
        let dy = i64::from(live.y) - i64::from(submitted.y);
        let mut configuration = current.configuration.clone();
        for entry in &mut configuration {
            if entry.role == OutputRole::Extend {
                let update = self
                    .configuration
                    .iter()
                    .find(|c| c.id == entry.id)
                    .ok_or(LayoutError::MissingOutput)?;
                *entry = OutputConfiguration {
                    x: i32::try_from(i64::from(update.x) + dx)
                        .map_err(|_| LayoutError::CoordinatesOverflow)?,
                    y: i32::try_from(i64::from(update.y) + dy)
                        .map_err(|_| LayoutError::CoordinatesOverflow)?,
                    role: entry.role,
                    ..*update
                };
            }
        }
        Ok(Self {
            base_generation: self.base_generation,
            configuration,
        })
    }
}

/// A connection's initial output metrics. Native dimensions are physical;
/// desktop origin and extent are logical. They are negotiated before creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputContext {
    pub id: OutputId,
    pub scale_milli: u32,
    pub mode: OutputMode,
    pub rect: LogicalRect,
    pub tablet: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputRequest {
    /// Opt in to snapshots containing all connectors and their roles.
    Get,
    Apply(OutputTransaction),
    /// Legacy peers can see and update the extended portion of the desktop.
    GetLegacy,
    ApplyLegacy(OutputTransaction),
    Workspace(OutputId),
    ApplyWorkspace {
        output: OutputId,
        transaction: crate::workspace::WorkspaceTransaction,
    },
    Context(OutputId),
    Workarea {
        output: OutputId,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    Bind {
        window: u32,
        output: OutputId,
    },
    Modes(OutputId),
    SetMode {
        output: OutputId,
        mode: OutputMode,
    },
    GetBacklight,
    SetBacklight(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputEvent {
    Snapshot(OutputSnapshot),
    Context(OutputContext),
    /// Native buffer extent plus authoritative logical surface extent. Sending
    /// both avoids fractional-scale round trips growing a window by a pixel.
    Window {
        window: u32,
        output: OutputId,
        scale_milli: u32,
        width: u32,
        height: u32,
        logical_width: u32,
        logical_height: u32,
        output_x: i32,
        output_y: i32,
    },
    Workspace {
        output: OutputId,
        state: crate::workspace::WorkspaceState,
    },
    Applied,
    Modes {
        output: OutputId,
        modes: Vec<OutputMode>,
    },
    Backlight(u8),
}

pub fn encode_request(request: &OutputRequest) -> Vec<u8> {
    let mut out = Vec::new();
    match request {
        OutputRequest::Modes(output) => {
            put(&mut out, 7);
            put(&mut out, *output);
        }
        OutputRequest::SetMode { output, mode } => {
            for word in [
                8,
                *output,
                mode.pixel_width,
                mode.pixel_height,
                mode.refresh_millihz,
            ] {
                put(&mut out, word);
            }
        }
        OutputRequest::GetLegacy => put(&mut out, 0),
        OutputRequest::Get => put(&mut out, 11),
        OutputRequest::GetBacklight => put(&mut out, 9),
        OutputRequest::SetBacklight(percent) => {
            put(&mut out, 10);
            put(&mut out, u32::from(*percent));
        }
        OutputRequest::Apply(transaction) => {
            put(&mut out, 12);
            out.extend(encode_transaction(transaction));
        }
        OutputRequest::ApplyLegacy(transaction) => {
            put(&mut out, 1);
            out.extend(transaction_bytes(transaction, ConfigFormat::Legacy));
        }
        OutputRequest::Workspace(id) => {
            put(&mut out, 2);
            put(&mut out, *id);
        }
        OutputRequest::ApplyWorkspace {
            output,
            transaction,
        } => {
            put(&mut out, 3);
            put(&mut out, *output);
            out.extend(crate::workspace::encode_transaction(transaction));
        }
        OutputRequest::Context(id) => {
            put(&mut out, 4);
            put(&mut out, *id);
        }
        OutputRequest::Workarea {
            output,
            x,
            y,
            width,
            height,
        } => {
            for word in [5, *output, *x as u32, *y as u32, *width, *height] {
                put(&mut out, word);
            }
        }
        OutputRequest::Bind { window, output } => {
            for word in [6, *window, *output] {
                put(&mut out, word);
            }
        }
    }
    out
}

pub fn decode_request(bytes: &[u8]) -> Result<OutputRequest, crate::ProtocolError> {
    let mut r = Reader { bytes, offset: 0 };
    let request = match r.word()? {
        0 => OutputRequest::GetLegacy,
        11 => OutputRequest::Get,
        opcode @ (1 | 12) => {
            let (value, format) = r.transaction()?;
            match (opcode, format) {
                (1, ConfigFormat::Legacy) => OutputRequest::ApplyLegacy(value),
                (12, ConfigFormat::Roles) => OutputRequest::Apply(value),
                _ => return Err(crate::ProtocolError::MalformedPayload),
            }
        }
        2 => OutputRequest::Workspace(r.word()?),
        3 => {
            let output = r.word()?;
            let transaction = crate::workspace::decode_transaction(&bytes[r.offset..])?;
            r.offset = bytes.len();
            OutputRequest::ApplyWorkspace {
                output,
                transaction,
            }
        }
        4 => OutputRequest::Context(r.word()?),
        5 => OutputRequest::Workarea {
            output: r.word()?,
            x: r.word()? as i32,
            y: r.word()? as i32,
            width: r.word()?,
            height: r.word()?,
        },
        6 => OutputRequest::Bind {
            window: r.word()?,
            output: r.word()?,
        },
        7 => OutputRequest::Modes(r.word()?),
        8 => OutputRequest::SetMode {
            output: r.word()?,
            mode: OutputMode {
                pixel_width: r.word()?,
                pixel_height: r.word()?,
                refresh_millihz: r.word()?,
            },
        },
        9 => OutputRequest::GetBacklight,
        10 => OutputRequest::SetBacklight(r.percent()?),
        _ => return Err(crate::ProtocolError::MalformedPayload),
    };
    r.finish()?;
    Ok(request)
}

pub fn encode_event(event: &OutputEvent) -> Vec<u8> {
    let mut out = Vec::new();
    match event {
        OutputEvent::Snapshot(snapshot) => {
            put(&mut out, 0);
            out.extend(encode_snapshot(snapshot));
        }
        OutputEvent::Context(c) => {
            for word in [
                1,
                c.id,
                c.scale_milli,
                c.mode.pixel_width,
                c.mode.pixel_height,
                c.mode.refresh_millihz,
                c.rect.x as u32,
                c.rect.y as u32,
                c.rect.width,
                c.rect.height,
                u32::from(c.tablet),
            ] {
                put(&mut out, word);
            }
        }
        OutputEvent::Window {
            window,
            output,
            scale_milli,
            width,
            height,
            logical_width,
            logical_height,
            output_x,
            output_y,
        } => {
            for word in [
                2,
                *window,
                *output,
                *scale_milli,
                *width,
                *height,
                *logical_width,
                *logical_height,
                *output_x as u32,
                *output_y as u32,
            ] {
                put(&mut out, word);
            }
        }
        OutputEvent::Workspace { output, state } => {
            put(&mut out, 3);
            put(&mut out, *output);
            out.extend(crate::workspace::encode_state(state));
        }
        OutputEvent::Applied => put(&mut out, 4),
        OutputEvent::Backlight(percent) => {
            put(&mut out, 6);
            put(&mut out, u32::from(*percent));
        }
        OutputEvent::Modes { output, modes } => {
            for word in [5, *output, modes.len() as u32] {
                put(&mut out, word);
            }
            for mode in modes {
                for word in [mode.pixel_width, mode.pixel_height, mode.refresh_millihz] {
                    put(&mut out, word);
                }
            }
        }
    }
    out
}

/// Encode replies and notifications for clients that have never requested roles.
/// Their parser validates every advertised rectangle as part of one desktop.
pub fn encode_legacy_event(event: &OutputEvent) -> Vec<u8> {
    let OutputEvent::Snapshot(snapshot) = event else {
        return encode_event(event);
    };
    let primary = extended_primary(&snapshot.outputs, &snapshot.configuration);
    let mut desktop = snapshot.clone();
    desktop
        .configuration
        .retain(|c| c.role == OutputRole::Extend);
    desktop
        .outputs
        .retain(|d| desktop.configuration.iter().any(|c| c.id == d.id));
    for output in &mut desktop.outputs {
        output.primary = Some(output.id) == primary;
    }
    let mut bytes = Vec::new();
    put(&mut bytes, 0);
    bytes.extend(snapshot_bytes(&desktop, ConfigFormat::Legacy));
    bytes
}

pub fn decode_event(bytes: &[u8]) -> Result<OutputEvent, crate::ProtocolError> {
    let mut r = Reader { bytes, offset: 0 };
    let event = match r.word()? {
        0 => {
            let snapshot = decode_snapshot(&bytes[r.offset..])?;
            r.offset = bytes.len();
            OutputEvent::Snapshot(snapshot)
        }
        1 => {
            let id = r.word()?;
            let scale_milli = r.word()?;
            let mode = OutputMode {
                pixel_width: r.word()?,
                pixel_height: r.word()?,
                refresh_millihz: r.word()?,
            };
            let rect = LogicalRect {
                x: r.word()? as i32,
                y: r.word()? as i32,
                width: r.word()?,
                height: r.word()?,
            };
            let tablet = match r.word()? {
                0 => false,
                1 => true,
                _ => return Err(crate::ProtocolError::MalformedPayload),
            };
            if mode.logical_size(scale_milli).ok() != Some((rect.width, rect.height)) {
                return Err(crate::ProtocolError::MalformedPayload);
            }
            OutputEvent::Context(OutputContext {
                id,
                scale_milli,
                mode,
                rect,
                tablet,
            })
        }
        2 => {
            let window = r.word()?;
            let output = r.word()?;
            let scale_milli = r.word()?;
            let width = r.word()?;
            let height = r.word()?;
            let logical_width = r.word()?;
            let logical_height = r.word()?;
            let output_x = r.word()? as i32;
            let output_y = r.word()? as i32;
            if !(MIN_SCALE_MILLI..=MAX_SCALE_MILLI).contains(&scale_milli)
                || width == 0
                || height == 0
                || logical_width == 0
                || logical_height == 0
            {
                return Err(crate::ProtocolError::MalformedPayload);
            }
            OutputEvent::Window {
                window,
                output,
                scale_milli,
                width,
                height,
                logical_width,
                logical_height,
                output_x,
                output_y,
            }
        }
        3 => {
            let output = r.word()?;
            let state = crate::workspace::decode_state(&bytes[r.offset..])?;
            r.offset = bytes.len();
            OutputEvent::Workspace { output, state }
        }
        4 => OutputEvent::Applied,
        5 => {
            let output = r.word()?;
            let count = r.word()?;
            if count > 256 {
                return Err(crate::ProtocolError::MalformedPayload);
            }
            let mut modes = Vec::new();
            for _ in 0..count {
                let mode = OutputMode {
                    pixel_width: r.word()?,
                    pixel_height: r.word()?,
                    refresh_millihz: r.word()?,
                };
                mode.logical_size(1000)
                    .map_err(|_| crate::ProtocolError::MalformedPayload)?;
                modes.push(mode);
            }
            OutputEvent::Modes { output, modes }
        }
        6 => OutputEvent::Backlight(r.percent()?),
        _ => return Err(crate::ProtocolError::MalformedPayload),
    };
    r.finish()?;
    Ok(event)
}

fn put(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

// The high count bit tags six-word records even when every role is Extend.
// Untagged five-word records keep their original byte representation. Tagging
// the format also makes truncated role-aware payloads unambiguously invalid.
const ROLE_COUNT_FLAG: u32 = 1 << 31;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConfigFormat {
    Legacy,
    Roles,
}

impl ConfigFormat {
    fn count_word(self, count: usize) -> u32 {
        match self {
            Self::Legacy => count as u32,
            Self::Roles => count as u32 | ROLE_COUNT_FLAG,
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    fn percent(&mut self) -> Result<u8, crate::ProtocolError> {
        let percent = self.word()?;
        if percent > 100 {
            return Err(crate::ProtocolError::MalformedPayload);
        }
        Ok(percent as u8)
    }

    fn word(&mut self) -> Result<u32, crate::ProtocolError> {
        let bytes = self
            .bytes
            .get(self.offset..self.offset + 4)
            .ok_or(crate::ProtocolError::MalformedPayload)?;
        self.offset += 4;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    }
    fn string(&mut self) -> Result<String, crate::ProtocolError> {
        let count = self.word()? as usize;
        if count > MAX_CONNECTOR_KEY_BYTES {
            return Err(crate::ProtocolError::MalformedPayload);
        }
        let bytes = self
            .bytes
            .get(self.offset..self.offset + count)
            .ok_or(crate::ProtocolError::MalformedPayload)?;
        self.offset += count;
        let text =
            core::str::from_utf8(bytes).map_err(|_| crate::ProtocolError::MalformedPayload)?;
        if text.chars().any(char::is_control) {
            return Err(crate::ProtocolError::MalformedPayload);
        }
        Ok(String::from(text))
    }
    fn config_count(&mut self) -> Result<(usize, ConfigFormat), crate::ProtocolError> {
        let word = self.word()?;
        let count = (word & !ROLE_COUNT_FLAG) as usize;
        if count == 0 || count > MAX_OUTPUTS {
            return Err(crate::ProtocolError::MalformedPayload);
        }
        let format = if word & ROLE_COUNT_FLAG == 0 {
            ConfigFormat::Legacy
        } else {
            ConfigFormat::Roles
        };
        Ok((count, format))
    }
    fn configs(
        &mut self,
        count: usize,
        format: ConfigFormat,
    ) -> Result<Vec<OutputConfiguration>, crate::ProtocolError> {
        let mut configs = Vec::with_capacity(count);
        for _ in 0..count {
            configs.push(OutputConfiguration {
                id: self.word()?,
                x: self.word()? as i32,
                y: self.word()? as i32,
                scale_milli: self.word()?,
                experience: match self.word()? {
                    0 => OutputExperience::Auto,
                    1 => OutputExperience::Desktop,
                    2 => OutputExperience::Tablet,
                    _ => return Err(crate::ProtocolError::MalformedPayload),
                },
                role: match format {
                    ConfigFormat::Legacy => OutputRole::Extend,
                    ConfigFormat::Roles => OutputRole::try_from(self.word()?)?,
                },
            });
        }
        Ok(configs)
    }
    fn transaction(&mut self) -> Result<(OutputTransaction, ConfigFormat), crate::ProtocolError> {
        let base_generation = self.word()?;
        let (count, format) = self.config_count()?;
        let configuration = self.configs(count, format)?;
        Ok((
            OutputTransaction {
                base_generation,
                configuration,
            },
            format,
        ))
    }
    fn finish(&self) -> Result<(), crate::ProtocolError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(crate::ProtocolError::MalformedPayload)
        }
    }
}

fn encode_configs(out: &mut Vec<u8>, configs: &[OutputConfiguration], format: ConfigFormat) {
    for c in configs {
        for word in [
            c.id,
            c.x as u32,
            c.y as u32,
            c.scale_milli,
            c.experience as u32,
        ] {
            put(out, word);
        }
        if format == ConfigFormat::Roles {
            put(out, c.role as u32);
        }
    }
}

pub fn encode_transaction(transaction: &OutputTransaction) -> Vec<u8> {
    transaction_bytes(transaction, ConfigFormat::Roles)
}

fn transaction_bytes(transaction: &OutputTransaction, format: ConfigFormat) -> Vec<u8> {
    let mut out = Vec::new();
    put(&mut out, transaction.base_generation);
    put(&mut out, format.count_word(transaction.configuration.len()));
    encode_configs(&mut out, &transaction.configuration, format);
    out
}
/// Read a standalone transaction body in either format. Server dispatch uses
/// [`decode_request`] to distinguish legacy writes from explicit role changes.
pub fn decode_transaction(bytes: &[u8]) -> Result<OutputTransaction, crate::ProtocolError> {
    let mut reader = Reader { bytes, offset: 0 };
    let (transaction, _) = reader.transaction()?;
    reader.finish()?;
    Ok(transaction)
}

pub fn encode_snapshot(snapshot: &OutputSnapshot) -> Vec<u8> {
    snapshot_bytes(snapshot, ConfigFormat::Roles)
}

fn snapshot_bytes(snapshot: &OutputSnapshot, format: ConfigFormat) -> Vec<u8> {
    let mut out = Vec::new();
    put(&mut out, snapshot.generation);
    put(&mut out, format.count_word(snapshot.outputs.len()));
    for d in &snapshot.outputs {
        for word in [
            d.id,
            u32::from(d.primary)
                | (u32::from(d.internal) << 1)
                | (u32::from(d.identity_is_unique) << 2),
            d.mode.pixel_width,
            d.mode.pixel_height,
            d.mode.refresh_millihz,
            d.physical_width_mm,
            d.physical_height_mm,
        ] {
            put(&mut out, word);
        }
        for text in [&d.connector_key, &d.monitor_key, &d.name] {
            put(&mut out, text.len() as u32);
            out.extend_from_slice(text.as_bytes());
        }
    }
    encode_configs(&mut out, &snapshot.configuration, format);
    out
}
pub fn decode_snapshot(bytes: &[u8]) -> Result<OutputSnapshot, crate::ProtocolError> {
    let mut reader = Reader { bytes, offset: 0 };
    let generation = reader.word()?;
    let (count, format) = reader.config_count()?;
    let mut outputs = Vec::with_capacity(count);
    for _ in 0..count {
        let id = reader.word()?;
        let flags = reader.word()?;
        if flags & !7 != 0 {
            return Err(crate::ProtocolError::MalformedPayload);
        }
        let mode = OutputMode {
            pixel_width: reader.word()?,
            pixel_height: reader.word()?,
            refresh_millihz: reader.word()?,
        };
        let physical_width_mm = reader.word()?;
        let physical_height_mm = reader.word()?;
        let connector_key = reader.string()?;
        let monitor_key = reader.string()?;
        let name = reader.string()?;
        outputs.push(OutputDescriptor {
            id,
            connector_key,
            monitor_key,
            identity_is_unique: flags & 4 != 0,
            name,
            mode,
            primary: flags & 1 != 0,
            internal: flags & 2 != 0,
            physical_width_mm,
            physical_height_mm,
        });
    }
    let configuration = reader.configs(count, format)?;
    reader.finish()?;
    validate_layout(&outputs, &configuration)
        .map_err(|_| crate::ProtocolError::MalformedPayload)?;
    Ok(OutputSnapshot {
        generation,
        outputs,
        configuration,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    NoOutputs,
    NoExtendedOutput,
    NotExtended,
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

/// The mirror source and preferred desktop for clients using the legacy view.
/// Ties follow connector enumeration order.
pub fn extended_primary(
    outputs: &[OutputDescriptor],
    config: &[OutputConfiguration],
) -> Option<OutputId> {
    outputs
        .iter()
        .filter(|output| {
            config
                .iter()
                .any(|c| c.id == output.id && c.role == OutputRole::Extend)
        })
        .min_by_key(|output| (!output.internal, !output.primary))
        .map(|output| output.id)
}

struct LayoutRects {
    all: Vec<LogicalRect>,
    extended: Vec<usize>,
    primary: usize,
}

fn rectangles(
    outputs: &[OutputDescriptor],
    config: &[OutputConfiguration],
) -> Result<LayoutRects, LayoutError> {
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
    let mut extended = Vec::new();
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
        if current.role == OutputRole::Extend {
            extended.push(index);
        }
        result.push(OutputTransform::new(output.mode, *current)?.logical_rect());
    }
    if extended.is_empty() {
        return Err(LayoutError::NoExtendedOutput);
    }
    Ok(LayoutRects {
        all: result,
        extended,
        primary: primary.ok_or(LayoutError::MissingPrimary)?,
    })
}

fn validate_rectangles(rects: &[LogicalRect], extended: &[usize]) -> Result<(), LayoutError> {
    for (i, &index) in extended.iter().enumerate() {
        if extended[..i]
            .iter()
            .any(|&other| rects[index].overlaps(rects[other]))
        {
            return Err(LayoutError::Overlap);
        }
    }
    // A single connected component is stronger than requiring every monitor
    // to touch some monitor, which would permit two disconnected islands.
    let mut reached = [false; MAX_OUTPUTS];
    reached[extended[0]] = true;
    loop {
        let mut changed = false;
        for &i in extended {
            if reached[i] {
                continue;
            }
            if extended
                .iter()
                .any(|&j| reached[j] && rects[i].touches(rects[j]))
            {
                reached[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    if extended.iter().any(|&i| !reached[i]) {
        return Err(LayoutError::Disconnected);
    }
    Ok(())
}

/// Validate one complete replacement before saving or applying any output.
/// Returned rectangles include saved extend positions for every connector in
/// descriptor order. Only extended outputs participate in desktop topology.
pub fn validate_layout(
    outputs: &[OutputDescriptor],
    config: &[OutputConfiguration],
) -> Result<Vec<LogicalRect>, LayoutError> {
    let rects = rectangles(outputs, config)?;
    validate_rectangles(&rects.all, &rects.extended)?;
    Ok(rects.all)
}

/// Stable topology key consists solely of monitor identities, independent of
/// enumeration order and ports. Unknown or duplicated identities are ambiguous
/// and must not cause a saved arrangement to be silently restored.
pub fn topology_key(outputs: &[OutputDescriptor]) -> Option<Vec<String>> {
    if outputs.is_empty() || outputs.len() > MAX_OUTPUTS {
        return None;
    }
    if outputs
        .iter()
        .any(|output| !output.identity_is_unique || output.monitor_key.is_empty())
    {
        return None;
    }
    let mut keys = outputs
        .iter()
        .map(|output| output.monitor_key.clone())
        .collect::<Vec<_>>();
    keys.sort();
    if keys.windows(2).any(|pair| pair[0] == pair[1]) {
        return None;
    }
    Some(keys)
}

/// Normalize around the hardware primary even while it is docked and disabled.
/// Changing a role alone therefore preserves every connector's extend position.
pub fn canonical_layout(
    outputs: &[OutputDescriptor],
    config: &[OutputConfiguration],
) -> Result<Vec<OutputConfiguration>, LayoutError> {
    let rects = rectangles(outputs, config)?;
    validate_rectangles(&rects.all, &rects.extended)?;
    let origin = config
        .iter()
        .find(|c| c.id == outputs[rects.primary].id)
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
    let LayoutRects {
        all: mut rects,
        extended,
        ..
    } = rectangles(outputs, config)?;
    let index = outputs
        .iter()
        .position(|o| o.id == moving)
        .ok_or(LayoutError::MissingOutput)?;
    if !extended.contains(&index) {
        return Err(LayoutError::NotExtended);
    }
    if extended.len() == 1 {
        return Ok((
            i64::from(desired.0).min(i64::from(i32::MAX) - i64::from(rects[index].width)) as i32,
            i64::from(desired.1).min(i64::from(i32::MAX) - i64::from(rects[index].height)) as i32,
        ));
    }
    let original = rects[index];
    let mut best: Option<(i128, i32, i32)> = None;
    let mut evaluate = |rects: &mut [LogicalRect], x: i64, y: i64| {
        let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else {
            return;
        };
        rects[index] = LogicalRect { x, y, ..original };
        if rects[index].right() > i64::from(i32::MAX)
            || rects[index].bottom() > i64::from(i32::MAX)
            || validate_rectangles(rects, &extended).is_err()
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
    evaluate(&mut rects, i64::from(original.x), i64::from(original.y));
    for &other_index in &extended {
        if other_index == index {
            continue;
        }
        let r = rects[other_index];
        let left = i64::from(r.x) - i64::from(original.width);
        let right = r.right();
        let top = i64::from(r.y) - i64::from(original.height);
        let bottom = r.bottom();
        let x = i64::from(desired.0).clamp(left, right);
        let y = i64::from(desired.1).clamp(top, bottom);
        for edge_x in [left, right] {
            for edge_y in [top, y, bottom] {
                evaluate(&mut rects, edge_x, edge_y);
            }
        }
        for edge_y in [top, bottom] {
            evaluate(&mut rects, x, edge_y);
        }
    }
    best.map(|(_, x, y)| (x, y))
        .ok_or(LayoutError::Disconnected)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractional_window_configure_keeps_logical_extent_and_negative_origin() {
        // ceil(321 * 1.25) = 402; deriving the logical size by rounding up
        // again would incorrectly configure 322 pixels on each round trip.
        let event = OutputEvent::Window {
            window: 7,
            output: 2,
            scale_milli: 1250,
            width: 402,
            height: 250,
            logical_width: 321,
            logical_height: 200,
            output_x: -1536,
            output_y: -240,
        };
        let encoded = encode_event(&event);
        assert_eq!(decode_event(&encoded).unwrap(), event);
        assert!(decode_event(&encoded[..encoded.len() - 1]).is_err());
    }
    fn output(id: u32, primary: bool, width: u32, height: u32) -> OutputDescriptor {
        OutputDescriptor {
            id,
            connector_key: std::format!("connector-{id}"),
            monitor_key: std::format!("monitor-{id}"),
            identity_is_unique: true,
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
            role: OutputRole::Extend,
        }
    }

    #[test]
    fn role_changes_round_trip_the_extend_geometry_of_every_connector() {
        let outputs = [
            output(0, true, 100, 100),
            output(1, false, 100, 100),
            output(2, false, 100, 100),
        ];
        let initial = [at(2, -100, 0, 1000), at(0, 0, 0, 1000), at(1, 100, 0, 1000)];
        let mut config = initial;
        config[0].role = OutputRole::Mirror;
        config[1].role = OutputRole::Disabled;
        assert_eq!(extended_primary(&outputs, &config), Some(1));
        let mut docked = canonical_layout(&outputs, &config).unwrap();
        assert_eq!(docked, config);
        let snapshot = OutputSnapshot {
            generation: 42,
            outputs: outputs.to_vec(),
            configuration: docked.clone(),
        };
        assert_eq!(decode_snapshot(&encode_snapshot(&snapshot)), Ok(snapshot));
        for entry in &mut docked {
            entry.role = OutputRole::Extend;
        }
        assert_eq!(canonical_layout(&outputs, &docked).unwrap(), initial);
    }

    #[test]
    fn only_extended_outputs_take_part_in_overlap_adjacency_and_snapping() {
        let outputs = [
            output(0, true, 100, 100),
            output(1, false, 100, 100),
            output(2, false, 100, 100),
        ];
        for role in [OutputRole::Mirror, OutputRole::Disabled] {
            let mut config = [at(0, 0, 0, 1000), at(1, 100, 0, 1000), at(2, 200, 0, 1000)];
            config[1].role = role;
            // An inactive connector cannot bridge a gap or attract the moving desktop.
            assert_eq!(
                validate_layout(&outputs, &config),
                Err(LayoutError::Disconnected)
            );
            assert_eq!(
                snapped_position(&outputs, &config, 2, (200, 0)),
                Ok((100, 0))
            );
            assert_eq!(
                snapped_position(&outputs, &config, 1, (0, 0)),
                Err(LayoutError::NotExtended)
            );
            config[2].x = 100;
            assert!(validate_layout(&outputs, &config).is_ok());
            config[0].role = role;
            assert_eq!(
                snapped_position(&outputs, &config, 2, (-700, -89)),
                Ok((-700, -89))
            );
            config[2].role = role;
            assert_eq!(
                canonical_layout(&outputs, &config),
                Err(LayoutError::NoExtendedOutput)
            );
        }
    }

    #[test]
    fn inactive_connectors_still_require_valid_ids_modes_and_saved_geometry() {
        let mut outputs = [output(0, true, 100, 100), output(1, false, 100, 100)];
        let mut config = [at(0, 0, 0, 1000), at(1, 0, 0, 1000)];
        config[1].role = OutputRole::Disabled;
        for (id, scale, x, error) in [
            (2, 1000, 0, LayoutError::MissingOutput),
            (1, 0, 0, LayoutError::InvalidScale),
            (1, 1000, i32::MAX, LayoutError::CoordinatesOverflow),
        ] {
            config[1].id = id;
            config[1].scale_milli = scale;
            config[1].x = x;
            assert_eq!(validate_layout(&outputs, &config), Err(error));
        }
        config[1].x = 0;
        outputs[1].mode.pixel_width = 0;
        assert_eq!(
            validate_layout(&outputs, &config),
            Err(LayoutError::InvalidMode)
        );
    }

    #[test]
    fn mirror_source_prefers_extended_internal_then_primary_then_first() {
        let mut outputs = [
            output(1, false, 100, 100),
            output(2, true, 100, 100),
            output(3, false, 100, 100),
        ];
        outputs[1].internal = false;
        outputs[2].internal = true;
        let mut config = [at(3, 100, 0, 1000), at(2, 0, 0, 1000), at(1, -100, 0, 1000)];
        assert_eq!(extended_primary(&outputs, &config), Some(3));
        config[0].role = OutputRole::Mirror;
        assert_eq!(extended_primary(&outputs, &config), Some(2));
        config[1].role = OutputRole::Disabled;
        assert_eq!(extended_primary(&outputs, &config), Some(1));
    }

    #[test]
    fn tagged_transactions_distinguish_explicit_extend_from_a_legacy_write() {
        let entry = OutputConfiguration {
            experience: OutputExperience::Tablet,
            ..at(9, -10, 20, 1250)
        };
        let tx = OutputTransaction {
            base_generation: 7,
            configuration: std::vec![entry],
        };
        assert_eq!(encode_request(&OutputRequest::Get), 11u32.to_le_bytes());
        assert_eq!(
            encode_request(&OutputRequest::GetLegacy),
            0u32.to_le_bytes()
        );
        for (opcode, count, role) in [(1u32, 1u32, None), (12, 0x8000_0001, Some(0u32))] {
            let mut bytes = [opcode, 7, count, 9, (-10i32) as u32, 20, 1250, 2]
                .into_iter()
                .chain(role)
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>();
            let request = if role.is_some() {
                OutputRequest::Apply(tx.clone())
            } else {
                OutputRequest::ApplyLegacy(tx.clone())
            };
            assert_eq!(encode_request(&request), bytes);
            assert_eq!(decode_request(&bytes), Ok(request));
            assert_eq!(decode_transaction(&bytes[4..]), Ok(tx.clone()));
            assert!(decode_request(&bytes[..bytes.len() - 4]).is_err());
            bytes[0] = if opcode == 12 { 1 } else { 12 };
            assert!(decode_request(&bytes).is_err());
        }
        for (role, word) in [(OutputRole::Mirror, 1u32), (OutputRole::Disabled, 2)] {
            let mut tx = tx.clone();
            tx.configuration[0].role = role;
            let request = OutputRequest::Apply(tx);
            let mut bytes = encode_request(&request);
            let last_word = bytes.len() - 4;
            assert_eq!(bytes[last_word..], word.to_le_bytes());
            assert_eq!(decode_request(&bytes), Ok(request));
            bytes[last_word] = 3;
            assert!(decode_request(&bytes).is_err());
        }
    }

    #[test]
    fn legacy_snapshots_and_writes_preserve_the_hidden_connectors() {
        let mut snapshot = OutputSnapshot {
            generation: 7,
            outputs: std::vec![
                output(0, true, 100, 100),
                output(1, false, 100, 100),
                output(2, false, 100, 100)
            ],
            configuration: std::vec![at(2, -100, 0, 1000), at(0, 0, 0, 1000), at(1, 100, 0, 1000)],
        };
        snapshot.configuration[0].role = OutputRole::Mirror;
        snapshot.configuration[1].role = OutputRole::Disabled;
        let bytes = encode_legacy_event(&OutputEvent::Snapshot(snapshot.clone()));
        assert_eq!(&bytes[8..12], &1u32.to_le_bytes());
        let OutputEvent::Snapshot(legacy) = decode_event(&bytes).unwrap() else {
            panic!("expected snapshot")
        };
        assert_eq!(legacy.generation, snapshot.generation);
        assert_eq!(legacy.outputs.len(), 1);
        assert_eq!(legacy.outputs[0].id, 1);
        assert!(legacy.outputs[0].primary);
        assert_eq!(legacy.configuration, [snapshot.configuration[2]]);
        let mut tx = OutputTransaction {
            base_generation: 7,
            configuration: legacy.configuration,
        };
        tx.configuration[0].scale_milli = 1250;
        let merged = tx.merge_legacy(&snapshot).unwrap();
        let mut expected = snapshot.configuration.clone();
        expected[2].scale_milli = 1250;
        assert_eq!(merged.configuration, expected);
        assert_eq!(merged.base_generation, 7);
        assert!(validate_layout(&snapshot.outputs, &merged.configuration).is_ok());
        tx.configuration[0].id = 0;
        assert_eq!(tx.merge_legacy(&snapshot), Err(LayoutError::MissingOutput));
    }

    #[test]
    fn legacy_primary_normalization_preserves_parked_positions_on_reenable() {
        let mut snapshot = OutputSnapshot {
            generation: 7,
            outputs: std::vec![
                output(0, true, 100, 100),
                output(1, false, 100, 100),
                output(2, false, 100, 100),
                output(3, false, 100, 100),
            ],
            configuration: std::vec![
                at(2, 200, -25, 1000),
                at(0, 0, 0, 1000),
                at(3, -100, 0, 1000),
                at(1, 100, -25, 1000),
            ],
        };
        snapshot.configuration[1].role = OutputRole::Disabled;
        snapshot.configuration[2].role = OutputRole::Mirror;
        let bytes = encode_legacy_event(&OutputEvent::Snapshot(snapshot.clone()));
        let OutputEvent::Snapshot(mut view) = decode_event(&bytes).unwrap() else {
            panic!("expected snapshot")
        };
        // Move output 2 above its neighbor. Legacy Settings anchors its primary at (0, 0).
        view.configuration[0] = at(2, 100, -125, 1000);
        let configuration = canonical_layout(&view.outputs, &view.configuration).unwrap();
        assert_eq!(configuration, [at(2, 0, -100, 1000), at(1, 0, 0, 1000)]);
        let tx = OutputTransaction {
            base_generation: view.generation,
            configuration,
        };
        let mut merged = tx.merge_legacy(&snapshot).unwrap().configuration;
        let mut expected = snapshot.configuration.clone();
        expected[0] = at(2, 100, -125, 1000);
        assert_eq!(merged, expected);
        for c in &mut merged {
            c.role = OutputRole::Extend;
        }
        assert_eq!(
            canonical_layout(&snapshot.outputs, &merged).unwrap(),
            merged
        );
    }

    #[test]
    fn legacy_rebase_checks_the_final_coordinates() {
        let snapshot = OutputSnapshot {
            generation: 7,
            outputs: std::vec![output(0, true, 100, 100), output(1, false, 100, 100)],
            configuration: std::vec![at(0, 20, 0, 1000), at(1, 120, 0, 1000)],
        };
        let mut tx = OutputTransaction {
            base_generation: 7,
            configuration: std::vec![at(0, i32::MIN, 0, 1000), at(1, i32::MIN + 100, 0, 1000)],
        };
        // The translation exceeds i32, but both final positions fit.
        assert_eq!(
            tx.merge_legacy(&snapshot).unwrap().configuration,
            snapshot.configuration
        );
        tx.configuration[1].x = 0;
        assert_eq!(
            tx.merge_legacy(&snapshot),
            Err(LayoutError::CoordinatesOverflow)
        );
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
