//! Typed workspace state shared by SWS and the system shell.
//!
//! Workspace payloads are deliberately complete snapshots. The shell submits
//! one generation-checked desired state and SWS either accepts the whole state
//! or rejects it without partial effects.

use std::vec::Vec;

use crate::ProtocolError;

/// Lowest accepted tablet split ratio in milli-units.
pub const MIN_SPLIT_RATIO_MILLI: u32 = 200;

/// Highest accepted tablet split ratio in milli-units.
pub const MAX_SPLIT_RATIO_MILLI: u32 = 800;

/// Maximum number of workspaces carried by one protocol snapshot.
pub const MAX_WORKSPACES: usize = 64;

/// Maximum number of scene-root windows assigned to one workspace.
pub const MAX_WORKSPACE_MEMBERS: usize = 256;

/// Preferred share of a compact workspace rail in milli-units.
pub const COMPACT_WORKSPACE_RAIL_RATIO_MILLI: u32 = 100;

/// Minimum compact workspace-rail height in logical pixels.
pub const COMPACT_WORKSPACE_RAIL_MIN_HEIGHT: u32 = 96;

/// Maximum compact workspace-rail height in logical pixels.
pub const COMPACT_WORKSPACE_RAIL_MAX_HEIGHT: u32 = 112;

/// Provisional tablet share of the combined Overview reserved for cards.
///
/// Keeping this as a shared token lets tablet interaction testing change the
/// card/application balance without forking shell and compositor geometry.
pub const TABLET_OVERVIEW_WORKSPACE_RATIO_MILLI: u32 = 660;

/// Height of the exposed laptop drawer-sheet lip in logical pixels.
pub const DRAWER_SHEET_LIP_HEIGHT: u32 = 36;

/// Gap between the workspace region and the content below it in logical pixels.
pub const OVERVIEW_REGION_GAP: u32 = 12;

/// Corner radius of one live workspace card in Overview.
pub const OVERVIEW_CARD_CORNER_RADIUS: u32 = 14;

/// BGRA overlay applied to the active Overview card.
///
/// The faint white veil preserves the desktop underneath and distinguishes
/// selection without adding another outline.
pub const OVERVIEW_CARD_SELECTED_OVERLAY_BGRA: [u8; 4] = [255, 255, 255, 24];

/// BGRA overlay applied to inactive Overview cards.
///
/// The neutral black veil slightly recedes inactive workspaces without
/// replacing the desktop with an opaque, theme-tinted placeholder.
pub const OVERVIEW_CARD_INACTIVE_OVERLAY_BGRA: [u8; 4] = [0, 0, 0, 40];

/// Return the preferred workspace-card share for a posture.
///
/// # Arguments
///
/// * `tablet_mode` - Whether the output currently uses the tablet experience.
///
/// # Returns
///
/// A ratio in milli-units. The remaining output area belongs to the
/// application catalog.
pub const fn overview_workspace_ratio_milli(tablet_mode: bool) -> u32 {
    if tablet_mode {
        TABLET_OVERVIEW_WORKSPACE_RATIO_MILLI
    } else {
        COMPACT_WORKSPACE_RAIL_RATIO_MILLI
    }
}

/// Resolve the physical workspace-region height for one shell depth.
///
/// Laptop Overview and every expanded application drawer use the compact rail.
/// Tablet Overview retains the larger card region. Compact rails are clamped in
/// logical units before being converted to the output scale so shell and
/// compositor geometry remain identical on HiDPI outputs.
///
/// # Arguments
///
/// * `work_height` - Physical height available below system chrome.
/// * `tablet_mode` - Whether the output uses the tablet layout.
/// * `presentation` - Current shell presentation depth.
/// * `scale_milli` - Output scale in thousandths, where `1000` is 1×.
///
/// # Returns
///
/// Physical height assigned to workspace cards or the compact rail.
pub fn workspace_region_height(
    work_height: u32,
    tablet_mode: bool,
    presentation: ShellPresentation,
    scale_milli: u32,
) -> u32 {
    if work_height == 0 {
        return 0;
    }
    if tablet_mode && matches!(presentation, ShellPresentation::Overview) {
        return (((work_height as u64) * (TABLET_OVERVIEW_WORKSPACE_RATIO_MILLI as u64) / 1000)
            as u32)
            .max(1)
            .min(work_height);
    }

    let scale_milli = scale_milli.max(1) as u64;
    let minimum = ((COMPACT_WORKSPACE_RAIL_MIN_HEIGHT as u64) * scale_milli / 1000)
        .max(1)
        .min(u32::MAX as u64) as u32;
    let maximum = ((COMPACT_WORKSPACE_RAIL_MAX_HEIGHT as u64) * scale_milli / 1000)
        .max(u64::from(minimum))
        .min(u32::MAX as u64) as u32;
    let preferred = ((work_height as u64) * (COMPACT_WORKSPACE_RAIL_RATIO_MILLI as u64) / 1000)
        .max(1)
        .min(u32::MAX as u64) as u32;
    preferred.max(minimum).min(maximum).min(work_height)
}

