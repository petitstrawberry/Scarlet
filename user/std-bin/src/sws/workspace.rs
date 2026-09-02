//! Authoritative compositor-side workspace state machine.

use std::vec::Vec;
use sws_protocol::workspace::{
    MAX_WORKSPACES, ShellPresentation, TabletLayout, TransitionSpec, WorkspaceId,
    WorkspaceSnapshot, WorkspaceState, WorkspaceTransaction, validate_workspaces,
};

/// Result of accepting one system-shell transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AppliedTransaction {
    pub(super) state: WorkspaceState,
    pub(super) transition: TransitionSpec,
}

/// Typed transaction rejection used to select a stable protocol error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ApplyError {
    StaleGeneration,
    InvalidState,
}

#[derive(Clone)]
struct DesktopWorkspaceRestore {
    active_workspace: WorkspaceId,
    workspaces: Vec<WorkspaceSnapshot>,
}

/// Live workspace state retained independently of the shell connection.
pub(super) struct WorkspaceManager {
    generation: u32,
    active_workspace: WorkspaceId,
    normal_workspace: WorkspaceId,
    presentation: ShellPresentation,
    workspaces: Vec<WorkspaceSnapshot>,
    next_workspace_id: WorkspaceId,
    desktop_restore: Option<DesktopWorkspaceRestore>,
    auto_remove_empty: bool,
}

impl WorkspaceManager {
    /// Construct the mandatory initial empty workspace.
    pub(super) fn new() -> Self {
        Self::with_auto_remove_empty(false)
    }

    /// Construct a workspace manager with an explicit empty-space policy.
    ///
    /// # Arguments
    ///
    /// * `auto_remove_empty` - Remove unselected empty workspaces after scene
    ///   membership changes. The default product policy is `false`.
    pub(super) fn with_auto_remove_empty(auto_remove_empty: bool) -> Self {
        Self {
            generation: 1,
            active_workspace: 1,
            normal_workspace: 1,
            presentation: ShellPresentation::Workspace,
            workspaces: vec![WorkspaceSnapshot {
                id: 1,
                window_ids: Vec::new(),
                tablet_layout: TabletLayout::Empty,
            }],
            next_workspace_id: 2,
            desktop_restore: None,
            auto_remove_empty,
        }
    }

    /// Return a complete owned compositor snapshot.
    pub(super) fn snapshot(&self) -> WorkspaceState {
        WorkspaceState {
            generation: self.generation,
            active_workspace: self.active_workspace,
            normal_workspace: self.normal_workspace,
            presentation: self.presentation,
            workspaces: self.workspaces.clone(),
        }
    }

    /// Return the currently selected workspace.
    pub(super) const fn active_workspace(&self) -> WorkspaceId {
        self.active_workspace
    }

    /// Return the committed workspace restored when shell navigation closes.
    pub(super) const fn normal_workspace(&self) -> WorkspaceId {
        self.normal_workspace
    }

    /// Append one explicitly requested empty workspace.
    ///
    /// The new workspace is selected immediately and becomes the committed
    /// return destination even when it is created from shell navigation.
    ///
    /// # Returns
    ///
    /// The new stable workspace identifier, or `None` at the protocol limit.
    pub(super) fn create_workspace(&mut self) -> Option<WorkspaceId> {
        if self.workspaces.len() >= MAX_WORKSPACES {
            return None;
        }
        let id = self.allocate_workspace_id();
        self.workspaces.push(WorkspaceSnapshot {
            id,
            window_ids: Vec::new(),
            tablet_layout: TabletLayout::Empty,
        });
        self.active_workspace = id;
        self.normal_workspace = id;
        self.bump_generation();
        Some(id)
    }

    /// Create a workspace and move one scene into it atomically.
    ///
    /// This backs drops onto the non-workspace `+` tile in Overview. No
    /// intermediate empty workspace is published.
    ///
    /// # Arguments
    ///
    /// * `window_id` - Existing scene-root identifier to move.
    ///
    /// # Returns
    ///
    /// The new workspace identifier, or `None` when the scene is unknown or
    /// the workspace limit has been reached.
    pub(super) fn move_window_to_new_workspace(&mut self, window_id: u32) -> Option<WorkspaceId> {
        if self.workspaces.len() >= MAX_WORKSPACES {
            return None;
        }
        let source_index = self
            .workspaces
            .iter()
            .position(|workspace| workspace.window_ids.contains(&window_id))?;
        self.workspaces[source_index]
            .window_ids
            .retain(|candidate| *candidate != window_id);
        self.workspaces[source_index].tablet_layout =
            repaired_layout(&self.workspaces[source_index], window_id);

        let id = self.allocate_workspace_id();
        self.workspaces.push(WorkspaceSnapshot {
            id,
            window_ids: vec![window_id],
            tablet_layout: TabletLayout::Single { window_id },
        });
        self.active_workspace = id;
        self.normal_workspace = id;
        self.ensure_manual_workspace_invariants();
        self.bump_generation();
        Some(id)
    }

    /// Return whether one workspace can be explicitly removed safely.
    ///
    /// Empty workspaces are removed directly. A non-empty workspace migrates
    /// to its left neighbor (or right neighbor for the first workspace). In
    /// freeform mode `allow_freeform_merge` permits arbitrary membership to be
    /// combined; focused mode accepts only a move into an empty destination or
    /// a pair of single-scene layouts that can form a valid split.
    ///
    /// # Arguments
    ///
    /// * `workspace_id` - Existing workspace selected for removal.
    /// * `allow_freeform_merge` - Whether arbitrary scene groups may merge.
    ///
    /// # Returns
    ///
    /// `true` when removal can migrate every scene without loss.
    pub(super) fn can_remove_workspace(
        &self,
        workspace_id: WorkspaceId,
        allow_freeform_merge: bool,
    ) -> bool {
        if self.workspaces.len() <= 1 {
            return false;
        }
        let Some(source_index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == workspace_id)
        else {
            return false;
        };
        if self.workspaces[source_index].window_ids.is_empty() {
            return true;
        }
        let target_index = if source_index > 0 {
            source_index - 1
        } else {
            1
        };
        if self.workspaces[target_index].window_ids.is_empty() || allow_freeform_merge {
            return true;
        }
        matches!(
            (
                self.workspaces[target_index].tablet_layout,
                self.workspaces[source_index].tablet_layout,
            ),
            (TabletLayout::Single { .. }, TabletLayout::Single { .. })
        )
    }

