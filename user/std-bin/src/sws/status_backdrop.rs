//! Shared backdrop geometry and a full-resolution software filter. Three
//! separable box passes approximate the same soft blur used by shell artwork.
//! Only declared material regions are filtered; retained client layers stay intact.

pub(super) type Rect = (i32, i32, u32, u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BackdropGeometry {
    pub output: Rect,
    pub source: Rect,
    pub radius: u32,
    pub corner_radius: u32,
}

impl BackdropGeometry {
    pub fn new(width: u32, height: u32, output: Rect, radius: u32, corner: u32) -> Option<Self> {
        if width == 0 || height == 0 || output.2 == 0 || output.3 == 0 || radius == 0 {
            return None;
        }
        let left = i64::from(output.0).max(0);
        let top = i64::from(output.1).max(0);
        let right = (i64::from(output.0) + i64::from(output.2)).min(i64::from(width));
        let bottom = (i64::from(output.1) + i64::from(output.3)).min(i64::from(height));
        if right <= left || bottom <= top {
            return None;
        }
        let radius = radius.min(64);
        let halo = i64::from(radius) * 3;
        let sx = (left - halo).max(0);
        let sy = (top - halo).max(0);
        Some(Self {
            output: (
                left as i32,
                top as i32,
                (right - left) as u32,
                (bottom - top) as u32,
            ),
            source: (
                sx as i32,
                sy as i32,
                ((right + halo).min(i64::from(width)) - sx) as u32,
                ((bottom + halo).min(i64::from(height)) - sy) as u32,
            ),
            radius,
            corner_radius: corner
                .min((right - left) as u32 / 2)
                .min((bottom - top) as u32 / 2),
        })
    }

    /// Clip in an output's pixel grid, retaining global scene coordinates.
    pub fn new_on_output(
        origin: (i32, i32),
        width: u32,
        height: u32,
        output: Rect,
        radius: u32,
        corner: u32,
    ) -> Option<Self> {
        let local = (
            output.0.saturating_sub(origin.0),
            output.1.saturating_sub(origin.1),
            output.2,
            output.3,
        );
        let mut geometry = Self::new(width, height, local, radius, corner)?;
        geometry.output.0 += origin.0;
        geometry.output.1 += origin.1;
        geometry.source.0 += origin.0;
        geometry.source.1 += origin.1;
        Some(geometry)
    }

    /// Resolve one output-owned cache into the target's local pixel grid.
    pub fn relative_to(self, origin: (i32, i32), width: u32, height: u32) -> Option<Self> {
        let mut local = self;
        local.source.0 = local.source.0.checked_sub(origin.0)?;
        local.source.1 = local.source.1.checked_sub(origin.1)?;
        local.output.0 = local.output.0.checked_sub(origin.0)?;
        local.output.1 = local.output.1.checked_sub(origin.1)?;
        if local.source.0 < 0
            || local.source.1 < 0
            || u64::from(local.source.0 as u32) + u64::from(local.source.2) > u64::from(width)
            || u64::from(local.source.1 as u32) + u64::from(local.source.3) > u64::from(height)
        {
            return None;
        }
        Some(local)
    }

    pub fn intersects_source(self, rect: Rect) -> bool {
        intersects(self.source, rect)
    }

    /// Absolute x span for a row in the rounded output. CPU and GPU use the
    /// same mask, so the material never leaks outside the native Surface.
    pub fn row_span(self, y: u32) -> (u32, u32) {
        let (_, _, width, height) = self.output;
        let radius = self.corner_radius;
        if radius == 0 {
            return (self.output.0 as u32, width);
        }

        // Evaluate the circle at the centre of each pixel row. Using integer
        // row edges makes the first row jump inward by the full radius, which
        // produces a visibly stepped corner on small translucent controls.
        let sample_y = y as f64 + 0.5;
        let top_center = radius as f64;
        let bottom_center = height as f64 - radius as f64;
        let dy = if sample_y < top_center {
            top_center - sample_y
        } else if sample_y > bottom_center {
            sample_y - bottom_center
        } else {
            0.0
        };
        let inset = if dy <= 0.0 {
            0
        } else {
            let radius = radius as f64;
            let reach = (radius * radius - dy * dy).max(0.0).sqrt();
            (radius - reach - 0.5).ceil().max(0.0) as u32
        };
        (self.output.0 as u32 + inset, width - inset * 2)
    }
}

fn intersects(a: Rect, b: Rect) -> bool {
    a.2 > 0
        && a.3 > 0
        && b.2 > 0
        && b.3 > 0
        && i64::from(a.0) < i64::from(b.0) + i64::from(b.2)
        && i64::from(b.0) < i64::from(a.0) + i64::from(a.2)
        && i64::from(a.1) < i64::from(b.1) + i64::from(b.3)
        && i64::from(b.1) < i64::from(a.1) + i64::from(a.3)
}

fn union(a: Rect, b: Rect) -> Rect {
    let x = a.0.min(b.0);
    let y = a.1.min(b.1);
    (
        x,
        y,
        ((i64::from(a.0) + i64::from(a.2)).max(i64::from(b.0) + i64::from(b.2)) - i64::from(x))
            as u32,
        ((i64::from(a.1) + i64::from(a.3)).max(i64::from(b.1) + i64::from(b.3)) - i64::from(y))
            as u32,
    )
}

/// Redraw every affected sampling halo from clean background pixels. Repeat
/// for overlapping materials; never sample old labels/cursors from the
/// persistent composition buffer or blur the same region twice per frame.
pub(super) fn expand_damage(geometries: &[BackdropGeometry], rects: &mut Vec<Rect>) {
    loop {
        let mut changed = false;
        for geometry in geometries {
            let mut merged = geometry.source;
            let mut matches = 0;
            let mut previous = merged;
            for &rect in rects
                .iter()
                .filter(|&&rect| geometry.intersects_source(rect))
            {
                previous = rect;
                merged = union(merged, rect);
                matches += 1;
            }
            if matches == 0 || (matches == 1 && merged == previous) {
                continue;
            }
            rects.retain(|&rect| !geometry.intersects_source(rect));
            rects.push(merged);
            changed = true;
        }
        if !changed {
            break;
        }
    }
}

/// Source identity is retained separately from presentation/repair damage.
/// Only native content commits may skip their own (and lower) backdrops.
/// Geometry, stacking, opacity and unknown damage have no foreground owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BackdropDamage {
    pub rect: Rect,
    pub foreground: Option<u32>,
}

