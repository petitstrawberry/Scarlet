//! Protocol adapter for SWS compound surfaces. Wayland pending/cached state is
//! resolved here; the atomic SWS message contains buffer IDs, never pixel copies.
use super::*;

impl WaylandBridge {
    pub(super) fn output_scale_events(&self, output: u32) -> Vec<WaylandMessage> {
        if self.object_versions.get(&output).copied().unwrap_or(1) < 2 {
            return Vec::new();
        }
        let mut scale = WaylandMessage::new(output, protocol::output_event::SCALE);
        scale.add_arg(WaylandArg::Int(self.output_scale));
        Vec::from([
            scale,
            WaylandMessage::new(output, protocol::output_event::DONE),
        ])
    }

    /// Output membership follows the published transaction, including mapped
    /// subsurfaces. Clients use enter to associate a surface with wl_output.scale.
    pub(super) fn update_surface_output(&mut self, root: u32, surfaces: Vec<u32>) {
        let old: Vec<_> = self
            .output_surfaces
            .iter()
            .filter_map(|(&id, &owner)| (owner == root).then_some(id))
            .collect();
        let mut messages = Vec::new();
        for (id, enter) in old
            .iter()
            .filter(|id| !surfaces.contains(id))
            .map(|&id| (id, false))
            .chain(
                surfaces
                    .iter()
                    .filter(|id| !old.contains(id))
                    .map(|&id| (id, true)),
            )
        {
            for (&output, interface) in &self.objects {
                if interface != "wl_output" {
                    continue;
                }
                let mut message = WaylandMessage::new(
                    id,
                    if enter {
                        protocol::surface_event::ENTER
                    } else {
                        protocol::surface_event::LEAVE
                    },
                );
                message.add_arg(WaylandArg::Object(output));
                messages.push(message);
            }
        }
        self.output_surfaces.retain(|_, owner| *owner != root);
        for id in surfaces {
            self.output_surfaces.insert(id, root);
        }
        self.queue_input_messages(messages);
    }

    pub(super) fn scene_protocol_error(&self, object: u32, error: &'static str) -> WaylandMessage {
        let viewport = self
            .viewports
            .iter()
            .find_map(|(&id, &surface)| (surface == object).then_some(id));
        let interface = self.objects.get(&object).map(String::as_str).unwrap_or("");
        let (object, code) = if error == "Viewport source outside buffer" {
            (viewport.unwrap_or(object), 2)
        } else if error == "Viewport destination must be integral" {
            (viewport.unwrap_or(object), 1)
        } else if error == "Viewport has no surface" {
            (object, 3)
        } else if matches!(
            interface,
            "wl_subcompositor" | "wl_subsurface" | "wp_viewporter" | "wp_viewport"
        ) {
            (object, 0)
        } else {
            (object, 3)
        };
        let mut message = WaylandMessage::new(1, protocol::display_event::ERROR);
        message.add_arg(WaylandArg::Object(object));
        message.add_arg(WaylandArg::Uint(code));
        message.add_arg(WaylandArg::String(error.as_bytes().to_vec()));
        message
    }

    pub(super) fn update_scene_pointer_focus(&mut self) {
        let Some(root) = self.focused_surface else {
            return;
        };
        if !self.scene.enabled(root) || !self.pointer_buttons.is_empty() {
            return;
        }
        let target = self.scene.hit(root, self.pointer_x, self.pointer_y);
        let next = target.map(|v| v.0);
        if self.pointer_surface == next {
            return;
        }
        let Some(pointer) = self.focused_pointer else {
            return;
        };
        if let Some(old) = self.pointer_surface {
            let mut message = WaylandMessage::new(pointer, input::pointer_event::LEAVE);
            message.add_arg(WaylandArg::Uint(self.allocate_serial()));
            message.add_arg(WaylandArg::Object(old));
            self.pending_pointer_messages.push(message);
        }
        self.pointer_surface = next;
        if let Some((id, x, y)) = target {
            let mut message = WaylandMessage::new(pointer, input::pointer_event::ENTER);
            message.add_arg(WaylandArg::Uint(self.allocate_serial()));
            message.add_arg(WaylandArg::Object(id));
            message.add_arg(WaylandArg::Fixed(x.saturating_mul(256)));
            message.add_arg(WaylandArg::Fixed(y.saturating_mul(256)));
            self.pending_pointer_messages.push(message);
        }
        self.pending_pointer_id = Some(pointer);
    }