/// Stable identifier for one workspace.
pub type WorkspaceId = u32;

/// Shell-level output presentation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum ShellPresentation {
    /// Present the session-global Home surface.
    Home = 0,
    /// Present the active workspace at its normal scale.
    #[default]
    Workspace = 1,
    /// Present the zoomed-out workspace overview.
    Overview = 2,
}

impl ShellPresentation {
    /// Decode a stable protocol value.
    ///
    /// # Arguments
    ///
    /// * `raw` - Numeric wire value.
    ///
    /// # Returns
    ///
    /// The matching presentation, or `None` for an unknown value.
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Home),
            1 => Some(Self::Workspace),
            2 => Some(Self::Overview),
            _ => None,
        }
    }

    /// Return the stable protocol value.
    ///
    /// # Returns
    ///
    /// Numeric wire representation.
    pub const fn as_raw(self) -> u32 {
        self as u32
    }
}

/// Axis used by a two-scene tablet composition.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum SplitAxis {
    /// Place the first scene to the left of the second scene.
    #[default]
    Horizontal = 0,
    /// Place the first scene above the second scene.
    Vertical = 1,
}

impl SplitAxis {
    /// Decode a stable protocol value.
    ///
    /// # Arguments
    ///
    /// * `raw` - Numeric wire value.
    ///
    /// # Returns
    ///
    /// The matching axis, or `None` for an unknown value.
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Horizontal),
            1 => Some(Self::Vertical),
            _ => None,
        }
    }

    /// Return the stable protocol value.
    ///
    /// # Returns
    ///
    /// Numeric wire representation.
    pub const fn as_raw(self) -> u32 {
        self as u32
    }
}

/// Tablet projection retained for one workspace.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TabletLayout {
    /// The workspace has no presented scene.
    #[default]
    Empty,
    /// Present one scene root across the work area.
    Single {
        /// Scene-root window identifier.
        window_id: u32,
    },
    /// Present two scene roots separated by a system divider.
    Split {
        /// Layout direction.
        axis: SplitAxis,
        /// First scene-root window identifier.
        first_window_id: u32,
        /// Second scene-root window identifier.
        second_window_id: u32,
        /// Share occupied by the first scene in milli-units.
        ratio_milli: u32,
    },
}

impl TabletLayout {
    const EMPTY_KIND: u32 = 0;
    const SINGLE_KIND: u32 = 1;
    const SPLIT_KIND: u32 = 2;

    fn wire_fields(self) -> (u32, u32, u32, u32, u32) {
        match self {
            Self::Empty => (Self::EMPTY_KIND, 0, 0, SplitAxis::Horizontal.as_raw(), 500),
            Self::Single { window_id } => (
                Self::SINGLE_KIND,
                window_id,
                0,
                SplitAxis::Horizontal.as_raw(),
                500,
            ),
            Self::Split {
                axis,
                first_window_id,
                second_window_id,
                ratio_milli,
            } => (
                Self::SPLIT_KIND,
                first_window_id,
                second_window_id,
                axis.as_raw(),
                ratio_milli,
            ),
        }
    }

