//! Authoritative session workspace registry and output-scoped state machines.

use std::{
    collections::BTreeMap,
    ops::Deref,
    string::{String, ToString},
    vec::Vec,
};
use sws_protocol::outputs::OutputId;
use sws_protocol::workspace::{
    MAX_WORKSPACES, ShellPresentation, TabletLayout, TransitionSpec, WorkspaceId,
    WorkspaceSnapshot, WorkspaceState, WorkspaceTransaction, validate_workspaces,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AppliedTransaction {
    pub(super) state: WorkspaceState,
    pub(super) transition: TransitionSpec,
}

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

#[derive(Debug, Clone, Copy)]
struct OutputWorkspace {
    active_workspace: WorkspaceId,
    normal_workspace: WorkspaceId,
    presentation: ShellPresentation,
}

/// One partition of the session registry. Empty detached partitions retain
/// connector identity; their workspaces have been recovered onto the primary.
#[derive(Clone)]
struct WorkspacePartition {
    output: Option<OutputId>,
    connector_key: String,
    state: OutputWorkspaces,
}

#[derive(Clone)]
pub(super) struct OutputWorkspaces {
    generation: u32,
    tablet_experience: bool,
    selection: OutputWorkspace,
    workspaces: Vec<WorkspaceSnapshot>,
    desktop_restore: Option<DesktopWorkspaceRestore>,
    auto_remove_empty: bool,
}

/// IDs and scene ownership are shared across all partitions, including those
/// retained for disconnected outputs. They cannot be minted independently by
/// per-monitor shell instances. Reservations survive tablet restoration.
#[derive(Clone)]
struct WorkspaceNamespace {
    next_id: WorkspaceId,
    ids: BTreeMap<WorkspaceId, usize>,
    window_owners: BTreeMap<u32, (usize, WorkspaceId)>,
    workspace_count: usize,
}

impl WorkspaceNamespace {
    fn allocate(&mut self, group: usize) -> WorkspaceId {
        loop {
            let id = self.next_id.max(1);
            self.next_id = id.wrapping_add(1).max(1);
            if let std::collections::btree_map::Entry::Vacant(entry) = self.ids.entry(id) {
                entry.insert(group);
                return id;
            }
        }
    }
}

/// Explicit mutable output scope. No process-global or mutable "current output"
/// exists. Queries borrow their partition without copying scene snapshots.
pub(super) struct WorkspaceScope<'a> {
    state: &'a mut OutputWorkspaces,
    namespace: &'a mut WorkspaceNamespace,
    group: usize,
    original_count: usize,
    membership_dirty: bool,
}

impl Deref for WorkspaceScope<'_> {
    type Target = OutputWorkspaces;
    fn deref(&self) -> &Self::Target {
        self.state
    }
}

impl Drop for WorkspaceScope<'_> {
    fn drop(&mut self) {
        if !self.membership_dirty {
            return;
        }
        self.namespace.ids.retain(|_, owner| *owner != self.group);
        for workspace in &self.state.workspaces {
            self.namespace.ids.insert(workspace.id, self.group);
        }
        if let Some(restore) = &self.state.desktop_restore {
            for workspace in &restore.workspaces {
                self.namespace.ids.insert(workspace.id, self.group);
            }
        }
        self.namespace.workspace_count =
            self.namespace.workspace_count - self.original_count + self.state.workspaces.len();
        self.namespace
            .window_owners
            .retain(|_, (owner, _)| *owner != self.group);
        for workspace in &self.state.workspaces {
            for window in &workspace.window_ids {
                self.namespace
                    .window_owners
                    .insert(*window, (self.group, workspace.id));
            }
        }
    }
}

/// The single authoritative registry. Storage is partitioned by stable
/// connector so mode changes and pruning never rewrite another output's data.
#[derive(Clone)]
pub(super) struct WorkspaceManager {
    partitions: Vec<WorkspacePartition>,
    namespace: WorkspaceNamespace,
    auto_remove_empty: bool,
}

impl OutputWorkspaces {
    fn initial(id: WorkspaceId, auto_remove_empty: bool) -> Self {
        Self {
            generation: 1,
            tablet_experience: false,
            selection: OutputWorkspace {
                active_workspace: id,
                normal_workspace: id,
                presentation: ShellPresentation::Workspace,
            },
            workspaces: vec![WorkspaceSnapshot {
                id,
                window_ids: Vec::new(),
                tablet_layout: TabletLayout::Empty,
            }],
            desktop_restore: None,
            auto_remove_empty,
        }
    }
    /// Return a complete owned compositor snapshot.
    pub(super) fn snapshot(&self) -> WorkspaceState {
        WorkspaceState {
            generation: self.generation,
            active_workspace: self.selection.active_workspace,
            normal_workspace: self.selection.normal_workspace,
            presentation: self.selection.presentation,
            workspaces: self.workspaces.clone(),
        }
    }

    /// Return the currently selected workspace.
    pub(super) fn active_workspace(&self) -> WorkspaceId {
        self.selection.active_workspace
    }

    /// Return the committed workspace restored when shell navigation closes.
    pub(super) fn normal_workspace(&self) -> WorkspaceId {
        self.selection.normal_workspace
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

    /// Return the current shell presentation.
    pub(super) fn presentation(&self) -> ShellPresentation {
        self.selection.presentation
    }

    pub(super) fn tablet_experience(&self) -> bool {
        self.tablet_experience
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
}

impl WorkspaceScope<'_> {
    /// Append one explicitly requested empty workspace.
    ///
    /// The new workspace is selected immediately and becomes the committed
    /// return destination even when it is created from shell navigation.
    ///
    /// # Returns
    ///
    /// The new stable workspace identifier, or `None` at the protocol limit.
    pub(super) fn create_workspace(&mut self) -> Option<WorkspaceId> {
        self.membership_dirty = true;
        if self.namespace.workspace_count - self.original_count + self.state.workspaces.len()
            >= MAX_WORKSPACES
        {
            return None;
        }
        let id = self.allocate_workspace_id();
        self.state.workspaces.push(WorkspaceSnapshot {
            id,
            window_ids: Vec::new(),
            tablet_layout: TabletLayout::Empty,
        });
        self.state.selection.active_workspace = id;
        self.state.selection.normal_workspace = id;
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
        self.membership_dirty = true;
        if self.namespace.workspace_count - self.original_count + self.state.workspaces.len()
            >= MAX_WORKSPACES
        {
            return None;
        }
        let source_index = self
            .state
            .workspaces
            .iter()
            .position(|workspace| workspace.window_ids.contains(&window_id))?;
        self.state.workspaces[source_index]
            .window_ids
            .retain(|candidate| *candidate != window_id);
        self.state.workspaces[source_index].tablet_layout =
            repaired_layout(&self.state.workspaces[source_index], window_id);

        let id = self.allocate_workspace_id();
        self.state.workspaces.push(WorkspaceSnapshot {
            id,
            window_ids: vec![window_id],
            tablet_layout: TabletLayout::Single { window_id },
        });
        self.state.selection.active_workspace = id;
        self.state.selection.normal_workspace = id;
        self.ensure_manual_workspace_invariants();
        self.bump_generation();
        Some(id)
    }

    /// Explicitly remove one workspace without destroying any scene.
    ///
    /// See [`Self::can_remove_workspace`] for the migration constraints.
    pub(super) fn remove_workspace(
        &mut self,
        workspace_id: WorkspaceId,
        allow_freeform_merge: bool,
    ) -> bool {
        self.membership_dirty = true;
        if !self.can_remove_workspace(workspace_id, allow_freeform_merge) {
            return false;
        }
        let Some(source_index) = self
            .state
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
        let target_id = self.state.workspaces[target_index].id;
        let source_members = self.state.workspaces[source_index].window_ids.clone();
        let source_layout = self.state.workspaces[source_index].tablet_layout;
        let target_members = self.state.workspaces[target_index].window_ids.clone();
        let target_layout = self.state.workspaces[target_index].tablet_layout;

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
                if !self.state.workspaces[target_index]
                    .window_ids
                    .contains(&window_id)
                {
                    self.state.workspaces[target_index]
                        .window_ids
                        .push(window_id);
                }
            }
            self.state.workspaces[target_index].tablet_layout = merged_layout;
        }