    /// Explicitly remove one workspace without destroying any scene.
    ///
    /// See [`Self::can_remove_workspace`] for the migration constraints.
    pub(super) fn remove_workspace(
        &mut self,
        workspace_id: WorkspaceId,
        allow_freeform_merge: bool,
    ) -> bool {
        if !self.can_remove_workspace(workspace_id, allow_freeform_merge) {
            return false;
        }
        let Some(source_index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == workspace_id)
        else {
            return false;
        };
        let target_index = if source_index > 0 {
            source_index - 1
        } else {
            1
        };
        let target_id = self.workspaces[target_index].id;
        let source_members = self.workspaces[source_index].window_ids.clone();
        let source_layout = self.workspaces[source_index].tablet_layout;
        let target_members = self.workspaces[target_index].window_ids.clone();
        let target_layout = self.workspaces[target_index].tablet_layout;

        if !source_members.is_empty() {
            let merged_layout = if target_members.is_empty() {
                source_layout
            } else if allow_freeform_merge {
                target_layout
            } else {
                match (target_layout, source_layout) {
                    (
                        TabletLayout::Single {
                            window_id: first_window_id,
                        },
                        TabletLayout::Single {
                            window_id: second_window_id,
                        },
                    ) => TabletLayout::Split {
                        axis: sws_protocol::workspace::SplitAxis::Horizontal,
                        first_window_id,
                        second_window_id,
                        ratio_milli: 500,
                    },
                    _ => return false,
                }
            };
            for window_id in source_members {
                if !self.workspaces[target_index]
                    .window_ids
                    .contains(&window_id)
                {
                    self.workspaces[target_index].window_ids.push(window_id);
                }
            }
            self.workspaces[target_index].tablet_layout = merged_layout;
        }

        self.workspaces.remove(source_index);
        if self.active_workspace == workspace_id {
            self.active_workspace = target_id;
        }
        if self.normal_workspace == workspace_id {
            self.normal_workspace = target_id;
        }
        self.ensure_manual_workspace_invariants();
        self.bump_generation();
        true
    }

    /// Return the current shell presentation.
    pub(super) const fn presentation(&self) -> ShellPresentation {
        self.presentation
    }

    /// Return the retained tablet layout for a workspace.
    pub(super) fn tablet_layout(&self, workspace_id: WorkspaceId) -> TabletLayout {
        self.workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .map_or(TabletLayout::Empty, |workspace| workspace.tablet_layout)
    }

    /// Return the workspace containing one scene root.
    pub(super) fn workspace_for_window(&self, window_id: u32) -> Option<WorkspaceId> {
        self.workspaces.iter().find_map(|workspace| {
            workspace
                .window_ids
                .contains(&window_id)
                .then_some(workspace.id)
        })
    }