    fn from_wire_fields(
        kind: u32,
        first_window_id: u32,
        second_window_id: u32,
        axis: u32,
        ratio_milli: u32,
    ) -> Result<Self, ProtocolError> {
        match kind {
            Self::EMPTY_KIND
                if first_window_id == 0
                    && second_window_id == 0
                    && SplitAxis::from_raw(axis).is_some() =>
            {
                Ok(Self::Empty)
            }
            Self::SINGLE_KIND
                if first_window_id != 0
                    && second_window_id == 0
                    && SplitAxis::from_raw(axis).is_some() =>
            {
                Ok(Self::Single {
                    window_id: first_window_id,
                })
            }
            Self::SPLIT_KIND => {
                let axis = SplitAxis::from_raw(axis).ok_or(ProtocolError::MalformedPayload)?;
                if first_window_id == 0
                    || second_window_id == 0
                    || first_window_id == second_window_id
                    || !(MIN_SPLIT_RATIO_MILLI..=MAX_SPLIT_RATIO_MILLI).contains(&ratio_milli)
                {
                    return Err(ProtocolError::MalformedPayload);
                }
                Ok(Self::Split {
                    axis,
                    first_window_id,
                    second_window_id,
                    ratio_milli,
                })
            }
            _ => Err(ProtocolError::MalformedPayload),
        }
    }
}

/// One workspace in a compositor snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    /// Stable non-zero workspace identifier.
    pub id: WorkspaceId,
    /// Ordered scene-root windows belonging to this workspace.
    pub window_ids: Vec<u32>,
    /// Retained tablet layout for the workspace.
    pub tablet_layout: TabletLayout,
}

/// Complete authoritative workspace state published by SWS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceState {
    /// Monotonically increasing SWS workspace generation.
    pub generation: u32,
    /// Workspace currently selected on the initial output.
    pub active_workspace: WorkspaceId,
    /// Normal workspace restored when Overview is toggled closed.
    ///
    /// This may reference an empty workspace. Occupancy never determines
    /// whether a workspace is a valid navigation destination.
    pub normal_workspace: WorkspaceId,
    /// Shell-level presentation on the initial output.
    pub presentation: ShellPresentation,
    /// Workspace order and membership.
    pub workspaces: Vec<WorkspaceSnapshot>,
}

/// Semantic compositor transition requested with a shell transaction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum TransitionKind {
    /// Apply the state at the next frame boundary without interpolation.
    #[default]
    Immediate = 0,
    /// Slide between adjacent workspaces.
    WorkspaceSlide = 1,
    /// Transition between a workspace and Home.
    Home = 2,
    /// Transition between a workspace and Overview.
    Overview = 3,
    /// Settle an interactive split-divider change.
    Split = 4,
}

impl TransitionKind {
    /// Decode a stable protocol value.
    ///
    /// # Arguments
    ///
    /// * `raw` - Numeric wire value.
    ///
    /// # Returns
    ///
    /// The matching transition, or `None` for an unknown value.
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Immediate),
            1 => Some(Self::WorkspaceSlide),
            2 => Some(Self::Home),
            3 => Some(Self::Overview),
            4 => Some(Self::Split),
            _ => None,
        }
    }

    /// Return the stable protocol value.
    ///
    /// # Returns
    ///
    /// Numeric wire representation.
    pub const fn as_raw(self) -> u32 {
        self as u32
    }
}

/// Animation specification attached to one atomic workspace transaction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransitionSpec {
    /// Semantic transition to execute.
    pub kind: TransitionKind,
    /// Nominal settle duration in milliseconds.
    pub duration_ms: u32,
}

/// Complete generation-checked desired workspace state submitted by the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceTransaction {
    /// Generation on which this edit was based.
    pub base_generation: u32,
    /// Workspace that should become active.
    pub active_workspace: WorkspaceId,
    /// Shell-level presentation that should become visible.
    pub presentation: ShellPresentation,
    /// Complete ordered desired workspace list.
    pub workspaces: Vec<WorkspaceSnapshot>,
    /// Compositor transition to use when applying the state.
    pub transition: TransitionSpec,
}

