//! Serialize display changes until a frame at the requested size is presented.
type Size = (u32, u32);

#[derive(Default)]
pub struct Resize {
    expected: Option<Size>,
    pending: Option<Size>,
    settle_ticks: u8,
}

impl Resize {
    pub fn queue(&mut self, size: Size) {
        self.pending = Some(size);
        self.settle_ticks = 15;
    }

    pub fn next(&mut self, current: Size) -> Option<Size> {
        if self.settle_ticks != 0 {
            self.settle_ticks -= 1;
            return None;
        }
        if self.expected.is_some() {
            return None;
        }
        let size = self.pending.take()?;
        if size == current {
            return None;
        }
        self.expected = Some(size);
        Some(size)
    }

    pub fn presented(&mut self, size: Size) {
        // Old scanout frames may arrive after the control request. They are
        // still drawable, but cannot acknowledge it or restore old geometry.
        if self.expected == Some(size) {
            self.expected = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(resize: &mut Resize, current: Size) {
        for _ in 0..15 {
            assert_eq!(resize.next(current), None);
        }
    }

    #[test]
    fn old_frames_do_not_consume_resize_or_release_next_request() {
        let old = (550, 734);
        let maximized = (1256, 714);
        let intermediate = (845, 541);
        let mut resize = Resize::default();
        resize.queue(maximized);
        settle(&mut resize, old);
        assert_eq!(resize.next(old), Some(maximized));
        resize.queue(intermediate);
        settle(&mut resize, old);
        for _ in 0..3 {
            resize.presented(old);
            assert_eq!(resize.next(old), None);
        }
        resize.presented(maximized);
        // A newer window geometry must also survive the first matching frame.
        assert_eq!(resize.next(maximized), Some(intermediate));
        resize.presented(maximized);
        assert_eq!(resize.next(maximized), None);
        resize.presented(intermediate);
        resize.queue(old);
        settle(&mut resize, intermediate);
        assert_eq!(resize.next(intermediate), Some(old));
    }

    #[test]
    fn drag_coalesces_and_return_to_current_cannot_cancel_inflight_change() {
        let old = (550, 734);
        let mut resize = Resize::default();
        resize.queue((800, 500));
        resize.queue((845, 541));
        settle(&mut resize, old);
        assert_eq!(resize.next(old), Some((845, 541)));
        resize.queue(old);
        settle(&mut resize, old);
        assert_eq!(resize.next(old), None);
        resize.presented((845, 541));
        assert_eq!(resize.next((845, 541)), Some(old));
        resize.presented(old);
        resize.queue(old);
        settle(&mut resize, old);
        assert_eq!(resize.next(old), None);
    }
}
