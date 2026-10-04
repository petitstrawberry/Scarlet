//! Physical output modes and xdg-output logical geometry for HiDPI clients.
use super::*;

impl WaylandBridge {
    fn xdg_output_geometry(&self, id: u32, initial: bool) -> Vec<WaylandMessage> {
        let version = self.object_versions.get(&id).copied().unwrap_or(1);
        let mut position = WaylandMessage::new(id, 0);
        position.add_arg(WaylandArg::Int(0));
        position.add_arg(WaylandArg::Int(0));
        let scale = self.output_scale.max(1) as u32;
        let mut size = WaylandMessage::new(id, 1);
        size.add_arg(WaylandArg::Int(self.output_size.0.div_ceil(scale) as i32));
        size.add_arg(WaylandArg::Int(self.output_size.1.div_ceil(scale) as i32));
        let mut messages = Vec::from([position, size]);
        if initial && version >= 2 {
            let mut name = WaylandMessage::new(id, 3);
            name.add_arg(WaylandArg::String(b"Scarlet-0".to_vec()));
            messages.push(name);
            let mut description = WaylandMessage::new(id, 4);
            description.add_arg(WaylandArg::String(b"Scarlet virtual display".to_vec()));
            messages.push(description);
        }
        let output = self.xdg_outputs[&id];
        // wl_output v1 cannot receive done. Retain the deprecated xdg-output
        // delimiter for those clients, even if they bound the manager at v3.
        if version < 3 || self.object_versions.get(&output).copied().unwrap_or(1) < 2 {
            messages.push(WaylandMessage::new(id, 2));
        }
        messages
    }

    pub(super) fn output_update_events(&self, output: u32) -> Vec<WaylandMessage> {
        let mut mode = WaylandMessage::new(output, protocol::output_event::MODE);
        mode.add_arg(WaylandArg::Uint(3)); // current and preferred
        mode.add_arg(WaylandArg::Int(self.output_size.0 as i32));
        mode.add_arg(WaylandArg::Int(self.output_size.1 as i32));
        mode.add_arg(WaylandArg::Int(60000));
        let mut messages = Vec::from([mode]);
        for (&id, &associated_output) in &self.xdg_outputs {
            if associated_output == output {
                messages.extend(self.xdg_output_geometry(id, false));
            }
        }
        // The wl_output.done delimiter must follow the xdg-output events.
        messages.extend(self.output_scale_events(output));
        messages
    }

    pub(super) fn handle_xdg_output_message(
        &mut self,
        object: u32,
        opcode: u16,
        payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        if opcode == 0 && payload.is_empty() {
            self.remove_object(object);
            let mut delete = WaylandMessage::new(1, protocol::display_event::DELETE_ID);
            delete.add_arg(WaylandArg::Uint(object));
            return Ok(Vec::from([delete]));
        }
        if self.objects.get(&object).map(String::as_str) != Some("zxdg_output_manager_v1")
            || opcode != 1
            || payload.len() != 8
        {
            return Err("Invalid xdg-output request");
        }
        let id = u32::from_ne_bytes(payload[0..4].try_into().unwrap());
        let output = u32::from_ne_bytes(payload[4..8].try_into().unwrap());
        if id == 0
            || self.objects.contains_key(&id)
            || self.objects.get(&output).map(String::as_str) != Some("wl_output")
        {
            return Err("Invalid xdg-output object");
        }
        let version = self.object_versions.get(&object).copied().unwrap_or(1);
        self.add_object(id, String::from("zxdg_output_v1"));
        self.object_versions.insert(id, version);
        self.xdg_outputs.insert(id, output);
        let mut messages = self.xdg_output_geometry(id, true);
        if version >= 3 && self.object_versions.get(&output).copied().unwrap_or(1) >= 2 {
            messages.push(WaylandMessage::new(output, protocol::output_event::DONE));
        }
        Ok(messages)
    }
}
