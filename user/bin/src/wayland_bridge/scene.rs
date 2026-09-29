//! Wayland transaction state only. Pixels, crop/scale rendering and retained
//! buffer ownership belong to SWS. No client SHM is mapped by this module.
use std::collections::BTreeMap;
use std::vec::Vec;
use sws_protocol::surface_scene::{Commit, Layer, MAX_LAYERS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Buffer {
    pub id: u32,
    pub width: u32,
    pub height: u32,
}
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub source: Option<(i32, i32, i32, i32)>,
    pub destination: Option<(u32, u32)>,
    pub scale: i32,
    pub transform: u32,
}
impl Default for View {
    fn default() -> Self {
        Self {
            source: None,
            destination: None,
            scale: 1,
            transform: 0,
        }
    }
}
impl View {
    fn layer(
        self,
        id: u32,
        buffer: Buffer,
        x: i32,
        y: i32,
        output_scale: u32,
    ) -> Result<Layer, &'static str> {
        let (w, h) = if self.transform & 1 != 0 {
            (buffer.height, buffer.width)
        } else {
            (buffer.width, buffer.height)
        };
        if self.scale <= 0 || self.transform > 7 {
            return Err("Invalid surface scale or transform");
        }
        let scale = self.scale as i64;
        let (sx, sy, sw, sh) = self
            .source
            .map(|(x, y, w, h)| (x as i64, y as i64, w as i64, h as i64))
            .unwrap_or((0, 0, w as i64 * 256 / scale, h as i64 * 256 / scale));
        if sx < 0
            || sy < 0
            || sw <= 0
            || sh <= 0
            || (sx + sw) * scale > w as i64 * 256
            || (sy + sh) * scale > h as i64 * 256
        {
            return Err("Viewport source outside buffer");
        }
        let (dw, dh) = match self.destination {
            Some(v) => v,
            None => {
                if sw % 256 != 0 || sh % 256 != 0 {
                    return Err("Viewport destination must be integral");
                }
                ((sw / 256) as u32, (sh / 256) as u32)
            }
        };
        let mut l = Layer {
            surface_id: id,
            buffer_id: buffer.id,
            x,
            y,
            width: dw
                .checked_mul(output_scale)
                .ok_or("Surface width overflow")?,
            height: dh
                .checked_mul(output_scale)
                .ok_or("Surface height overflow")?,
            source_x: 0,
            source_y: 0,
            source_width: 0,
            source_height: 0,
            transform: self.transform,
        };
        l.source_x = i32::try_from(sx * scale).map_err(|_| "Crop overflow")?;
        l.source_y = i32::try_from(sy * scale).map_err(|_| "Crop overflow")?;
        l.source_width = i32::try_from(sw * scale).map_err(|_| "Crop overflow")?;
        l.source_height = i32::try_from(sh * scale).map_err(|_| "Crop overflow")?;
        if !l.fits_buffer(buffer.width, buffer.height) {
            return Err("Invalid viewport");
        }
        Ok(l)
    }
}
#[derive(Clone, Default)]
struct State {
    buffer: Option<Buffer>,
    view: View,
    children: Vec<(u32, i32, i32)>,
    input: Option<super::region::Region>,
}
struct Node {
    parent: Option<u32>,
    sync: bool,
    enabled: bool,
    position: (i32, i32),
    order: Vec<u32>,
    view: View,
    input: Option<super::region::Region>,
    current: State,
    cached: Option<State>,
    callbacks: Vec<u32>,
    cached_callbacks: Vec<u32>,
}
impl Node {
    fn new(id: u32) -> Self {
        Self {
            parent: None,
            sync: true,
            enabled: false,
            position: (0, 0),
            order: std::vec![id],
            view: View::default(),
            input: None,
            current: State::default(),
            cached: None,
            callbacks: Vec::new(),
            cached_callbacks: Vec::new(),
        }
    }
}
pub struct Scene {
    nodes: BTreeMap<u32, Node>,
    output_scale: i32,
}
impl Scene {
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            output_scale: 1,
        }
    }
    pub fn set_output_scale(&mut self, scale: i32) {
        self.output_scale = scale.max(1);
    }
    pub fn create(&mut self, id: u32) {
        self.nodes.insert(id, Node::new(id));
    }
    pub fn root(&self, mut id: u32) -> u32 {
        while let Some(p) = self.nodes.get(&id).and_then(|n| n.parent) {
            id = p;
        }
        id
    }
    pub fn enabled(&self, id: u32) -> bool {
        self.nodes.get(&self.root(id)).is_some_and(|n| n.enabled)
    }
    pub fn enable(&mut self, id: u32) {
        let root = self.root(id);
        if let Some(n) = self.nodes.get_mut(&root) {
            n.enabled = true;
        }
    }
    pub fn view_mut(&mut self, id: u32) -> Result<&mut View, &'static str> {
        Ok(&mut self
            .nodes
            .get_mut(&id)
            .ok_or("Viewport has no surface")?
            .view)
    }
    pub fn input(&mut self, id: u32, input: Option<super::region::Region>) {
        if let Some(n) = self.nodes.get_mut(&id) {
            n.input = input;
        }
    }
    pub fn attach_child(&mut self, id: u32, parent: u32) -> Result<(), &'static str> {
        if id == parent
            || !self.nodes.contains_key(&id)
            || !self.nodes.contains_key(&parent)
            || self.nodes[&id].parent.is_some()
        {
            return Err("Invalid subsurface");
        }
        let child_root = self.root(id);
        let parent_root = self.root(parent);
        let count = self
            .nodes
            .keys()
            .filter(|candidate| {
                let root = self.root(**candidate);
                root == child_root || root == parent_root
            })
            .count();
        if count > MAX_LAYERS {
            return Err("Surface tree too large");
        }
        let mut p = parent;
        let mut depth = 0;
        loop {
            if p == id {
                return Err("Subsurface cycle");
            }
            depth += 1;
            if depth >= MAX_LAYERS {
                return Err("Surface tree too deep");
            }
            match self.nodes[&p].parent {
                Some(v) => p = v,
                None => break,
            }
        }
        let child = self.nodes.get_mut(&id).unwrap();
        child.parent = Some(parent);
        child.sync = true;
        child.position = (0, 0);
        self.nodes.get_mut(&parent).unwrap().order.push(id);
        self.enable(parent);
        Ok(())
    }
    pub fn position(&mut self, id: u32, x: i32, y: i32) -> Result<(), &'static str> {
        self.nodes.get_mut(&id).ok_or("Dead subsurface")?.position = (x, y);
        Ok(())
    }
    pub fn restack(&mut self, id: u32, sibling: u32, above: bool) -> Result<(), &'static str> {
        let parent = self
            .nodes
            .get(&id)
            .and_then(|n| n.parent)
            .ok_or("Dead subsurface")?;
        let n = self.nodes.get_mut(&parent).ok_or("Dead parent")?;
        if id == sibling || !n.order.contains(&sibling) {
            return Err("Invalid subsurface sibling");
        }
        n.order.retain(|v| *v != id);
        let i = n.order.iter().position(|v| *v == sibling).unwrap();
        n.order.insert(i + usize::from(above), id);
        Ok(())
    }
    pub fn synchronized(&self, mut id: u32) -> bool {
        while let Some(n) = self.nodes.get(&id) {
            let Some(p) = n.parent else {
                return false;
            };
            if n.sync {
                return true;
            }
            id = p;
        }
        false
    }
    pub fn set_sync(&mut self, id: u32, sync: bool) -> Result<Option<u32>, &'static str> {
        let was_synchronized = self.synchronized(id);
        self.nodes.get_mut(&id).ok_or("Dead subsurface")?.sync = sync;
        if was_synchronized && !self.synchronized(id) {
            self.apply(id, true)?;
            return Ok(Some(self.root(id)));
        }
        Ok(None)
    }
    pub fn commit(
        &mut self,
        id: u32,
        buffer: Option<Option<Buffer>>,
        callbacks: Vec<u32>,
    ) -> Result<Option<u32>, &'static str> {
        let sync = self.synchronized(id);
        let n = self.nodes.get(&id).ok_or("Unknown surface")?;
        let children = n
            .order
            .iter()
            .filter_map(|v| {
                self.nodes
                    .get(v)
                    .map(|child| (*v, child.position.0, child.position.1))
            })
            .collect();
        let n = self.nodes.get_mut(&id).unwrap();
        let mut state = n.cached.as_ref().unwrap_or(&n.current).clone();
        if let Some(buffer) = buffer {
            state.buffer = buffer;
        }
        state.view = n.view;
        state.children = children;
        state.input = n.input.clone();
        n.cached = Some(state);
        n.cached_callbacks.extend(callbacks);
        if sync {
            return Ok(None);
        }
        self.apply(id, false)?;
        Ok(Some(self.root(id)))
    }
    fn apply(&mut self, id: u32, inherited: bool) -> Result<(), &'static str> {
        let Some(n) = self.nodes.get_mut(&id) else {
            return Ok(());
        };
        if let Some(state) = n.cached.take() {
            if let Some(buffer) = state.buffer {
                state.view.layer(id, buffer, 0, 0, 1)?;
            }
            n.current = state;
        }
        n.callbacks.append(&mut n.cached_callbacks);
        let children = n.current.children.clone();
        for (child, _, _) in children {
            if child != id && self.nodes.get(&child).is_some_and(|n| inherited || n.sync) {
                self.apply(child, true)?;
            }
        }
        Ok(())
    }
    pub fn detach(&mut self, id: u32) -> Option<u32> {
        let parent = self.nodes.get(&id)?.parent?;
        let root = self.root(parent);
        if let Some(n) = self.nodes.get_mut(&parent) {
            n.order.retain(|v| *v != id);
            n.current.children.retain(|v| v.0 != id);
            if let Some(c) = n.cached.as_mut() {
                c.children.retain(|v| v.0 != id);
            }
        }
        let n = self.nodes.get_mut(&id)?;
        n.parent = None;
        n.current.buffer = None;
        n.cached = None;
        Some(root)
    }
    pub fn destroy(&mut self, id: u32) -> (Option<u32>, Vec<u32>) {
        let root = self.detach(id);
        let mut callbacks = Vec::new();
        if let Some(mut n) = self.nodes.remove(&id) {
            callbacks.append(&mut n.callbacks);
            callbacks.append(&mut n.cached_callbacks);
        }
        for n in self.nodes.values_mut() {
            if n.parent == Some(id) {
                n.parent = None;
                n.current.buffer = None;
                n.cached = None;
            }
        }
        (root, callbacks)
    }
    pub fn take_callbacks(&mut self, root: u32) -> Vec<u32> {
        let ids: Vec<_> = self
            .nodes
            .keys()
            .copied()
            .filter(|id| self.root(*id) == root)
            .collect();
        let mut out = Vec::new();
        for id in ids {
            out.append(&mut self.nodes.get_mut(&id).unwrap().callbacks);
        }
        out
    }
    pub fn scale(&self, id: u32) -> u32 {
        self.nodes
            .get(&id)
            .map(|n| n.current.view.scale.max(1) as u32)
            .unwrap_or(1)
    }
    pub fn buffer(&self, id: u32) -> Option<Buffer> {
        self.nodes.get(&id).and_then(|n| n.current.buffer)
    }
    pub fn forget_buffer(&mut self, id: u32) {
        if let Some(n) = self.nodes.get_mut(&id) {
            n.current.buffer = None;
        }
    }
    pub fn take_own_callbacks(&mut self, id: u32) -> Vec<u32> {
        let mut result = Vec::new();
        if let Some(n) = self.nodes.get_mut(&id) {
            result.append(&mut n.callbacks);
            result.append(&mut n.cached_callbacks);
        }
        result
    }
    pub fn references(&self) -> Vec<u32> {
        let mut ids = Vec::new();
        for n in self.nodes.values() {
            for s in [Some(&n.current), n.cached.as_ref()].into_iter().flatten() {
                if let Some(b) = s.buffer {
                    if !ids.contains(&b.id) {
                        ids.push(b.id);
                    }
                }
            }
        }
        ids
    }
    fn layers(
        &self,
        id: u32,
        x: i32,
        y: i32,
        scale: u32,
        out: &mut Vec<Layer>,
    ) -> Result<(), &'static str> {
        let Some(n) = self.nodes.get(&id) else {
            return Ok(());
        };
        let Some(buffer) = n.current.buffer else {
            return Ok(());
        };
        for &(child, cx, cy) in &n.current.children {
            if child == id {
                out.push(n.current.view.layer(id, buffer, x, y, scale)?);
                if out.len() > MAX_LAYERS {
                    return Err("Too many surface layers");
                }
            } else {
                let cx = i32::try_from(x as i64 + cx as i64 * scale as i64)
                    .map_err(|_| "Surface position overflow")?;
                let cy = i32::try_from(y as i64 + cy as i64 * scale as i64)
                    .map_err(|_| "Surface position overflow")?;
                self.layers(child, cx, cy, scale, out)?;
            }
        }
        Ok(())
    }
    pub fn scene(&self, root: u32, window_id: u32, serial: u64) -> Result<Commit, &'static str> {
        let scale = self.output_scale as u32;
        let mut layers = Vec::new();
        self.layers(root, 0, 0, scale, &mut layers)?;
        let x = layers.iter().map(|l| l.x).min().unwrap_or(0);
        let y = layers.iter().map(|l| l.y).min().unwrap_or(0);
        let right = layers
            .iter()
            .map(|l| l.x as i64 + l.width as i64)
            .max()
            .unwrap_or(0);
        let bottom = layers
            .iter()
            .map(|l| l.y as i64 + l.height as i64)
            .max()
            .unwrap_or(0);
        for l in &mut layers {
            l.x = i32::try_from(l.x as i64 - x as i64).map_err(|_| "Scene origin overflow")?;
            l.y = i32::try_from(l.y as i64 - y as i64).map_err(|_| "Scene origin overflow")?;
        }
        let scene = Commit {
            external_client_id: root,
            window_id,
            serial,
            origin_x: x,
            origin_y: y,
            width: u32::try_from(right - x as i64).map_err(|_| "Scene width overflow")?,
            height: u32::try_from(bottom - y as i64).map_err(|_| "Scene height overflow")?,
            layers,
        };
        scene.validate().map_err(|_| "Invalid surface scene")?;
        Ok(scene)
    }
    pub fn coordinates(&self, root: u32, target: u32, x: i32, y: i32) -> Option<(i32, i32)> {
        let scene = self.scene(root, 0, 1).ok()?;
        let scale = self.output_scale;
        let layer = scene.layers.iter().find(|l| l.surface_id == target)?;
        Some((
            x.saturating_sub(layer.x) / scale,
            y.saturating_sub(layer.y) / scale,
        ))
    }
    pub fn hit(&self, root: u32, x: i32, y: i32) -> Option<(u32, i32, i32)> {
        let scene = self.scene(root, 0, 1).ok()?;
        let scale = self.output_scale;
        for l in scene.layers.iter().rev() {
            let lx = x as i64 - l.x as i64;
            let ly = y as i64 - l.y as i64;
            if lx >= 0 && ly >= 0 && lx < l.width as i64 && ly < l.height as i64 {
                let lx = lx as i32 / scale;
                let ly = ly as i32 / scale;
                if self.nodes[&l.surface_id]
                    .current
                    .input
                    .as_ref()
                    .is_none_or(|r| r.contains(lx, ly))
                {
                    return Some((l.surface_id, lx, ly));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn buffer(id: u32) -> Option<Option<Buffer>> {
        Some(Some(Buffer {
            id,
            width: 8,
            height: 8,
        }))
    }
    fn tree() -> Scene {
        let mut s = Scene::new();
        s.create(1);
        s.create(2);
        s.attach_child(2, 1).unwrap();
        s
    }
    #[test]
    fn synchronized_child_is_atomic_with_parent_and_keeps_callbacks() {
        let mut s = tree();
        s.commit(1, buffer(11), Vec::new()).unwrap();
        assert_eq!(s.commit(2, buffer(22), std::vec![100]).unwrap(), None);
        assert_eq!(s.scene(1, 1, 1).unwrap().layers.len(), 1);
        assert!(s.take_callbacks(1).is_empty());
        s.commit(1, None, Vec::new()).unwrap();
        let c = s.scene(1, 1, 1).unwrap();
        assert_eq!(
            c.layers.iter().map(|l| l.buffer_id).collect::<Vec<_>>(),
            std::vec![11, 22]
        );
        assert_eq!(s.take_callbacks(1), std::vec![100]);
    }
    #[test]
    fn desync_child_cannot_latch_pending_parent_position() {
        let mut s = tree();
        s.set_sync(2, false).unwrap();
        s.commit(2, buffer(22), Vec::new()).unwrap();
        s.commit(1, buffer(11), Vec::new()).unwrap();
        s.position(2, -3, 4).unwrap();
        s.commit(2, None, Vec::new()).unwrap();
        assert_eq!(s.scene(1, 1, 1).unwrap().origin_x, 0);
        s.commit(1, None, Vec::new()).unwrap();
        let c = s.scene(1, 1, 1).unwrap();
        assert_eq!((c.origin_x, c.width, c.height), (-3, 11, 12));
        assert_eq!(s.hit(1, 0, 4), Some((2, 0, 0)));
    }
    #[test]
    fn ancestor_sync_caches_desync_descendants_and_desync_flushes_them() {
        let mut s = tree();
        s.create(3);
        s.attach_child(3, 2).unwrap();
        s.set_sync(3, false).unwrap();
        s.commit(1, buffer(11), Vec::new()).unwrap();
        s.commit(2, buffer(22), Vec::new()).unwrap();
        s.commit(3, buffer(33), Vec::new()).unwrap();
        assert_eq!(s.scene(1, 1, 1).unwrap().layers.len(), 1);
        s.set_sync(2, false).unwrap();
        assert_eq!(s.scene(1, 1, 1).unwrap().layers.len(), 3);
    }
    #[test]
    fn parent_pending_state_does_not_leak_through_grandparent_commit() {
        let mut s = tree();
        s.create(3);
        s.attach_child(3, 2).unwrap();
        s.commit(3, buffer(33), Vec::new()).unwrap();
        s.commit(2, buffer(22), Vec::new()).unwrap();
        s.position(3, 4, 0).unwrap();
        s.commit(1, buffer(11), Vec::new()).unwrap();
        assert_eq!(s.scene(1, 1, 1).unwrap().layers[2].x, 0);
        s.commit(2, None, Vec::new()).unwrap();
        s.commit(1, None, Vec::new()).unwrap();
        assert_eq!(s.scene(1, 1, 1).unwrap().layers[2].x, 4);
    }
    #[test]
    fn stacking_and_unmapping_hide_descendants_without_dropping_buffer_ownership() {
        let mut s = tree();
        s.commit(2, buffer(22), Vec::new()).unwrap();
        s.restack(2, 1, false).unwrap();
        s.commit(1, buffer(11), Vec::new()).unwrap();
        assert_eq!(s.scene(1, 1, 1).unwrap().layers[0].surface_id, 2);
        s.commit(1, Some(None), Vec::new()).unwrap();
        assert!(s.scene(1, 1, 1).unwrap().layers.is_empty());
        assert!(s.references().contains(&22));
    }
    #[test]
    fn viewport_is_double_buffered_and_uses_post_scale_coordinates() {
        let mut s = Scene::new();
        s.create(1);
        s.enable(1);
        s.commit(1, buffer(11), Vec::new()).unwrap();
        let v = s.view_mut(1).unwrap();
        v.scale = 2;
        v.source = Some((128, 256, 512, 256));
        v.destination = Some((4, 3));
        assert_eq!(s.scene(1, 1, 1).unwrap().width, 8);
        s.commit(1, None, Vec::new()).unwrap();
        let c = s.scene(1, 1, 1).unwrap();
        let l = c.layers[0];
        assert_eq!(
            (
                l.source_x,
                l.source_y,
                l.source_width,
                l.source_height,
                l.width,
                l.height
            ),
            (256, 512, 1024, 512, 4, 3)
        );
    }
    #[test]
    fn output_scale_is_independent_of_buffer_scale_and_hit_coordinates() {
        let mut s = tree();
        s.set_output_scale(2);
        s.view_mut(1).unwrap().scale = 2;
        s.position(2, 2, 1).unwrap();
        s.commit(2, buffer(22), Vec::new()).unwrap();
        s.commit(1, buffer(11), Vec::new()).unwrap();
        let scene = s.scene(1, 1, 1).unwrap();
        assert_eq!((scene.layers[0].width, scene.layers[1].width), (8, 16));
        assert_eq!((scene.layers[1].x, scene.layers[1].y), (4, 2));
        assert_eq!(s.hit(1, 6, 4), Some((2, 1, 1)));
        assert_eq!(s.coordinates(1, 2, 6, 4), Some((1, 1)));
        s.set_output_scale(1);
        let scene = s.scene(1, 1, 2).unwrap();
        assert_eq!((scene.layers[0].width, scene.layers[1].width), (4, 8));
        assert_eq!(scene.layers[0].source_width, 8 * 256);
    }
    #[test]
    fn reject_cycle_fractional_unsized_crop_and_out_of_buffer() {
        let mut s = tree();
        assert!(s.attach_child(1, 2).is_err());
        assert!(s.restack(2, 2, true).is_err());
        s.view_mut(1).unwrap().source = Some((0, 0, 257, 256));
        assert!(s.commit(1, buffer(11), Vec::new()).is_err());
        s.view_mut(1).unwrap().source = Some((2048, 0, 256, 256));
        assert!(s.commit(1, buffer(11), Vec::new()).is_err());
    }
    #[test]
    fn destroyed_child_is_immediately_removed_from_cached_parent_order() {
        let mut s = tree();
        s.commit(2, buffer(22), Vec::new()).unwrap();
        s.commit(1, buffer(11), Vec::new()).unwrap();
        s.destroy(2);
        s.create(2);
        s.commit(2, buffer(99), Vec::new()).unwrap();
        assert_eq!(s.scene(1, 1, 1).unwrap().layers.len(), 1);
    }
}