    fn delete_scene_object(&mut self, id: u32) -> Vec<WaylandMessage> {
        self.remove_object(id);
        let mut message = WaylandMessage::new(1, protocol::display_event::DELETE_ID);
        message.add_arg(WaylandArg::Uint(id));
        Vec::from([message])
    }
    fn scene_new_id(&self, payload: &[u8]) -> Result<u32, &'static str> {
        let id = Self::parse_u32(payload, 0).ok_or("Missing new object ID")?;
        if id == 0 || self.objects.contains_key(&id) {
            return Err("Invalid new object ID");
        }
        Ok(id)
    }
    pub(super) fn handle_scene_message(
        &mut self,
        id: u32,
        interface: &str,
        opcode: u16,
        payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        let before = self.scene.references();
        let mut messages = Vec::new();
        match (interface, opcode) {
            ("wl_subcompositor", 0) | ("wp_viewporter", 0) => {
                if !payload.is_empty() {
                    return Err("Invalid destroy request");
                }
                messages = self.delete_scene_object(id);
            }
            ("wl_subcompositor", 1) => {
                if payload.len() != 12 {
                    return Err("Invalid get_subsurface request");
                }
                let object = self.scene_new_id(payload)?;
                let child = Self::parse_u32(payload, 4).unwrap();
                let parent = Self::parse_u32(payload, 8).unwrap();
                if self.surface_manager.get_surface(child).is_none_or(|s| {
                    s.role.is_some() && s.role != Some(surface::SurfaceRole::Subsurface)
                }) || self.subsurfaces.values().any(|s| *s == child)
                    || self
                        .xdg_shell_manager
                        .get_xdg_surface_ids_by_wl_surface(child)
                        .is_some()
                {
                    return Err("Surface already has a role");
                }
                self.scene.attach_child(child, parent)?;
                self.surface_manager
                    .get_surface_mut(child)
                    .unwrap()
                    .set_role(surface::SurfaceRole::Subsurface);
                self.subsurfaces.insert(object, child);
                self.add_object(object, String::from("wl_subsurface"));
            }
            ("wl_subsurface", 0) => {
                if !payload.is_empty() {
                    return Err("Invalid subsurface destroy");
                }
                let child = self.subsurfaces.remove(&id).ok_or("Unknown subsurface")?;
                let callbacks = self.scene.take_own_callbacks(child);
                self.discard_callbacks(callbacks);
                if let Some(root) = self.scene.detach(child) {
                    self.publish_scene(root)?;
                }
                messages = self.delete_scene_object(id);
            }
            ("wl_subsurface", 1) => {
                if payload.len() != 8 {
                    return Err("Invalid subsurface position");
                }
                let child = *self.subsurfaces.get(&id).ok_or("Unknown subsurface")?;
                self.scene.position(
                    child,
                    Self::parse_i32(payload, 0).unwrap(),
                    Self::parse_i32(payload, 4).unwrap(),
                )?;
            }
            ("wl_subsurface", 2) | ("wl_subsurface", 3) => {
                if payload.len() != 4 {
                    return Err("Invalid subsurface stacking request");
                }
                let child = *self.subsurfaces.get(&id).ok_or("Unknown subsurface")?;
                self.scene
                    .restack(child, Self::parse_u32(payload, 0).unwrap(), opcode == 2)?;
            }
            ("wl_subsurface", 4) | ("wl_subsurface", 5) => {
                if !payload.is_empty() {
                    return Err("Invalid subsurface synchronization request");
                }
                let child = *self.subsurfaces.get(&id).ok_or("Unknown subsurface")?;
                if let Some(root) = self.scene.set_sync(child, opcode == 4)? {
                    self.publish_scene(root)?;
                }
            }
            ("wp_viewporter", 1) => {
                if payload.len() != 8 {
                    return Err("Invalid get_viewport request");
                }
                let object = self.scene_new_id(payload)?;
                let surface = Self::parse_u32(payload, 4).unwrap();
                if self.surface_manager.get_surface(surface).is_none() {
                    return Err("Unknown viewport surface");
                }
                if self.viewports.values().any(|s| *s == surface) {
                    return Err("Viewport already exists");
                }
                self.scene.enable(surface);
                self.viewports.insert(object, surface);
                self.add_object(object, String::from("wp_viewport"));
            }
            ("wp_viewport", 0) => {
                if !payload.is_empty() {
                    return Err("Invalid viewport destroy");
                }
                let surface = self.viewports.remove(&id).ok_or("Unknown viewport")?;
                if let Ok(view) = self.scene.view_mut(surface) {
                    view.source = None;
                    view.destination = None;
                }
                messages = self.delete_scene_object(id);
            }
            ("wp_viewport", 1) => {
                if payload.len() != 16 {
                    return Err("Invalid viewport source");
                }
                let surface = *self.viewports.get(&id).ok_or("Unknown viewport")?;
                let r = (
                    Self::parse_i32(payload, 0).unwrap(),
                    Self::parse_i32(payload, 4).unwrap(),
                    Self::parse_i32(payload, 8).unwrap(),
                    Self::parse_i32(payload, 12).unwrap(),
                );
                let source = if r == (-256, -256, -256, -256) {
                    None
                } else {
                    if r.0 < 0 || r.1 < 0 || r.2 <= 0 || r.3 <= 0 {
                        return Err("Invalid viewport source values");
                    }
                    Some(r)
                };
                self.scene.view_mut(surface)?.source = source;
            }
            ("wp_viewport", 2) => {
                if payload.len() != 8 {
                    return Err("Invalid viewport destination");
                }
                let surface = *self.viewports.get(&id).ok_or("Unknown viewport")?;
                let w = Self::parse_i32(payload, 0).unwrap();
                let h = Self::parse_i32(payload, 4).unwrap();
                let destination = if (w, h) == (-1, -1) {
                    None
                } else {
                    if w <= 0 || h <= 0 {
                        return Err("Invalid viewport destination values");
                    }
                    Some((w as u32, h as u32))
                };
                self.scene.view_mut(surface)?.destination = destination;
            }
            _ => return Err("Unknown surface scene request"),
        }
        self.collect_scene_buffers(before)?;
        Ok(messages)
    }
    pub(super) fn scene_initial_configure(
        &mut self,
        surface_id: u32,
        empty: bool,
    ) -> Vec<WaylandMessage> {
        let mut messages = Vec::new();
        if !empty {
            return messages;
        }
        let Some((xdg_id, Some(toplevel_id))) = self
            .xdg_shell_manager
            .get_xdg_surface_ids_by_wl_surface(surface_id)
        else {
            return messages;
        };
        if self
            .xdg_shell_manager
            .get_xdg_surface(xdg_id)
            .is_none_or(|s| s.last_configure_serial.is_some())
        {
            return messages;
        }
        let serial = self.allocate_serial();
        let surface = self.xdg_shell_manager.get_xdg_surface_mut(xdg_id).unwrap();
        surface.last_configure_serial = Some(serial);
        let (max, full) = surface
            .toplevel
            .as_ref()
            .map(|v| (v.maximized, v.fullscreen))
            .unwrap_or((false, false));
        let mut configure =
            WaylandMessage::new(toplevel_id, xdg_shell::xdg_toplevel_event::CONFIGURE);
        configure.add_arg(WaylandArg::Int(0));
        configure.add_arg(WaylandArg::Int(0));
        configure.add_arg(WaylandArg::Array(Self::xdg_toplevel_state_bytes(max, full)));
        messages.push(configure);
        let mut configure = WaylandMessage::new(xdg_id, xdg_shell::xdg_surface_event::CONFIGURE);
        configure.add_arg(WaylandArg::Uint(serial));
        messages.push(configure);
        messages
    }
    /// Coalesce state, not pixels, while a previous scene is being presented.
    pub(super) fn publish_scene(&mut self, root: u32) -> Result<(), &'static str> {
        if !self.surface_manager.get_surface(root).is_some_and(|s| {
            matches!(
                s.role,
                Some(surface::SurfaceRole::XdgToplevel | surface::SurfaceRole::XdgPopup)
            )
        }) {
            return Ok(());
        }
        if self.surface_frame_request_outstanding.contains_key(&root) {
            self.dirty_scenes.insert(root);
            return Ok(());
        }
        // Finish a legacy queued commit before switching this root to scene mode.
        self.flush_pending_surface_commit(root)?;
        if self.surface_frame_request_outstanding.contains_key(&root) {
            self.dirty_scenes.insert(root);
            return Ok(());
        }
        let serial = self.allocate_extension_commit_serial();
        let mut commit = self.scene.scene(root, 0, serial)?;
        if !self.surface_to_window.contains_key(&root) {
            if commit.layers.is_empty() {
                return Ok(());
            }
            self.create_sws_window_with_size(root, commit.width, commit.height)?;
        }
        commit.window_id = self.surface_to_window[&root];
        let payload = commit.encode().map_err(|_| "Invalid SWS scene commit")?;
        self.send_sws_async_message(protocol_sws::client_msg::EXTENSION_COMMIT_SCENE, &payload)?;
        for layer in &commit.layers {
            self.sws_busy_buffers.insert(layer.buffer_id, commit.serial);
        }
        self.update_surface_output(
            root,
            commit.layers.iter().map(|layer| layer.surface_id).collect(),
        );
        self.submitted_scenes
            .insert(root, commit.layers.iter().map(|l| l.buffer_id).collect());
        self.submitted_surface_buffers.remove(&root);
        let callbacks = self.scene.take_callbacks(root);
        if commit.layers.is_empty() {
            // An unmapped surface has no presentation fence to wait for.
            let mut messages = Vec::new();
            let time = (monotonic_time_ns() / 1_000_000) as u32;
            for id in callbacks {
                self.append_callback_done(&mut messages, id, time);
            }
            self.queue_input_messages(messages);
        } else {
            self.pending_frame_callbacks
                .entry(root)
                .or_insert_with(Vec::new)
                .extend(callbacks);
            self.ensure_sws_frame_request(root, true)?;
            if self.focused_surface.is_none() {
                self.queue_focus_events(root);
            }
        }
        Ok(())
    }
    /// A hidden/cached child may outlive its last visible SWS use. In that case
    /// keep client ownership until the Wayland transaction also retires it.
    pub(super) fn collect_scene_buffers(&mut self, before: Vec<u32>) -> Result<(), &'static str> {
        let retained = self.scene.references();
        for id in before {
            if !retained.contains(&id)
                && !self.sws_busy_buffers.contains_key(&id)
                && !self
                    .submitted_surface_buffers
                    .values()
                    .any(|v| *v == Some(id))
                && !self.submitted_scenes.values().any(|v| v.contains(&id))
            {
                self.deferred_scene_releases.insert(id);
            }
        }
        let releases: Vec<_> = self
            .deferred_scene_releases
            .iter()
            .copied()
            .filter(|id| !retained.contains(id) && !self.sws_busy_buffers.contains_key(id))
            .collect();
        let mut messages = Vec::new();
        for id in releases {
            self.deferred_scene_releases.remove(&id);
            if let Some(buffer) = self.shm_manager.get_buffer_by_sws_id(id) {
                let id = buffer.buffer_id;
                self.append_buffer_release(&mut messages, id);
            }
        }
        self.queue_input_messages(messages);
        let destroys: Vec<_> = self
            .deferred_scene_destroys
            .iter()
            .copied()
            .filter(|id| !retained.contains(id))
            .collect();
        for id in destroys {
            self.deferred_scene_destroys.remove(&id);
            self.destroy_extension_buffer(id)?;
        }
        Ok(())
    }
}
