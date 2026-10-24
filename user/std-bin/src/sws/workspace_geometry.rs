//! App allocations depend only on shell workarea and retained workspace layout.
//! Keyboard occlusion is deliberately not an input to this allocator.
use sws_protocol::workspace::TabletLayout;

pub(super) fn tablet_slot(
    layout: TabletLayout,
    window_id: u32,
    (x, y, width, height): (i32, i32, u32, u32),
) -> Option<(i32, i32, u32, u32)> {
    let divider = 8u32.min(width.saturating_sub(2));
    match layout {
        sws_protocol::workspace::TabletLayout::Empty => None,
        sws_protocol::workspace::TabletLayout::Single {
            window_id: presented,
        } => (presented == window_id).then_some((x, y, width.max(1), height.max(1))),
        sws_protocol::workspace::TabletLayout::Split {
            axis,
            first_window_id,
            second_window_id,
            ratio_milli,
        } => match axis {
            sws_protocol::workspace::SplitAxis::Horizontal => {
                let available = width.saturating_sub(divider).max(2);
                let first_width = ((u64::from(available) * u64::from(ratio_milli)) / 1000)
                    .clamp(1, u64::from(available.saturating_sub(1)))
                    as u32;
                let second_width = available.saturating_sub(first_width).max(1);
                if first_window_id == window_id {
                    Some((x, y, first_width, height.max(1)))
                } else if second_window_id == window_id {
                    Some((
                        x.saturating_add(first_width.saturating_add(divider) as i32),
                        y,
                        second_width,
                        height.max(1),
                    ))
                } else {
                    None
                }
            }
            sws_protocol::workspace::SplitAxis::Vertical => {
                let available = height.saturating_sub(divider).max(2);
                let first_height = ((u64::from(available) * u64::from(ratio_milli)) / 1000)
                    .clamp(1, u64::from(available.saturating_sub(1)))
                    as u32;
                let second_height = available.saturating_sub(first_height).max(1);
                if first_window_id == window_id {
                    Some((x, y, width.max(1), first_height))
                } else if second_window_id == window_id {
                    Some((
                        x,
                        y.saturating_add(first_height.saturating_add(divider) as i32),
                        width.max(1),
                        second_height,
                    ))
                } else {
                    None
                }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sws_protocol::workspace::SplitAxis;

    #[test]
    fn single_scene_uses_full_shell_workarea() {
        let layout = TabletLayout::Single { window_id: 7 };
        assert_eq!(
            tablet_slot(layout, 7, (0, 32, 1200, 868)),
            Some((0, 32, 1200, 868))
        );
        assert_eq!(tablet_slot(layout, 8, (0, 32, 1200, 868)), None);
    }

    #[test]
    fn horizontal_and_vertical_splits_keep_origin_gap_and_ratio() {
        let layout = TabletLayout::Split {
            axis: SplitAxis::Horizontal,
            first_window_id: 1,
            second_window_id: 2,
            ratio_milli: 500,
        };
        assert_eq!(
            tablet_slot(layout, 1, (0, 32, 1200, 868)),
            Some((0, 32, 596, 868))
        );
        assert_eq!(
            tablet_slot(layout, 2, (0, 32, 1200, 868)),
            Some((604, 32, 596, 868))
        );
        let layout = TabletLayout::Split {
            axis: SplitAxis::Vertical,
            first_window_id: 1,
            second_window_id: 2,
            ratio_milli: 500,
        };
        assert_eq!(
            tablet_slot(layout, 1, (10, 32, 1200, 868)),
            Some((10, 32, 1200, 430))
        );
        assert_eq!(
            tablet_slot(layout, 2, (10, 32, 1200, 868)),
            Some((10, 470, 1200, 430))
        );
    }
}