    /// Assign a newly created scene root to the active manual workspace.
    ///
    /// Workspace creation is always explicit. Desktop launches join the
    /// current freeform workspace. In focused mode the new scene becomes the
    /// presented single scene without silently creating another workspace;
    /// existing members remain retained for the desktop projection.
    pub(super) fn add_window(&mut self, window_id: u32, tablet_experience: bool) -> WorkspaceId {
        if let Some(workspace_id) = self.workspace_for_window(window_id) {
            return workspace_id;
        }

        let active_index = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == self.active_workspace)
            .unwrap_or(0);
        let workspace = &mut self.workspaces[active_index];
        workspace.window_ids.push(window_id);
        if tablet_experience || matches!(workspace.tablet_layout, TabletLayout::Empty) {
            workspace.tablet_layout = TabletLayout::Single { window_id };
        }
        let workspace_id = workspace.id;
        self.active_workspace = workspace_id;
        self.normal_workspace = workspace_id;
        self.presentation = ShellPresentation::Workspace;
        self.ensure_manual_workspace_invariants();
        self.bump_generation();
        workspace_id
    }

    /// Remove a destroyed scene root and repair every referencing layout.
    pub(super) fn remove_window(&mut self, window_id: u32) -> bool {
        let mut changed = false;
        for workspace in &mut self.workspaces {
            let previous_len = workspace.window_ids.len();
            workspace
                .window_ids
                .retain(|candidate| *candidate != window_id);
            if previous_len != workspace.window_ids.len() {
                workspace.tablet_layout = repaired_layout(workspace, window_id);
                changed = true;
            }
        }
        if changed {
            self.ensure_manual_workspace_invariants();
        }
        if changed {
            self.bump_generation();
        }
        changed
    }

    /// Reconcile configured lifecycle policy after scene roots close.
    ///
    /// Manual mode deliberately permits the active workspace to remain empty
    /// in both desktop and tablet presentations. Optional automatic removal
    /// affects only unselected empty workspaces.
    pub(super) fn settle_after_close(&mut self, _tablet_experience: bool) -> bool {
        let changed = self.ensure_manual_workspace_invariants();

        if changed {
            self.bump_generation();
        }
        changed
    }

    /// Activate the workspace containing a focused scene.
    ///
    /// In tablet experience, a parked scene replaces the single foreground
    /// slot. Explicit split composition remains stable while either side is
    /// focused.
    pub(super) fn activate_window(&mut self, window_id: u32, tablet_experience: bool) -> bool {
        let Some(index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.window_ids.contains(&window_id))
        else {
            return false;
        };
        let workspace_id = self.workspaces[index].id;
        let mut changed = self.active_workspace != workspace_id
            || self.normal_workspace != workspace_id
            || self.presentation != ShellPresentation::Workspace;
        self.active_workspace = workspace_id;
        self.normal_workspace = workspace_id;
        self.presentation = ShellPresentation::Workspace;
        if tablet_experience {
            let layout_contains_window = match self.workspaces[index].tablet_layout {
                TabletLayout::Empty => false,
                TabletLayout::Single {
                    window_id: presented,
                } => presented == window_id,
                TabletLayout::Split {
                    first_window_id,
                    second_window_id,
                    ..
                } => first_window_id == window_id || second_window_id == window_id,
            };
            if !layout_contains_window {
                self.workspaces[index].tablet_layout = TabletLayout::Single { window_id };
                changed = true;
            }
        }
        if changed {
            self.bump_generation();
        }
        changed
    }

    /// Change the global presentation without altering workspace membership.
    pub(super) fn set_presentation(&mut self, presentation: ShellPresentation) -> bool {
        let previous_presentation = self.presentation;
        let previous_active = self.active_workspace;
        let previous_normal = self.normal_workspace;
        if presentation == ShellPresentation::Overview
            && previous_presentation == ShellPresentation::Workspace
        {
            self.normal_workspace = self.active_workspace;
        } else if presentation == ShellPresentation::Workspace {
            self.active_workspace = self.normal_workspace;
        } else if presentation == ShellPresentation::Home
            && previous_presentation == ShellPresentation::Workspace
        {
            self.normal_workspace = self.active_workspace;
        }
        self.presentation = presentation;
        if self.presentation == previous_presentation
            && self.active_workspace == previous_active
            && self.normal_workspace == previous_normal
        {
            return false;
        }
        self.bump_generation();
        true
    }

    /// Toggle between normal workspace presentation and Workspace Overview.
    ///
    /// Home and Overview are both shell-navigation presentations, so toggling
    /// either one enters the workspace currently selected in shell navigation.
    ///
    /// # Returns
    ///
    /// `true` when the presentation or active workspace changed.
    pub(super) fn toggle_overview(&mut self) -> bool {
        if self.presentation == ShellPresentation::Workspace {
            self.set_presentation(ShellPresentation::Overview)
        } else {
            self.return_to_workspace()
        }
    }

    /// Leave shell navigation for the committed workspace selection.
    ///
    /// Occupancy is irrelevant: an explicitly selected empty workspace is a
    /// complete normal destination and is restored exactly like an occupied
    /// workspace.
    pub(super) fn return_to_workspace(&mut self) -> bool {
        let previous_workspace = self.active_workspace;
        let previous_presentation = self.presentation;
        self.active_workspace = self.normal_workspace;
        self.presentation = ShellPresentation::Workspace;
        let changed = self.active_workspace != previous_workspace
            || self.presentation != previous_presentation;
        if changed {
            self.bump_generation();
        }
        changed
    }

    /// Select a card from shell navigation.
    ///
    /// Both occupied and empty cards become the normal active workspace. The
    /// status-bar Home control remains available when an empty workspace needs
    /// its first application.
    pub(super) fn select_workspace_from_overview(&mut self, workspace_id: WorkspaceId) -> bool {
        if self.presentation == ShellPresentation::Workspace {
            return false;
        }
        let Some(_workspace) = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
        else {
            return false;
        };
        let presentation = ShellPresentation::Workspace;
        if self.active_workspace == workspace_id && self.presentation == presentation {
            return false;
        }
        self.active_workspace = workspace_id;
        self.normal_workspace = workspace_id;
        self.presentation = presentation;
        self.bump_generation();
        true
    }

    /// Select an adjacent ordered workspace without wrapping at either edge.
    pub(super) fn cycle_workspace(&mut self, direction: i32) -> bool {
        if self.workspaces.len() < 2 || direction == 0 {
            return false;
        }
        let index = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == self.active_workspace)
            .unwrap_or(0);
        let next = if direction < 0 {
            let Some(previous) = index.checked_sub(1) else {
                return false;
            };
            previous
        } else {
            let next = index.saturating_add(1);
            if next >= self.workspaces.len() {
                return false;
            }
            next
        };
        self.active_workspace = self.workspaces[next].id;
        self.normal_workspace = self.active_workspace;
        self.presentation = ShellPresentation::Workspace;
        self.bump_generation();
        true
    }

    /// Move one member scene to the adjacent workspace and follow it.
    ///
    /// A negative direction targets the workspace toward index zero and is
    /// rejected at the first workspace. A positive direction targets the next
    /// existing ordered workspace. The destination becomes active as part of
    /// the same state transition, matching relative workspace moves in
    /// Mutter. At either edge the action is rejected; it never creates or
    /// removes a workspace implicitly. An emptied source remains valid.
    ///
    /// # Arguments
    ///
    /// * `window_id` - Scene-root identifier that must already be a member.
    /// * `direction` - Negative moves toward index zero, positive moves after
    ///   the current position.
    ///
    /// # Returns
    ///
    /// `true` when workspace membership changed.
    pub(super) fn move_window_to_adjacent_workspace(
        &mut self,
        window_id: u32,
        direction: i32,
    ) -> bool {
        let Some(source_index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.window_ids.contains(&window_id))
        else {
            return false;
        };
        let target_index = if direction < 0 {
            match source_index.checked_sub(1) {
                Some(index) => index,
                None => return false,
            }
        } else {
            source_index.saturating_add(1)
        };
        if target_index >= self.workspaces.len() {
            return false;
        }

        self.move_window_to_workspace_at(window_id, source_index, target_index, true)
    }

    /// Move one member scene to an explicitly selected workspace.
    ///
    /// This is the existing-card drop handler for Overview drag-and-drop.
    /// Membership and layout repair are atomic, while both source and target
    /// retain their explicit manual lifecycle.
    ///
    /// # Arguments
    ///
    /// * `window_id` - Scene-root identifier that must already be a member.
    /// * `target_workspace_id` - Destination workspace identifier.
    ///
    /// # Returns
    ///
    /// `true` when workspace membership changed.
    pub(super) fn move_window_to_workspace(
        &mut self,
        window_id: u32,
        target_workspace_id: WorkspaceId,
    ) -> bool {
        let Some(source_index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.window_ids.contains(&window_id))
        else {
            return false;
        };
        let Some(target_index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == target_workspace_id)
        else {
            return false;
        };

        self.move_window_to_workspace_at(window_id, source_index, target_index, false)
    }

    /// Move one scene onto a single-scene workspace as a tablet split.
    ///
    /// This is the focused-mode Overview drop primitive. It repairs the
    /// source composition, preserves the current shell presentation, and
    /// forms a horizontal 50/50 split at the destination. A destination that
    /// is empty, already split, or identical to the source is rejected.
    ///
    /// # Arguments
    ///
    /// * `window_id` - Scene-root identifier to move.
    /// * `target_workspace_id` - Single-scene destination workspace.
    ///
    /// # Returns
    ///
    /// `true` when the scene was moved and the split was formed.
    pub(super) fn move_window_to_workspace_as_split(
        &mut self,
        window_id: u32,
        target_workspace_id: WorkspaceId,
    ) -> bool {
        let Some(source_index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.window_ids.contains(&window_id))
        else {
            return false;
        };
        let Some(target_index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == target_workspace_id)
        else {
            return false;
        };
        if source_index == target_index {
            return false;
        }
        let TabletLayout::Single {
            window_id: destination_window_id,
        } = self.workspaces[target_index].tablet_layout
        else {
            return false;
        };

        self.workspaces[source_index]
            .window_ids
            .retain(|candidate| *candidate != window_id);
        self.workspaces[source_index].tablet_layout =
            repaired_layout(&self.workspaces[source_index], window_id);
        if !self.workspaces[target_index]
            .window_ids
            .contains(&window_id)
        {
            self.workspaces[target_index].window_ids.push(window_id);
        }
        self.workspaces[target_index].tablet_layout = TabletLayout::Split {
            axis: sws_protocol::workspace::SplitAxis::Horizontal,
            first_window_id: destination_window_id,
            second_window_id: window_id,
            ratio_milli: 500,
        };

        self.ensure_manual_workspace_invariants();
        self.bump_generation();
        true
    }

    fn move_window_to_workspace_at(
        &mut self,
        window_id: u32,
        source_index: usize,
        target_index: usize,
        activate_target: bool,
    ) -> bool {
        if source_index == target_index {
            return false;
        }

        let target_id = self.workspaces[target_index].id;
        let source = &mut self.workspaces[source_index];
        source
            .window_ids
            .retain(|candidate| *candidate != window_id);
        source.tablet_layout = repaired_layout(source, window_id);
        let target = &mut self.workspaces[target_index];
        if let TabletLayout::Empty = target.tablet_layout {
            target.tablet_layout = TabletLayout::Single { window_id };
        }
        target.window_ids.push(window_id);

        if activate_target {
            self.active_workspace = target_id;
            self.normal_workspace = target_id;
            self.presentation = ShellPresentation::Workspace;
        }

        self.ensure_manual_workspace_invariants();
        self.bump_generation();
        true
    }

    /// Move the selected Overview card without leaving Overview.
    ///
    /// Unlike normal workspace cycling, Overview selection stops at the first
    /// and last cards. This gives horizontal scroll and keyboard navigation a
    /// stable edge instead of unexpectedly wrapping the whole row. Selection
    /// also commits the card as the destination restored when shell navigation
    /// closes.
    pub(super) fn move_overview_selection(&mut self, direction: i32) -> bool {
        if self.presentation == ShellPresentation::Workspace
            || self.workspaces.len() < 2
            || direction == 0
        {
            return false;
        }
        let index = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == self.active_workspace)
            .unwrap_or(0);
        let next = if direction < 0 {
            index.saturating_sub(1)
        } else {
            index.saturating_add(1).min(self.workspaces.len() - 1)
        };
        if next == index {
            return false;
        }
        self.active_workspace = self.workspaces[next].id;
        self.normal_workspace = self.active_workspace;
        self.bump_generation();
        true
    }

    /// Materialize every tablet composition before applying focused layout.
    ///
    /// Desktop workspaces may contain several floating scenes. On entry to the
    /// tablet experience, the retained split (if any) stays together and every
    /// other scene becomes a reachable Single workspace. The original desktop
    /// grouping is retained for restoration.
    pub(super) fn enter_tablet_experience(&mut self, preferred_window_id: Option<u32>) -> bool {
        if self.desktop_restore.is_some() {
            return false;
        }
        self.desktop_restore = Some(DesktopWorkspaceRestore {
            active_workspace: self.active_workspace,
            workspaces: self.workspaces.clone(),
        });

        let original = self.workspaces.clone();
        let mut expanded = Vec::new();
        let mut next_active = None;
        for workspace in original {
            if workspace.window_ids.is_empty() {
                expanded.push(workspace);
                continue;
            }

            let retained_split = match workspace.tablet_layout {
                TabletLayout::Split {
                    first_window_id,
                    second_window_id,
                    ..
                } if workspace.window_ids.contains(&first_window_id)
                    && workspace.window_ids.contains(&second_window_id) =>
                {
                    Some((first_window_id, second_window_id, workspace.tablet_layout))
                }
                _ => None,
            };
            let preferred =
                preferred_window_id.filter(|window_id| workspace.window_ids.contains(window_id));
            let first_window_id = retained_split
                .map(|(first, _, _)| first)
                .or(preferred)
                .or_else(|| match workspace.tablet_layout {
                    TabletLayout::Single { window_id }
                        if workspace.window_ids.contains(&window_id) =>
                    {
                        Some(window_id)
                    }
                    _ => workspace.window_ids.last().copied(),
                })
                .unwrap_or(workspace.window_ids[0]);
            let grouped = if let Some((first, second, layout)) = retained_split {
                WorkspaceSnapshot {
                    id: workspace.id,
                    window_ids: vec![first, second],
                    tablet_layout: layout,
                }
            } else {
                WorkspaceSnapshot {
                    id: workspace.id,
                    window_ids: vec![first_window_id],
                    tablet_layout: TabletLayout::Single {
                        window_id: first_window_id,
                    },
                }
            };
            if preferred.is_some_and(|window_id| grouped.window_ids.contains(&window_id)) {
                next_active = Some(grouped.id);
            }
            let grouped_ids = grouped.window_ids.clone();
            expanded.push(grouped);

            for window_id in workspace
                .window_ids
                .into_iter()
                .filter(|window_id| !grouped_ids.contains(window_id))
            {
                let id = self.allocate_workspace_id();
                if preferred == Some(window_id) {
                    next_active = Some(id);
                }
                expanded.push(WorkspaceSnapshot {
                    id,
                    window_ids: vec![window_id],
                    tablet_layout: TabletLayout::Single { window_id },
                });
            }
        }
        let previous = self.workspaces.clone();
        let previous_active = self.active_workspace;
        self.workspaces = expanded;
        self.ensure_manual_workspace_invariants();
        self.active_workspace = next_active.unwrap_or_else(|| {
            self.workspaces
                .iter()
                .find(|workspace| workspace.id == self.active_workspace)
                .map_or(self.workspaces[0].id, |workspace| workspace.id)
        });
        let changed = self.workspaces != previous || self.active_workspace != previous_active;
        if changed {
            self.bump_generation();
        }
        changed
    }

    /// Restore desktop virtual-desktop grouping after tablet presentation.
    ///
    /// Scenes created during tablet use join the restored group containing a
    /// member of their current composition, or remain in a new workspace when
    /// no prior group applies. Closed scenes are simply omitted.
    pub(super) fn leave_tablet_experience(&mut self) -> bool {
        let Some(restore) = self.desktop_restore.take() else {
            return false;
        };
        let current = self.workspaces.clone();
        let live_window_ids = current
            .iter()
            .flat_map(|workspace| workspace.window_ids.iter().copied())
            .collect::<Vec<_>>();
        let active_member = current
            .iter()
            .find(|workspace| workspace.id == self.active_workspace)
            .and_then(|workspace| {
                presented_window_id(workspace).or_else(|| workspace.window_ids.first().copied())
            });
        let mut restored = restore
            .workspaces
            .into_iter()
            .map(|mut workspace| {
                workspace
                    .window_ids
                    .retain(|window_id| live_window_ids.contains(window_id));
                workspace.tablet_layout =
                    retained_layout_for_members(workspace.tablet_layout, &workspace.window_ids);
                workspace
            })
            .collect::<Vec<_>>();
        let mut assigned = restored
            .iter()
            .flat_map(|workspace| workspace.window_ids.iter().copied())
            .collect::<Vec<_>>();

        for current_workspace in &current {
            let new_members = current_workspace
                .window_ids
                .iter()
                .copied()
                .filter(|window_id| !assigned.contains(window_id))
                .collect::<Vec<_>>();
            if new_members.is_empty() {
                continue;
            }
            let destination = current_workspace.window_ids.iter().find_map(|window_id| {
                restored
                    .iter()
                    .position(|workspace| workspace.window_ids.contains(window_id))
            });
            if let Some(index) = destination {
                restored[index].window_ids.extend_from_slice(&new_members);
                restored[index].tablet_layout = retained_layout_for_members(
                    current_workspace.tablet_layout,
                    &restored[index].window_ids,
                );
            } else {
                let mut id = current_workspace.id;
                if restored.iter().any(|workspace| workspace.id == id) {
                    id = self.allocate_workspace_id();
                }
                restored.push(WorkspaceSnapshot {
                    id,
                    window_ids: new_members.clone(),
                    tablet_layout: retained_layout_for_members(
                        current_workspace.tablet_layout,
                        &new_members,
                    ),
                });
            }
            assigned.extend_from_slice(&new_members);
        }
        if restored.is_empty() {
            restored.push(WorkspaceSnapshot {
                id: restore.active_workspace,
                window_ids: Vec::new(),
                tablet_layout: TabletLayout::Empty,
            });
        }

        let previous = self.workspaces.clone();
        let previous_active = self.active_workspace;
        self.workspaces = restored;
        self.active_workspace = active_member
            .and_then(|window_id| self.workspace_for_window(window_id))
            .or_else(|| {
                self.workspaces
                    .iter()
                    .any(|workspace| workspace.id == restore.active_workspace)
                    .then_some(restore.active_workspace)
            })
            .unwrap_or(self.workspaces[0].id);
        self.next_workspace_id = self
            .workspaces
            .iter()
            .map(|workspace| workspace.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1);
        self.ensure_manual_workspace_invariants();
        let changed = self.workspaces != previous || self.active_workspace != previous_active;
        if changed {
            self.bump_generation();
        }
        changed
    }

    /// Merge two single-scene workspaces into a horizontal tablet split.
    ///
    /// The dragged source scene becomes the second side of the destination
    /// workspace. Parked source members remain in their original workspace;
    /// an emptied source workspace remains available until explicit removal.
    pub(super) fn merge_workspaces_as_split(
        &mut self,
        source_workspace_id: WorkspaceId,
        destination_workspace_id: WorkspaceId,
    ) -> bool {
        if source_workspace_id == destination_workspace_id {
            return false;
        }
        let Some(source_index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == source_workspace_id)
        else {
            return false;
        };
        let Some(destination_index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == destination_workspace_id)
        else {
            return false;
        };
        let TabletLayout::Single {
            window_id: source_window_id,
        } = self.workspaces[source_index].tablet_layout
        else {
            return false;
        };
        let TabletLayout::Single {
            window_id: destination_window_id,
        } = self.workspaces[destination_index].tablet_layout
        else {
            return false;
        };

        self.workspaces[source_index]
            .window_ids
            .retain(|window_id| *window_id != source_window_id);
        self.workspaces[source_index].tablet_layout =
            repaired_layout(&self.workspaces[source_index], source_window_id);

        let destination_index = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == destination_workspace_id)
            .unwrap_or(destination_index);
        if !self.workspaces[destination_index]
            .window_ids
            .contains(&source_window_id)
        {
            self.workspaces[destination_index]
                .window_ids
                .push(source_window_id);
        }
        self.workspaces[destination_index].tablet_layout = TabletLayout::Split {
            axis: sws_protocol::workspace::SplitAxis::Horizontal,
            first_window_id: destination_window_id,
            second_window_id: source_window_id,
            ratio_milli: 500,
        };
        self.active_workspace = destination_workspace_id;
        self.normal_workspace = destination_workspace_id;
        self.presentation = ShellPresentation::Workspace;
        self.ensure_manual_workspace_invariants();
        self.bump_generation();
        true
    }

    /// Validate and atomically accept a complete system-shell transaction.
    pub(super) fn apply_transaction(
        &mut self,
        transaction: WorkspaceTransaction,
        live_window_ids: &[u32],
    ) -> Result<AppliedTransaction, ApplyError> {
        if transaction.base_generation != self.generation {
            return Err(ApplyError::StaleGeneration);
        }
        if validate_workspaces(transaction.active_workspace, &transaction.workspaces).is_err()
            || !same_members(&transaction.workspaces, live_window_ids)
        {
            return Err(ApplyError::InvalidState);
        }

        let previous_active = self.active_workspace;
        let previous_normal = self.normal_workspace;
        let previous_presentation = self.presentation;
        self.active_workspace = transaction.active_workspace;
        self.presentation = transaction.presentation;
        self.workspaces = transaction.workspaces;
        self.normal_workspace = if self.presentation == ShellPresentation::Workspace
            || self.active_workspace != previous_active
        {
            self.active_workspace
        } else if previous_presentation == ShellPresentation::Workspace
            && self
                .workspaces
                .iter()
                .any(|workspace| workspace.id == previous_active)
        {
            previous_active
        } else if self
            .workspaces
            .iter()
            .any(|workspace| workspace.id == previous_normal)
        {
            previous_normal
        } else {
            self.active_workspace
        };
        self.next_workspace_id = self
            .workspaces
            .iter()
            .map(|workspace| workspace.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1);
        self.ensure_manual_workspace_invariants();
        self.bump_generation();
        Ok(AppliedTransaction {
            state: self.snapshot(),
            transition: transaction.transition,
        })
    }

    fn allocate_workspace_id(&mut self) -> WorkspaceId {
        loop {
            let candidate = self.next_workspace_id.max(1);
            self.next_workspace_id = candidate.wrapping_add(1).max(1);
            if !self
                .workspaces
                .iter()
                .any(|workspace| workspace.id == candidate)
            {
                return candidate;
            }
        }
    }

    fn ensure_manual_workspace_invariants(&mut self) -> bool {
        let mut changed = false;
        if self.auto_remove_empty && self.workspaces.len() > 1 {
            let previous_len = self.workspaces.len();
            let active_workspace = self.active_workspace;
            let normal_workspace = self.normal_workspace;
            self.workspaces.retain(|workspace| {
                !workspace.window_ids.is_empty()
                    || workspace.id == active_workspace
                    || workspace.id == normal_workspace
            });
            changed |= self.workspaces.len() != previous_len;
        }
        if self.workspaces.is_empty() {
            let id = self.allocate_workspace_id();
            self.workspaces.push(WorkspaceSnapshot {
                id,
                window_ids: Vec::new(),
                tablet_layout: TabletLayout::Empty,
            });
            self.active_workspace = id;
            self.normal_workspace = id;
            changed = true;
        }

        if !self
            .workspaces
            .iter()
            .any(|workspace| workspace.id == self.active_workspace)
        {
            self.active_workspace = self
                .workspaces
                .iter()
                .find(|workspace| workspace.id == self.normal_workspace)
                .map_or(self.workspaces[0].id, |workspace| workspace.id);
            changed = true;
        }
        if !self
            .workspaces
            .iter()
            .any(|workspace| workspace.id == self.normal_workspace)
        {
            self.normal_workspace = self.active_workspace;
            changed = true;
        }

        changed
    }

    fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1).max(1);
    }
}

