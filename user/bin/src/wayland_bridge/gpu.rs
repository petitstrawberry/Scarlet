//! Shared SGFX capabilities use ordinary wl_buffer attach/commit/release state.
//! This is a private backing protocol, not Linux dma-buf emulation.
use super::*;

impl WaylandBridge {
    pub(super) fn buffer_view(&self, id: u32) -> Option<scene::Buffer> {
        self.gpu_buffers.get(&id).copied().or_else(|| {
            self.shm_manager.get_buffer(id).map(|b| scene::Buffer {
                id: b.sws_buffer_id,
                width: b.width as u32,
                height: b.height as u32,
            })
        })
    }

    pub(super) fn wayland_buffer_for_resource(&self, resource: u32) -> Option<u32> {
        self.gpu_buffers
            .iter()
            .find_map(|(&id, b)| (b.id == resource).then_some(id))
            .or_else(|| {
                self.shm_manager
                    .get_buffer_by_sws_id(resource)
                    .map(|b| b.buffer_id)
            })
    }

    pub(super) fn handle_gpu_message(
        &mut self,
        object: u32,
        opcode: u16,
        payload: &[u8],
        handle: Option<Handle>,
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        match opcode {
            0 if payload.is_empty() && handle.is_none() => {
                self.remove_object(object);
                let mut message = WaylandMessage::new(1, protocol::display_event::DELETE_ID);
                message.add_arg(WaylandArg::Uint(object));
                Ok(Vec::from([message]))
            }
            1 if payload.len() == 12 => {
                let id = Self::parse_u32(payload, 0).unwrap();
                let width = Self::parse_u32(payload, 4).unwrap();
                let height = Self::parse_u32(payload, 8).unwrap();
                if id == 0
                    || self.objects.contains_key(&id)
                    || width == 0
                    || height == 0
                    || width > 16384
                    || height > 16384
                    || self.compositor_epoch == 0
                {
                    return Err("Invalid SGFX Wayland buffer");
                }
                let handle = handle.ok_or("Missing SGFX image capability")?;
                let resource = self.allocate_extension_resource_id();
                let payload = protocol_sws::payload_extension_define_gpu_buffer(
                    resource,
                    self.compositor_epoch,
                    width,
                    height,
                );
                // SWS validates image type, BGRA8 format and actual extent on import.
                let request = self.send_sws_handle_request(
                    protocol_sws::client_msg::EXTENSION_DEFINE_GPU_BUFFER,
                    &payload,
                    &handle,
                )?;
                let response = self.wait_for_sws_message(request, |message| {
                    matches!(
                        message,
                        protocol_sws::ServerMessage::ExtensionGpuBufferDefined { .. }
                    )
                })?;
                if !matches!(response, protocol_sws::ServerMessage::ExtensionGpuBufferDefined { buffer_id } if buffer_id == resource)
                {
                    return Err("Unexpected GPU buffer registration response");
                }
                self.gpu_buffers.insert(
                    id,
                    scene::Buffer {
                        id: resource,
                        width,
                        height,
                    },
                );
                self.add_object(id, String::from("wl_buffer"));
                Ok(Vec::new())
            }
            _ => Err("Invalid SGFX Wayland request"),
        }
    }
}
