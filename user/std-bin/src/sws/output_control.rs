//! Output-scoped session control; all mutations run on the compositor thread.
use super::*;
use sws_protocol::outputs::{
    OutputConfiguration, OutputContext, OutputDescriptor, OutputEvent, OutputRequest, OutputRole,
};

impl Compositor {
    pub(super) fn pointer_output_id(&self, x: i32, y: i32) -> u32 {
        self.outputs
            .output_at(x, y)
            .map_or(self.outputs.desktop_primary().id, |output| output.id)
    }

    pub(super) fn keyboard_output_id(&self) -> u32 {
        self.window_manager
            .get_focused_window_id()
            .and_then(|id| self.window_manager.get_window(id))
            .filter(|window| self.outputs.desktop(window.output_id).is_some())
            .map_or_else(
                || self.pointer_output_id(self.cursor.x, self.cursor.y),
                |window| window.output_id,
            )
    }

    pub(super) fn keyboard_workspace(&self) -> &super::super::workspace::OutputWorkspaces {
        self.workspace_manager
            .output(self.keyboard_output_id())
            .expect("connected output workspace")
    }

    pub(super) fn keyboard_workspace_mut(&mut self) -> super::super::workspace::WorkspaceScope<'_> {
        let output = self.keyboard_output_id();
        self.workspace_manager
            .output_mut(output)
            .expect("connected output workspace")
    }

    /// Change a transient group's output together with its workspace and frame
    /// clock. Source images remain retained until the new-scale frame arrives.
    pub(super) fn bind_window_output(&mut self, window: u32, output: u32) -> bool {
        let Some(context) = self.output_context_for(output) else {
            return false;
        };
        let root = self.top_level_window_id(window);
        if self.window_manager.get_window(root).is_none() {
            return false;
        }
        let group: Vec<_> = self
            .window_manager
            .get_windows()
            .iter()
            .filter(|w| self.top_level_window_id(w.id) == root)
            .map(|w| w.id)
            .collect();
        self.workspace_manager.rebind_scene(root, output);
        let destination = self.outputs.get(output).unwrap();
        let counter = destination.present_counter;
        let bounds = destination.rect;
        for id in group {
            let old_output = self.window_output_id(id);
            if let Some(w) = self.window_manager.get_window_mut(id) {
                if old_output != output {
                    if w.fullscreen
                        && let Some(source) = self
                            .outputs
                            .get(old_output)
                            .map(|o| o.rect)
                            .or_else(|| self.detached_output_geometry.get(&old_output).copied())
                    {
                        // Translate the saved desktop rectangle along with its
                        // owner before configuring the new fullscreen bounds.
                        w.translate_output(
                            bounds.x.saturating_sub(source.x),
                            bounds.y.saturating_sub(source.y),
                        );
                    }
                    w.last_frame_submission_counter = Some(counter);
                    w.last_frame_credit_tick = None;
                    for callback in &mut self.pending_frame_callbacks {
                        if callback.window_id == id && callback.requested_after_present.is_some() {
                            callback.requested_after_present = Some(counter);
                        }
                    }
                }
                w.output_id = output;
                if w.owner_client_id.is_some_and(|client| {
                    super::super::ipc::client_output_context(client).is_some()
                }) {
                    w.prefer_scale(context.scale_milli);
                }
            }
            if let Some(source) = self.outputs.get_mut(old_output) {
                source.target.full_redraw = true;
            }
            if old_output != output
                && self.window_manager.resize_fullscreen_window_on(
                    id,
                    (bounds.x, bounds.y, bounds.width, bounds.height),
                )
            {
                self.send_fullscreen_configure(id);
            } else {
                self.send_current_window_configure(id);
            }
        }
        self.outputs.get_mut(output).unwrap().target.full_redraw = true;
        if let Some(owner) = self
            .window_manager
            .get_window(window)
            .and_then(|w| w.owner_client_id)
        {
            self.project_client_environment(owner);
        }
        let panel = super::super::ipc::input_panel_state();
        if panel.context.active() && self.top_level_window_id(panel.context.window_id) == root {
            self.refresh_input_panel();
        }
        true
    }

    pub(super) fn finish_window_output_drag(&mut self, window: u32) {
        let Some(w) = self.window_manager.get_window(window) else {
            return;
        };
        let (x, y, width, height) = w.window_geometry();
        let previous = w.output_id;
        // Choose the greatest visible area after release. Keeping the buffer
        // scale stable during the drag avoids reallocating on every crossing.
        let Some((output, area)) = self
            .outputs
            .desktops()
            .map(|output| {
                let r = output.rect;
                let width = (i64::from(x) + i64::from(width))
                    .min(i64::from(r.x) + i64::from(r.width))
                    - i64::from(x.max(r.x));
                let height = (i64::from(y) + i64::from(height))
                    .min(i64::from(r.y) + i64::from(r.height))
                    - i64::from(y.max(r.y));
                (output.id, width.max(0) * height.max(0))
            })
            .max_by_key(|(id, area)| (*area, *id == previous))
        else {
            return;
        };
        if area == 0 || output == previous || !self.bind_window_output(window, output) {
            return;
        }
        if let Some(mut state) = self.workspace_manager.output_mut(output) {
            state.return_to_workspace();
        }
        self.apply_workspace_presentation_policy_on(Some(output));
        self.publish_workspace_state();
    }

    pub(super) fn output_window_position(
        &self,
        output: u32,
        width: u32,
        height: u32,
        centered: bool,
    ) -> (i32, i32) {
        if !centered {
            return self.default_window_position(output, width, height);
        }
        let area = self.output_workarea(output);
        let x = area.2.saturating_sub(width) / 2;
        let y = area.3.saturating_sub(height) / 2;
        (
            area.0.saturating_add(x as i32),
            area.1.saturating_add(y as i32),
        )
    }

    pub(super) fn output_context_for(&self, id: u32) -> Option<OutputContext> {
        let output = self.outputs.desktop(id)?;
        Some(OutputContext {
            id,
            scale_milli: output.scale_milli,
            mode: output.metadata.mode,
            rect: output.logical_rect,
            tablet: self.output_is_tablet(id),
        })
    }

    pub(super) fn notify_window_output(&self, window: u32) {
        let Some(w) = self.window_manager.get_window(window) else {
            return;
        };
        // Legacy clients cannot decode this additive event. Only connections
        // which requested an output context opt into logical configures.
        if !w
            .owner_client_id
            .is_some_and(|client| super::super::ipc::client_output_context(client).is_some())
        {
            return;
        }
        let (width, height) = w.configured_pixel_extent();
        let (_, _, scene_width, scene_height) = w.surface_geometry();
        let Some(output) = self.outputs.get(w.output_id) else {
            return;
        };
        super::super::ipc::send_message_to_window(
            window,
            sws_protocol::server_msg::OUTPUT_CONTROL,
            sws_protocol::outputs::encode_event(&OutputEvent::Window {
                window,
                output: w.output_id,
                scale_milli: w.preferred_scale_milli,
                width,
                height,
                logical_width: scene_width.div_ceil(2),
                logical_height: scene_height.div_ceil(2),
                output_x: output.logical_rect.x,
                output_y: output.logical_rect.y,
            }),
        );
    }

    pub(super) fn publish_outputs(&self) {
        let contexts = self
            .outputs
            .desktops()
            .filter_map(|output| self.output_context_for(output.id))
            .collect::<Vec<_>>();
        super::super::ipc::refresh_output_contexts(&contexts);
        super::super::ipc::publish_output_event(OutputEvent::Snapshot(self.outputs.snapshot()));
        for state in self.workspace_manager.session_snapshot() {
            if let Some(output) = state
                .output_id
                .filter(|id| self.outputs.desktop(*id).is_some())
            {
                super::super::ipc::publish_output_event(OutputEvent::Workspace {
                    output,
                    state: state.state,
                });
            }
        }
    }

    pub(super) fn handle_output_control(
        &mut self,
        client: usize,
        request: u8,
        command: OutputRequest,
    ) -> Result<(), &'static str> {
        let result = self.execute_output_control(client, command);
        match result {
            Ok(event) => send_response_to_client(
                client,
                sws_protocol::server_msg::OUTPUT_CONTROL,
                request,
                super::super::ipc::output_event_payload(client, &event),
            ),
            Err(code) => send_response_to_client(
                client,
                sws_protocol::server_msg::ERROR,
                request,
                sws_protocol::payload_error(code).to_vec(),
            ),
        }
        Ok(())
    }

    pub(super) fn output_workarea(&self, id: u32) -> (i32, i32, u32, u32) {
        let output = self
            .outputs
            .get(id)
            .unwrap_or_else(|| self.outputs.desktop_primary());
        output.workarea.unwrap_or((
            output.rect.x,
            output.rect.y,
            output.rect.width,
            output.rect.height,
        ))
    }

    pub(super) fn window_output_id(&self, id: u32) -> u32 {
        self.window_manager
            .get_window(id)
            .map_or(self.outputs.desktop_primary().id, |window| window.output_id)
    }

    /// The wire environment is per connection. A provider follows its editor;
    /// a pinned application without an output binding follows its focused or
    /// topmost owned window. Headless legacy clients use the primary output.
    pub(super) fn project_client_environment(&self, client: usize) {
        let panel = super::super::ipc::input_panel_state();
        let editor = panel
            .provider
            .filter(|(provider, _)| *provider == client && panel.context.active())
            .and_then(|_| self.window_manager.get_window(panel.context.window_id));
        let bound = super::super::ipc::client_output_context(client);
        let owned = || {
            self.window_manager
                .get_focused_window_id()
                .and_then(|id| self.window_manager.get_window(id))
                .filter(|w| w.owner_client_id == Some(client))
                .or_else(|| {
                    self.window_manager
                        .get_windows()
                        .iter()
                        .rev()
                        .find(|w| w.owner_client_id == Some(client))
                })
        };
        let id = editor
            .map(|w| w.output_id)
            .or(bound.map(|c| c.id))
            .or_else(|| owned().map(|w| w.output_id))
            .filter(|id| self.outputs.desktop(*id).is_some())
            .unwrap_or(self.outputs.desktop_primary().id);
        super::super::ipc::project_client_input_environment(client, id);
    }

    pub(super) fn window_uses_tablet_experience(&self, id: u32) -> bool {
        self.output_is_tablet(self.window_output_id(id))
    }

    pub(super) fn output_is_tablet(&self, id: u32) -> bool {
        self.workspace_manager
            .output(id)
            .is_some_and(|scope| scope.tablet_experience())
    }

    pub(super) fn publish_input_environment(
        &self,
        posture: Option<Option<bool>>,
        lid: Option<Option<bool>>,
    ) {
        let primary = self.outputs.desktop_primary();
        let outputs = std::iter::once(primary)
            .chain(
                self.outputs
                    .desktops()
                    .filter(|output| output.id != primary.id),
            )
            .map(|output| input_environment::OutputPolicy {
                id: output.id,
                tablet: self.output_is_tablet(output.id),
            })
            .collect();
        if input_environment::update_outputs(outputs, posture, lid) {
            super::super::ipc::broadcast_input_environment_changed();
            super::super::input_environment_sbus::queue_state_changed();
        }
    }

    /// Posture changes cannot be rejected. If materializing all cards would
    /// exceed the workspace limit, keep reporting the actual projection and
    /// retry when a workspace operation frees capacity.
    pub(super) fn reconcile_output_experiences(&mut self, folded: bool) -> bool {
        let mut changed = false;
        self.experience_retry_pending = false;
        let focused = self
            .window_manager
            .get_focused_window_id()
            .map(|id| self.top_level_window_id(id));
        for output in self.outputs.desktops() {
            match self.workspace_manager.apply_output_experience(
                output.id,
                output.experience,
                output.metadata.internal,
                folded,
                focused,
            ) {
                Ok(applied) => changed |= applied,
                Err(_) => self.experience_retry_pending = true,
            }
        }
        if changed {
            self.outputs.bump_generation();
        }
        changed
    }

    pub(super) fn restore_output_configuration(&mut self) {
        for (id, mode) in super::super::config::saved_output_modes(&self.outputs.snapshot().outputs)
        {
            if self
                .outputs
                .get(id)
                .is_some_and(|output| output.metadata.mode != mode)
            {
                if let Some(endpoint) = self
                    .secondary_endpoints
                    .iter()
                    .find(|endpoint| endpoint.output == Some(id))
                {
                    let _ = endpoint.control.request_mode(framebuffer::DisplayMode {
                        width: mode.pixel_width,
                        height: mode.pixel_height,
                        refresh_millihz: mode.refresh_millihz,
                    });
                }
            }
        }
        if let Some(configuration) =
            super::super::config::saved_output_layout(&self.outputs.snapshot().outputs)
        {
            if let Err(error) = self.install_output_configuration(&configuration, false) {
                println!(
                    "[Compositor] Saved display layout could not be restored: {}",
                    error
                );
            }
        }
    }

    /// A physical disconnect, replacement or modeset can precede the hotplug poll.
    /// Check the entire topology, including the desktop that is meant to stay lit.
    fn check_live_output_topology(&self) -> Result<(), u32> {
        let stale = sws_protocol::error_codes::STALE_OUTPUT_GENERATION;
        for output in self.outputs.iter() {
            let display = &output.target.display;
            let backing = display.get_info().map_err(|_| stale)?;
            if !Self::live_output_matches(&output.metadata, backing, display.get_output_info().ok())
            {
                return Err(stale);
            }
        }
        for endpoint in self
            .secondary_endpoints
            .iter()
            .filter(|e| e.output.is_none())
        {
            if endpoint
                .control
                .get_output_info()
                .is_ok_and(|info| info.is_connected())
            {
                return Err(stale);
            }
        }
        Ok(())
    }

    fn live_output_matches(
        expected: &OutputDescriptor,
        backing: framebuffer::DisplayInfo,
        connector: Option<framebuffer::DisplayOutputInfo>,
    ) -> bool {
        let mode = expected.mode;
        if (backing.width, backing.height) != (mode.pixel_width, mode.pixel_height) {
            return false;
        }
        // Legacy endpoints have only a session ID and a retained display handle.
        // Use that handle's live native extent when connector metadata is absent.
        let session_key = expected.connector_key == format!("session:output{}", expected.id);
        let Some(info) = connector else {
            return session_key && expected.monitor_key.is_empty();
        };
        info.is_connected()
            && (info.width == 0 || info.width == mode.pixel_width)
            && (info.height == 0 || info.height == mode.pixel_height)
            && (mode.refresh_millihz == 0 || info.refresh_mhz == mode.refresh_millihz)
            && (session_key || info.connector_key().as_ref() == Some(&expected.connector_key))
            && info.monitor_key().unwrap_or("") == expected.monitor_key
    }

    fn blank_output(&mut self, id: OutputId) -> Result<(), u32> {
        use sws_protocol::error_codes as errors;
        self.check_live_output_topology()?;
        self.drain_output_presents(id)
            .map_err(|_| errors::INVALID_OUTPUT_CONFIGURATION)?;
        self.check_live_output_topology()?;
        let output = self.outputs.get_mut(id).ok_or(errors::UNKNOWN_OUTPUT)?;
        // Arm rollback before the first write. A live framebuffer can change even
        // if its present fails. Until commit, the renderer still owns the old role
        // and will repaint its entire source or mirror on the next frame.
        self.full_redraw_needed = true;
        output.target.full_redraw = true;
        output.target.presented_damage.clear();
        output.target.deferred_gpu_damage = None;
        let mode = output.metadata.mode;
        if output
            .target
            .display
            .present_black_frame(mode.pixel_width, mode.pixel_height)
            .is_err()
        {
            self.check_live_output_topology()?;
            return Err(errors::INVALID_OUTPUT_CONFIGURATION);
        }
        Ok(())
    }

    /// Reapply scanout policy after hotplug or backing replacement. OutputSet
    /// has already promoted a surviving desktop if the last one was unplugged.
    pub(super) fn sync_enabled_outputs(&mut self) -> Result<(), &'static str> {
        self.full_redraw_needed = true;
        let mut disabled = Vec::new();
        for output in self.outputs.iter_mut() {
            output.target.full_redraw = true;
            output.target.presented_damage.clear();
            output.target.deferred_gpu_damage = None;
            if output.role == OutputRole::Disabled {
                disabled.push(output.id);
            }
        }
        let mut result = Ok(());
        for id in disabled {
            if self.blank_output(id).is_err() {
                result = Err("Could not blank a disabled output after hotplug");
            }
        }
        result
    }

    pub(super) fn install_output_configuration(
        &mut self,
        configuration: &[OutputConfiguration],
        persist: bool,
    ) -> Result<(), u32> {
        use sws_protocol::error_codes as errors;
        let folded = input_environment::snapshot().hardware_tablet_mode();
        let was_tablet = self.output_is_tablet(self.outputs.desktop_primary().id);
        let focused = self
            .window_manager
            .get_focused_window_id()
            .map(|id| self.top_level_window_id(id));
        let mut workspaces = self.workspace_manager.clone();
        // Restore desktop groups first so freed workspace IDs count toward
        // tablet expansion within the same atomic transaction.
        let mut ordered: Vec<_> = configuration
            .iter()
            .filter(|entry| entry.role == OutputRole::Extend)
            .collect();
        ordered.sort_by_key(|entry| {
            self.outputs.get(entry.id).is_some_and(|o| {
                entry
                    .experience
                    .tablet_experience(o.metadata.internal, folded)
            })
        });
        for entry in ordered {
            let output = self.outputs.get(entry.id).ok_or(errors::UNKNOWN_OUTPUT)?;
            workspaces
                .apply_output_experience(
                    entry.id,
                    entry.experience,
                    output.metadata.internal,
                    folded,
                    focused,
                )
                .map_err(|_| errors::INVALID_OUTPUT_CONFIGURATION)?;
        }
        let old = self.outputs.snapshot();
        let destination = sws_protocol::outputs::extended_primary(&old.outputs, configuration)
            .ok_or(errors::INVALID_OUTPUT_CONFIGURATION)?;
        for entry in configuration
            .iter()
            .filter(|entry| entry.role != OutputRole::Extend)
        {
            if self.outputs.desktop(entry.id).is_some() {
                workspaces
                    .stage_desktop_demotion(entry.id, destination)
                    .map_err(|_| errors::INVALID_OUTPUT_CONFIGURATION)?;
            }
        }
        let proposed = sws_protocol::outputs::OutputSnapshot {
            generation: old.generation,
            outputs: old.outputs.clone(),
            configuration: configuration.to_vec(),
        };
        let mut roles_changed = false;
        for entry in configuration {
            let previous = self.outputs.get(entry.id).ok_or(errors::UNKNOWN_OUTPUT)?;
            if entry.role != previous.role {
                roles_changed = true;
                if entry.role == OutputRole::Disabled {
                    self.blank_output(entry.id)?;
                }
            }
        }
        if roles_changed {
            self.check_live_output_topology()?;
        }
        if persist {
            super::super::config::persist_outputs(&proposed)
                .map_err(|_| errors::OUTPUT_CONFIGURATION_NOT_SAVED)?;
        }
        if configuration.iter().any(|entry| {
            entry.id == self.outputs.primary_id() && entry.role != self.outputs.primary().role
        }) {
            self.cancel_primary_experience_gestures();
            let sources: Vec<_> = self
                .direct_touch_grabs
                .iter()
                .map(|grab| grab.source)
                .collect();
            for source in sources {
                self.handle_direct_touch_frame(TouchFrame::cancel(
                    source,
                    super::super::input::TouchSurface::Touchscreen,
                ));
            }
        }
        self.workspace_manager = workspaces;
        self.outputs.apply_configuration(configuration);
        let desktop = self.outputs.desktop_primary().id;
        self.workspace_manager.set_desktop_primary(desktop);
        self.experience_retry_pending = false;
        if was_tablet != self.output_is_tablet(self.outputs.desktop_primary().id) {
            self.cancel_primary_experience_gestures();
        }
        self.publish_input_environment(None, None);
        super::super::input::set_screen_size(
            self.touch_desktop().width(),
            self.touch_desktop().height(),
        );
        for entry in configuration {
            let previous = old
                .configuration
                .iter()
                .find(|previous| previous.id == entry.id)
                .unwrap();
            if entry == previous {
                continue;
            }
            let mut notifications = Vec::new();
            for window in self.window_manager.get_windows_mut() {
                if window.output_id != entry.id {
                    continue;
                }
                window.translate_output(
                    entry.x.saturating_sub(previous.x).saturating_mul(2),
                    entry.y.saturating_sub(previous.y).saturating_mul(2),
                );
                if window.preferred_scale_milli != entry.scale_milli {
                    if window.owner_client_id.is_some_and(|client| {
                        super::super::ipc::client_output_context(client).is_some()
                    }) {
                        window.prefer_scale(entry.scale_milli);
                    }
                    notifications.push(window.id);
                }
            }
            if let Some(output) = self.outputs.get_mut(entry.id) {
                output.target.full_redraw = true;
            }
            for window in notifications {
                self.notify_window_output(window);
            }
        }
        if roles_changed {
            for entry in configuration
                .iter()
                .filter(|entry| entry.role != OutputRole::Extend)
            {
                let previous_bounds = self.outputs.get(entry.id).unwrap().rect;
                self.relocate_output_applications(entry.id, previous_bounds);
            }
            if let Some(gpu) = self.gpu_compositor.as_mut() {
                gpu.outputs_changed();
            }
            for output in self.outputs.iter_mut() {
                output.target.full_redraw = true;
                output.target.presented_damage.clear();
            }
        }
        self.update_desktop_workarea();
        self.reflow_maximized_windows_to_workarea();
        self.full_redraw_needed = true;
        self.confine_cursor_to_outputs();
        self.apply_workspace_presentation_policy();
        self.refresh_input_panel();
        self.publish_workspace_state();
        Ok(())
    }

    pub(super) fn update_desktop_workarea(&mut self) {
        let desktop = self.outputs.desktop_primary();
        self.workarea = desktop.workarea;
        let area = self.output_workarea(desktop.id);
        self.window_manager
            .set_workarea(area.0, area.1, area.2, area.3);
    }

    /// Keep each application and its transient surfaces together when a display
    /// leaves the desktop. The shell manages its connector-bound surfaces.
    pub(super) fn relocate_output_applications(&mut self, source: OutputId, old: OutputRect) {
        let desktop = self.outputs.desktop_primary();
        let destination = desktop.id;
        let bounds = desktop.rect;
        let roots = self
            .window_manager
            .get_windows()
            .iter()
            .filter(|window| {
                window.parent.is_none()
                    && matches!(
                        window.window_type,
                        WindowType::Normal | WindowType::AlwaysOnTop
                    )
                    && !is_shell_app_id(window.app_id.as_deref().unwrap_or(b""))
                    && window.output_id == source
            })
            .map(|window| window.id)
            .collect::<Vec<_>>();
        for root in roots {
            let group = self
                .window_manager
                .get_windows()
                .iter()
                .filter(|w| self.top_level_window_id(w.id) == root)
                .map(|w| w.id)
                .collect::<Vec<_>>();
            let Some(window) = self.window_manager.get_window(root) else {
                continue;
            };
            let (x, y, width, height) = window.surface_geometry();
            let place = |value: i32, origin: i32, start: i32, span: u32, extent: u32| {
                start.saturating_add(value.saturating_sub(origin).clamp(
                    0,
                    i32::try_from(span.saturating_sub(extent)).unwrap_or(i32::MAX),
                ))
            };
            let dx = place(x, old.x, bounds.x, bounds.width, width).saturating_sub(x);
            let dy = place(y, old.y, bounds.y, bounds.height, height).saturating_sub(y);
            for id in group {
                if let Some(w) = self.window_manager.get_window_mut(id)
                    && !w.fullscreen
                {
                    w.translate_output(dx, dy);
                }
            }
            self.bind_window_output(root, destination);
        }
    }

    fn execute_output_control(
        &mut self,
        client: usize,
        command: OutputRequest,
    ) -> Result<OutputEvent, u32> {
        use sws_protocol::error_codes as errors;
        match command {
            OutputRequest::Get | OutputRequest::GetLegacy => {
                Ok(OutputEvent::Snapshot(self.outputs.snapshot()))
            }
            OutputRequest::GetBacklight | OutputRequest::SetBacklight(_) => {
                Err(errors::BACKLIGHT_UNAVAILABLE)
            }
            OutputRequest::Modes(id) => {
                let output = self.outputs.get(id).ok_or(errors::UNKNOWN_OUTPUT)?;
                let modes = self
                    .secondary_endpoints
                    .iter()
                    .find(|endpoint| endpoint.output == Some(id))
                    .and_then(|endpoint| endpoint.control.modes().ok())
                    .map(|modes| {
                        modes
                            .into_iter()
                            .map(|mode| sws_protocol::outputs::OutputMode {
                                pixel_width: mode.width,
                                pixel_height: mode.height,
                                refresh_millihz: mode.refresh_millihz,
                            })
                            .collect()
                    })
                    .unwrap_or_else(|| vec![output.metadata.mode]);
                Ok(OutputEvent::Modes { output: id, modes })
            }
            OutputRequest::SetMode { output, mode } => {
                let endpoint = self
                    .secondary_endpoints
                    .iter()
                    .find(|endpoint| endpoint.output == Some(output))
                    .ok_or(errors::UNKNOWN_OUTPUT)?;
                endpoint
                    .control
                    .request_mode(framebuffer::DisplayMode {
                        width: mode.pixel_width,
                        height: mode.pixel_height,
                        refresh_millihz: mode.refresh_millihz,
                    })
                    .map_err(|_| errors::INVALID_OUTPUT_CONFIGURATION)?;
                self.pending_output_modes.retain(|(id, _)| *id != output);
                self.pending_output_modes.push((output, mode));
                Ok(OutputEvent::Applied)
            }
            OutputRequest::Context(requested) => {
                let id = if requested == u32::MAX {
                    self.outputs
                        .output_at(self.cursor.x, self.cursor.y)
                        .map_or(self.outputs.desktop_primary().id, |output| output.id)
                } else {
                    requested
                };
                let context = self.output_context_for(id).ok_or(errors::UNKNOWN_OUTPUT)?;
                super::super::ipc::set_client_output_context(client, context);
                self.project_client_environment(client);
                Ok(OutputEvent::Context(context))
            }
            OutputRequest::ApplyLegacy(transaction) => {
                let snapshot = self.outputs.snapshot();
                if transaction.base_generation != snapshot.generation {
                    return Err(errors::STALE_OUTPUT_GENERATION);
                }
                let full = transaction
                    .merge_legacy(&snapshot)
                    .map_err(|_| errors::INVALID_OUTPUT_CONFIGURATION)?;
                self.execute_output_control(client, OutputRequest::Apply(full))
            }
            OutputRequest::Apply(transaction) => {
                let configuration = self.outputs.validated_configuration(&transaction).map_err(
                    |error| match error {
                        super::super::output::ConfigurationError::Stale => {
                            errors::STALE_OUTPUT_GENERATION
                        }
                        super::super::output::ConfigurationError::Invalid => {
                            errors::INVALID_OUTPUT_CONFIGURATION
                        }
                    },
                )?;
                self.install_output_configuration(&configuration, true)?;
                Ok(OutputEvent::Snapshot(self.outputs.snapshot()))
            }
            OutputRequest::Workspace(output) => {
                self.outputs.desktop(output).ok_or(errors::UNKNOWN_OUTPUT)?;
                let state = self
                    .workspace_manager
                    .output(output)
                    .ok_or(errors::UNKNOWN_OUTPUT)?
                    .snapshot();
                Ok(OutputEvent::Workspace { output, state })
            }
            OutputRequest::ApplyWorkspace {
                output,
                transaction,
            } => {
                if !super::super::ipc::is_system_shell_client(client) {
                    return Err(errors::SYSTEM_SHELL_REQUIRED);
                }
                self.outputs.desktop(output).ok_or(errors::UNKNOWN_OUTPUT)?;
                let live = self
                    .live_workspace_window_ids()
                    .into_iter()
                    .filter(|window| {
                        self.workspace_manager.output_for_window(*window) == Some(output)
                    })
                    .collect::<Vec<_>>();
                self.workspace_manager
                    .output_mut(output)
                    .ok_or(errors::UNKNOWN_OUTPUT)?
                    .apply_transaction(transaction, &live)
                    .map_err(|error| match error {
                        super::super::workspace::ApplyError::StaleGeneration => {
                            errors::STALE_WORKSPACE_GENERATION
                        }
                        super::super::workspace::ApplyError::InvalidState => {
                            errors::INVALID_WORKSPACE_STATE
                        }
                    })?;
                self.apply_workspace_presentation_policy_on(Some(output));
                self.publish_workspace_state();
                if let Some(target) = self.outputs.get_mut(output) {
                    target.target.full_redraw = true;
                }
                let state = self.workspace_manager.output(output).unwrap().snapshot();
                Ok(OutputEvent::Workspace { output, state })
            }
            OutputRequest::Workarea {
                output,
                x,
                y,
                width,
                height,
            } => {
                let context = self
                    .output_context_for(output)
                    .ok_or(errors::UNKNOWN_OUTPUT)?;
                let convert =
                    |value: u32| (u64::from(value) * 2000 / u64::from(context.scale_milli)) as u32;
                let area = (
                    context.rect.x.saturating_mul(2).saturating_add(
                        (i64::from(x) * 2000 / i64::from(context.scale_milli)) as i32,
                    ),
                    context.rect.y.saturating_mul(2).saturating_add(
                        (i64::from(y) * 2000 / i64::from(context.scale_milli)) as i32,
                    ),
                    convert(width),
                    convert(height),
                );
                if area.2 == 0
                    || area.3 == 0
                    || area.0 < context.rect.x.saturating_mul(2)
                    || area.1 < context.rect.y.saturating_mul(2)
                    || i64::from(area.0) + i64::from(area.2) > context.rect.right() * 2
                    || i64::from(area.1) + i64::from(area.3) > context.rect.bottom() * 2
                {
                    return Err(errors::INVALID_OUTPUT_CONFIGURATION);
                }
                self.outputs.get_mut(output).unwrap().workarea = Some(area);
                if output == self.outputs.desktop_primary().id {
                    self.workarea = Some(area);
                    self.window_manager
                        .set_workarea(area.0, area.1, area.2, area.3);
                }
                self.apply_workspace_presentation_policy();
                Ok(OutputEvent::Applied)
            }
            OutputRequest::Bind { window, output } => {
                if !self.client_owns_window(client, window) {
                    return Err(errors::WINDOW_NOT_OWNED);
                }
                if !self.bind_window_output(window, output) {
                    return Err(errors::UNKNOWN_OUTPUT);
                }
                self.apply_workspace_presentation_policy();
                self.publish_workspace_state();
                Ok(OutputEvent::Applied)
            }
        }
    }
}

