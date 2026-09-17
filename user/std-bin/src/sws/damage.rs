//! Output damage bookkeeping shared by SWS compositor paths.

use std::vec::Vec;

/// Screen-space rectangle represented as `(x, y, width, height)`.
pub(super) type DamageRect = (i32, i32, u32, u32);

/// `None` means full-output damage; `Some` contains bounded damage rectangles.
pub(super) type PresentDamage = Option<Vec<DamageRect>>;

/// Window identifier paired with its screen-space geometry.
pub(super) type WindowGeometrySnapshot = (u32, DamageRect);

/// Convert registered-image damage to the geometry used when sampling it.
/// A resized window scales the retained image, including its filter footprint.
/// Repaint that complete geometry rather than applying image coordinates to
/// output pixels and leaving the expanded portion of the window stale.
pub(super) fn shared_frame_damage(
    image_extent: (u32, u32),
    geometry: DamageRect,
    rects: &[(u32, u32, u32, u32)],
) -> Vec<DamageRect> {
    if (geometry.2, geometry.3) != image_extent {
        return vec![geometry];
    }
    rects
        .iter()
        .map(|&(x, y, width, height)| {
            (
                geometry.0.saturating_add(x as i32),
                geometry.1.saturating_add(y as i32),
                width,
                height,
            )
        })
        .collect()
}

/// Calculate the old and new rectangles affected by window geometry changes.
///
/// # Arguments
///
/// * `before` - Visible window geometry before the scene mutation.
/// * `after` - Visible window geometry after the scene mutation.
///
/// # Returns
///
/// Rectangles that must be recomposited to remove old content and draw new content.
pub(super) fn changed_geometry_damage(
    before: &[WindowGeometrySnapshot],
    after: &[WindowGeometrySnapshot],
) -> Vec<DamageRect> {
    let mut changed_rects = Vec::new();

    for (window_id, old_rect) in before.iter().copied() {
        match after.iter().find(|(id, _)| *id == window_id) {
            Some((_, new_rect)) if *new_rect != old_rect => {
                changed_rects.push(old_rect);
                changed_rects.push(*new_rect);
            }
            None => changed_rects.push(old_rect),
            Some(_) => {}
        }
    }

    for (window_id, new_rect) in after.iter().copied() {
        if !before.iter().any(|(id, _)| *id == window_id) {
            changed_rects.push(new_rect);
        }
    }

    changed_rects
}

#[cfg(test)]
mod tests {
    use super::{WindowGeometrySnapshot, changed_geometry_damage, shared_frame_damage};

    #[test]
    fn expanded_fullscreen_image_damages_the_entire_new_extent() {
        assert_eq!(
            shared_frame_damage((1280, 800), (0, 0, 2076, 1298), &[(0, 0, 1280, 800)]),
            [(0, 0, 2076, 1298)]
        );
    }

    #[test]
    fn scaled_partial_image_damage_repaints_its_sampled_geometry() {
        for extent in [(2076, 1298), (988, 618)] {
            assert_eq!(
                shared_frame_damage(
                    (1280, 800),
                    (20, 30, extent.0, extent.1),
                    &[(50, 60, 10, 20)]
                ),
                [(20, 30, extent.0, extent.1)]
            );
        }
    }

    #[test]
    fn unscaled_image_keeps_partial_damage_and_window_position() {
        assert_eq!(
            shared_frame_damage((1280, 800), (-20, 30, 1280, 800), &[(50, 60, 10, 20)]),
            [(30, 90, 10, 20)]
        );
    }

    #[test]
    fn moving_window_damages_only_old_and_new_geometry() {
        let before: [WindowGeometrySnapshot; 2] =
            [(1, (10, 20, 300, 200)), (2, (700, 500, 100, 100))];
        let after: [WindowGeometrySnapshot; 2] =
            [(1, (16, 24, 300, 200)), (2, (700, 500, 100, 100))];

        assert_eq!(
            changed_geometry_damage(&before, &after),
            [(10, 20, 300, 200), (16, 24, 300, 200)]
        );
    }

    #[test]
    fn moving_transient_group_damages_each_changed_window() {
        let before: [WindowGeometrySnapshot; 3] = [
            (1, (10, 20, 300, 200)),
            (2, (40, 50, 120, 80)),
            (3, (700, 500, 100, 100)),
        ];
        let after: [WindowGeometrySnapshot; 3] = [
            (1, (16, 24, 300, 200)),
            (2, (46, 54, 120, 80)),
            (3, (700, 500, 100, 100)),
        ];

        assert_eq!(
            changed_geometry_damage(&before, &after),
            [
                (10, 20, 300, 200),
                (16, 24, 300, 200),
                (40, 50, 120, 80),
                (46, 54, 120, 80),
            ]
        );
    }

    #[test]
    fn visibility_changes_damage_only_affected_geometry() {
        let before: [WindowGeometrySnapshot; 1] = [(1, (10, 20, 300, 200))];
        let after: [WindowGeometrySnapshot; 1] = [(2, (700, 500, 100, 100))];

        assert_eq!(
            changed_geometry_damage(&before, &after),
            [(10, 20, 300, 200), (700, 500, 100, 100)]
        );
    }
}
