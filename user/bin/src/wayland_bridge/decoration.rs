//! Server decorations are a ScarletUI Window around the client's SWS scene.
//! Client pixels stay in their original SHM/GPU resources. Only chrome is
//! rasterized here, and its published backing is immutable until retirement.
use super::decoration_state::{Mode, State};
use super::*;
use scarlet_ui_core::color::Color;
use scarlet_ui_core::event::{Event, MouseButton, MouseEvent, WindowEvent};
use scarlet_ui_core::geometry::{Rect, Size};
use scarlet_ui_core::pipeline::RenderingPipeline;
use scarlet_ui_core::renderer::{CpuPaintRenderer, PaintContext};
use scarlet_ui_core::view::View;
use scarlet_ui_core::views::{Spacer, Window, WindowContentLayout};

pub(super) struct Decoration {
    pub state: State,
    frame: Option<Frame>,
}

struct Frame {
    pipeline: RenderingPipeline,
    title: String,
    width: u32,
    height: u32,
    scale: u32,
    resource: Option<ChromeResource>,
    chrome_margin: u32,
    mask: Vec<(u32, u32, u32, u32)>, // x, y, width, height from ScarletUI alpha
    grabbed_buttons: BTreeSet<u16>,
}

struct ChromeResource {
    pool: u32,
    parts: Vec<(u32, u32, u32, u32, u32)>, // buffer, x, y, width, height
}

impl Drop for Frame {
    fn drop(&mut self) {
        self.pipeline.teardown();
    }
}

impl Frame {
    fn new(title: String, width: u32, height: u32, scale: u32) -> Self {
        let window = Window::new(title.clone(), Spacer::new())
            .size(Size::new(
                width as f32 / scale as f32,
                height as f32 / scale as f32,
            ))
            .shadow(false)
            .background_color(Color::TRANSPARENT)
            .opaque(false);
        let mut pipeline = RenderingPipeline::new();
        pipeline.set_scale_milli(scale * 1000);
        pipeline.set_root(window.create_element());
        let info = pipeline.layout_initial();
        // Rasterize ScarletUI's own window clip using its resolved radius.
        // This mask clips the external image, while the transparent Window
        // paints titlebar/border above it just like Window::paint_overlay.
        let mut paint = PaintContext::new();
        paint.fill_rounded_rect(
            Rect::from_xywh(0.0, 0.0, info.size.width, info.size.height),
            info.effective_corner_radius(),
            Color::WHITE,
        );
        let mut mask_renderer = CpuPaintRenderer::new(info.size, scale * 1000, Color::TRANSPARENT);
        mask_renderer.execute(&paint);
        let mask = WaylandBridge::chrome_mask(mask_renderer.buffer().data(), width, height);
        let chrome_margin = (info.effective_corner_radius() * scale as f32) as u32 + scale;
        Self {
            pipeline,
            title,
            width,
            height,
            scale,
            resource: None,
            chrome_margin,
            mask,
            grabbed_buttons: BTreeSet::new(),
        }
    }
}