impl BackdropDamage {
    /// Keep the same bounded budget as presentation damage. Overflow forgets
    /// ownership conservatively, never drops a background invalidation.
    pub fn push_bounded(entries: &mut Vec<Self>, next: Self, limit: usize) {
        if let Some(entry) = entries
            .iter_mut()
            .find(|entry| entry.foreground == next.foreground && intersects(entry.rect, next.rect))
        {
            entry.rect = union(entry.rect, next.rect);
        } else if entries.len() < limit {
            entries.push(next);
        } else {
            let rect = entries
                .iter()
                .fold(next.rect, |rect, entry| union(rect, entry.rect));
            entries.clear();
            entries.push(Self {
                rect,
                foreground: None,
            });
        }
    }
}

/// Decide which background captures changed, in native bottom-to-top order.
/// The material is captured BEFORE its owner's foreground. Repairing an old
/// swapchain image still expands presentation damage separately; it must not
/// turn a foreground-only update into a fresh background capture.
pub(super) fn backdrop_refreshes(
    backdrops: &[(u32, BackdropGeometry)],
    window_order: &[u32],
    damage: Option<&[BackdropDamage]>,
) -> Vec<bool> {
    let Some(damage) = damage else {
        return vec![true; backdrops.len()];
    };
    let ranks: Vec<_> = backdrops
        .iter()
        .map(|(owner, _)| window_order.iter().position(|id| id == owner))
        .collect();
    let mut refresh: Vec<_> = backdrops
        .iter()
        .zip(&ranks)
        .map(|((_, geometry), rank)| {
            let Some(rank) = rank else {
                return true;
            };
            damage.iter().any(|damage| {
                geometry.intersects_source(damage.rect)
                    && damage
                        .foreground
                        .and_then(|source| window_order.iter().position(|id| *id == source))
                        .is_none_or(|source_rank| source_rank < *rank)
            })
        })
        .collect();
    // A lower material can change pixels beyond the original damage through
    // its blur kernel. Propagate its OUTPUT to upper sampling halos. Materials
    // belonging to the same window all sample before that window is composed.
    for rank in 0..window_order.len() {
        for (index, (_, geometry)) in backdrops.iter().enumerate() {
            if ranks[index] != Some(rank) || refresh[index] {
                continue;
            }
            let upstream_changed = backdrops.iter().enumerate().any(|(lower, (_, material))| {
                refresh[lower]
                    && ranks[lower].is_none_or(|lower_rank| lower_rank < rank)
                    && geometry.intersects_source(material.output)
            });
            refresh[index] = upstream_changed;
        }
    }
    refresh
}

