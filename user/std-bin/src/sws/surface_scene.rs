//! SWS surface-scene sampling geometry and software-backend renderer.
//! GPU composition samples the original layers; no CPU precomposition is needed.
use sws_protocol::surface_scene::Layer;

#[derive(Clone, Copy)]
pub struct Pixels<'a> {
    pub bytes: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub opaque: bool,
    /// A registered shared GPU source; bytes are empty and never read on CPU.
    pub gpu_buffer: Option<(usize, u32)>,
}

/// Borrowed only during one synchronous frame. The pool registry owns the
/// mappings and the retained scene owns each buffer lease.
pub(super) struct SceneView<'a> {
    pub scene: &'a sws_protocol::surface_scene::Commit,
    pub sources: Vec<Pixels<'a>>,
}

/// Texture-coordinate corners after inverse Wayland buffer transformation.
/// Edge coordinates intentionally use W/H, not W-1/H-1 (pixel centers).
pub(super) fn source_uv(layer: &Layer, width: u32, height: u32) -> [[f32; 2]; 4] {
    let x = layer.source_x as f32 / 256.0;
    let y = layer.source_y as f32 / 256.0;
    let r = x + layer.source_width as f32 / 256.0;
    let b = y + layer.source_height as f32 / 256.0;
    let w = width as f32;
    let h = height as f32;
    [[x, y], [x, b], [r, b], [r, y]].map(|[x, y]| {
        let (x, y) = match layer.transform {
            0 => (x, y),
            1 => (w - y, x),
            2 => (w - x, h - y),
            3 => (y, h - x),
            4 => (w - x, y),
            5 => (y, x),
            6 => (x, h - y),
            _ => (w - y, h - x),
        };
        [x / w, y / h]
    })
}

pub(super) fn render(view: &SceneView<'_>, pixels: &mut Vec<u8>) -> Result<(), &'static str> {
    let len = view.scene.width as usize * view.scene.height as usize * 4;
    pixels
        .try_reserve(len.saturating_sub(pixels.len()))
        .map_err(|_| "Scene allocation failed")?;
    pixels.resize(len, 0);
    pixels.fill(0);
    for (layer, source) in view.scene.layers.iter().zip(&view.sources) {
        composite(layer, *source, pixels, view.scene.width, view.scene.height)?;
    }
    finish(pixels);
    Ok(())
}

/// Composite one cropped/orthogonally transformed layer using premultiplied BGRA.
/// Every source and destination extent is validated before writing any pixel.
pub fn composite(
    layer: &Layer,
    source: Pixels<'_>,
    target: &mut [u8],
    width: u32,
    height: u32,
) -> Result<(), &'static str> {
    if source.gpu_buffer.is_some() { return Err("GPU scene source cannot be rendered on CPU"); }
    let row = source.width as usize * 4;
    let required = (source.height as usize)
        .checked_sub(1)
        .and_then(|h| h.checked_mul(source.stride))
        .and_then(|v| v.checked_add(row))
        .ok_or("Invalid scene source")?;
    if !layer.fits_buffer(source.width, source.height)
        || source.stride < row
        || source.bytes.len() < required
        || layer.x < 0
        || layer.y < 0
        || layer.x as u64 + layer.width as u64 > width as u64
        || layer.y as u64 + layer.height as u64 > height as u64
        || target.len() != width as usize * height as usize * 4
    {
        return Err("Invalid scene layer bounds");
    }
    for y in 0..layer.height {
        let sy = (layer.source_y as i64
            + (2 * y as i64 + 1) * layer.source_height as i64 / (2 * layer.height as i64))
            / 256;
        for x in 0..layer.width {
            let sx = (layer.source_x as i64
                + (2 * x as i64 + 1) * layer.source_width as i64 / (2 * layer.width as i64))
                / 256;
            let w = source.width as i64;
            let h = source.height as i64;
            let (bx, by) = match layer.transform {
                0 => (sx, sy),
                1 => (w - 1 - sy, sx),
                2 => (w - 1 - sx, h - 1 - sy),
                3 => (sy, h - 1 - sx),
                4 => (w - 1 - sx, sy),
                5 => (sy, sx),
                6 => (sx, h - 1 - sy),
                _ => (w - 1 - sy, h - 1 - sx),
            };
            let src = by as usize * source.stride + bx as usize * 4;
            let dst =
                ((layer.y as usize + y as usize) * width as usize + layer.x as usize + x as usize)
                    * 4;
            let alpha = if source.opaque {
                255
            } else {
                source.bytes[src + 3] as u32
            };
            for c in 0..3 {
                target[dst + c] = (source.bytes[src + c] as u32
                    + (target[dst + c] as u32 * (255 - alpha) + 127) / 255)
                    .min(255) as u8;
            }
            target[dst + 3] =
                (alpha + (target[dst + 3] as u32 * (255 - alpha) + 127) / 255).min(255) as u8;
        }
    }
    Ok(())
}

