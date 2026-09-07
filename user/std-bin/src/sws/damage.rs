//! Output damage bookkeeping shared by SWS compositor paths.

use std::vec::Vec;

/// Screen-space rectangle represented as `(x, y, width, height)`.
pub(super) type DamageRect = (i32, i32, u32, u32);

/// `None` means full-output damage; `Some` contains bounded damage rectangles.
pub(super) type PresentDamage = Option<Vec<DamageRect>>;

/// Window identifier paired with its screen-space geometry.
pub(super) type WindowGeometrySnapshot = (u32, DamageRect);

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
    use super::{WindowGeometrySnapshot, changed_geometry_damage};

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