impl WaylandBridge {
    pub(super) fn sync_toplevel_metadata(&mut self, root: u32) -> Result<(), &'static str> {
        let Some(window) = self.surface_to_window.get(&root).copied() else {
            return Ok(());
        };
        let Some((title, min, max)) = self
            .xdg_shell_manager
            .get_xdg_surface_ids_by_wl_surface(root)
            .and_then(|(surface, _)| self.xdg_shell_manager.get_xdg_surface(surface))
            .and_then(|surface| surface.toplevel.as_ref())
            .map(|top| {
                (
                    top.title.clone().unwrap_or_default(),
                    top.min_size,
                    top.max_size,
                )
            })
        else {
            return Ok(());
        };
        let payload = protocol_sws::payload_set_window_title(window, title.as_bytes());
        self.send_sws_async_message(protocol_sws::client_msg::SET_WINDOW_TITLE, &payload)?;
        let scale = self.output_scale.max(1) as u32;
        let (_, _, extra_w, extra_h) = self.decoration_insets(root);
        let dimension = |logical: i32, extra: u32, unbounded: bool| {
            if unbounded && logical <= 0 {
                0
            } else {
                (logical.max(1) as u32)
                    .saturating_mul(scale)
                    .saturating_add(extra)
            }
        };
        let min = min.unwrap_or((0, 0));
        let max = max.unwrap_or((0, 0));
        let payload = protocol_sws::payload_set_window_size_limits(
            window,
            dimension(min.0, extra_w, false),
            dimension(min.1, extra_h, false),
            dimension(max.0, extra_w, true),
            dimension(max.1, extra_h, true),
        );
        self.send_sws_async_message(protocol_sws::client_msg::SET_WINDOW_SIZE_LIMITS, &payload)
    }

    fn decoration_surface(&self, object: u32) -> Option<u32> {
        self.decorations.iter().find_map(|(&root, decoration)| {
            (decoration.state.object == Some(object)).then_some(root)
        })
    }

    fn configure_decoration(
        &mut self,
        root: u32,
        mode: Mode,
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        let (surface, toplevel) = self
            .xdg_shell_manager
            .get_xdg_surface_ids_by_wl_surface(root)
            .ok_or("Decoration toplevel was destroyed")?;
        let toplevel = toplevel.ok_or("Decoration toplevel was destroyed")?;
        let serial = self.allocate_serial();
        let decoration = self
            .decorations
            .get_mut(&root)
            .ok_or("Unknown decoration")?;
        let object = decoration.state.object.ok_or("Unknown decoration")?;
        decoration.state.configure(mode, serial);
        let xdg = self.xdg_shell_manager.get_xdg_surface_mut(surface).unwrap();
        xdg.last_configure_serial = Some(serial);
        let top = xdg.toplevel.as_ref().unwrap();
        let states = Self::xdg_toplevel_state_bytes(top.maximized, top.fullscreen);
        let mut mode_event = WaylandMessage::new(object, 0);
        mode_event.add_arg(WaylandArg::Uint(mode as u32));
        // Zero lets the client retain its size; decorations aren't client area.
        let mut top_event = WaylandMessage::new(toplevel, xdg_shell::xdg_toplevel_event::CONFIGURE);
        top_event.add_arg(WaylandArg::Int(0));
        top_event.add_arg(WaylandArg::Int(0));
        top_event.add_arg(WaylandArg::Array(states));
        let mut configure = WaylandMessage::new(surface, xdg_shell::xdg_surface_event::CONFIGURE);
        configure.add_arg(WaylandArg::Uint(serial));
        bridge_info!(
            "[wayland-bridge] client={} decoration={} surface={} mode={:?} configure={}",
            self.client_id,
            object,
            root,
            mode,
            serial
        );
        Ok(Vec::from([mode_event, top_event, configure]))
    }

    pub(super) fn handle_decoration_message(
        &mut self,
        object: u32,
        interface: &str,
        opcode: u16,
        payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        if interface == "zxdg_decoration_manager_v1" {
            match opcode {
                0 if payload.is_empty() => return Ok(self.delete_decoration_object(object)),
                1 if payload.len() == 8 => {
                    let id = Self::parse_u32(payload, 0).unwrap();
                    let toplevel = Self::parse_u32(payload, 4).unwrap();
                    if id == 0 || self.objects.contains_key(&id) {
                        return Err("Invalid decoration new ID");
                    }
                    let root = self
                        .xdg_shell_manager
                        .get_toplevel_mut(toplevel)
                        .map(|(_, root)| root)
                        .ok_or("Decoration requires a live toplevel")?;
                    if self
                        .decorations
                        .get(&root)
                        .is_some_and(|d| d.state.object.is_some())
                    {
                        return Err("Toplevel already has a decoration");
                    }
                    if self
                        .surface_manager
                        .get_surface(root)
                        .is_some_and(|s| s.buffer_id.is_some())
                        || self.surface_to_window.contains_key(&root)
                    {
                        return Err("Decoration constructed after mapping");
                    }
                    self.decorations.insert(
                        root,
                        Decoration {
                            state: State::new(id),
                            frame: None,
                        },
                    );
                    self.add_object(id, String::from("zxdg_toplevel_decoration_v1"));
                    self.scene.enable(root);
                    return self.configure_decoration(root, Mode::Server);
                }
                _ => return Err("Invalid decoration manager request"),
            }
        }
        let root = self
            .decoration_surface(object)
            .ok_or("Unknown decoration")?;
        match opcode {
            0 if payload.is_empty() => {
                self.decorations.get_mut(&root).unwrap().state.destroy();
                Ok(self.delete_decoration_object(object))
            }
            1 if payload.len() == 4 => {
                let mode = Mode::parse(Self::parse_u32(payload, 0).unwrap())
                    .ok_or("Invalid decoration mode")?;
                self.configure_decoration(root, mode)
            }
            2 if payload.is_empty() => self.configure_decoration(root, Mode::Server),
            _ => Err("Invalid decoration request"),
        }
    }

    fn delete_decoration_object(&mut self, object: u32) -> Vec<WaylandMessage> {
        self.remove_object(object);
        let mut deleted = WaylandMessage::new(1, protocol::display_event::DELETE_ID);
        deleted.add_arg(WaylandArg::Uint(object));
        Vec::from([deleted])
    }

    pub(super) fn decoration_ack(&mut self, xdg_surface: u32, serial: u32) {
        if let Some(xdg) = self.xdg_shell_manager.get_xdg_surface(xdg_surface)
            && let Some(decoration) = self.decorations.get_mut(&xdg.wl_surface_id)
        {
            decoration.state.ack(serial);
        }
    }

    pub(super) fn decoration_commit(&mut self, root: u32) -> Result<(), &'static str> {
        let changed = if let Some(decoration) = self.decorations.get_mut(&root) {
            let previous = decoration.state.active;
            decoration.state.commit();
            previous != decoration.state.active
        } else {
            false
        };
        if changed {
            self.sync_toplevel_metadata(root)?;
        }
        Ok(())
    }

    pub(super) fn decoration_visible(&self, root: u32) -> bool {
        self.decorations
            .get(&root)
            .is_some_and(|d| d.state.active == Mode::Server)
            && self
                .xdg_shell_manager
                .get_xdg_surface_ids_by_wl_surface(root)
                .and_then(|(surface, _)| self.xdg_shell_manager.get_xdg_surface(surface))
                .and_then(|surface| surface.toplevel.as_ref())
                .is_some_and(|top| !top.fullscreen)
    }

    /// Insets are physical pixels. Keep them from the same ScarletUI metrics
    /// used to paint the frame, rather than duplicating titlebar constants.
    pub(super) fn decoration_insets(&self, root: u32) -> (i32, i32, u32, u32) {
        if !self.decoration_visible(root) {
            return (0, 0, 0, 0);
        }
        let layout = WindowContentLayout::new(true);
        let offset = layout.offset();
        let size = layout.decoration_size();
        let scale = self.output_scale.max(1) as f32;
        (
            (offset.x * scale) as i32,
            (offset.y * scale) as i32,
            (size.width * scale) as u32,
            (size.height * scale) as u32,
        )
    }

    pub(super) fn scene_pointer_position(&self, root: u32, x: i32, y: i32) -> (i32, i32) {
        let (dx, dy, _, _) = self.decoration_insets(root);
        (x.saturating_sub(dx), y.saturating_sub(dy))
    }

    fn retire_decoration_resource(
        &mut self,
        resource: Option<ChromeResource>,
    ) -> Result<(), &'static str> {
        if let Some(resource) = resource {
            // SWS defers these destructions while a published scene retains
            // the buffer. No backing is overwritten while the GPU samples it.
            for part in resource.parts {
                self.destroy_extension_buffer(part.0)?;
            }
            self.destroy_extension_shm_pool(resource.pool)?;
        }
        Ok(())
    }

    pub(super) fn remove_decoration(&mut self, root: u32) -> Result<(), &'static str> {
        let resource = self
            .decorations
            .remove(&root)
            .and_then(|mut d| d.frame.take().and_then(|mut f| f.resource.take()));
        self.retire_decoration_resource(resource)
    }

    pub(super) fn decorate_scene(
        &mut self,
        root: u32,
        scene: &mut protocol_sws::surface_scene::Commit,
    ) -> Result<(), &'static str> {
        if scene.layers.is_empty() || !self.decoration_visible(root) {
            let resource = self
                .decorations
                .get_mut(&root)
                .and_then(|d| d.frame.take().and_then(|mut f| f.resource.take()));
            return self.retire_decoration_resource(resource);
        }
        // SSD is a toplevel scene. CSD subsurface trees remain untouched.
        let (dx, dy, extra_w, extra_h) = self.decoration_insets(root);
        let width = scene
            .width
            .checked_add(extra_w)
            .ok_or("Decoration width overflow")?;
        let height = scene
            .height
            .checked_add(extra_h)
            .ok_or("Decoration height overflow")?;
        let scale = self.output_scale.max(1) as u32;
        if u64::from(width) * u64::from(height) > 16_000_000 {
            return Err("Decoration backing exceeds pixel limit");
        }
        let title = self
            .xdg_shell_manager
            .get_xdg_surface_ids_by_wl_surface(root)
            .and_then(|(surface, _)| self.xdg_shell_manager.get_xdg_surface(surface))
            .and_then(|surface| surface.toplevel.as_ref())
            .and_then(|top| top.title.clone())
            .unwrap_or_default();
        let rebuild = self.decorations[&root].frame.as_ref().is_none_or(|f| {
            f.width != width || f.height != height || f.scale != scale || f.title != title
        });
        if rebuild {
            let old = self
                .decorations
                .get_mut(&root)
                .unwrap()
                .frame
                .take()
                .and_then(|mut f| f.resource.take());
            self.retire_decoration_resource(old)?;
            self.decorations.get_mut(&root).unwrap().frame =
                Some(Frame::new(title, width, height, scale));
        }
        let frame = self
            .decorations
            .get_mut(&root)
            .unwrap()
            .frame
            .as_mut()
            .unwrap();
        let repaint = frame.resource.is_none() || frame.pipeline.has_dirty();
        if repaint {
            let pixels = frame
                .pipeline
                .render()
                .ok_or("ScarletUI decoration render failed")?
                .data()
                .to_vec();
            let old = frame.resource.take();
            let mut packed = Vec::new();
            let mut parts = Vec::new();
            let top = dy as u32;
            let margin = self.decorations[&root]
                .frame
                .as_ref()
                .unwrap()
                .chrome_margin;
            let left = margin.max(dx as u32).min(width / 2);
            let bottom = margin.max(extra_h - top).min(height - top);
            let right = margin.max(extra_w - dx as u32).min(width - left);
            let rectangles = [
                (0, 0, width, top),
                (0, top, left, height - top - bottom),
                (width - right, top, right, height - top - bottom),
                (0, height - bottom, width, bottom),
            ];
            for (x, y, w, h) in rectangles {
                if w == 0 || h == 0 {
                    continue;
                }
                let offset = packed.len() as u64;
                for row in y..y + h {
                    let start = ((row * width + x) * 4) as usize;
                    packed.extend_from_slice(&pixels[start..start + w as usize * 4]);
                }
                parts.push((self.allocate_extension_resource_id(), x, y, w, h, offset));
            }
            let size = packed.len();
            let shm = SharedMemory::create(size, permissions::READ_WRITE)
                .map_err(|_| "Decoration SHM allocation failed")?;
            let mapper = shm
                .as_handle()
                .as_memory_mapping()
                .map_err(|_| "Decoration SHM mapping unsupported")?;
            // SAFETY: Initialize private chrome strips before transferring
            // the backing; keep published buffers immutable until retired.
            let address = unsafe {
                mapper.mmap(
                    0,
                    size,
                    permissions::READ_WRITE,
                    std::handle::capability::memory_mapping::flags::SHARED,
                    0,
                )
            }
            .map_err(|_| "Decoration SHM mmap failed")?;
            unsafe {
                core::ptr::copy_nonoverlapping(packed.as_ptr(), address as *mut u8, size);
            }
            unsafe { std::handle::capability::memory_mapping::munmap(address, size) }
                .map_err(|_| "Decoration SHM munmap failed")?;
            let pool = self.allocate_extension_resource_id();
            self.register_extension_shm_pool(pool, size, shm.as_handle())?;
            let mut resources = Vec::new();
            for (buffer, x, y, w, h, offset) in parts {
                let payload = protocol_sws::payload_extension_define_buffer(
                    buffer,
                    pool,
                    offset,
                    w,
                    h,
                    w * 4,
                    shm::shm_format::ARGB8888,
                );
                self.send_sws_async_message(
                    protocol_sws::client_msg::EXTENSION_DEFINE_BUFFER,
                    &payload,
                )?;
                resources.push((buffer, x, y, w, h));
            }
            let frame = self
                .decorations
                .get_mut(&root)
                .unwrap()
                .frame
                .as_mut()
                .unwrap();
            frame.resource = Some(ChromeResource {
                pool,
                parts: resources,
            });
            bridge_info!(
                "[wayland-bridge] client={} surface={} ScarletUI chrome bytes={} full_bytes={} mask_bands={}",
                self.client_id,
                root,
                size,
                pixels.len(),
                frame.mask.len()
            );
            self.retire_decoration_resource(old)?;
        }
        for layer in &mut scene.layers {
            layer.x = layer
                .x
                .checked_add(dx)
                .ok_or("Decoration position overflow")?;
            layer.y = layer
                .y
                .checked_add(dy)
                .ok_or("Decoration position overflow")?;
        }
        let frame = self.decorations[&root].frame.as_ref().unwrap();
        let mut used: BTreeSet<u32> = scene.layers.iter().map(|layer| layer.surface_id).collect();
        let mut internal_id = u32::MAX;
        let mut chrome_layers = Vec::new();
        for &(buffer, x, y, w, h) in &frame.resource.as_ref().unwrap().parts {
            let id = Self::chrome_layer_id(&mut used, &mut internal_id);
            chrome_layers.push(protocol_sws::surface_scene::Layer {
                surface_id: id,
                buffer_id: buffer,
                x: x as i32,
                y: y as i32,
                width: w,
                height: h,
                source_x: 0,
                source_y: 0,
                source_width: (w * 256) as i32,
                source_height: (h * 256) as i32,
                transform: 0,
            });
        }
        // Use the actual ScarletUI alpha outline to clip imported client
        // images. No guessed radius, client readback, or custom frame drawing.
        let mut layers = Vec::new();
        for layer in &scene.layers {
            let mut first = true;
            for &(x, y, w, h) in &frame.mask {
                let x0 = (layer.x as i64).max(x as i64);
                let y0 = (layer.y as i64).max(y as i64);
                let x1 = (layer.x as i64 + layer.width as i64).min((x + w) as i64);
                let y1 = (layer.y as i64 + layer.height as i64).min((y + h) as i64);
                if x0 >= x1 || y0 >= y1 {
                    continue;
                }
                let mut cropped = *layer;
                let sx0 = layer.source_width as i64 * (x0 - layer.x as i64) / layer.width as i64;
                let sy0 = layer.source_height as i64 * (y0 - layer.y as i64) / layer.height as i64;
                let sx1 = layer.source_width as i64 * (x1 - layer.x as i64) / layer.width as i64;
                let sy1 = layer.source_height as i64 * (y1 - layer.y as i64) / layer.height as i64;
                cropped.x = x0 as i32;
                cropped.y = y0 as i32;
                cropped.width = (x1 - x0) as u32;
                cropped.height = (y1 - y0) as u32;
                cropped.source_x += sx0 as i32;
                cropped.source_y += sy0 as i32;
                cropped.source_width = (sx1 - sx0) as i32;
                cropped.source_height = (sy1 - sy0) as i32;
                if !first {
                    cropped.surface_id = Self::chrome_layer_id(&mut used, &mut internal_id);
                }
                first = false;
                layers.push(cropped);
            }
        }
        layers.extend(chrome_layers);
        scene.layers = layers;
        scene.width = width;
        scene.height = height;
        scene.validate().map_err(|_| "Invalid decorated scene")?;
        Ok(())
    }

    fn chrome_layer_id(used: &mut BTreeSet<u32>, next: &mut u32) -> u32 {
        while used.contains(next) {
            *next -= 1;
        }
        let id = *next;
        used.insert(id);
        *next -= 1;
        id
    }

    fn chrome_mask(pixels: &[u8], width: u32, height: u32) -> Vec<(u32, u32, u32, u32)> {
        let mut bands: Vec<(u32, u32, u32, u32)> = Vec::new();
        for y in 0..height {
            let row =
                &pixels[y as usize * width as usize * 4..(y + 1) as usize * width as usize * 4];
            let left = (0..width).find(|x| row[*x as usize * 4 + 3] >= 128);
            let right = (0..width).rev().find(|x| row[*x as usize * 4 + 3] >= 128);
            let (Some(left), Some(right)) = (left, right) else {
                continue;
            };
            let span = right - left + 1;
            if let Some(last) = bands.last_mut()
                && last.0 == left
                && last.2 == span
                && last.1 + last.3 == y
            {
                last.3 += 1;
            } else {
                bands.push((left, y, span, 1));
            }
        }
        bands
    }

    pub(super) fn decoration_mouse(&mut self, root: u32, event: MouseEvent) {
        let Some(frame) = self
            .decorations
            .get_mut(&root)
            .and_then(|d| d.frame.as_mut())
        else {
            return;
        };
        frame.pipeline.handle_event(&Event::Mouse(event));
        for event in frame.pipeline.take_emitted_events() {
            if let Event::Window(action) = event {
                self.decoration_actions.push((root, action));
            }
        }
        if frame.pipeline.has_dirty() {
            self.decoration_updates.insert(root);
        }
    }

    pub(super) fn decoration_motion(&mut self) {
        if let Some(root) = self.focused_surface
            && self.decoration_visible(root)
        {
            let scale = self.output_scale.max(1);
            self.decoration_mouse(
                root,
                MouseEvent::Moved {
                    x: self.pointer_x / scale,
                    y: self.pointer_y / scale,
                },
            );
        }
    }

    pub(super) fn decoration_button(&mut self, code: u16, value: i32) -> bool {
        let Some(root) = self.focused_surface else {
            return false;
        };
        if !self.decoration_visible(root) {
            return false;
        }
        let Some(frame) = self
            .decorations
            .get_mut(&root)
            .and_then(|d| d.frame.as_mut())
        else {
            return false;
        };
        let grabbed = frame.grabbed_buttons.contains(&code);
        let captured = if value == 0 {
            frame.grabbed_buttons.remove(&code);
            grabbed
        } else {
            let layout = WindowContentLayout::new(true);
            let scale = frame.scale as i32;
            let x = self.pointer_x / scale;
            let y = self.pointer_y / scale;
            let offset = layout.offset();
            let extra = layout.decoration_size();
            let in_content = x >= offset.x as i32
                && y >= offset.y as i32
                && x < frame.width as i32 / scale - (extra.width - offset.x) as i32
                && y < frame.height as i32 / scale - (extra.height - offset.y) as i32;
            // A client press retains its implicit grab across frame boundaries.
            let capture = grabbed || (!in_content && self.pointer_buttons.is_empty());
            if capture {
                frame.grabbed_buttons.insert(code);
            }
            capture
        };
        if captured && code == 0x110 {
            let scale = self.output_scale.max(1);
            let x = self.pointer_x / scale;
            let y = self.pointer_y / scale;
            let event = if value == 0 {
                MouseEvent::ButtonReleased {
                    button: MouseButton::Left,
                    x,
                    y,
                    click_count: 1,
                }
            } else {
                MouseEvent::ButtonPressed {
                    button: MouseButton::Left,
                    x,
                    y,
                    click_count: 1,
                }
            };
            self.decoration_mouse(root, event);
        }
        captured
    }

    pub(super) fn process_decoration_updates(&mut self) -> Result<(), &'static str> {
        for (root, action) in core::mem::take(&mut self.decoration_actions) {
            let Some(window) = self.surface_to_window.get(&root).copied() else {
                continue;
            };
            bridge_info!(
                "[wayland-bridge] client={} surface={} decoration action={:?}",
                self.client_id,
                root,
                action
            );
            match action {
                WindowEvent::CloseRequested => {
                    if let Some((_, Some(top))) = self
                        .xdg_shell_manager
                        .get_xdg_surface_ids_by_wl_surface(root)
                    {
                        self.queue_input_messages(Vec::from([WaylandMessage::new(
                            top,
                            xdg_shell::xdg_toplevel_event::CLOSE,
                        )]));
                    }
                }
                WindowEvent::MoveRequested => self.send_request_move_window(window)?,
                WindowEvent::MaximizeRequested | WindowEvent::RestoreRequested => {
                    // Rebuilding the UI after a resize resets its local toggle.
                    // SWS/xdg state remains authoritative across frame rebuilds.
                    let maximized = self
                        .xdg_shell_manager
                        .get_xdg_surface_ids_by_wl_surface(root)
                        .and_then(|(surface, _)| self.xdg_shell_manager.get_xdg_surface(surface))
                        .and_then(|surface| surface.toplevel.as_ref())
                        .is_some_and(|top| top.maximized);
                    if maximized {
                        self.send_restore_window(window)?;
                    } else {
                        self.send_maximize_window(window)?;
                    }
                }
                WindowEvent::MinimizeRequested => self.send_minimize_window(window)?,
            }
        }
        for root in core::mem::take(&mut self.decoration_updates) {
            if self.surface_to_window.contains_key(&root) {
                self.publish_scene(root)?;
            }
        }
        Ok(())
    }
}