fn repaired_layout(workspace: &WorkspaceSnapshot, removed_window_id: u32) -> TabletLayout {
    let fallback = || {
        workspace
            .window_ids
            .last()
            .copied()
            .map_or(TabletLayout::Empty, |window_id| TabletLayout::Single {
                window_id,
            })
    };
    match workspace.tablet_layout {
        TabletLayout::Empty => fallback(),
        TabletLayout::Single { window_id } if window_id == removed_window_id => fallback(),
        TabletLayout::Single { window_id } if workspace.window_ids.contains(&window_id) => {
            TabletLayout::Single { window_id }
        }
        TabletLayout::Single { .. } => fallback(),
        TabletLayout::Split {
            first_window_id,
            second_window_id,
            ..
        } if first_window_id == removed_window_id
            && workspace.window_ids.contains(&second_window_id) =>
        {
            TabletLayout::Single {
                window_id: second_window_id,
            }
        }
        TabletLayout::Split {
            first_window_id,
            second_window_id,
            ..
        } if second_window_id == removed_window_id
            && workspace.window_ids.contains(&first_window_id) =>
        {
            TabletLayout::Single {
                window_id: first_window_id,
            }
        }
        layout @ TabletLayout::Split {
            first_window_id,
            second_window_id,
            ..
        } if workspace.window_ids.contains(&first_window_id)
            && workspace.window_ids.contains(&second_window_id) =>
        {
            layout
        }
        TabletLayout::Split { .. } => fallback(),
    }
}

