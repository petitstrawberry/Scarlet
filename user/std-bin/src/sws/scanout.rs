//! Fit a complete native desktop image onto a physical mirror.
pub(crate) fn mirror_viewport(source: (u32, u32), target: (u32, u32)) -> (u32, u32, u32, u32) {
    let (sw, sh) = (u64::from(source.0), u64::from(source.1));
    let (tw, th) = (u64::from(target.0), u64::from(target.1));
    if sw == 0 || sh == 0 || tw == 0 || th == 0 {
        return (0, 0, 0, 0);
    }
    let (w, h) = if tw * sh <= th * sw {
        (tw, (tw * sh / sw).max(1))
    } else {
        ((th * sw / sh).max(1), th)
    };
    (
        ((tw - w) / 2) as u32,
        ((th - h) / 2) as u32,
        w as u32,
        h as u32,
    )
}

/// Both backbuffers contain tightly packed, opaque BGRA pixels.
pub(crate) fn mirror_bgra(
    source: &[u8],
    source_size: (u32, u32),
    target: &mut [u8],
    target_size: (u32, u32),
) -> Result<(), &'static str> {
    let length = |(w, h): (u32, u32)| (w as usize).checked_mul(h as usize)?.checked_mul(4);
    let (x, y, w, h) = mirror_viewport(source_size, target_size);
    if w == 0
        || h == 0
        || Some(source.len()) != length(source_size)
        || Some(target.len()) != length(target_size)
    {
        return Err("Invalid desktop mirror buffers");
    }
    if source_size == target_size {
        target.copy_from_slice(source);
        return Ok(());
    }
    let black = |bar: &mut [u8]| {
        for pixel in bar.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[0, 0, 0, 255]);
        }
    };
    let stride = source_size.0 as usize * 4;
    let samples: Vec<_> = (0..w)
        .map(|dx| (u64::from(dx) * u64::from(source_size.0) / u64::from(w)) as usize * 4)
        .collect();
    for (dy, row) in target
        .chunks_exact_mut(target_size.0 as usize * 4)
        .enumerate()
    {
        if dy < y as usize || dy >= (y + h) as usize {
            black(row);
            continue;
        }
        let sy = (dy - y as usize) as u64 * u64::from(source_size.1) / u64::from(h);
        let source_row = &source[sy as usize * stride..][..stride];
        let (left, rest) = row.split_at_mut(x as usize * 4);
        let (image, right) = rest.split_at_mut(w as usize * 4);
        black(left);
        black(right);
        for (pixel, &offset) in image.chunks_exact_mut(4).zip(&samples) {
            pixel.copy_from_slice(&source_row[offset..offset + 4]);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_centers_wide_and_tall_desktops_including_odd_bars() {
        assert_eq!(
            mirror_viewport((1920, 1080), (1024, 768)),
            (0, 96, 1024, 576)
        );
        assert_eq!(mirror_viewport((3, 4), (10, 4)), (3, 0, 3, 4));
        assert_eq!(mirror_viewport((u32::MAX, 1), (1, 1)), (0, 0, 1, 1));
    }

    #[test]
    fn pixels_come_from_the_finished_source_and_bars_replace_previous_pixels() {
        let source = [21, 22, 23, 255, 34, 35, 36, 255];
        let mut target = [0xee; 4 * 4 * 4];
        mirror_bgra(&source, (2, 1), &mut target, (4, 4)).unwrap();
        let black = [0, 0, 0, 255];
        for (i, pixel) in target.chunks_exact(4).enumerate() {
            let expected = match (i / 4, i % 4) {
                (0 | 3, _) => black.as_slice(),
                (_, 0 | 1) => &source[..4],
                _ => &source[4..],
            };
            assert_eq!(pixel, expected);
        }
    }

    #[test]
    fn equal_modes_copy_exactly_and_short_buffers_return_an_error() {
        let mut target = [0; 8];
        mirror_bgra(&[27; 8], (2, 1), &mut target, (2, 1)).unwrap();
        assert_eq!(target, [27; 8]);
        assert!(mirror_bgra(&[0; 4], (2, 1), &mut target, (2, 1)).is_err());
    }
}