pub(super) struct CpuBackdrop {
    pub geometry: BackdropGeometry,
    pixels: Vec<u32>,
    scratch: Vec<u32>,
    ready: bool,
}

impl CpuBackdrop {
    pub fn new(geometry: BackdropGeometry) -> Self {
        let len = geometry.source.2 as usize * geometry.source.3 as usize;
        Self {
            geometry,
            pixels: vec![0; len],
            scratch: vec![0; len],
            ready: false,
        }
    }

    /// Prepare all regions of a window before compositing any of them. Their
    /// sampling halos can overlap even when the visible controls do not.
    pub fn prepare(&mut self, buffer: &[u8], stride: u32) {
        let (x, y, width, height) = self.geometry.source;
        self.ready = false;
        if stride < (x as u32 + width).saturating_mul(4)
            || buffer.len() < stride as usize * (y as usize + height as usize)
        {
            return;
        }
        for row in 0..height as usize {
            let start = (y as usize + row) * stride as usize + x as usize * 4;
            for (dest, src) in self.pixels[row * width as usize..(row + 1) * width as usize]
                .iter_mut()
                .zip(buffer[start..start + width as usize * 4].chunks_exact(4))
            {
                *dest = u32::from_le_bytes(src.try_into().unwrap());
            }
        }
        for _ in 0..3 {
            box_pass(
                &self.pixels,
                &mut self.scratch,
                width as usize,
                height as usize,
                self.geometry.radius as usize,
                true,
            );
            box_pass(
                &self.scratch,
                &mut self.pixels,
                width as usize,
                height as usize,
                self.geometry.radius as usize,
                false,
            );
        }
        self.ready = true;
    }

    pub fn composite(&self, buffer: &mut [u8], stride: u32) {
        if !self.ready {
            return;
        }
        let geometry = self.geometry;
        for row in 0..geometry.output.3 {
            let y = geometry.output.1 as u32 + row;
            let (x, width) = geometry.row_span(row);
            let source_start = (y - geometry.source.1 as u32) as usize * geometry.source.2 as usize
                + (x - geometry.source.0 as u32) as usize;
            let dest = y as usize * stride as usize + x as usize * 4;
            for (pixel, rgba) in buffer[dest..dest + width as usize * 4]
                .chunks_exact_mut(4)
                .zip(&self.pixels[source_start..source_start + width as usize])
            {
                pixel.copy_from_slice(&rgba.to_le_bytes());
            }
        }
    }
}

