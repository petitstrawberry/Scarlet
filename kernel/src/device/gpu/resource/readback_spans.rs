//! Checked packing of backend-described physical spans into one logical row.

pub(super) fn pack_row(
    x: u32,
    y: u32,
    width: u32,
    texel_bytes: usize,
    backing_size: u64,
    output: &mut [u8],
    mut span: impl FnMut(u32, u32, u32) -> Result<(u64, u32), &'static str>,
    mut read: impl FnMut(u64, &mut [u8]) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    if width == 0
        || texel_bytes == 0
        || x.checked_add(width).is_none()
        || (width as usize).checked_mul(texel_bytes) != Some(output.len())
    {
        return Err("GPU readback row extent is invalid");
    }
    let mut column = 0;
    while column < width {
        let (offset, count) = span(x + column, y, width - column)?;
        if count == 0 || count > width - column {
            return Err("GPU backend returned an invalid readback span");
        }
        let length = (count as usize)
            .checked_mul(texel_bytes)
            .ok_or("GPU readback span length overflows")?;
        if offset
            .checked_add(length as u64)
            .is_none_or(|end| end > backing_size)
        {
            return Err("GPU readback span exceeds immutable image layout");
        }
        let start = column as usize * texel_bytes;
        read(offset, &mut output[start..start + length])?;
        column += count;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn packs_fragmented_odd_rectangle_without_copying_guards() {
        let physical = [10u8, 11, 20, 21, 12, 13, 22, 23, 14, 15, 24, 25];
        let mut guarded = [0xa5; 5];
        pack_row(
            1,
            1,
            3,
            1,
            physical.len() as u64,
            &mut guarded[1..4],
            |x, y, remaining| {
                Ok((
                    ((x / 2) * 4 + y * 2 + (x & 1)) as u64,
                    remaining.min(2 - (x & 1)),
                ))
            },
            |offset, out| {
                out.copy_from_slice(&physical[offset as usize..offset as usize + out.len()]);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(guarded, [0xa5, 21, 22, 23, 0xa5]);
    }

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn rejects_bad_spans_before_touching_backing() {
        for bad in [(0, 0), (0, 5), (15, 1), (u64::MAX, 1)] {
            let mut row = [0xa5; 16];
            assert!(
                pack_row(
                    0,
                    0,
                    4,
                    4,
                    16,
                    &mut row,
                    |_, _, _| Ok(bad),
                    |_, _| panic!("invalid span must not read backing")
                )
                .is_err()
            );
            assert_eq!(row, [0xa5; 16]);
        }
    }

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn propagates_mapping_and_backing_errors() {
        let mut row = [0xa5; 4];
        assert_eq!(
            pack_row(
                0,
                0,
                1,
                4,
                4,
                &mut row,
                |_, _, _| Err("mapping failed"),
                |_, _| panic!()
            ),
            Err("mapping failed")
        );
        assert_eq!(
            pack_row(
                0,
                0,
                1,
                4,
                4,
                &mut row,
                |_, _, _| Ok((0, 1)),
                |_, _| Err("backing failed")
            ),
            Err("backing failed")
        );
    }

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn rejects_overflow_and_wrong_output_size() {
        for (x, width, bytes) in [(u32::MAX, 2, 2), (0, 2, usize::MAX), (0, 1, 0), (0, 2, 4)] {
            assert!(
                pack_row(
                    x,
                    0,
                    width,
                    bytes,
                    16,
                    &mut [0; 4],
                    |_, _, _| panic!(),
                    |_, _| panic!()
                )
                .is_err()
            );
        }
    }
}
