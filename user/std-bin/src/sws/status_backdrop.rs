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

    pub fn intersects_source(self, rect: Rect) -> bool {
        intersects(self.source, rect)
    }

    /// Absolute x span for a row in the rounded output. CPU and GPU use the
    /// same mask, so the material never leaks outside the native Surface.
    pub fn row_span(self, y: u32) -> (u32, u32) {
        let (_, _, width, height) = self.output;
        let radius = self.corner_radius;
        let dy = if y < radius {
            radius - y
        } else if y >= height - radius {
            y - (height - radius - 1)
        } else {
            0
        };
        let inset = if dy == 0 {
            0
        } else {
            let square = u64::from(radius) * u64::from(radius) - u64::from(dy) * u64::from(dy);
            radius - (square as f64).sqrt() as u32
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
    use super::*;

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
