//! Bounded screenshot pixels, selection geometry and PNG export.
#[cfg(test)]
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub const MAX_PIXELS: usize = 32 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
impl Region {
    pub fn between(a: (u32, u32), b: (u32, u32)) -> Self {
        Self {
            x: a.0.min(b.0),
            y: a.1.min(b.1),
            width: a.0.abs_diff(b.0).saturating_add(1),
            height: a.1.abs_diff(b.1).saturating_add(1),
        }
    }
}
pub fn byte_length(width: u32, height: u32) -> Result<usize, String> {
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or("Image dimensions overflow")?;
    if pixels == 0 || pixels > MAX_PIXELS {
        return Err("Image must contain 1 to 32 million pixels".into());
    }
    pixels
        .checked_mul(4)
        .ok_or_else(|| "Image byte count overflows".into())
}
impl Pixels {
    pub fn new(width: u32, height: u32, bgra: Vec<u8>) -> Result<Self, String> {
        if bgra.len() != byte_length(width, height)? {
            return Err("Incomplete image pixels".into());
        }
        Ok(Self {
            width,
            height,
            bgra,
        })
    }
    pub fn full(&self) -> Region {
        Region {
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
        }
    }
    pub fn crop(&self, r: Region) -> Result<Self, String> {
        if r.width == 0
            || r.height == 0
            || r.x.checked_add(r.width).is_none_or(|v| v > self.width)
            || r.y.checked_add(r.height).is_none_or(|v| v > self.height)
        {
            return Err("Selection is outside the image".into());
        }
        let mut bytes = Vec::with_capacity(byte_length(r.width, r.height)?);
        for y in r.y..r.y + r.height {
            let start = (y as usize * self.width as usize + r.x as usize) * 4;
            bytes.extend_from_slice(&self.bgra[start..start + r.width as usize * 4]);
        }
        Self::new(r.width, r.height, bytes)
    }
    pub fn arrow(&mut self, a: (u32, u32), b: (u32, u32)) {
        let clamp = |p: (u32, u32)| {
            (
                p.0.min(self.width - 1) as i32,
                p.1.min(self.height - 1) as i32,
            )
        };
        let a = clamp(a);
        let b = clamp(b);
        self.line(a, b);
        let dx = (a.0 - b.0) as f64;
        let dy = (a.1 - b.1) as f64;
        let len = dx.hypot(dy);
        if len >= 2.0 {
            let size = 18.0f64.min(len * 0.4);
            let ux = dx / len;
            let uy = dy / len;
            for side in [-1.0, 1.0] {
                self.line(
                    b,
                    (
                        (b.0 as f64 + size * (ux + side * uy * 0.5)).round() as i32,
                        (b.1 as f64 + size * (uy - side * ux * 0.5)).round() as i32,
                    ),
                );
            }
        }
    }
    fn line(&mut self, mut a: (i32, i32), b: (i32, i32)) {
        let dx = (b.0 - a.0).abs();
        let sx = if a.0 < b.0 { 1 } else { -1 };
        let dy = -(b.1 - a.1).abs();
        let sy = if a.1 < b.1 { 1 } else { -1 };
        let mut error = dx + dy;
        loop {
            for oy in -1..=1 {
                for ox in -1..=1 {
                    let x = a.0 + ox;
                    let y = a.1 + oy;
                    if x >= 0 && y >= 0 && x < self.width as i32 && y < self.height as i32 {
                        let i = (y as usize * self.width as usize + x as usize) * 4;
                        self.bgra[i..i + 4].copy_from_slice(&[35, 45, 235, 255]);
                    }
                }
            }
            if a == b {
                break;
            }
            let e = 2 * error;
            if e >= dy {
                error += dy;
                a.0 += sx;
            }
            if e <= dx {
                error += dx;
                a.1 += sy;
            }
        }
    }
    pub fn encode_png(&self, output: impl Write) -> Result<(), String> {
        let mut encoder = png::Encoder::new(output, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        let mut rgba = self.bgra.clone();
        for p in rgba.chunks_exact_mut(4) {
            p.swap(0, 2);
            p[3] = 255;
        }
        writer.write_image_data(&rgba).map_err(|e| e.to_string())?;
        writer.finish().map_err(|e| e.to_string())
    }
}

pub fn destination_exists(path: &Path) -> bool {
    scarlet_fs::symlink_metadata(path).is_ok()
}

/// Never replaces an existing path without explicit confirmation. Existing links
/// and non-regular files are rejected even with confirmation.
pub fn save_png(image: &Pixels, path: &Path, replace: bool) -> Result<(), String> {
    if !replace {
        let mut file = scarlet_fs::create_new(path).map_err(|e| e.to_string())?;
        let result = image
            .encode_png(&mut file)
            .and_then(|()| file.sync_all().map_err(|e| e.to_string()));
        drop(file);
        return finish_or_cleanup(result, path);
    }
    let metadata = scarlet_fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Only an existing regular file can be replaced".into());
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    for _ in 0..128 {
        let temporary = parent.join(format!(
            ".screenshot-{}-{}.png",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = match scarlet_fs::create_new_with_permissions(
            &temporary,
            Some(&metadata.permissions()),
        ) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        };
        let result = image
            .encode_png(&mut file)
            .and_then(|()| file.sync_all().map_err(|e| e.to_string()));
        drop(file);
        let result =
            result.and_then(|()| std::fs::rename(&temporary, path).map_err(|e| e.to_string()));
        return finish_or_cleanup(result, &temporary);
    }
    Err("Could not reserve a temporary PNG file".into())
}

fn finish_or_cleanup(result: Result<(), String>, path: &Path) -> Result<(), String> {
    #[cfg(target_os = "scarlet")]
    {
        return result.map_err(|error| {
            format!(
                "{error}; partial output retained at {} (conditional unlink unavailable)",
                path.display()
            )
        });
    }
    #[cfg(not(target_os = "scarlet"))]
    match result {
        Ok(()) => Ok(()),
        Err(error) => match std::fs::remove_file(path) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!(
                "{error}; partial output remains at {}: {cleanup}",
                path.display()
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_and_crop() {
        assert!(byte_length(0, 1).is_err());
        assert!(byte_length(u32::MAX, u32::MAX).is_err());
        let p = Pixels::new(2, 2, (0..16).collect()).unwrap();
        assert_eq!(
            p.crop(Region {
                x: 1,
                y: 0,
                width: 1,
                height: 2
            })
            .unwrap()
            .bgra,
            vec![4, 5, 6, 7, 12, 13, 14, 15]
        );
        assert!(
            p.crop(Region {
                x: u32::MAX,
                y: 0,
                width: 2,
                height: 1
            })
            .is_err()
        );
        assert_eq!(
            Region::between((4, 5), (2, 3)),
            Region {
                x: 2,
                y: 3,
                width: 3,
                height: 3
            }
        );
    }
    #[test]
    fn arrow_is_bounded_and_png_roundtrips() {
        let mut p = Pixels::new(20, 15, vec![0; 20 * 15 * 4]).unwrap();
        p.arrow((u32::MAX, 3), (0, 14));
        let mut bytes = Vec::new();
        p.encode_png(&mut bytes).unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
            .read_info()
            .unwrap();
        let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut decoded).unwrap();
        assert_eq!((info.width, info.height), (20, 15));
        assert!(decoded.chunks_exact(4).any(|p| p == [235, 45, 35, 255]));
    }
    #[test]
    fn export_refuses_collision_and_replaces_only_with_confirmation() {
        let root = std::env::temp_dir().join(format!("screenshot-test-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let p = root.join("image.png");
        fs::write(&p, b"original").unwrap();
        let image = Pixels::new(1, 1, vec![1, 2, 3, 255]).unwrap();
        assert!(save_png(&image, &p, false).is_err());
        assert_eq!(fs::read(&p).unwrap(), b"original");
        save_png(&image, &p, true).unwrap();
        assert!(fs::read(&p).unwrap().starts_with(b"\x89PNG"));
        assert!(save_png(&image, &root, true).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