/// The existing Window pixel contract uses straight alpha. Convert once after
/// all premultiplied layers have been combined, never between layers.
pub fn finish(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = pixel[3] as u32;
        if alpha > 0 && alpha < 255 {
            for c in 0..3 {
                pixel[c] = ((pixel[c] as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn layer() -> Layer {
        Layer {
            surface_id: 1,
            buffer_id: 2,
            x: 0,
            y: 0,
            width: 2,
            height: 1,
            source_x: 0,
            source_y: 0,
            source_width: 512,
            source_height: 256,
            transform: 0,
        }
    }
    #[test]
    fn all_gpu_uv_transforms_match_software_pixel_orientation() {
        let expected = [
            [1, 2, 3, 4, 5, 6],
            [2, 4, 6, 1, 3, 5],
            [6, 5, 4, 3, 2, 1],
            [5, 3, 1, 6, 4, 2],
            [2, 1, 4, 3, 6, 5],
            [1, 3, 5, 2, 4, 6],
            [5, 6, 3, 4, 1, 2],
            [6, 4, 2, 5, 3, 1],
        ];
        let bytes: Vec<_> = (1..=6u8).flat_map(|v| [v, 0, 0, 255]).collect();
        for transform in 0..8 {
            let (w, h) = if transform & 1 == 0 { (2, 3) } else { (3, 2) };
            let mut l = layer();
            l.transform = transform;
            l.width = w;
            l.height = h;
            l.source_width = w as i32 * 256;
            l.source_height = h as i32 * 256;
            let uv = source_uv(&l, 2, 3);
            let mut software = vec![0; 24];
            composite(
                &l,
                Pixels {
                    bytes: &bytes,
                    width: 2,
                    height: 3,
                    stride: 8,
                    opaque: false,
                    gpu_buffer: None,
                },
                &mut software,
                w,
                h,
            )
            .unwrap();
            for y in 0..h {
                for x in 0..w {
                    let fx = (x as f32 + 0.5) / w as f32;
                    let fy = (y as f32 + 0.5) / h as f32;
                    let u = uv[0][0] + fx * (uv[3][0] - uv[0][0]) + fy * (uv[1][0] - uv[0][0]);
                    let v = uv[0][1] + fx * (uv[3][1] - uv[0][1]) + fy * (uv[1][1] - uv[0][1]);
                    let i = (y * w + x) as usize;
                    let sampled = bytes[((v * 3.0) as usize * 2 + (u * 2.0) as usize) * 4];
                    assert_eq!(sampled, expected[transform as usize][i]);
                    assert_eq!(software[i * 4], sampled);
                }
            }
        }
    }

    #[test]
    fn fractional_crop_keeps_subtexel_edges_for_gpu_sampling() {
        let mut l = layer();
        l.source_x = 64;
        l.source_y = 128;
        l.source_width = 256;
        l.source_height = 128;
        assert_eq!(
            source_uv(&l, 2, 1),
            [[0.125, 0.5], [0.125, 1.0], [0.625, 1.0], [0.625, 0.5]]
        );
    }

    #[test]
    fn crop_scale_and_alpha_compose_in_stack_order() {
        let src = [0, 0, 255, 255, 0, 128, 0, 128];
        let mut dst = [255, 0, 0, 255, 255, 0, 0, 255];
        let mut l = layer();
        l.source_x = 256;
        l.source_width = 256;
        composite(
            &l,
            Pixels {
                bytes: &src,
                width: 2,
                height: 1,
                stride: 8,
                opaque: false,
                    gpu_buffer: None,
            },
            &mut dst,
            2,
            1,
        )
        .unwrap();
        assert_eq!(dst, [127, 128, 0, 255, 127, 128, 0, 255]);
    }
    #[test]
    fn rotation_respects_stride_and_xrgb_alpha() {
        let src = [1, 2, 3, 0, 4, 5, 6, 0, 99, 99, 99, 99];
        let mut dst = [0; 8];
        let mut l = layer();
        l.width = 1;
        l.height = 2;
        l.transform = 1;
        l.source_width = 256;
        l.source_height = 512;
        composite(
            &l,
            Pixels {
                bytes: &src,
                width: 2,
                height: 1,
                stride: 12,
                opaque: true,
                    gpu_buffer: None,
            },
            &mut dst,
            1,
            2,
        )
        .unwrap();
        assert_eq!(dst, [4, 5, 6, 255, 1, 2, 3, 255]);
    }
    #[test]
    fn invalid_source_does_not_modify_destination() {
        let mut dst = [7; 8];
        let mut l = layer();
        l.source_x = 1;
        assert!(
            composite(
                &l,
                Pixels {
                    bytes: &[0; 8],
                    width: 2,
                    height: 1,
                    stride: 8,
                    opaque: false
                },
                &mut dst,
                2,
                1
            )
            .is_err()
        );
        assert_eq!(dst, [7; 8]);
    }
}