pub(super) fn describe_output(
    id: u32,
    info: Option<framebuffer::DisplayOutputInfo>,
    width: u32,
    height: u32,
    primary: bool,
) -> OutputDescriptor {
    let key = info
        .as_ref()
        .and_then(|info| info.connector_key())
        .unwrap_or_else(|| format!("session:output{}", id));
    let monitor_key = info
        .as_ref()
        .and_then(|info| info.monitor_key())
        .unwrap_or("")
        .to_string();
    let name = info
        .as_ref()
        .map(|info| {
            if info.monitor_name().is_empty() {
                info.name()
            } else {
                info.monitor_name()
            }
        })
        .unwrap_or("Display")
        .to_string();
    OutputDescriptor {
        id,
        connector_key: key,
        monitor_key,
        identity_is_unique: info.as_ref().is_some_and(|info| info.monitor_has_serial()),
        name,
        mode: sws_protocol::outputs::OutputMode {
            pixel_width: width,
            pixel_height: height,
            refresh_millihz: info.as_ref().map_or(0, |info| info.refresh_mhz),
        },
        primary,
        internal: info.as_ref().map_or(primary, |info| {
            info.kind == framebuffer::DISPLAY_CONNECTOR_KIND_INTERNAL
        }),
        physical_width_mm: info.as_ref().map_or(0, |info| info.physical_width_mm),
        physical_height_mm: info.as_ref().map_or(0, |info| info.physical_height_mm),
    }
}