/// Sliding sums keep cost independent of blur radius. Clamp-to-edge also
/// avoids dark borders when a material touches the edge of the display.
fn box_pass(
    source: &[u32],
    output: &mut [u32],
    width: usize,
    height: usize,
    radius: usize,
    horizontal: bool,
) {
    let (lines, length, step) = if horizontal {
        (height, width, 1)
    } else {
        (width, height, width)
    };
    let taps = (radius * 2 + 1) as u32;
    for line in 0..lines {
        let base = if horizontal { line * width } else { line };
        let at = |p: isize| base + p.clamp(0, length as isize - 1) as usize * step;
        let channels = |p: u32| [p & 255, (p >> 8) & 255, (p >> 16) & 255];
        let mut sums = [0u32; 3];
        for p in -(radius as isize)..=radius as isize {
            let c = channels(source[at(p)]);
            for i in 0..3 {
                sums[i] += c[i];
            }
        }
        for p in 0..length {
            output[base + p * step] = 0xff000000
                | ((sums[0] + taps / 2) / taps)
                | (((sums[1] + taps / 2) / taps) << 8)
                | (((sums[2] + taps / 2) / taps) << 16);
            let enter = channels(source[at(p as isize + radius as isize + 1)]);
            let leave = channels(source[at(p as isize - radius as isize)]);
            for i in 0..3 {
                sums[i] = sums[i] + enter[i] - leave[i];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn spanning_material_has_separate_bounded_sampling_halos() {
        let region = (90, 10, 30, 20);
        let left = super::BackdropGeometry::new_on_output((0, 0), 100, 60, region, 4, 0).unwrap();
        let right = super::BackdropGeometry::new_on_output((100, 0), 80, 60, region, 4, 0).unwrap();
        assert_eq!(left.output, (90, 10, 10, 20));
        assert_eq!(right.output, (100, 10, 20, 20));
        assert_eq!(left.source.0 as u32 + left.source.2, 100);
        assert_eq!(right.source.0, 100);
        assert!(left.relative_to((100, 0), 80, 60).is_none());
        assert!(right.relative_to((0, 0), 100, 60).is_none());
        let local = right.relative_to((100, 0), 80, 60).unwrap();
        assert_eq!(
            local,
            super::BackdropGeometry::new(80, 60, (-10, 10, 30, 20), 4, 0).unwrap()
        );
    }
    use super::*;

    #[test]
    fn native_content_damage_only_refreshes_materials_above_its_owner() {
        let geometry = BackdropGeometry::new(200, 200, (30, 30, 100, 100), 4, 8).unwrap();
        let materials = [(10, geometry), (20, geometry)];
        let order = [1, 10, 20, 30];
        let damage = |foreground| {
            [BackdropDamage {
                rect: (50, 50, 4, 4),
                foreground,
            }]
        };
        assert_eq!(
            backdrop_refreshes(&materials, &order, Some(&damage(Some(20)))),
            [false, false]
        );
        assert_eq!(
            backdrop_refreshes(&materials, &order, Some(&damage(Some(10)))),
            [false, true]
        );
        assert_eq!(
            backdrop_refreshes(&materials, &order, Some(&damage(Some(1)))),
            [true, true]
        );
        assert_eq!(
            backdrop_refreshes(&materials, &order, Some(&damage(Some(30)))),
            [false, false]
        );
        // Move, resize, reorder, opacity and removed/unknown owners cannot use
        // the foreground exclusion. A full repaint also invalidates all.
        for owner in [None, Some(99)] {
            assert_eq!(
                backdrop_refreshes(&materials, &order, Some(&damage(owner))),
                [true, true]
            );
        }
        assert_eq!(backdrop_refreshes(&materials, &order, None), [true, true]);
        assert_eq!(
            backdrop_refreshes(&materials, &order, Some(&[])),
            [false, false]
        );
        assert_eq!(
            backdrop_refreshes(&materials, &[1, 20, 10, 30], Some(&damage(Some(10)))),
            [false, false]
        );
    }

    #[test]
    fn lower_blur_output_propagates_upward_and_damage_overflow_stays_conservative() {
        let lower = BackdropGeometry::new(300, 200, (30, 30, 100, 100), 4, 8).unwrap();
        let upper = BackdropGeometry::new(300, 200, (120, 30, 80, 100), 4, 8).unwrap();
        let damage = BackdropDamage {
            rect: (25, 50, 2, 2),
            foreground: Some(1),
        };
        assert!(!upper.intersects_source(damage.rect));
        // Deliberately unsorted material list: stacking order is authoritative.
        assert_eq!(
            backdrop_refreshes(&[(20, upper), (10, lower)], &[1, 10, 20], Some(&[damage])),
            [true, true]
        );
        let own = BackdropDamage {
            rect: (50, 50, 2, 2),
            foreground: Some(20),
        };
        let behind = BackdropDamage {
            rect: (130, 50, 2, 2),
            foreground: Some(1),
        };
        let mut entries = Vec::new();
        BackdropDamage::push_bounded(&mut entries, own, 1);
        BackdropDamage::push_bounded(&mut entries, behind, 1);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].foreground, None);
        assert_eq!(
            backdrop_refreshes(&[(20, upper)], &[1, 20], Some(&entries)),
            [true]
        );
    }

    #[test]
    fn overlapping_material_halos_share_one_clean_redraw() {
        let a = BackdropGeometry::new(400, 300, (10, 250, 80, 40), 8, 10).unwrap();
        let b = BackdropGeometry::new(400, 300, (98, 250, 80, 40), 8, 10).unwrap();
        let unrelated = (250, 80, 20, 20);
        let mut damage = vec![(5, 240, 10, 10), unrelated, (175, 270, 8, 8)];
        expand_damage(&[a, b], &mut damage);
        assert_eq!(damage, [unrelated, union(a.source, b.source)]);
        let old = damage.clone();
        expand_damage(&[a, b], &mut damage);
        assert_eq!(damage, old);
    }

    #[test]
    fn rounded_material_rows_are_symmetric_and_do_not_collapse_at_the_tip() {
        let geometry = BackdropGeometry::new(100, 60, (10, 10, 80, 40), 8, 10).unwrap();
        let spans: Vec<_> = (0..geometry.output.3)
            .map(|row| geometry.row_span(row))
            .collect();

        for row in 0..spans.len() {
            assert_eq!(spans[row], spans[spans.len() - row - 1]);
        }
        assert_eq!(spans[0], (17, 66));
        assert_eq!(spans[1], (15, 70));
        assert!(spans.windows(2).take(10).all(|pair| pair[1].1 >= pair[0].1));
    }

    #[test]
    fn full_resolution_blur_is_smooth_at_edges_and_preserves_unfiltered_pixels() {
        let geometry = BackdropGeometry::new(193, 100, (10, 0, 170, 40), 8, 10).unwrap();
        let stride = 196 * 4;
        let mut pixels = vec![73; stride * 100];
        for y in 0..100 {
            for x in 0..193 {
                let value = if x < 96 { 0 } else { 255 };
                pixels[(y * 196 + x) * 4..(y * 196 + x) * 4 + 4]
                    .copy_from_slice(&[value, value, value, 255]);
            }
        }
        let before = pixels.clone();
        let mut filter = CpuBackdrop::new(geometry);
        let capacity = filter.pixels.capacity();
        filter.prepare(&pixels, stride as u32);
        filter.composite(&mut pixels, stride as u32);
        let row = &pixels[20 * stride..21 * stride];
        assert!(
            row[80 * 4] > 0 && row[112 * 4] < 255,
            "three passes must soften a broad band"
        );
        assert!(
            (73..118).all(|x| row[x * 4].abs_diff(row[(x + 1) * 4]) <= 13),
            "no coarse upsampling blocks"
        );
        for y in 0..100 {
            let (x, width) = if y < 40 {
                geometry.row_span(y as u32)
            } else {
                (0, 0)
            };
            for col in 0..196 {
                if col < x as usize || col >= (x + width) as usize {
                    let p = (y * 196 + col) * 4;
                    assert_eq!(&pixels[p..p + 4], &before[p..p + 4]);
                }
            }
        }
        pixels.copy_from_slice(&before);
        filter.prepare(&pixels, stride as u32);
        assert_eq!(capacity, filter.pixels.capacity());
    }

    #[test]
    fn uniform_material_remains_uniform_even_on_tiny_outputs() {
        for (w, h) in [(1, 1), (257, 91), (1280, 720)] {
            let geometry = BackdropGeometry::new(w, h, (0, 0, w, h.min(32)), 8, 0).unwrap();
            let mut pixels = [200, 120, 80, 255].repeat((w * h) as usize);
            let mut filter = CpuBackdrop::new(geometry);
            filter.prepare(&pixels, w * 4);
            filter.composite(&mut pixels, w * 4);
            assert!(pixels.chunks_exact(4).all(|p| p == [200, 120, 80, 255]));
        }
    }
}