/// Why a typed workspace state violates the protocol invariants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationError {
    /// No workspace was supplied or the protocol count limit was exceeded.
    InvalidWorkspaceCount,
    /// A workspace identifier was zero or repeated.
    InvalidWorkspaceId,
    /// The active workspace does not exist in the snapshot.
    ActiveWorkspaceMissing,
    /// The normal workspace does not exist in the snapshot.
    NormalWorkspaceMissing,
    /// A member identifier was zero, repeated, or exceeded the member limit.
    InvalidMember,
    /// A tablet layout references a non-member or otherwise violates layout rules.
    InvalidTabletLayout,
}

/// Validate one complete workspace list and selected workspace.
///
/// # Arguments
///
/// * `active_workspace` - Workspace selected by the enclosing state.
/// * `workspaces` - Complete ordered workspace list.
///
/// # Returns
///
/// `Ok(())` when every cross-reference and cardinality invariant holds.
pub fn validate_workspaces(
    active_workspace: WorkspaceId,
    workspaces: &[WorkspaceSnapshot],
) -> Result<(), ValidationError> {
    if workspaces.is_empty() || workspaces.len() > MAX_WORKSPACES {
        return Err(ValidationError::InvalidWorkspaceCount);
    }
    for (workspace_index, workspace) in workspaces.iter().enumerate() {
        if workspace.id == 0
            || workspaces[..workspace_index]
                .iter()
                .any(|candidate| candidate.id == workspace.id)
        {
            return Err(ValidationError::InvalidWorkspaceId);
        }
        if workspace.window_ids.len() > MAX_WORKSPACE_MEMBERS {
            return Err(ValidationError::InvalidMember);
        }
        for (member_index, window_id) in workspace.window_ids.iter().copied().enumerate() {
            if window_id == 0
                || workspace.window_ids[..member_index].contains(&window_id)
                || workspaces[..workspace_index]
                    .iter()
                    .any(|candidate| candidate.window_ids.contains(&window_id))
            {
                return Err(ValidationError::InvalidMember);
            }
        }

        let member = |window_id| workspace.window_ids.contains(&window_id);
        let layout_valid = match workspace.tablet_layout {
            TabletLayout::Empty => true,
            TabletLayout::Single { window_id } => window_id != 0 && member(window_id),
            TabletLayout::Split {
                first_window_id,
                second_window_id,
                ratio_milli,
                ..
            } => {
                first_window_id != second_window_id
                    && member(first_window_id)
                    && member(second_window_id)
                    && (MIN_SPLIT_RATIO_MILLI..=MAX_SPLIT_RATIO_MILLI).contains(&ratio_milli)
            }
        };
        if !layout_valid {
            return Err(ValidationError::InvalidTabletLayout);
        }
    }
    if active_workspace == 0
        || !workspaces
            .iter()
            .any(|workspace| workspace.id == active_workspace)
    {
        return Err(ValidationError::ActiveWorkspaceMissing);
    }
    Ok(())
}

/// Serialize an authoritative workspace state.
///
/// # Arguments
///
/// * `state` - State to encode.
///
/// # Returns
///
/// Little-endian workspace-state payload.
pub fn encode_state(state: &WorkspaceState) -> Vec<u8> {
    let mut payload = Vec::new();
    push_u32(&mut payload, state.generation);
    push_u32(&mut payload, state.active_workspace);
    push_u32(&mut payload, state.normal_workspace);
    push_u32(&mut payload, state.presentation.as_raw());
    encode_workspaces(&mut payload, &state.workspaces);
    payload
}

/// Parse an authoritative workspace-state payload.
///
/// # Arguments
///
/// * `payload` - Complete little-endian payload.
///
/// # Returns
///
/// Typed validated state, or a protocol error for malformed input.
pub fn decode_state(payload: &[u8]) -> Result<WorkspaceState, ProtocolError> {
    if payload.len() < 20 {
        return Err(ProtocolError::MalformedPayload);
    }
    let generation = read_u32(payload, 0)?;
    let active_workspace = read_u32(payload, 4)?;
    let normal_workspace = read_u32(payload, 8)?;
    let presentation = ShellPresentation::from_raw(read_u32(payload, 12)?)
        .ok_or(ProtocolError::MalformedPayload)?;
    let (workspaces, offset) = decode_workspaces(payload, 16)?;
    if offset != payload.len()
        || validate_workspaces(active_workspace, &workspaces).is_err()
        || !workspaces
            .iter()
            .any(|workspace| workspace.id == normal_workspace)
    {
        return Err(ProtocolError::MalformedPayload);
    }
    Ok(WorkspaceState {
        generation,
        active_workspace,
        normal_workspace,
        presentation,
        workspaces,
    })
}