fn presented_window_id(workspace: &WorkspaceSnapshot) -> Option<u32> {
    match workspace.tablet_layout {
        TabletLayout::Empty => None,
        TabletLayout::Single { window_id } => Some(window_id),
        TabletLayout::Split {
            first_window_id, ..
        } => Some(first_window_id),
    }
}

fn retained_layout_for_members(layout: TabletLayout, window_ids: &[u32]) -> TabletLayout {
    let fallback = || {
        window_ids
            .last()
            .copied()
            .map_or(TabletLayout::Empty, |window_id| TabletLayout::Single {
                window_id,
            })
    };
    match layout {
        TabletLayout::Empty if window_ids.is_empty() => TabletLayout::Empty,
        TabletLayout::Single { window_id } if window_ids.contains(&window_id) => {
            TabletLayout::Single { window_id }
        }
        split @ TabletLayout::Split {
            first_window_id,
            second_window_id,
            ..
        } if window_ids.contains(&first_window_id) && window_ids.contains(&second_window_id) => {
            split
        }
        _ => fallback(),
    }
}

fn same_members(workspaces: &[WorkspaceSnapshot], live_window_ids: &[u32]) -> bool {
    let mut proposed = workspaces
        .iter()
        .flat_map(|workspace| workspace.window_ids.iter().copied())
        .collect::<Vec<_>>();
    let mut live = live_window_ids.to_vec();
    proposed.sort_unstable();
    live.sort_unstable();
    proposed == live
}