        self.state.workspaces.remove(source_index);
        if self.state.selection.active_workspace == workspace_id {
            self.state.selection.active_workspace = target_id;
        }
        if self.state.selection.normal_workspace == workspace_id {
            self.state.selection.normal_workspace = target_id;
        }
        self.ensure_manual_workspace_invariants();
        self.bump_generation();
        true
    }

    /// Assign a newly created scene root to the active manual workspace.
    ///
    /// Workspace creation is always explicit. Desktop launches join the
    /// current freeform workspace. In focused mode the new scene becomes the
    /// presented single scene without silently creating another workspace;
    /// existing members remain retained for the desktop projection.
    pub(super) fn add_window(&mut self, window_id: u32, tablet_experience: bool) -> WorkspaceId {
        self.membership_dirty = true;
        if let Some((owner, workspace)) = self.namespace.window_owners.get(&window_id) {
            if *owner != self.group {
                return *workspace;
            }
        }

        if let Some(workspace_id) = self.workspace_for_window(window_id) {
            return workspace_id;
        }

        let active_index = self
            .state
            .workspaces
            .iter()
            .position(|workspace| workspace.id == self.state.selection.active_workspace)
            .unwrap_or(0);
        let workspace = &mut self.state.workspaces[active_index];
        workspace.window_ids.push(window_id);
        if tablet_experience || matches!(workspace.tablet_layout, TabletLayout::Empty) {
            workspace.tablet_layout = TabletLayout::Single { window_id };
        }
        let workspace_id = workspace.id;
        self.state.selection.active_workspace = workspace_id;
        self.state.selection.normal_workspace = workspace_id;
        self.state.selection.presentation = ShellPresentation::Workspace;
        self.ensure_manual_workspace_invariants();
        self.bump_generation();
        workspace_id
    }

    /// Assign a newly confirmed top-level app scene to its launch workspace.
    ///
    /// Desktop launches remain members of the selected virtual desktop. In
    /// tablet experience an occupied composition is never silently replaced:
    /// an empty selected workspace is filled, otherwise a new `Single`
    /// workspace is appended and selected atomically. At the protocol limit an
    /// existing empty workspace is preferred before falling back to the active
    /// workspace.
    ///
    /// # Arguments
    ///
    /// * `window_id` - Confirmed top-level scene-root identifier.
    /// * `tablet_experience` - Whether launches use one composition per
    ///   workspace.
    /// * `focused_layout` - Whether the active projection should immediately
    ///   present the new scene as `Single`.
    ///
    /// # Returns
    ///
    /// The workspace containing the scene after assignment.
    pub(super) fn add_scene_root(
        &mut self,
        window_id: u32,
        tablet_experience: bool,
        focused_layout: bool,
    ) -> WorkspaceId {
        self.membership_dirty = true;
        if let Some((owner, workspace)) = self.namespace.window_owners.get(&window_id) {
            if *owner != self.group {
                return *workspace;
            }
        }

        if let Some(workspace_id) = self.workspace_for_window(window_id) {
            return workspace_id;
        }
        if !tablet_experience {
            return self.add_window(window_id, focused_layout);
        }

        let active_index = self
            .state
            .workspaces
            .iter()
            .position(|workspace| workspace.id == self.state.selection.active_workspace)
            .unwrap_or(0);
        if self.state.workspaces[active_index].window_ids.is_empty() {
            return self.add_window(window_id, true);
        }

        let target_index = if self.namespace.workspace_count - self.original_count
            + self.state.workspaces.len()
            < MAX_WORKSPACES
        {
            let id = self.allocate_workspace_id();
            self.state.workspaces.push(WorkspaceSnapshot {
                id,
                window_ids: Vec::new(),
                tablet_layout: TabletLayout::Empty,
            });
            self.state.workspaces.len() - 1
        } else if let Some(index) = self
            .state
            .workspaces
            .iter()
            .position(|workspace| workspace.window_ids.is_empty())
        {
            index
        } else {
            return self.add_window(window_id, true);
        };

        let workspace = &mut self.state.workspaces[target_index];
        workspace.window_ids.push(window_id);
        workspace.tablet_layout = TabletLayout::Single { window_id };
        let workspace_id = workspace.id;
        self.state.selection.active_workspace = workspace_id;
        self.state.selection.normal_workspace = workspace_id;
        self.state.selection.presentation = ShellPresentation::Workspace;
        self.ensure_manual_workspace_invariants();
        self.bump_generation();
        workspace_id
    }

    /// Remove a destroyed scene root and repair every referencing layout.
    pub(super) fn remove_window(&mut self, window_id: u32) -> bool {
        self.membership_dirty = true;
        let mut changed = false;
        for workspace in &mut self.state.workspaces {
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

    /// Reconcile workspace lifecycle after a scene closes or moves away.
    ///
    /// Tablet experience removes empty compositions, selecting the nearest
    /// surviving predecessor (or the first successor) when necessary. When
    /// every workspace is empty, the active workspace remains as a placeholder.
    /// Desktop experience retains manually created empty workspaces.
    pub(super) fn settle_after_close(&mut self, tablet_experience: bool) -> bool {
        self.membership_dirty = true;
        let mut changed = self.ensure_manual_workspace_invariants();
        if tablet_experience {
            changed |= prune_empty_workspaces(
                &mut self.state.workspaces,
                &mut self.state.selection.active_workspace,
                &mut self.state.selection.normal_workspace,
            );

            // Keep original desktop groups while any of their scenes survive,
            // even when the group's original tablet card has been removed.
            // Empty saved groups must not resurrect pruned cards on mode exit.
            if let Some(restore) = &mut self.state.desktop_restore {
                for workspace in &mut restore.workspaces {
                    workspace.window_ids.retain(|window_id| {
                        self.state
                            .workspaces
                            .iter()
                            .any(|current| current.window_ids.contains(window_id))
                    });
                    workspace.tablet_layout =
                        retained_layout_for_members(workspace.tablet_layout, &workspace.window_ids);
                }
                if restore
                    .workspaces
                    .iter()
                    .all(|workspace| workspace.window_ids.is_empty())
                {
                    restore.active_workspace = self.state.selection.active_workspace;
                    restore.workspaces = vec![
                        self.state
                            .workspaces
                            .iter()
                            .find(|workspace| workspace.id == self.state.selection.active_workspace)
                            .unwrap()
                            .clone(),
                    ];
                } else {
                    let mut normal_workspace = restore.active_workspace;
                    prune_empty_workspaces(
                        &mut restore.workspaces,
                        &mut restore.active_workspace,
                        &mut normal_workspace,
                    );
                }
            }
        }

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
            .state
            .workspaces
            .iter()
            .position(|workspace| workspace.window_ids.contains(&window_id))
        else {
            return false;
        };
        let workspace_id = self.state.workspaces[index].id;
        let mut changed = self.state.selection.active_workspace != workspace_id
            || self.state.selection.normal_workspace != workspace_id
            || self.state.selection.presentation != ShellPresentation::Workspace;
        self.state.selection.active_workspace = workspace_id;
        self.state.selection.normal_workspace = workspace_id;
        self.state.selection.presentation = ShellPresentation::Workspace;
        if tablet_experience {
            let layout_contains_window = match self.state.workspaces[index].tablet_layout {
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
                self.state.workspaces[index].tablet_layout = TabletLayout::Single { window_id };
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
        let previous_presentation = self.state.selection.presentation;
        let previous_active = self.state.selection.active_workspace;
        let previous_normal = self.state.selection.normal_workspace;
        if presentation == ShellPresentation::Overview
            && previous_presentation == ShellPresentation::Workspace
        {
            self.state.selection.normal_workspace = self.state.selection.active_workspace;
        } else if presentation == ShellPresentation::Workspace {
            self.state.selection.active_workspace = self.state.selection.normal_workspace;
        } else if presentation == ShellPresentation::Home
            && previous_presentation == ShellPresentation::Workspace
        {
            self.state.selection.normal_workspace = self.state.selection.active_workspace;
        }
        self.state.selection.presentation = presentation;
        if self.state.selection.presentation == previous_presentation
            && self.state.selection.active_workspace == previous_active
            && self.state.selection.normal_workspace == previous_normal
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
        if self.state.selection.presentation == ShellPresentation::Workspace {
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
        let previous_workspace = self.state.selection.active_workspace;
        let previous_presentation = self.state.selection.presentation;
        self.state.selection.active_workspace = self.state.selection.normal_workspace;
        self.state.selection.presentation = ShellPresentation::Workspace;
        let changed = self.state.selection.active_workspace != previous_workspace
            || self.state.selection.presentation != previous_presentation;
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
        if self.state.selection.presentation == ShellPresentation::Workspace {
            return false;
        }
        let Some(_workspace) = self
            .state
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
        else {
            return false;
        };
        let presentation = ShellPresentation::Workspace;
        if self.state.selection.active_workspace == workspace_id
            && self.state.selection.presentation == presentation
        {
            return false;
        }
        self.state.selection.active_workspace = workspace_id;
        self.state.selection.normal_workspace = workspace_id;
        self.state.selection.presentation = presentation;
        self.bump_generation();
        true
    }

    /// Select an adjacent ordered workspace without wrapping at either edge.
    pub(super) fn cycle_workspace(&mut self, direction: i32) -> bool {
        if self.state.workspaces.len() < 2 || direction == 0 {
            return false;
        }
        let index = self
            .state
            .workspaces
            .iter()
            .position(|workspace| workspace.id == self.state.selection.active_workspace)
            .unwrap_or(0);
        let next = if direction < 0 {
            let Some(previous) = index.checked_sub(1) else {
                return false;
            };
            previous
        } else {
            let next = index.saturating_add(1);
            if next >= self.state.workspaces.len() {
                return false;
            }
            next
        };
        self.state.selection.active_workspace = self.state.workspaces[next].id;
        self.state.selection.normal_workspace = self.state.selection.active_workspace;
        self.state.selection.presentation = ShellPresentation::Workspace;
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
            .state
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
        if target_index >= self.state.workspaces.len() {
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
            .state
            .workspaces
            .iter()
            .position(|workspace| workspace.window_ids.contains(&window_id))
        else {
            return false;
        };
        let Some(target_index) = self
            .state
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
        self.membership_dirty = true;
        let Some(source_index) = self
            .state
            .workspaces
            .iter()
            .position(|workspace| workspace.window_ids.contains(&window_id))
        else {
            return false;
        };
        let Some(target_index) = self
            .state
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
        } = self.state.workspaces[target_index].tablet_layout
        else {
            return false;
        };

        self.state.workspaces[source_index]
            .window_ids
            .retain(|candidate| *candidate != window_id);
        self.state.workspaces[source_index].tablet_layout =
            repaired_layout(&self.state.workspaces[source_index], window_id);
        if !self.state.workspaces[target_index]
            .window_ids
            .contains(&window_id)
        {
            self.state.workspaces[target_index]
                .window_ids
                .push(window_id);
        }
        self.state.workspaces[target_index].tablet_layout = TabletLayout::Split {
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
        self.membership_dirty = true;
        if source_index == target_index {
            return false;
        }

        let target_id = self.state.workspaces[target_index].id;
        let source = &mut self.state.workspaces[source_index];
        source
            .window_ids
            .retain(|candidate| *candidate != window_id);
        source.tablet_layout = repaired_layout(source, window_id);
        let target = &mut self.state.workspaces[target_index];
        if let TabletLayout::Empty = target.tablet_layout {
            target.tablet_layout = TabletLayout::Single { window_id };
        }
        target.window_ids.push(window_id);

        if activate_target {
            self.state.selection.active_workspace = target_id;
            self.state.selection.normal_workspace = target_id;
            self.state.selection.presentation = ShellPresentation::Workspace;
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
        if self.state.selection.presentation == ShellPresentation::Workspace
            || self.state.workspaces.len() < 2
            || direction == 0
        {
            return false;
        }
        let index = self
            .state
            .workspaces
            .iter()
            .position(|workspace| workspace.id == self.state.selection.active_workspace)
            .unwrap_or(0);
        let next = if direction < 0 {
            index.saturating_sub(1)
        } else {
            index.saturating_add(1).min(self.state.workspaces.len() - 1)
        };
        if next == index {
            return false;
        }
        self.state.selection.active_workspace = self.state.workspaces[next].id;
        self.state.selection.normal_workspace = self.state.selection.active_workspace;
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
        self.membership_dirty = true;
        if self.state.desktop_restore.is_some() {
            return false;
        }
        let expanded_count = self.state.workspaces.iter().map(|workspace| {
            let retained_split = matches!(workspace.tablet_layout, TabletLayout::Split { first_window_id, second_window_id, .. }
                if workspace.window_ids.contains(&first_window_id) && workspace.window_ids.contains(&second_window_id));
            workspace.window_ids.len().saturating_sub(usize::from(retained_split)).max(1)
        }).sum::<usize>();
        if self.namespace.workspace_count - self.original_count + expanded_count > MAX_WORKSPACES {
            return false;
        }
        self.state.desktop_restore = Some(DesktopWorkspaceRestore {
            active_workspace: self.state.selection.active_workspace,
            workspaces: self.state.workspaces.clone(),
        });
        self.state.tablet_experience = true;

        let original = self.state.workspaces.clone();
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
        let previous = self.state.workspaces.clone();
        let previous_active = self.state.selection.active_workspace;
        self.state.workspaces = expanded;
        self.ensure_manual_workspace_invariants();
        self.state.selection.active_workspace = next_active.unwrap_or_else(|| {
            self.state
                .workspaces
                .iter()
                .find(|workspace| workspace.id == self.state.selection.active_workspace)
                .map_or(self.state.workspaces[0].id, |workspace| workspace.id)
        });
        let changed = self.state.workspaces != previous
            || self.state.selection.active_workspace != previous_active;
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
        self.membership_dirty = true;
        let Some(restore) = self.state.desktop_restore.take() else {
            return false;
        };
        self.state.tablet_experience = false;
        let current = self.state.workspaces.clone();
        let live_window_ids = current
            .iter()
            .flat_map(|workspace| workspace.window_ids.iter().copied())
            .collect::<Vec<_>>();
        let active_member = current
            .iter()
            .find(|workspace| workspace.id == self.state.selection.active_workspace)
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
            let destination = current_workspace
                .window_ids
                .iter()
                .find_map(|window_id| {
                    restored
                        .iter()
                        .position(|workspace| workspace.window_ids.contains(window_id))
                })
                .or_else(|| {
                    restored.iter().position(|workspace| {
                        workspace.id == current_workspace.id && workspace.window_ids.is_empty()
                    })
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

        let previous = self.state.workspaces.clone();
        let previous_active = self.state.selection.active_workspace;
        self.state.workspaces = restored;
        self.state.selection.active_workspace = active_member
            .and_then(|window_id| self.workspace_for_window(window_id))
            .or_else(|| {
                self.state
                    .workspaces
                    .iter()
                    .any(|workspace| workspace.id == restore.active_workspace)
                    .then_some(restore.active_workspace)
            })
            .unwrap_or(self.state.workspaces[0].id);
        self.ensure_manual_workspace_invariants();
        let changed = self.state.workspaces != previous
            || self.state.selection.active_workspace != previous_active;
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
        self.membership_dirty = true;
        if source_workspace_id == destination_workspace_id {
            return false;
        }
        let Some(source_index) = self
            .state
            .workspaces
            .iter()
            .position(|workspace| workspace.id == source_workspace_id)
        else {
            return false;
        };
        let Some(destination_index) = self
            .state
            .workspaces
            .iter()
            .position(|workspace| workspace.id == destination_workspace_id)
        else {
            return false;
        };
        let TabletLayout::Single {
            window_id: source_window_id,
        } = self.state.workspaces[source_index].tablet_layout
        else {
            return false;
        };
        let TabletLayout::Single {
            window_id: destination_window_id,
        } = self.state.workspaces[destination_index].tablet_layout
        else {
            return false;
        };

        self.state.workspaces[source_index]
            .window_ids
            .retain(|window_id| *window_id != source_window_id);
        self.state.workspaces[source_index].tablet_layout =
            repaired_layout(&self.state.workspaces[source_index], source_window_id);

        let destination_index = self
            .state
            .workspaces
            .iter()
            .position(|workspace| workspace.id == destination_workspace_id)
            .unwrap_or(destination_index);
        if !self.state.workspaces[destination_index]
            .window_ids
            .contains(&source_window_id)
        {
            self.state.workspaces[destination_index]
                .window_ids
                .push(source_window_id);
        }
        self.state.workspaces[destination_index].tablet_layout = TabletLayout::Split {
            axis: sws_protocol::workspace::SplitAxis::Horizontal,
            first_window_id: destination_window_id,
            second_window_id: source_window_id,
            ratio_milli: 500,
        };
        self.state.selection.active_workspace = destination_workspace_id;
        self.state.selection.normal_workspace = destination_workspace_id;
        self.state.selection.presentation = ShellPresentation::Workspace;
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
        self.membership_dirty = true;
        if transaction.base_generation != self.state.generation {
            return Err(ApplyError::StaleGeneration);
        }
        if validate_workspaces(transaction.active_workspace, &transaction.workspaces).is_err()
            || !same_members(&transaction.workspaces, live_window_ids)
        {
            return Err(ApplyError::InvalidState);
        }

        // A shell may edit only this output's partition. Reserved IDs also
        // cover retained desktop layouts, so a tablet transition cannot steal
        // an identifier still needed by another output or by mode restoration.
        if self.namespace.workspace_count - self.original_count + transaction.workspaces.len()
            > MAX_WORKSPACES
            || transaction.workspaces.iter().any(|workspace| {
                self.namespace
                    .ids
                    .get(&workspace.id)
                    .is_some_and(|owner| *owner != self.group)
                    || workspace.window_ids.iter().any(|window| {
                        self.namespace
                            .window_owners
                            .get(window)
                            .is_some_and(|(owner, _)| *owner != self.group)
                    })
            })
        {
            return Err(ApplyError::InvalidState);
        }
        for workspace in &transaction.workspaces {
            self.namespace.ids.insert(workspace.id, self.group);
        }
        let previous_active = self.state.selection.active_workspace;
        let previous_normal = self.state.selection.normal_workspace;
        let previous_presentation = self.state.selection.presentation;
        self.state.selection.active_workspace = transaction.active_workspace;
        self.state.selection.presentation = transaction.presentation;
        self.state.workspaces = transaction.workspaces;
        self.state.selection.normal_workspace = if self.state.selection.presentation
            == ShellPresentation::Workspace
            || self.state.selection.active_workspace != previous_active
        {
            self.state.selection.active_workspace
        } else if previous_presentation == ShellPresentation::Workspace
            && self
                .state
                .workspaces
                .iter()
                .any(|workspace| workspace.id == previous_active)
        {
            previous_active
        } else if self
            .state
            .workspaces
            .iter()
            .any(|workspace| workspace.id == previous_normal)
        {
            previous_normal
        } else {
            self.state.selection.active_workspace
        };
        self.ensure_manual_workspace_invariants();
        self.bump_generation();
        Ok(AppliedTransaction {
            state: self.snapshot(),
            transition: transaction.transition,
        })
    }

    fn ensure_manual_workspace_invariants(&mut self) -> bool {
        let mut changed = false;
        if self.state.auto_remove_empty && self.state.workspaces.len() > 1 {
            let previous_len = self.state.workspaces.len();
            let active_workspace = self.state.selection.active_workspace;
            let normal_workspace = self.state.selection.normal_workspace;
            self.state.workspaces.retain(|workspace| {
                !workspace.window_ids.is_empty()
                    || workspace.id == active_workspace
                    || workspace.id == normal_workspace
            });
            changed |= self.state.workspaces.len() != previous_len;
        }
        if self.state.workspaces.is_empty() {
            let id = self.allocate_workspace_id();
            self.state.workspaces.push(WorkspaceSnapshot {
                id,
                window_ids: Vec::new(),
                tablet_layout: TabletLayout::Empty,
            });
            self.state.selection.active_workspace = id;
            self.state.selection.normal_workspace = id;
            changed = true;
        }

        if !self
            .state
            .workspaces
            .iter()
            .any(|workspace| workspace.id == self.state.selection.active_workspace)
        {
            self.state.selection.active_workspace = self
                .state
                .workspaces
                .iter()
                .find(|workspace| workspace.id == self.state.selection.normal_workspace)
                .map_or(self.state.workspaces[0].id, |workspace| workspace.id);
            changed = true;
        }
        if !self
            .state
            .workspaces
            .iter()
            .any(|workspace| workspace.id == self.state.selection.normal_workspace)
        {
            self.state.selection.normal_workspace = self.state.selection.active_workspace;
            changed = true;
        }

        changed
    }
    fn allocate_workspace_id(&mut self) -> WorkspaceId {
        self.namespace.allocate(self.group)
    }
    fn bump_generation(&mut self) {
        self.state.generation = self.state.generation.wrapping_add(1).max(1);
    }
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
        let mut namespace = WorkspaceNamespace {
            next_id: 1,
            ids: BTreeMap::new(),
            window_owners: BTreeMap::new(),
            workspace_count: 1,
        };
        let id = namespace.allocate(0);
        Self {
            partitions: vec![WorkspacePartition {
                output: Some(0),
                connector_key: "primary".to_string(),
                state: OutputWorkspaces::initial(id, auto_remove_empty),
            }],
            namespace,
            auto_remove_empty,
        }
    }

    fn primary(&self) -> &OutputWorkspaces {
        &self.partitions[0].state
    }
    fn scope_at(&mut self, group: usize) -> WorkspaceScope<'_> {
        let state = &mut self.partitions[group].state;
        let original_count = state.workspaces.len();
        WorkspaceScope {
            state,
            namespace: &mut self.namespace,
            group,
            original_count,
            membership_dirty: false,
        }
    }
    fn primary_mut(&mut self) -> WorkspaceScope<'_> {
        self.scope_at(0)
    }

    pub(super) fn output(&self, output: OutputId) -> Option<&OutputWorkspaces> {
        self.partitions
            .iter()
            .find(|part| part.output == Some(output))
            .map(|part| &part.state)
    }
    pub(super) fn output_mut(&mut self, output: OutputId) -> Option<WorkspaceScope<'_>> {
        let group = self
            .partitions
            .iter()
            .position(|part| part.output == Some(output))?;
        Some(self.scope_at(group))
    }

    /// Resolve posture for one output and change only its retained projection.
    /// The external Auto policy stays Desktop while the chassis is folded.
    pub(super) fn apply_output_experience(
        &mut self,
        output: OutputId,
        experience: sws_protocol::outputs::OutputExperience,
        internal: bool,
        tablet_posture: bool,
        preferred_window: Option<u32>,
    ) -> Result<bool, ApplyError> {
        let mut scope = self.output_mut(output).ok_or(ApplyError::InvalidState)?;
        let tablet = experience.tablet_experience(internal, tablet_posture);
        if tablet == scope.tablet_experience() {
            return Ok(false);
        }
        if tablet {
            scope.enter_tablet_experience(preferred_window);
            if !scope.tablet_experience() {
                return Err(ApplyError::InvalidState);
            }
        } else {
            scope.leave_tablet_experience();
        }
        Ok(true)
    }

    /// Restore by connector identity, even if enumeration assigned a new ID.
    /// An output always gets a distinct workspace on its first attachment.
    pub(super) fn attach_output(&mut self, output: OutputId, connector_key: &str) -> bool {
        if connector_key.is_empty() {
            return false;
        }
        if let Some(group) = self
            .partitions
            .iter()
            .position(|part| part.connector_key == connector_key)
        {
            if self.partitions[group].output == Some(output) {
                return true;
            }
            if self.partitions[group].output.is_some() || self.output(output).is_some() {
                return false;
            }
            if self.partitions[group].state.workspaces.is_empty() {
                if self.namespace.workspace_count >= MAX_WORKSPACES { return false; }
                let id = self.namespace.allocate(group);
                self.namespace.workspace_count += 1;
                self.partitions[group].state = OutputWorkspaces::initial(id, self.auto_remove_empty);
            }
            self.partitions[group].output = Some(output);
            return true;
        }
        if output == 0 && self.partitions[0].connector_key == "primary" {
            self.partitions[0].connector_key = connector_key.to_string();
            return true;
        }
        if self.output(output).is_some() || self.namespace.workspace_count >= MAX_WORKSPACES {
            return false;
        }
        let group = self.partitions.len();
        let id = self.namespace.allocate(group);
        self.namespace.workspace_count += 1;
        self.partitions.push(WorkspacePartition {
            output: Some(output),
            connector_key: connector_key.to_string(),
            state: OutputWorkspaces::initial(id, self.auto_remove_empty),
        });
        true
    }

    pub(super) fn detach_output(&mut self, output: OutputId) -> bool {
        if output == 0 {
            return false;
        }
        let Some(group) = self
            .partitions
            .iter()
            .position(|part| part.output == Some(output))
        else {
            return false;
        };
        // Merge the desktop projections first so leaving tablet mode later
        // cannot discard or resurrect migrated scenes. Stable workspace IDs
        // and membership are retained, and the primary selection stays put.
        let tablet = self.partitions[0].state.tablet_experience;
        self.scope_at(group).leave_tablet_experience();
        self.primary_mut().leave_tablet_experience();
        let recovered = {
            let mut source = self.scope_at(group);
            source.membership_dirty = true;
            core::mem::take(&mut source.state.workspaces)
        };
        self.partitions[group].output = None;
        {
            let mut primary = self.primary_mut();
            primary.membership_dirty = true;
            primary.state.workspaces.extend(recovered);
            primary.bump_generation();
        }
        if tablet { self.primary_mut().enter_tablet_experience(None); }
        true
    }

    pub(super) fn session_snapshot(&self) -> Vec<sws_protocol::workspace::OutputWorkspaceState> {
        self.partitions
            .iter()
            .filter(|partition| !partition.state.workspaces.is_empty())
            .map(|partition| sws_protocol::workspace::OutputWorkspaceState {
                output_id: partition.output,
                connector_key: partition.connector_key.clone(),
                tablet_experience: partition.state.tablet_experience,
                state: partition.state.snapshot(),
            })
            .collect()
    }

    pub(super) fn output_for_workspace(&self, id: WorkspaceId) -> Option<OutputId> {
        self.partitions
            .iter()
            .find(|part| {
                part.state
                    .workspaces
                    .iter()
                    .any(|workspace| workspace.id == id)
            })
            .and_then(|part| part.output)
    }

    pub(super) fn output_for_window(&self, window: u32) -> Option<OutputId> {
        let (group, _) = *self.namespace.window_owners.get(&window)?;
        self.partitions[group].output
    }

    /// Move a scene between output partitions atomically, repairing the source
    /// composition. Neither output's selected workspace or Home state changes.
    pub(super) fn transfer_window(&mut self, window: u32, destination: WorkspaceId) -> bool {
        let Some(&(source, source_workspace)) = self.namespace.window_owners.get(&window) else {
            return false;
        };
        if source_workspace == destination {
            return false;
        }
        let Some(target) = self.partitions.iter().position(|part| {
            part.state
                .workspaces
                .iter()
                .any(|workspace| workspace.id == destination)
        }) else {
            return false;
        };
        if source == target {
            return self
                .scope_at(source)
                .move_window_to_workspace(window, destination);
        }
        // Remove from the saved desktop projection too. Otherwise leaving
        // tablet mode could restore the migrated scene onto both monitors.
        {
            let mut scope = self.scope_at(source);
            scope.remove_window(window);
            if let Some(restore) = &mut scope.state.desktop_restore {
                for workspace in &mut restore.workspaces {
                    workspace.window_ids.retain(|id| *id != window);
                    workspace.tablet_layout = repaired_layout(workspace, window);
                }
            }
        }
        let mut scope = self.scope_at(target);
        scope.membership_dirty = true;
        let workspace = scope
            .state
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == destination)
            .unwrap();
        workspace.window_ids.push(window);
        if matches!(workspace.tablet_layout, TabletLayout::Empty) {
            workspace.tablet_layout = TabletLayout::Single { window_id: window };
        }
        scope.bump_generation();
        true
    }

    pub(super) fn snapshot(&self) -> WorkspaceState {
        self.primary().snapshot()
    }

    pub(super) fn active_workspace(&self) -> WorkspaceId {
        self.primary().active_workspace()
    }

    pub(super) fn normal_workspace(&self) -> WorkspaceId {
        self.primary().normal_workspace()
    }

    pub(super) fn create_workspace(&mut self) -> Option<WorkspaceId> {
        self.primary_mut().create_workspace()
    }

    pub(super) fn move_window_to_new_workspace(&mut self, window_id: u32) -> Option<WorkspaceId> {
        let (group, _) = *self.namespace.window_owners.get(&window_id)?;
        self.scope_at(group).move_window_to_new_workspace(window_id)
    }

    pub(super) fn can_remove_workspace(
        &self,
        workspace_id: WorkspaceId,
        allow_freeform_merge: bool,
    ) -> bool {
        self.output_for_workspace(workspace_id).and_then(|output| self.output(output))
            .is_some_and(|state| state.can_remove_workspace(workspace_id, allow_freeform_merge))
    }

    pub(super) fn remove_workspace(
        &mut self,
        workspace_id: WorkspaceId,
        allow_freeform_merge: bool,
    ) -> bool {
        let Some(output) = self.output_for_workspace(workspace_id) else { return false; };
        self.output_mut(output).is_some_and(|mut state| state.remove_workspace(workspace_id, allow_freeform_merge))
    }

    pub(super) fn presentation(&self) -> ShellPresentation {
        self.primary().presentation()
    }

    pub(super) fn tablet_layout(&self, workspace_id: WorkspaceId) -> TabletLayout {
        self.partitions
            .iter()
            .find_map(|part| {
                part.state
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.id == workspace_id)
            })
            .map_or(TabletLayout::Empty, |workspace| workspace.tablet_layout)
    }

    pub(super) fn workspace_for_window(&self, window_id: u32) -> Option<WorkspaceId> {
        self.namespace
            .window_owners
            .get(&window_id)
            .map(|(_, workspace)| *workspace)
    }

    pub(super) fn add_window(&mut self, window_id: u32, tablet_experience: bool) -> WorkspaceId {
        self.primary_mut().add_window(window_id, tablet_experience)
    }

    pub(super) fn add_scene_root(
        &mut self,
        window_id: u32,
        tablet_experience: bool,
        focused_layout: bool,
    ) -> WorkspaceId {
        self.primary_mut()
            .add_scene_root(window_id, tablet_experience, focused_layout)
    }

    pub(super) fn remove_window(&mut self, window_id: u32) -> bool {
        let Some(&(group, _)) = self.namespace.window_owners.get(&window_id) else {
            return false;
        };
        self.scope_at(group).remove_window(window_id)
    }

    pub(super) fn settle_after_close(&mut self, tablet_experience: bool) -> bool {
        self.primary_mut().settle_after_close(tablet_experience)
    }

    pub(super) fn activate_window(&mut self, window_id: u32, tablet_experience: bool) -> bool {
        let Some(&(group, _)) = self.namespace.window_owners.get(&window_id) else {
            return false;
        };
        if self.partitions[group].output.is_none() {
            return false;
        }
        self.scope_at(group)
            .activate_window(window_id, tablet_experience)
    }

    pub(super) fn set_presentation(&mut self, presentation: ShellPresentation) -> bool {
        self.primary_mut().set_presentation(presentation)
    }

    pub(super) fn toggle_overview(&mut self) -> bool {
        self.primary_mut().toggle_overview()
    }

    pub(super) fn return_to_workspace(&mut self) -> bool {
        self.primary_mut().return_to_workspace()
    }

    pub(super) fn select_workspace_from_overview(&mut self, workspace_id: WorkspaceId) -> bool {
        let Some(output) = self.output_for_workspace(workspace_id) else { return false; };
        self.output_mut(output).is_some_and(|mut state| state.select_workspace_from_overview(workspace_id))
    }

    pub(super) fn cycle_workspace(&mut self, direction: i32) -> bool {
        self.primary_mut().cycle_workspace(direction)
    }

    pub(super) fn move_window_to_adjacent_workspace(
        &mut self,
        window_id: u32,
        direction: i32,
    ) -> bool {
        let Some(&(group, _)) = self.namespace.window_owners.get(&window_id) else { return false; };
        self.scope_at(group).move_window_to_adjacent_workspace(window_id, direction)
    }

    pub(super) fn move_window_to_workspace(
        &mut self,
        window_id: u32,
        target_workspace_id: WorkspaceId,
    ) -> bool {
        self.transfer_window(window_id, target_workspace_id)
    }

    pub(super) fn move_window_to_workspace_as_split(
        &mut self,
        window_id: u32,
        target_workspace_id: WorkspaceId,
    ) -> bool {
        self.primary_mut()
            .move_window_to_workspace_as_split(window_id, target_workspace_id)
    }

    pub(super) fn move_overview_selection(&mut self, direction: i32) -> bool {
        self.primary_mut().move_overview_selection(direction)
    }

    pub(super) fn enter_tablet_experience(&mut self, preferred_window_id: Option<u32>) -> bool {
        self.primary_mut()
            .enter_tablet_experience(preferred_window_id)
    }

    pub(super) fn leave_tablet_experience(&mut self) -> bool {
        self.primary_mut().leave_tablet_experience()
    }

    pub(super) fn merge_workspaces_as_split(
        &mut self,
        source_workspace_id: WorkspaceId,
        destination_workspace_id: WorkspaceId,
    ) -> bool {
        self.primary_mut()
            .merge_workspaces_as_split(source_workspace_id, destination_workspace_id)
    }

    pub(super) fn apply_transaction(
        &mut self,
        transaction: WorkspaceTransaction,
        live_window_ids: &[u32],
    ) -> Result<AppliedTransaction, ApplyError> {
        self.primary_mut()
            .apply_transaction(transaction, live_window_ids)
    }
}

/// Remove empty cards while retaining stable selections and one placeholder.
fn prune_empty_workspaces(
    workspaces: &mut Vec<WorkspaceSnapshot>,
    active_workspace: &mut WorkspaceId,
    normal_workspace: &mut WorkspaceId,
) -> bool {
    let placeholder = *active_workspace;
    let all_empty = workspaces
        .iter()
        .all(|workspace| workspace.window_ids.is_empty());
    let destination = |selected| {
        let index = workspaces
            .iter()
            .position(|workspace| workspace.id == selected)
            .unwrap_or(0);
        workspaces[..=index]
            .iter()
            .rev()
            .find(|workspace| !workspace.window_ids.is_empty())
            .or_else(|| {
                workspaces
                    .iter()
                    .find(|workspace| !workspace.window_ids.is_empty())
            })
            .map_or(placeholder, |workspace| workspace.id)
    };
    *active_workspace = destination(*active_workspace);
    *normal_workspace = destination(*normal_workspace);
    let previous_len = workspaces.len();
    workspaces.retain(|workspace| {
        !workspace.window_ids.is_empty() || (all_empty && workspace.id == placeholder)
    });
    previous_len != workspaces.len()
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

    fn assert_valid_state(manager: &WorkspaceManager) {
        let state = manager.snapshot();
        assert_eq!(
            sws_protocol::workspace::decode_state(&sws_protocol::workspace::encode_state(&state)),
            Ok(state)
        );
    }

    #[test]
    fn output_navigation_and_tablet_projection_are_independent() {
        let mut manager = WorkspaceManager::new();
        manager.add_scene_root(10, false, false);
        manager.add_scene_root(11, false, false);
        assert!(manager.attach_output(8, "platform:mdss/DP-1"));
        let internal = manager.snapshot();
        let external_workspace;
        {
            let mut external = manager.output_mut(8).unwrap();
            external_workspace = external.add_scene_root(20, false, false);
            external.add_scene_root(21, false, false);
            external.enter_tablet_experience(Some(21));
            external.set_presentation(ShellPresentation::Overview);
        }
        assert_ne!(external_workspace, internal.active_workspace);
        assert_eq!(manager.snapshot(), internal);
        let external = manager.output(8).unwrap().snapshot();
        assert_eq!(external.workspaces.len(), 2);
        assert_eq!(external.presentation, ShellPresentation::Overview);
        manager.set_presentation(ShellPresentation::Home);
        assert_eq!(manager.output(8).unwrap().snapshot(), external);
        manager.output_mut(8).unwrap().leave_tablet_experience();
        assert_eq!(manager.output(8).unwrap().snapshot().workspaces.len(), 1);
        assert_eq!(manager.snapshot().workspaces, internal.workspaces);
    }

    #[test]
    fn folding_the_chassis_preserves_the_external_desktop() {
        use sws_protocol::outputs::OutputExperience;
        let mut manager = WorkspaceManager::new();
        manager.add_scene_root(10, false, false);
        manager.add_scene_root(11, false, false);
        assert!(manager.attach_output(2, "external"));
        manager
            .output_mut(2)
            .unwrap()
            .add_scene_root(20, false, false);
        let external = manager.output(2).unwrap().snapshot();
        assert_eq!(
            manager.apply_output_experience(0, OutputExperience::Auto, true, true, Some(11)),
            Ok(true)
        );
        assert_eq!(
            manager.apply_output_experience(2, OutputExperience::Auto, false, true, None),
            Ok(false)
        );
        assert!(manager.output(0).unwrap().tablet_experience());
        assert!(!manager.output(2).unwrap().tablet_experience());
        assert_eq!(manager.output(2).unwrap().snapshot(), external);
        assert_eq!(
            manager.apply_output_experience(0, OutputExperience::Auto, true, false, None),
            Ok(true)
        );
        assert_eq!(manager.snapshot().workspaces[0].window_ids, [10, 11]);
        let internal = manager.snapshot();
        assert_eq!(
            manager.apply_output_experience(2, OutputExperience::Tablet, false, false, None),
            Ok(true)
        );
        assert!(manager.output(2).unwrap().tablet_experience());
        assert_eq!(manager.snapshot(), internal);
    }

    #[test]
    fn disconnect_recovers_workspaces_and_reconnect_does_not_take_them_back() {
        let mut manager = WorkspaceManager::new();
        assert!(manager.attach_output(2, "platform:mdss/DP-1"));
        manager
            .output_mut(2)
            .unwrap()
            .add_scene_root(20, false, false);
        let before = manager.output(2).unwrap().snapshot();
        assert!(!manager.attach_output(3, "platform:mdss/DP-1"));
        assert!(manager.detach_output(2));
        assert_eq!(manager.output_for_window(20), Some(0));
        assert_eq!(
            manager.workspace_for_window(20),
            Some(before.active_workspace)
        );
        assert!(
            manager
                .session_snapshot()
                .iter()
                .any(|p| p.output_id == Some(0) && p.state.workspaces.contains(&before.workspaces[0]))
        );
        assert!(manager.attach_output(9, "platform:mdss/DP-1"));
        assert!(manager.output(9).unwrap().snapshot().workspaces[0].window_ids.is_empty());
        assert_eq!(
            manager.output_for_workspace(before.active_workspace),
            Some(0)
        );
        assert_eq!(manager.output_for_window(20), Some(0));
    }

    #[test]
    fn recovered_scenes_survive_tablet_restore() {
        let mut manager = WorkspaceManager::new();
        manager.add_scene_root(10, false, false);
        manager.enter_tablet_experience(Some(10));
        assert!(manager.attach_output(2, "external"));
        manager.output_mut(2).unwrap().add_scene_root(20, false, false);
        manager.output_mut(2).unwrap().add_scene_root(21, false, false);
        assert!(manager.detach_output(2));
        assert!(manager.output(0).unwrap().tablet_experience());
        manager.leave_tablet_experience();
        for id in [10, 20, 21] { assert_eq!(manager.output_for_window(id), Some(0)); }
        let members = manager.snapshot().workspaces.into_iter().flat_map(|w| w.window_ids).collect::<Vec<_>>();
        assert_eq!(members.len(), 3);
    }

    #[test]
    fn scene_transfer_cannot_be_undone_by_tablet_restore() {
        let mut manager = WorkspaceManager::new();
        manager.add_scene_root(10, false, false);
        manager.add_scene_root(11, false, false);
        manager.enter_tablet_experience(Some(10));
        assert!(manager.attach_output(2, "external"));
        let target = manager.output(2).unwrap().active_workspace();
        assert!(manager.transfer_window(10, target));
        manager.leave_tablet_experience();
        assert_eq!(manager.output_for_window(10), Some(2));
        assert_eq!(manager.workspace_for_window(10), Some(target));
        assert_eq!(manager.output_for_window(11), Some(0));
        let occurrences = manager
            .session_snapshot()
            .iter()
            .flat_map(|p| &p.state.workspaces)
            .flat_map(|w| &w.window_ids)
            .filter(|id| **id == 10)
            .count();
        assert_eq!(occurrences, 1);
    }

    #[test]
    fn shell_transaction_cannot_steal_another_outputs_workspace_or_scene() {
        let mut manager = WorkspaceManager::new();
        manager.add_scene_root(10, false, false);
        assert!(manager.attach_output(2, "external"));
        manager
            .output_mut(2)
            .unwrap()
            .add_scene_root(20, false, false);
        let before = manager.output(2).unwrap().snapshot();
        let stolen = manager.snapshot();
        for (workspace, members) in [
            (stolen.active_workspace, vec![20]),
            (before.active_workspace, vec![10, 20]),
        ] {
            let transaction = WorkspaceTransaction {
                base_generation: before.generation,
                active_workspace: workspace,
                presentation: ShellPresentation::Home,
                workspaces: vec![WorkspaceSnapshot {
                    id: workspace,
                    window_ids: members.clone(),
                    tablet_layout: TabletLayout::Empty,
                }],
                transition: TransitionSpec::default(),
            };
            assert_eq!(
                manager
                    .output_mut(2)
                    .unwrap()
                    .apply_transaction(transaction, &members),
                Err(ApplyError::InvalidState)
            );
            assert_eq!(manager.output(2).unwrap().snapshot(), before);
            assert_eq!(manager.snapshot(), stolen);
        }
    }

    #[test]
    fn workspace_limit_includes_other_and_disconnected_outputs() {
        let mut manager = WorkspaceManager::new();
        assert!(manager.attach_output(2, "external"));
        assert!(manager.detach_output(2));
        for _ in 0..MAX_WORKSPACES - 2 {
            assert!(manager.create_workspace().is_some());
        }
        assert!(manager.create_workspace().is_none());
        assert!(!manager.attach_output(3, "other"));
        assert!(!manager.attach_output(4, "external"));
    }

    #[test]
    fn shared_allocator_does_not_rewind_on_tablet_restore_or_shell_transaction() {
        let mut manager = WorkspaceManager::new();
        manager.add_scene_root(10, false, false);
        manager.add_scene_root(11, false, false);
        manager.enter_tablet_experience(Some(10));
        assert!(manager.attach_output(2, "external"));
        let external_id = manager.output(2).unwrap().active_workspace();
        manager.leave_tablet_experience();
        let state = manager.snapshot();
        manager
            .apply_transaction(
                WorkspaceTransaction {
                    base_generation: state.generation,
                    active_workspace: state.active_workspace,
                    presentation: state.presentation,
                    workspaces: state.workspaces,
                    transition: TransitionSpec::default(),
                },
                &[10, 11],
            )
            .unwrap();
        assert_ne!(manager.create_workspace().unwrap(), external_id);
        let external_next = manager.output_mut(2).unwrap().create_workspace().unwrap();
        assert_ne!(external_next, manager.active_workspace());
    }

    #[test]
    fn tablet_top_level_launches_create_separate_workspace_compositions() {
        let mut manager = WorkspaceManager::new();
        assert_eq!(manager.add_scene_root(10, true, true), 1);
        let generation = manager.snapshot().generation;
        let second = manager.add_scene_root(11, true, true);
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
        assert_eq!(state.generation, generation.wrapping_add(1));
        assert_eq!(state.presentation, ShellPresentation::Workspace);
    }

    #[test]
    fn tablet_top_level_launch_fills_the_selected_empty_workspace() {
        let mut manager = WorkspaceManager::new();
        manager.add_scene_root(10, true, true);
        let empty = manager.create_workspace().unwrap();
        let workspace_count = manager.snapshot().workspaces.len();

        assert_eq!(manager.add_scene_root(11, true, true), empty);
        assert_eq!(manager.snapshot().workspaces.len(), workspace_count);
        assert_eq!(
            manager.tablet_layout(empty),
            TabletLayout::Single { window_id: 11 }
        );
    }

    #[test]
    fn desktop_launches_share_the_active_virtual_desktop() {
        let mut manager = WorkspaceManager::new();
        manager.add_scene_root(10, false, false);
        manager.add_scene_root(11, false, false);
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

        assert_eq!(manager.add_scene_root(999, true, true), 1);
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
    fn tablet_launch_filling_a_retained_empty_workspace_survives_desktop_restore() {
        let mut manager = WorkspaceManager::new();
        manager.add_scene_root(10, false, false);
        let empty = manager.create_workspace().unwrap();
        manager.enter_tablet_experience(None);

        assert_eq!(manager.active_workspace(), empty);
        assert_eq!(manager.add_scene_root(11, true, true), empty);
        manager.leave_tablet_experience();
        assert_eq!(manager.workspace_for_window(11), Some(empty));
        assert_eq!(manager.snapshot().workspaces.len(), 2);
    }

    #[test]
    fn tablet_launch_appended_workspace_survives_desktop_restore() {
        let mut manager = WorkspaceManager::new();
        manager.add_scene_root(10, false, false);
        manager.enter_tablet_experience(Some(10));

        let launched = manager.add_scene_root(11, true, true);
        assert_ne!(launched, 1);
        manager.leave_tablet_experience();
        assert_eq!(manager.workspace_for_window(11), Some(launched));
        assert_eq!(manager.snapshot().workspaces.len(), 2);
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
    fn desktop_move_preserves_the_vacated_workspace() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        manager.add_window(11, false);
        manager.create_workspace();
        manager.cycle_workspace(-1);
        manager.move_window_to_adjacent_workspace(11, 1);

        assert!(manager.move_window_to_adjacent_workspace(11, -1));
        assert!(!manager.settle_after_close(false));
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
        assert!(!manager.settle_after_close(false));
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
        assert!(manager.settle_after_close(true));
        assert_eq!(manager.snapshot().workspaces.len(), 1);
        assert_eq!(manager.active_workspace(), first);
        assert_eq!(manager.normal_workspace(), first);
        assert_eq!(manager.presentation(), ShellPresentation::Overview);
        assert_valid_state(&manager);
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
    fn closing_active_tablet_composition_selects_previous_workspace() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, true);
        let second = manager.create_workspace().unwrap();
        manager.add_window(11, true);
        assert_eq!(manager.active_workspace(), second);
        assert!(manager.remove_window(11));
        let generation = manager.snapshot().generation;
        assert!(manager.settle_after_close(true));
        assert_eq!(manager.presentation(), ShellPresentation::Workspace);
        assert_eq!(manager.snapshot().workspaces.len(), 1);
        assert_eq!(manager.active_workspace(), 1);
        assert_eq!(manager.normal_workspace(), 1);
        assert_eq!(manager.snapshot().generation, generation + 1);
        let settled = manager.snapshot();
        assert!(!manager.settle_after_close(true));
        assert_eq!(manager.snapshot(), settled);
        assert_valid_state(&manager);
    }

    #[test]
    fn desktop_close_preserves_empty_virtual_desktop() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        manager.create_workspace();
        manager.create_workspace();
        manager.add_window(11, false);
        manager.remove_window(11);
        assert!(manager.remove_window(10));
        let before = manager.snapshot();
        assert!(!manager.settle_after_close(false));
        assert_eq!(manager.snapshot(), before);
        assert_eq!(manager.presentation(), ShellPresentation::Workspace);
        assert_eq!(manager.snapshot().workspaces.len(), 3);
        assert_valid_state(&manager);
    }

    #[test]
    fn tablet_close_selects_nearest_surviving_predecessor_across_empty_cards() {
        for presentation in [
            ShellPresentation::Workspace,
            ShellPresentation::Overview,
            ShellPresentation::Home,
        ] {
            let mut manager = WorkspaceManager::new();
            manager.add_scene_root(10, true, true);
            manager.create_workspace();
            manager.create_workspace();
            let closing = manager.create_workspace().unwrap();
            manager.add_scene_root(11, true, true);
            let successor = manager.add_scene_root(12, true, true);
            manager.activate_window(11, true);
            manager.set_presentation(presentation);

            assert!(manager.remove_window(11));
            assert!(manager.settle_after_close(true));
            let state = manager.snapshot();
            assert_eq!(
                state
                    .workspaces
                    .iter()
                    .map(|workspace| workspace.id)
                    .collect::<Vec<_>>(),
                vec![1, successor]
            );
            assert_ne!(state.active_workspace, closing);
            assert_eq!(state.active_workspace, 1);
            assert_eq!(state.normal_workspace, 1);
            assert_eq!(state.presentation, presentation);
            assert_valid_state(&manager);
        }
    }

    #[test]
    fn tablet_close_at_first_workspace_selects_successor() {
        let mut manager = WorkspaceManager::new();
        manager.add_scene_root(10, true, true);
        let successor = manager.add_scene_root(11, true, true);
        manager.activate_window(10, true);
        manager.remove_window(10);

        assert!(manager.settle_after_close(true));
        assert_eq!(manager.active_workspace(), successor);
        assert_eq!(manager.normal_workspace(), successor);
        assert_valid_state(&manager);
    }

    #[test]
    fn tablet_close_repairs_normal_destination_without_changing_live_selection() {
        let mut manager = WorkspaceManager::new();
        manager.add_scene_root(10, true, true);
        let successor = manager.add_scene_root(11, true, true);
        let selected = manager.add_scene_root(12, true, true);
        manager.set_presentation(ShellPresentation::Overview);
        manager.partitions[0].state.selection.normal_workspace = 1;
        manager.remove_window(10);

        assert!(manager.settle_after_close(true));
        assert_eq!(manager.active_workspace(), selected);
        assert_eq!(manager.normal_workspace(), successor);
        assert_valid_state(&manager);
        assert!(manager.return_to_workspace());
        assert_eq!(manager.active_workspace(), successor);
        assert_valid_state(&manager);
    }

    #[test]
    fn tablet_move_prunes_source_and_preserves_selected_survivor() {
        for move_kind in 0..3 {
            let mut manager = WorkspaceManager::new();
            manager.add_scene_root(10, true, true);
            let second = manager.add_scene_root(11, true, true);
            manager.set_presentation(ShellPresentation::Overview);
            let selected = match move_kind {
                0 => {
                    assert!(manager.move_window_to_workspace(10, second));
                    second
                }
                1 => {
                    assert!(manager.move_window_to_adjacent_workspace(10, 1));
                    second
                }
                2 => manager.move_window_to_new_workspace(10).unwrap(),
                _ => unreachable!(),
            };
            assert!(manager.settle_after_close(true));
            assert_eq!(manager.active_workspace(), selected);
            assert_eq!(manager.normal_workspace(), selected);
            assert!(
                manager
                    .snapshot()
                    .workspaces
                    .iter()
                    .all(|workspace| !workspace.window_ids.is_empty())
            );
            assert_eq!(manager.workspace_for_window(10), Some(selected));
            assert_valid_state(&manager);
        }
    }

    #[test]
    fn tablet_all_closed_retains_one_selected_placeholder_and_can_launch_again() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        manager.create_workspace();
        let selected = manager.create_workspace().unwrap();
        manager.add_window(11, false);
        manager.enter_tablet_experience(Some(11));
        manager.remove_window(10);
        manager.remove_window(11);

        assert!(manager.settle_after_close(true));
        let state = manager.snapshot();
        assert_eq!(state.workspaces.len(), 1);
        assert_eq!(state.active_workspace, selected);
        assert_eq!(state.normal_workspace, selected);
        assert_eq!(state.workspaces[0].tablet_layout, TabletLayout::Empty);
        assert!(state.workspaces[0].window_ids.is_empty());
        assert!(!manager.settle_after_close(true));
        assert!(!manager.leave_tablet_experience());
        assert_eq!(manager.snapshot(), state);
        assert_eq!(manager.add_scene_root(12, true, true), selected);
        assert_eq!(manager.snapshot().workspaces.len(), 1);
        assert_valid_state(&manager);
    }

    #[test]
    fn tablet_close_does_not_resurrect_empty_desktop_groups() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        manager.add_window(11, false);
        let closing = manager.create_workspace().unwrap();
        manager.add_window(12, false);
        manager.create_workspace();
        manager.enter_tablet_experience(Some(12));
        manager.remove_window(12);

        assert!(manager.settle_after_close(true));
        manager.leave_tablet_experience();
        assert_eq!(manager.snapshot().workspaces.len(), 1);
        assert_eq!(manager.snapshot().workspaces[0].window_ids, vec![10, 11]);
        assert_ne!(manager.active_workspace(), closing);
        assert_valid_state(&manager);
    }

    #[test]
    fn tablet_close_of_all_original_scenes_restores_only_live_new_apps() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        manager.enter_tablet_experience(Some(10));
        let launched = manager.add_scene_root(11, true, true);
        manager.remove_window(10);

        assert!(manager.settle_after_close(true));
        let settled = manager.snapshot();
        assert!(!manager.leave_tablet_experience());
        assert_eq!(manager.snapshot(), settled);
        assert_eq!(manager.workspace_for_window(11), Some(launched));
        assert_eq!(manager.snapshot().workspaces.len(), 1);
        assert_valid_state(&manager);
    }

    #[test]
    fn tablet_close_preserves_desktop_group_when_original_card_was_pruned() {
        let mut manager = WorkspaceManager::new();
        manager.add_window(10, false);
        manager.add_window(11, false);
        manager.add_window(12, false);
        manager.enter_tablet_experience(Some(10));
        manager.remove_window(10);

        assert!(manager.settle_after_close(true));
        assert!(
            manager
                .snapshot()
                .workspaces
                .iter()
                .all(|workspace| workspace.id != 1)
        );
        assert!(manager.leave_tablet_experience());
        assert_eq!(manager.snapshot().workspaces.len(), 1);
        assert_eq!(manager.snapshot().workspaces[0].window_ids, vec![11, 12]);
        assert_valid_state(&manager);
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