#[cfg(test)]
mod output_liveness_tests {
    use super::*;
    use framebuffer::{DISPLAY_OUTPUT_FLAG_CONNECTED, DisplayInfo, DisplayOutputInfo};

    #[test]
    fn legacy_display_uses_its_live_framebuffer_extent() {
        let descriptor = describe_output(0, None, 800, 600, true);
        let mut backing = DisplayInfo {
            width: 800,
            height: 600,
            ..DisplayInfo::default()
        };
        assert!(Compositor::live_output_matches(&descriptor, backing, None));
        backing.width = 1024;
        assert!(!Compositor::live_output_matches(&descriptor, backing, None));
    }

    #[test]
    fn unplug_and_same_mode_monitor_replacement_invalidate_the_transaction() {
        // SAFETY: this kernel record consists entirely of integers and byte arrays.
        let mut info: DisplayOutputInfo = unsafe { core::mem::zeroed() };
        info.size = core::mem::size_of::<DisplayOutputInfo>() as u32;
        info.flags = DISPLAY_OUTPUT_FLAG_CONNECTED;
        info.width = 960;
        info.height = 540;
        info.device_key[0] = b'd';
        info.monitor_key[0] = b'a';
        let descriptor = describe_output(1, Some(info), info.width, info.height, false);
        let backing = DisplayInfo {
            width: info.width,
            height: info.height,
            ..DisplayInfo::default()
        };
        assert!(Compositor::live_output_matches(
            &descriptor,
            backing,
            Some(info)
        ));
        assert!(!Compositor::live_output_matches(&descriptor, backing, None));
        info.monitor_key[0] = b'b';
        assert!(!Compositor::live_output_matches(
            &descriptor,
            backing,
            Some(info)
        ));
        info.monitor_key[0] = b'a';
        info.flags = 0;
        assert!(!Compositor::live_output_matches(
            &descriptor,
            backing,
            Some(info)
        ));
    }
}
