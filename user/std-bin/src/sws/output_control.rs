//! Output-scoped session control; all mutations run on the compositor thread.
use super::*;
use sws_protocol::outputs::{OutputConfiguration, OutputContext, OutputDescriptor, OutputEvent, OutputRequest};

impl Compositor {
    pub(super) fn pointer_output_id(&self, x: i32, y: i32) -> u32 {
        self.outputs.output_at(x, y).map_or(self.outputs.primary_id(), |output| output.id)
    }

    pub(super) fn keyboard_output_id(&self) -> u32 {
        self.window_manager.get_focused_window_id()
            .and_then(|id| self.window_manager.get_window(id))
            .filter(|window| self.outputs.get(window.output_id).is_some())
            .map_or_else(|| self.pointer_output_id(self.cursor.x, self.cursor.y), |window| window.output_id)
    }

    pub(super) fn keyboard_workspace(&self) -> &super::super::workspace::OutputWorkspaces {
        self.workspace_manager.output(self.keyboard_output_id()).expect("connected output workspace")
    }

    pub(super) fn keyboard_workspace_mut(&mut self) -> super::super::workspace::WorkspaceScope<'_> {
        let output = self.keyboard_output_id();
        self.workspace_manager.output_mut(output).expect("connected output workspace")
    }

    /// Change a transient group's output together with its workspace and frame
    /// clock. Source images remain retained until the new-scale frame arrives.
    pub(super) fn bind_window_output(&mut self, window: u32, output: u32) -> bool {
        let Some(context) = self.output_context_for(output) else { return false; };
        let root = self.top_level_window_id(window);
        if self.window_manager.get_window(root).is_none() { return false; }
        let group: Vec<_> = self.window_manager.get_windows().iter()
            .filter(|w| self.top_level_window_id(w.id) == root).map(|w| w.id).collect();
        if let Some(destination) = self.workspace_manager.output(output).map(|state| state.active_workspace()) {
            self.workspace_manager.transfer_window(root, destination);
        }
        let counter = self.outputs.get(output).unwrap().present_counter;
        for id in group {
            let old_output = self.window_output_id(id);
            if let Some(w) = self.window_manager.get_window_mut(id) {
                if old_output != output {
                    w.last_frame_submission_counter = Some(counter);
                    w.last_frame_credit_tick = None;
                    for callback in &mut self.pending_frame_callbacks {
                        if callback.window_id == id && callback.requested_after_present.is_some() {
                            callback.requested_after_present = Some(counter);
                        }
                    }
                }
                w.output_id = output;
                if w.owner_client_id.is_some_and(|client| super::super::ipc::client_output_context(client).is_some()) {
                    w.prefer_scale(context.scale_milli);
                }
            }
            if let Some(source) = self.outputs.get_mut(old_output) { source.target.full_redraw = true; }
            self.send_current_window_configure(id);
        }
        self.outputs.get_mut(output).unwrap().target.full_redraw = true;
        true
    }

    pub(super) fn finish_window_output_drag(&mut self, window: u32) {
        let Some(w) = self.window_manager.get_window(window) else { return; };
        let (x, y, width, height) = w.window_geometry();
        let previous = w.output_id;
        // Choose the greatest visible area after release. Keeping the buffer
        // scale stable during the drag avoids reallocating on every crossing.
        let Some((output, area)) = self.outputs.iter().map(|output| {
            let r = output.rect;
            let width = (i64::from(x) + i64::from(width)).min(i64::from(r.x) + i64::from(r.width)) - i64::from(x.max(r.x));
            let height = (i64::from(y) + i64::from(height)).min(i64::from(r.y) + i64::from(r.height)) - i64::from(y.max(r.y));
            (output.id, width.max(0) * height.max(0))
        }).max_by_key(|(id, area)| (*area, *id == previous)) else { return; };
        if area == 0 || output == previous || !self.bind_window_output(window, output) { return; }
        if let Some(mut state) = self.workspace_manager.output_mut(output) { state.return_to_workspace(); }
        self.apply_workspace_presentation_policy_on(Some(output));
        self.publish_workspace_state();
    }

    pub(super) fn output_window_position(&self, output: u32, width: u32, height: u32, centered: bool) -> (i32, i32) {
        if !centered { return self.default_window_position(output, width, height); }
        let area = self.output_workarea(output);
        let x = area.2.saturating_sub(width) / 2;
        let y = area.3.saturating_sub(height) / 2;
        (area.0.saturating_add(x as i32), area.1.saturating_add(y as i32))
    }

    pub(super) fn output_context_for(&self, id: u32) -> Option<OutputContext> {
        let output = self.outputs.get(id)?;
        Some(OutputContext { id, scale_milli: output.scale_milli, mode: output.metadata.mode,
            rect: output.logical_rect, tablet: output.experience.tablet_experience(output.metadata.internal, self.tablet_mode) })
    }

    pub(super) fn notify_window_output(&self, window: u32) {
        let Some(w) = self.window_manager.get_window(window) else { return; };
        // Legacy clients cannot decode this additive event. Only connections
        // which requested an output context opt into logical configures.
        if !w.owner_client_id.is_some_and(|client| super::super::ipc::client_output_context(client).is_some()) { return; }
        let (width, height) = w.configured_pixel_extent();
        let (_, _, scene_width, scene_height) = w.surface_geometry();
        let Some(output) = self.outputs.get(w.output_id) else { return; };
        super::super::ipc::send_message_to_window(window, sws_protocol::server_msg::OUTPUT_CONTROL,
            sws_protocol::outputs::encode_event(&OutputEvent::Window { window, output: w.output_id, scale_milli: w.preferred_scale_milli, width, height,
                logical_width: scene_width.div_ceil(2), logical_height: scene_height.div_ceil(2), output_x: output.logical_rect.x, output_y: output.logical_rect.y }));
    }

    pub(super) fn publish_outputs(&self) {
        let contexts = self.outputs.iter().filter_map(|output| self.output_context_for(output.id)).collect::<Vec<_>>();
        super::super::ipc::refresh_output_contexts(&contexts);
        super::super::ipc::publish_output_event(OutputEvent::Snapshot(self.outputs.snapshot()));
        for state in self.workspace_manager.session_snapshot() {
            if let Some(output) = state.output_id {
                super::super::ipc::publish_output_event(OutputEvent::Workspace { output, state: state.state });
            }
        }
    }

    pub(super) fn handle_output_control(&mut self, client: usize, request: u8, command: OutputRequest) -> Result<(), &'static str> {
        let result = self.execute_output_control(client, command);
        match result {
            Ok(event) => send_response_to_client(client, sws_protocol::server_msg::OUTPUT_CONTROL, request, sws_protocol::outputs::encode_event(&event)),
            Err(code) => send_response_to_client(client, sws_protocol::server_msg::ERROR, request, sws_protocol::payload_error(code).to_vec()),
        }
        Ok(())
    }

    pub(super) fn output_workarea(&self, id: u32) -> (i32, i32, u32, u32) {
        let output = self.outputs.get(id).unwrap_or_else(|| self.outputs.primary());
        output.workarea.unwrap_or((output.rect.x, output.rect.y, output.rect.width, output.rect.height))
    }

    pub(super) fn window_output_id(&self, id: u32) -> u32 {
        self.window_manager.get_window(id).map_or(self.outputs.primary_id(), |window| window.output_id)
    }

    pub(super) fn window_uses_focused_mode(&self, id: u32) -> bool {
        let output = self.window_output_id(id);
        let Some(context) = self.output_context_for(output) else { return false; };
        context.tablet || (output == self.outputs.primary_id() && self.windowing_mode == sws_protocol::WindowingMode::Focused)
    }

    pub(super) fn restore_output_configuration(&mut self) {
        if let Some(configuration) = super::super::config::saved_output_layout(&self.outputs.snapshot().outputs) {
            if let Err(error) = self.install_output_configuration(&configuration, false) {
                println!("[Compositor] Saved display layout could not be restored: {}", error);
            }
        }
    }

    fn install_output_configuration(&mut self, configuration: &[OutputConfiguration], persist: bool) -> Result<(), u32> {
        use sws_protocol::error_codes as errors;
        let mut workspaces = self.workspace_manager.clone();
        for entry in configuration {
            let output = self.outputs.get(entry.id).ok_or(errors::UNKNOWN_OUTPUT)?;
            workspaces.apply_output_experience(entry.id, entry.experience, output.metadata.internal, self.tablet_mode, None)
                .map_err(|_| errors::INVALID_OUTPUT_CONFIGURATION)?;
        }
        let old = self.outputs.snapshot();
        let proposed = sws_protocol::outputs::OutputSnapshot { generation: old.generation, outputs: old.outputs.clone(), configuration: configuration.to_vec() };
        if persist { super::super::config::persist_outputs(&proposed).map_err(|_| errors::OUTPUT_CONFIGURATION_NOT_SAVED)?; }
        self.workspace_manager = workspaces;
        self.outputs.apply_configuration(configuration);
        super::super::input::set_screen_size(self.screen_width(), self.screen_height());
        for entry in configuration {
            let previous = old.configuration.iter().find(|previous| previous.id == entry.id).unwrap();
            if entry == previous { continue; }
            let mut notifications = Vec::new();
            for window in self.window_manager.get_windows_mut() {
                if window.output_id != entry.id { continue; }
                window.translate_output(entry.x.saturating_sub(previous.x).saturating_mul(2), entry.y.saturating_sub(previous.y).saturating_mul(2));
                if window.preferred_scale_milli != entry.scale_milli {
                    if window.owner_client_id.is_some_and(|client| super::super::ipc::client_output_context(client).is_some()) {
                        window.prefer_scale(entry.scale_milli);
                    }
                    notifications.push(window.id);
                }
            }
            if let Some(output) = self.outputs.get_mut(entry.id) { output.target.full_redraw = true; }
            for window in notifications { self.notify_window_output(window); }
        }
        self.workarea = self.outputs.primary().workarea;
        if let Some(area) = self.workarea { self.window_manager.set_workarea(area.0, area.1, area.2, area.3); }
        self.reflow_maximized_windows_to_workarea();
        self.full_redraw_needed = true;
        self.confine_cursor_to_outputs();
        self.apply_workspace_presentation_policy();
        self.publish_outputs();
        Ok(())
    }

    fn execute_output_control(&mut self, client: usize, command: OutputRequest) -> Result<OutputEvent, u32> {
        use sws_protocol::error_codes as errors;
        match command {
            OutputRequest::Get => Ok(OutputEvent::Snapshot(self.outputs.snapshot())),
            OutputRequest::Context(requested) => {
                let id = if requested == u32::MAX {
                    self.outputs.output_at(self.cursor.x, self.cursor.y).map_or(self.outputs.primary_id(), |output| output.id)
                } else { requested };
                let context = self.output_context_for(id).ok_or(errors::UNKNOWN_OUTPUT)?;
                super::super::ipc::set_client_output_context(client, context);
                Ok(OutputEvent::Context(context))
            }
            OutputRequest::Apply(transaction) => {
                let configuration = self.outputs.validated_configuration(&transaction).map_err(|error| match error {
                    super::super::output::ConfigurationError::Stale => errors::STALE_OUTPUT_GENERATION,
                    super::super::output::ConfigurationError::Invalid => errors::INVALID_OUTPUT_CONFIGURATION,
                })?;
                self.install_output_configuration(&configuration, true)?;
                Ok(OutputEvent::Snapshot(self.outputs.snapshot()))
            }
            OutputRequest::Workspace(output) => {
                let state = self.workspace_manager.output(output).ok_or(errors::UNKNOWN_OUTPUT)?.snapshot();
                Ok(OutputEvent::Workspace { output, state })
            }
            OutputRequest::ApplyWorkspace { output, transaction } => {
                if !super::super::ipc::is_system_shell_client(client) { return Err(errors::SYSTEM_SHELL_REQUIRED); }
                let live = self.live_workspace_window_ids().into_iter().filter(|window| self.workspace_manager.output_for_window(*window) == Some(output)).collect::<Vec<_>>();
                let state = self.workspace_manager.output_mut(output).ok_or(errors::UNKNOWN_OUTPUT)?
                    .apply_transaction(transaction, &live).map_err(|error| match error {
                        super::super::workspace::ApplyError::StaleGeneration => errors::STALE_WORKSPACE_GENERATION,
                        super::super::workspace::ApplyError::InvalidState => errors::INVALID_WORKSPACE_STATE,
                    })?.state;
                self.apply_workspace_presentation_policy_on(Some(output));
                self.publish_outputs();
                if let Some(target) = self.outputs.get_mut(output) { target.target.full_redraw = true; }
                Ok(OutputEvent::Workspace { output, state })
            }
            OutputRequest::Workarea { output, x, y, width, height } => {
                let context = self.output_context_for(output).ok_or(errors::UNKNOWN_OUTPUT)?;
                let convert = |value: u32| (u64::from(value) * 2000 / u64::from(context.scale_milli)) as u32;
                let area = (context.rect.x.saturating_mul(2).saturating_add((i64::from(x) * 2000 / i64::from(context.scale_milli)) as i32),
                    context.rect.y.saturating_mul(2).saturating_add((i64::from(y) * 2000 / i64::from(context.scale_milli)) as i32), convert(width), convert(height));
                if area.2 == 0 || area.3 == 0 || area.0 < context.rect.x.saturating_mul(2) || area.1 < context.rect.y.saturating_mul(2)
                    || i64::from(area.0) + i64::from(area.2) > context.rect.right() * 2
                    || i64::from(area.1) + i64::from(area.3) > context.rect.bottom() * 2 { return Err(errors::INVALID_OUTPUT_CONFIGURATION); }
                self.outputs.get_mut(output).unwrap().workarea = Some(area);
                if output == self.outputs.primary_id() {
                    self.workarea = Some(area);
                    self.window_manager.set_workarea(area.0, area.1, area.2, area.3);
                    self.publish_window_creation_environment();
                }
                self.apply_workspace_presentation_policy();
                Ok(OutputEvent::Applied)
            }
            OutputRequest::Bind { window, output } => {
                if !self.client_owns_window(client, window) { return Err(errors::WINDOW_NOT_OWNED); }
                if !self.bind_window_output(window, output) { return Err(errors::UNKNOWN_OUTPUT); }
                self.apply_workspace_presentation_policy();
                self.publish_workspace_state();
                Ok(OutputEvent::Applied)
            }
        }
    }
}

pub(super) fn describe_output(id: u32, info: Option<framebuffer::DisplayOutputInfo>, width: u32, height: u32, primary: bool) -> OutputDescriptor {
    let key = info.as_ref().and_then(|info| info.connector_key()).unwrap_or_else(|| format!("session:output{}", id));
    let monitor_key = info.as_ref().and_then(|info| info.monitor_key()).unwrap_or("").to_string();
    let name = info.as_ref().map(|info| if info.monitor_name().is_empty() { info.name() } else { info.monitor_name() }).unwrap_or("Display").to_string();
    OutputDescriptor { id, connector_key: key, monitor_key,
        identity_is_unique: info.as_ref().is_some_and(|info| info.monitor_has_serial()), name,
        mode: sws_protocol::outputs::OutputMode { pixel_width: width, pixel_height: height, refresh_millihz: info.as_ref().map_or(0, |info| info.refresh_mhz) },
        primary, internal: info.as_ref().map_or(primary, |info| info.kind == framebuffer::DISPLAY_CONNECTOR_KIND_INTERNAL),
        physical_width_mm: info.as_ref().map_or(0, |info| info.physical_width_mm), physical_height_mm: info.as_ref().map_or(0, |info| info.physical_height_mm) }
}