#[cfg(test)]
mod tests {
    use super::*;
    use sws_protocol::workspace::{SplitAxis, TransitionKind, ValidationError};

    #[test]
    fn tablet_launches_use_explicitly_created_workspace_compositions() {
        let mut manager = WorkspaceManager::new();
        assert_eq!(manager.add_window(10, true), 1);
        let second = manager.create_workspace().unwrap();
        assert_eq!(manager.add_window(11, true), second);
        assert_ne!(second, 1);
        assert_eq!(manager.active_workspace(), second);
        assert_eq!(
            manager.tablet_layout(1),
            TabletLayout::Single { window_id: 10 }
        );
        assert_eq!(
            manager.tablet_layout(second),
            TabletLayout::Single { window_id: 11 }
        );
        let state = manager.snapshot();
        assert_eq!(state.workspaces.len(), 2);
    }

    #[test]
    fn desktop_launches_share_the_active_virtual_desktop() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        manager.add_window(11, false);
        assert_eq!(manager.snapshot().workspaces[0].window_ids, vec![10, 11]);
        assert_eq!(manager.snapshot().workspaces.len(), 1);
    }

    #[test]
    fn workspace_limit_never_creates_an_unencodable_tablet_workspace() {
        let mut manager = WorkspaceManager::new();
        let workspaces = (1..=MAX_WORKSPACES as u32)
            .map(|id| WorkspaceSnapshot {
                id,
                window_ids: vec![id],
                tablet_layout: TabletLayout::Single { window_id: id },
            })
            .collect::<Vec<_>>();
        let live = (1..=MAX_WORKSPACES as u32).collect::<Vec<_>>();
        let generation = manager.snapshot().generation;
        manager
            .apply_transaction(
                WorkspaceTransaction {
                    base_generation: generation,
                    active_workspace: 1,
                    presentation: ShellPresentation::Workspace,
                    workspaces,
                    transition: TransitionSpec::default(),
                },
                &live,
            )
            .unwrap();

        assert_eq!(manager.add_window(999, true), 1);
        assert_eq!(manager.snapshot().workspaces.len(), MAX_WORKSPACES);
    }

    #[test]
    fn entering_tablet_materializes_every_desktop_scene() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        manager.add_window(11, false);
        manager.add_window(12, false);

        assert!(manager.enter_tablet_experience(Some(11)));
        let state = manager.snapshot();
        assert_eq!(state.workspaces.len(), 3);
        assert_eq!(manager.workspace_for_window(10).is_some(), true);
        assert_eq!(
            manager.workspace_for_window(11),
            Some(state.active_workspace)
        );
        assert_eq!(manager.workspace_for_window(12).is_some(), true);
        assert!(state.workspaces.iter().all(|workspace| {
            matches!(workspace.tablet_layout, TabletLayout::Single { .. })
                && workspace.window_ids.len() == 1
        }));
    }

    #[test]
    fn leaving_tablet_restores_desktop_grouping() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        manager.add_window(11, false);
        manager.add_window(12, false);
        manager.enter_tablet_experience(Some(12));

        assert!(manager.leave_tablet_experience());
        let state = manager.snapshot();
        assert_eq!(state.workspaces.len(), 1);
        assert_eq!(state.workspaces[0].window_ids, vec![10, 11, 12]);
        assert_eq!(state.active_workspace, state.workspaces[0].id);
    }

    #[test]
    fn entering_tablet_keeps_a_valid_split_composition_together() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        manager.add_window(11, false);
        manager.add_window(12, false);
        let state = manager.snapshot();
        manager
            .apply_transaction(
                WorkspaceTransaction {
                    base_generation: state.generation,
                    active_workspace: 1,
                    presentation: ShellPresentation::Workspace,
                    workspaces: vec![WorkspaceSnapshot {
                        id: 1,
                        window_ids: vec![10, 11, 12],
                        tablet_layout: TabletLayout::Split {
                            axis: SplitAxis::Horizontal,
                            first_window_id: 10,
                            second_window_id: 11,
                            ratio_milli: 600,
                        },
                    }],
                    transition: TransitionSpec::default(),
                },
                &[10, 11, 12],
            )
            .unwrap();

        assert!(manager.enter_tablet_experience(Some(12)));
        let state = manager.snapshot();
        assert_eq!(state.workspaces.len(), 2);
        assert_eq!(
            state.workspaces[0].tablet_layout,
            TabletLayout::Split {
                axis: SplitAxis::Horizontal,
                first_window_id: 10,
                second_window_id: 11,
                ratio_milli: 600,
            }
        );
        assert_eq!(
            manager.workspace_for_window(12),
            Some(state.active_workspace)
        );
    }

    #[test]
    fn overview_selection_moves_without_leaving_or_wrapping() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, true);
        let second = manager.create_workspace().unwrap();
        manager.add_window(11, true);
        assert!(manager.set_presentation(ShellPresentation::Overview));
        assert_eq!(manager.active_workspace(), second);

        assert!(!manager.move_overview_selection(1));
        assert_eq!(manager.presentation(), ShellPresentation::Overview);
        assert!(manager.move_overview_selection(-1));
        assert_eq!(manager.active_workspace(), 1);
        assert_eq!(manager.normal_workspace(), 1);
        assert_eq!(manager.presentation(), ShellPresentation::Overview);
        assert!(!manager.move_overview_selection(-1));
        assert!(manager.move_overview_selection(1));
        assert_eq!(manager.active_workspace(), second);
        assert_eq!(manager.normal_workspace(), second);
    }

    #[test]
    fn home_workspace_selection_moves_without_closing_the_drawer() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, true);
        let second = manager.create_workspace().unwrap();
        manager.add_window(11, true);
        assert!(manager.set_presentation(ShellPresentation::Home));

        assert!(manager.move_overview_selection(-1));
        assert_eq!(manager.active_workspace(), 1);
        assert_eq!(manager.normal_workspace(), 1);
        assert_eq!(manager.presentation(), ShellPresentation::Home);
        assert!(!manager.move_overview_selection(-1));
        assert!(manager.move_overview_selection(1));
        assert_eq!(manager.active_workspace(), second);
        assert_eq!(manager.normal_workspace(), second);
        assert_eq!(manager.presentation(), ShellPresentation::Home);
    }

    #[test]
    fn normal_workspace_navigation_stops_at_both_edges() {
        let mut manager = WorkspaceManager::new();
        let second = manager.create_workspace().unwrap();

        assert!(!manager.cycle_workspace(1));
        assert_eq!(manager.active_workspace(), second);
        assert!(manager.cycle_workspace(-1));
        assert_eq!(manager.active_workspace(), 1);
        assert!(!manager.cycle_workspace(-1));
        assert_eq!(manager.active_workspace(), 1);
    }

    #[test]
    fn overview_toggle_round_trips_and_dismisses_home() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);

        assert!(manager.toggle_overview());
        assert_eq!(manager.presentation(), ShellPresentation::Overview);
        assert!(manager.toggle_overview());
        assert_eq!(manager.presentation(), ShellPresentation::Workspace);

        manager.set_presentation(ShellPresentation::Home);
        assert!(manager.toggle_overview());
        assert_eq!(manager.presentation(), ShellPresentation::Workspace);
    }

    #[test]
    fn moving_a_window_right_follows_it_atomically() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        manager.add_window(11, false);
        let second = manager.create_workspace().unwrap();
        manager.cycle_workspace(-1);

        assert!(manager.move_window_to_adjacent_workspace(11, 1));
        let state = manager.snapshot();
        assert_eq!(state.workspaces[0].window_ids, vec![10]);
        assert_eq!(state.workspaces[1].window_ids, vec![11]);
        assert_eq!(manager.active_workspace(), second);
        assert_eq!(manager.normal_workspace(), second);
        assert_eq!(manager.presentation(), ShellPresentation::Workspace);
        assert_eq!(
            state.workspaces[1].tablet_layout,
            TabletLayout::Single { window_id: 11 }
        );
    }

    #[test]
    fn moving_left_from_the_first_workspace_is_rejected() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);

        assert!(!manager.move_window_to_adjacent_workspace(10, -1));
    }

    #[test]
    fn moving_the_last_member_back_preserves_the_vacated_workspace() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        manager.add_window(11, false);
        manager.create_workspace();
        manager.cycle_workspace(-1);
        manager.move_window_to_adjacent_workspace(11, 1);

        assert!(manager.move_window_to_adjacent_workspace(11, -1));
        let state = manager.snapshot();
        assert_eq!(state.workspaces.len(), 2);
        assert_eq!(state.workspaces[0].window_ids, vec![10, 11]);
        assert!(state.workspaces[1].window_ids.is_empty());
    }

    #[test]
    fn drag_and_drop_moves_directly_to_an_arbitrary_workspace() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, true);
        manager.create_workspace();
        manager.add_window(11, true);
        manager.create_workspace();
        manager.add_window(12, true);
        let state = manager.snapshot();
        let first = state.workspaces[0].id;
        let second = state.workspaces[1].id;
        let third = state.workspaces[2].id;
        let active = manager.active_workspace();
        manager.set_presentation(ShellPresentation::Overview);

        assert!(manager.move_window_to_workspace(12, first));
        let state = manager.snapshot();
        assert_eq!(state.workspaces[0].window_ids, vec![10, 12]);
        assert_eq!(state.workspaces[1].window_ids, vec![11]);
        assert!(
            manager
                .workspace_for_window(12)
                .is_some_and(|id| id == first)
        );
        assert_ne!(second, third);
        assert_eq!(manager.active_workspace(), active);
        assert_eq!(manager.presentation(), ShellPresentation::Overview);
    }

    #[test]
    fn focused_drop_forms_a_split_without_leaving_overview() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, true);
        manager.create_workspace();
        manager.add_window(11, true);
        let state = manager.snapshot();
        let first = state.workspaces[0].id;
        let second = state.workspaces[1].id;
        manager.set_presentation(ShellPresentation::Overview);

        assert!(manager.move_window_to_workspace_as_split(11, first));
        assert_eq!(manager.presentation(), ShellPresentation::Overview);
        assert_eq!(manager.active_workspace(), second);
        assert_eq!(
            manager.tablet_layout(first),
            TabletLayout::Split {
                axis: SplitAxis::Horizontal,
                first_window_id: 10,
                second_window_id: 11,
                ratio_milli: 500,
            }
        );
        assert_eq!(manager.workspace_for_window(11), Some(first));
        assert!(
            manager
                .snapshot()
                .workspaces
                .iter()
                .any(|workspace| workspace.id == second && workspace.window_ids.is_empty())
        );
    }

    #[test]
    fn destroying_one_split_side_repairs_to_single() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        manager.add_window(11, false);
        let generation = manager.snapshot().generation;
        manager
            .apply_transaction(
                WorkspaceTransaction {
                    base_generation: generation,
                    active_workspace: 1,
                    presentation: ShellPresentation::Workspace,
                    workspaces: vec![WorkspaceSnapshot {
                        id: 1,
                        window_ids: vec![10, 11],
                        tablet_layout: TabletLayout::Split {
                            axis: SplitAxis::Horizontal,
                            first_window_id: 10,
                            second_window_id: 11,
                            ratio_milli: 500,
                        },
                    }],
                    transition: TransitionSpec {
                        kind: TransitionKind::Immediate,
                        duration_ms: 0,
                    },
                },
                &[10, 11],
            )
            .unwrap();
        assert!(manager.remove_window(10));
        assert_eq!(
            manager.tablet_layout(1),
            TabletLayout::Single { window_id: 11 }
        );
    }

    #[test]
    fn closing_active_tablet_composition_preserves_its_empty_workspace() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, true);
        let second = manager.create_workspace().unwrap();
        manager.add_window(11, true);
        assert_eq!(manager.active_workspace(), second);
        assert!(manager.remove_window(11));
        assert!(!manager.settle_after_close(true));
        assert_eq!(manager.presentation(), ShellPresentation::Workspace);
        assert_eq!(manager.snapshot().workspaces.len(), 2);
        assert_eq!(manager.active_workspace(), second);
        assert!(
            manager
                .snapshot()
                .workspaces
                .iter()
                .find(|workspace| workspace.id == second)
                .unwrap()
                .window_ids
                .is_empty()
        );
    }

    #[test]
    fn desktop_close_preserves_empty_virtual_desktop() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        assert!(manager.remove_window(10));
        assert!(!manager.settle_after_close(false));
        assert_eq!(manager.presentation(), ShellPresentation::Workspace);
        assert_eq!(manager.snapshot().workspaces.len(), 1);
    }

    #[test]
    fn selecting_and_filling_an_explicit_empty_workspace_does_not_create_another() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        let empty = manager.create_workspace().unwrap();
        manager.cycle_workspace(-1);
        manager.set_presentation(ShellPresentation::Overview);

        assert!(manager.select_workspace_from_overview(empty));
        assert_eq!(manager.presentation(), ShellPresentation::Workspace);
        assert_eq!(manager.add_window(11, false), empty);

        let state = manager.snapshot();
        assert_eq!(state.presentation, ShellPresentation::Workspace);
        assert_eq!(state.workspaces.len(), 2);
        assert_eq!(state.workspaces[1].window_ids, vec![11]);
    }

    #[test]
    fn dismissing_overview_enters_the_selected_empty_workspace() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        let empty = manager.create_workspace().unwrap();
        manager.cycle_workspace(-1);
        manager.set_presentation(ShellPresentation::Overview);
        assert!(manager.move_overview_selection(1));
        assert_eq!(manager.active_workspace(), empty);
        assert_eq!(manager.normal_workspace(), empty);

        assert!(manager.return_to_workspace());
        assert_eq!(manager.active_workspace(), empty);
        assert_eq!(manager.normal_workspace(), empty);
        assert_eq!(manager.presentation(), ShellPresentation::Workspace);
    }

    #[test]
    fn every_shell_exit_path_uses_the_latest_overview_selection() {
        for exit_path in 0..3 {
            let mut manager = WorkspaceManager::new();
            manager.add_window(10, false);
            let second = manager.create_workspace().unwrap();
            manager.set_presentation(ShellPresentation::Overview);
            assert!(manager.move_overview_selection(-1));
            assert_eq!(manager.active_workspace(), 1);
            assert_eq!(manager.normal_workspace(), 1);

            match exit_path {
                0 => assert!(manager.return_to_workspace()),
                1 => assert!(manager.toggle_overview()),
                2 => {
                    assert!(manager.set_presentation(ShellPresentation::Home));
                    assert!(manager.return_to_workspace());
                }
                _ => unreachable!(),
            }

            assert_eq!(manager.active_workspace(), 1);
            assert_eq!(manager.normal_workspace(), 1);
            assert_eq!(manager.presentation(), ShellPresentation::Workspace);
            assert_ne!(manager.active_workspace(), second);
        }
    }

    #[test]
    fn dropping_on_add_target_creates_and_moves_once() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        manager.add_window(11, false);
        manager.set_presentation(ShellPresentation::Overview);
        let generation = manager.snapshot().generation;

        let created = manager.move_window_to_new_workspace(11).unwrap();
        let state = manager.snapshot();
        assert_eq!(state.generation, generation + 1);
        assert_eq!(state.workspaces.len(), 2);
        assert_eq!(state.workspaces[0].window_ids, vec![10]);
        assert_eq!(state.workspaces[1].id, created);
        assert_eq!(state.workspaces[1].window_ids, vec![11]);
        assert_eq!(state.active_workspace, created);
        assert_eq!(state.normal_workspace, created);
        assert_eq!(state.presentation, ShellPresentation::Overview);
    }

    #[test]
    fn explicit_removal_migrates_scenes_and_never_destroys_them() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        let second = manager.create_workspace().unwrap();
        manager.add_window(11, false);

        assert!(manager.remove_workspace(second, true));
        let state = manager.snapshot();
        assert_eq!(state.workspaces.len(), 1);
        assert_eq!(state.workspaces[0].window_ids, vec![10, 11]);
        assert_eq!(state.active_workspace, state.workspaces[0].id);
        assert_eq!(state.normal_workspace, state.workspaces[0].id);
    }

    #[test]
    fn focused_removal_forms_a_split_without_losing_either_scene() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, true);
        let second = manager.create_workspace().unwrap();
        manager.add_window(11, true);
        manager.set_presentation(ShellPresentation::Overview);

        assert!(manager.remove_workspace(second, false));
        let state = manager.snapshot();
        assert_eq!(state.workspaces.len(), 1);
        assert_eq!(state.workspaces[0].window_ids, vec![10, 11]);
        assert_eq!(
            state.workspaces[0].tablet_layout,
            TabletLayout::Split {
                axis: SplitAxis::Horizontal,
                first_window_id: 10,
                second_window_id: 11,
                ratio_milli: 500,
            }
        );
        assert_eq!(manager.workspace_for_window(10), Some(1));
        assert_eq!(manager.workspace_for_window(11), Some(1));
    }

    #[test]
    fn automatic_empty_removal_is_opt_in() {
        let mut manual = WorkspaceManager::new();
        manual.create_workspace();
        manual.cycle_workspace(-1);
        manual.add_window(10, false);
        assert_eq!(manual.snapshot().workspaces.len(), 2);

        let mut automatic = WorkspaceManager::with_auto_remove_empty(true);
        automatic.create_workspace();
        automatic.cycle_workspace(-1);
        automatic.add_window(10, false);
        assert_eq!(automatic.snapshot().workspaces.len(), 1);
    }

    #[test]
    fn stale_transaction_has_no_partial_effect() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        let before = manager.snapshot();
        let result = manager.apply_transaction(
            WorkspaceTransaction {
                base_generation: before.generation.saturating_sub(1),
                active_workspace: 1,
                presentation: ShellPresentation::Home,
                workspaces: before.workspaces.clone(),
                transition: TransitionSpec::default(),
            },
            &[10],
        );
        assert_eq!(result, Err(ApplyError::StaleGeneration));
        assert_eq!(manager.snapshot(), before);
    }

    #[test]
    fn transaction_cannot_drop_or_invent_live_scenes() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        let state = manager.snapshot();
        let result = manager.apply_transaction(
            WorkspaceTransaction {
                base_generation: state.generation,
                active_workspace: 1,
                presentation: ShellPresentation::Workspace,
                workspaces: vec![WorkspaceSnapshot {
                    id: 1,
                    window_ids: vec![99],
                    tablet_layout: TabletLayout::Single { window_id: 99 },
                }],
                transition: TransitionSpec::default(),
            },
            &[10],
        );
        assert_eq!(result, Err(ApplyError::InvalidState));
        assert_eq!(manager.snapshot(), state);
    }

    #[test]
    fn protocol_validation_error_type_remains_exhaustive_for_manager_mapping() {
        let errors = [
            ValidationError::InvalidWorkspaceCount,
            ValidationError::InvalidWorkspaceId,
            ValidationError::ActiveWorkspaceMissing,
            ValidationError::NormalWorkspaceMissing,
            ValidationError::InvalidMember,
            ValidationError::InvalidTabletLayout,
        ];
        assert_eq!(errors.len(), 6);
    }
}