/// Serialize a complete shell workspace transaction.
///
/// # Arguments
///
/// * `transaction` - Transaction to encode.
///
/// # Returns
///
/// Little-endian generation-checked transaction payload.
pub fn encode_transaction(transaction: &WorkspaceTransaction) -> Vec<u8> {
    let mut payload = Vec::new();
    push_u32(&mut payload, transaction.base_generation);
    push_u32(&mut payload, transaction.active_workspace);
    push_u32(&mut payload, transaction.presentation.as_raw());
    push_u32(&mut payload, transaction.transition.kind.as_raw());
    push_u32(&mut payload, transaction.transition.duration_ms);
    encode_workspaces(&mut payload, &transaction.workspaces);
    payload
}

/// Parse a complete shell workspace transaction.
///
/// # Arguments
///
/// * `payload` - Complete little-endian payload.
///
/// # Returns
///
/// Typed validated transaction, or a protocol error for malformed input.
pub fn decode_transaction(payload: &[u8]) -> Result<WorkspaceTransaction, ProtocolError> {
    if payload.len() < 24 {
        return Err(ProtocolError::MalformedPayload);
    }
    let base_generation = read_u32(payload, 0)?;
    let active_workspace = read_u32(payload, 4)?;
    let presentation = ShellPresentation::from_raw(read_u32(payload, 8)?)
        .ok_or(ProtocolError::MalformedPayload)?;
    let transition_kind =
        TransitionKind::from_raw(read_u32(payload, 12)?).ok_or(ProtocolError::MalformedPayload)?;
    let duration_ms = read_u32(payload, 16)?;
    let (workspaces, offset) = decode_workspaces(payload, 20)?;
    if offset != payload.len() || validate_workspaces(active_workspace, &workspaces).is_err() {
        return Err(ProtocolError::MalformedPayload);
    }
    Ok(WorkspaceTransaction {
        base_generation,
        active_workspace,
        presentation,
        workspaces,
        transition: TransitionSpec {
            kind: transition_kind,
            duration_ms,
        },
    })
}

fn encode_workspaces(payload: &mut Vec<u8>, workspaces: &[WorkspaceSnapshot]) {
    push_u32(payload, workspaces.len() as u32);
    for workspace in workspaces {
        let (kind, first, second, axis, ratio) = workspace.tablet_layout.wire_fields();
        push_u32(payload, workspace.id);
        push_u32(payload, kind);
        push_u32(payload, first);
        push_u32(payload, second);
        push_u32(payload, axis);
        push_u32(payload, ratio);
        push_u32(payload, workspace.window_ids.len() as u32);
        for window_id in &workspace.window_ids {
            push_u32(payload, *window_id);
        }
    }
}

fn decode_workspaces(
    payload: &[u8],
    mut offset: usize,
) -> Result<(Vec<WorkspaceSnapshot>, usize), ProtocolError> {
    let count = read_u32(payload, offset)? as usize;
    offset = offset
        .checked_add(4)
        .ok_or(ProtocolError::MalformedPayload)?;
    if count == 0 || count > MAX_WORKSPACES {
        return Err(ProtocolError::MalformedPayload);
    }
    let mut workspaces = Vec::with_capacity(count);
    for _ in 0..count {
        let fixed_end = offset
            .checked_add(28)
            .ok_or(ProtocolError::MalformedPayload)?;
        if fixed_end > payload.len() {
            return Err(ProtocolError::MalformedPayload);
        }
        let id = read_u32(payload, offset)?;
        let kind = read_u32(payload, offset + 4)?;
        let first = read_u32(payload, offset + 8)?;
        let second = read_u32(payload, offset + 12)?;
        let axis = read_u32(payload, offset + 16)?;
        let ratio = read_u32(payload, offset + 20)?;
        let member_count = read_u32(payload, offset + 24)? as usize;
        if member_count > MAX_WORKSPACE_MEMBERS {
            return Err(ProtocolError::MalformedPayload);
        }
        let members_end = fixed_end
            .checked_add(
                member_count
                    .checked_mul(4)
                    .ok_or(ProtocolError::MalformedPayload)?,
            )
            .ok_or(ProtocolError::MalformedPayload)?;
        if members_end > payload.len() {
            return Err(ProtocolError::MalformedPayload);
        }
        let mut window_ids = Vec::with_capacity(member_count);
        let mut member_offset = fixed_end;
        while member_offset < members_end {
            window_ids.push(read_u32(payload, member_offset)?);
            member_offset += 4;
        }
        workspaces.push(WorkspaceSnapshot {
            id,
            window_ids,
            tablet_layout: TabletLayout::from_wire_fields(kind, first, second, axis, ratio)?,
        });
        offset = members_end;
    }
    Ok((workspaces, offset))
}

fn push_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn read_u32(input: &[u8], offset: usize) -> Result<u32, ProtocolError> {
    let bytes = input
        .get(offset..offset.saturating_add(4))
        .ok_or(ProtocolError::MalformedPayload)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    fn workspace(id: u32, members: &[u32], layout: TabletLayout) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            id,
            window_ids: members.to_vec(),
            tablet_layout: layout,
        }
    }

    #[test]
    fn state_round_trip_preserves_order_and_split() {
        let state = WorkspaceState {
            generation: 41,
            active_workspace: 7,
            normal_workspace: 9,
            presentation: ShellPresentation::Overview,
            workspaces: vec![
                workspace(
                    7,
                    &[10, 11],
                    TabletLayout::Split {
                        axis: SplitAxis::Horizontal,
                        first_window_id: 10,
                        second_window_id: 11,
                        ratio_milli: 620,
                    },
                ),
                workspace(9, &[12], TabletLayout::Single { window_id: 12 }),
            ],
        };
        assert_eq!(decode_state(&encode_state(&state)), Ok(state));
    }

    #[test]
    fn transaction_round_trip_preserves_transition() {
        let transaction = WorkspaceTransaction {
            base_generation: 8,
            active_workspace: 1,
            presentation: ShellPresentation::Home,
            workspaces: vec![workspace(1, &[], TabletLayout::Empty)],
            transition: TransitionSpec {
                kind: TransitionKind::Home,
                duration_ms: 240,
            },
        };
        assert_eq!(
            decode_transaction(&encode_transaction(&transaction)),
            Ok(transaction)
        );
    }

    #[test]
    fn validation_rejects_duplicate_members_across_workspaces() {
        let workspaces = vec![
            workspace(1, &[4], TabletLayout::Single { window_id: 4 }),
            workspace(2, &[4], TabletLayout::Single { window_id: 4 }),
        ];
        assert_eq!(
            validate_workspaces(1, &workspaces),
            Err(ValidationError::InvalidMember)
        );
    }

    #[test]
    fn validation_rejects_layout_scene_outside_workspace() {
        let workspaces = vec![workspace(1, &[4], TabletLayout::Single { window_id: 5 })];
        assert_eq!(
            validate_workspaces(1, &workspaces),
            Err(ValidationError::InvalidTabletLayout)
        );
    }

    #[test]
    fn decoder_rejects_trailing_bytes() {
        let state = WorkspaceState {
            generation: 1,
            active_workspace: 1,
            normal_workspace: 1,
            presentation: ShellPresentation::Workspace,
            workspaces: vec![workspace(1, &[], TabletLayout::Empty)],
        };
        let mut bytes = encode_state(&state);
        bytes.push(0);
        assert_eq!(decode_state(&bytes), Err(ProtocolError::MalformedPayload));
    }

    #[test]
    fn decoder_rejects_a_missing_normal_workspace() {
        let state = WorkspaceState {
            generation: 1,
            active_workspace: 1,
            normal_workspace: 2,
            presentation: ShellPresentation::Overview,
            workspaces: vec![workspace(1, &[], TabletLayout::Empty)],
        };

        assert_eq!(
            decode_state(&encode_state(&state)),
            Err(ProtocolError::MalformedPayload)
        );
    }
}
