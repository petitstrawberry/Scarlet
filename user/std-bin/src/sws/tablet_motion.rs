//! Retained compositor scene interpolation. Never changes client geometry.
use crate::window::PresentationTransform;

const SETTLE_DURATION_NS: u64 = 140_000_000;
const DRAWER_DURATION_NS: u64 = 120_000_000;

pub(super) struct Motion {
    pub starts: Vec<(u32, PresentationTransform)>,
    pub progress: f32,
    pub started_ns: Option<u64>,
    duration_ns: u64,
    first_frame_pending: bool,
}

impl Motion {
    /// Direct manipulation remains available without a GPU and has no timer.
    pub fn drag(starts: Vec<(u32, PresentationTransform)>) -> Self {
        Self {
            starts,
            progress: 0.0,
            started_ns: None,
            duration_ns: SETTLE_DURATION_NS,
            first_frame_pending: false,
        }
    }

    pub fn settle(
        starts: Vec<(u32, PresentationTransform)>,
        now: u64,
        accelerated: bool,
    ) -> Option<Self> {
        accelerated.then(|| Self {
            started_ns: Some(now),
            first_frame_pending: true,
            ..Self::drag(starts)
        })
    }

    /// Reveal an already rendered transparent drawer with a short slide/fade.
    /// Its separate backdrop stays stationary. No client resize or animation
    /// redraw is required: only this retained surface is transformed.
    pub fn drawer(
        id: u32,
        target: PresentationTransform,
        scale_milli: u32,
        now: u64,
        accelerated: bool,
    ) -> Option<Self> {
        let offset = (24u64 * scale_milli.max(1) as u64 / 1000).max(1) as i32;
        let start = PresentationTransform {
            y: target.y.saturating_add(offset),
            opacity: 0.0,
            ..target
        };
        Self::settle(vec![(id, start)], now, accelerated).map(|motion| Self {
            duration_ns: DRAWER_DURATION_NS,
            ..motion
        })
    }

    pub fn contains(&self, id: u32) -> bool {
        self.starts.iter().any(|(candidate, _)| *candidate == id)
    }

    /// Start elapsed animation time only once its first frame reached output.
    /// A slow shell redraw or GPU submission must not consume the entire reveal.
    pub fn note_presented(&mut self, now: u64) {
        if self.first_frame_pending {
            self.first_frame_pending = false;
            self.started_ns = Some(now);
        }
    }

    pub fn advance(&mut self, now: u64) -> bool {
        if self.first_frame_pending {
            return false;
        }
        let Some(start) = self.started_ns else {
            return false;
        };
        let elapsed = now.saturating_sub(start);
        let t = (elapsed as f32 / self.duration_ns as f32).clamp(0.0, 1.0);
        self.progress = 1.0 - (1.0 - t).powi(3);
        elapsed >= self.duration_ns
    }

    pub fn project(&self, id: u32, target: PresentationTransform) -> PresentationTransform {
        let from = self
            .starts
            .iter()
            .find(|(candidate, _)| *candidate == id)
            .map(|(_, rect)| *rect)
            .unwrap_or(PresentationTransform {
                opacity: 0.0,
                ..target
            });
        let mix = |a: f32, b: f32| a + (b - a) * self.progress.clamp(0.0, 1.0);
        PresentationTransform {
            x: mix(from.x as f32, target.x as f32).round() as i32,
            y: mix(from.y as f32, target.y as f32).round() as i32,
            width: (mix(from.width as f32, target.width as f32).round() as u32).max(1),
            height: (mix(from.height as f32, target.height as f32).round() as u32).max(1),
            opacity: mix(from.opacity, target.opacity),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tablet_motion_preserves_exact_endpoints_and_interpolates_without_surface_resize() {
        let full = PresentationTransform {
            x: 0,
            y: 80,
            width: 1280,
            height: 720,
            opacity: 1.0,
        };
        let card = PresentationTransform {
            x: 40,
            y: 120,
            width: 400,
            height: 225,
            opacity: 1.0,
        };
        let mut motion = Motion::drag(vec![(1, full)]);
        assert_eq!(motion.project(1, card), full);
        motion.progress = 0.5;
        assert_eq!(motion.project(1, card).width, 840);
        motion.progress = 1.0;
        assert_eq!(motion.project(1, card), card);
        motion.started_ns = Some(100);
        assert!(!motion.advance(70_000_100));
        assert!(motion.progress > 0.5);
        assert!(motion.advance(140_000_100));
        assert_eq!(motion.project(1, card), card);
    }
    #[test]
    fn tablet_motion_newly_visible_cards_fade_in() {
        let card = PresentationTransform {
            x: 40,
            y: 120,
            width: 400,
            height: 225,
            opacity: 1.0,
        };
        let mut motion = Motion::drag(vec![]);
        motion.progress = 0.5;
        assert_eq!(motion.project(2, card).opacity, 0.5);
        assert_eq!(motion.project(2, card).width, card.width);
    }

    #[test]
    fn cpu_transitions_are_immediate_but_direct_drag_is_not_timed() {
        let mut drag = Motion::drag(vec![]);
        drag.progress = 0.4;
        assert!(!drag.advance(u64::MAX));
        assert_eq!(drag.progress, 0.4);
        assert!(Motion::settle(vec![], 100, false).is_none());
        let target = PresentationTransform {
            x: 0,
            y: 0,
            width: 1200,
            height: 800,
            opacity: 1.0,
        };
        assert!(Motion::drawer(1, target, 1000, 100, false).is_none());
    }

    #[test]
    fn laptop_overview_can_reverse_from_the_displayed_transform_without_resizing() {
        let mut window = crate::window::Window::new(1, 80, 48, 900, 600);
        let geometry = window.surface_geometry();
        let full = PresentationTransform {
            x: geometry.0,
            y: geometry.1,
            width: geometry.2,
            height: geometry.3,
            opacity: 1.0,
        };
        let card = PresentationTransform {
            x: 120,
            y: 90,
            width: 450,
            height: 300,
            opacity: 1.0,
        };
        let mut opening = Motion::settle(vec![(1, full)], 100, true).unwrap();
        opening.note_presented(100);
        assert!(!opening.advance(70_000_100));
        let halfway = opening.project(1, card);
        assert!(halfway.width < full.width && halfway.width > card.width);
        window.presentation_transform = Some(halfway);
        assert_eq!(window.surface_geometry(), geometry);
        let mut closing = Motion::settle(vec![(1, halfway)], 70_000_100, true).unwrap();
        closing.note_presented(70_000_100);
        assert_eq!(closing.project(1, full), halfway);
        assert!(!closing.advance(210_000_099));
        assert!(closing.advance(210_000_100));
        assert_eq!(closing.project(1, full), full);
        assert_eq!(window.surface_geometry(), geometry);
    }

    #[test]
    fn drawer_slide_is_scaled_and_finishes_in_120_ms_without_resizing() {
        let target = PresentationTransform {
            x: 0,
            y: 0,
            width: 2400,
            height: 1600,
            opacity: 1.0,
        };
        let mut motion = Motion::drawer(7, target, 2000, 1_000, true).unwrap();
        assert!(motion.contains(7));
        assert!(!motion.contains(8));
        let start = motion.project(7, target);
        assert_eq!(start.y, 48);
        assert_eq!(start.opacity, 0.0);
        // Even a late first frame must start at the first visible position.
        assert!(!motion.advance(500_001_000));
        assert_eq!(motion.project(7, target), start);
        motion.note_presented(500_001_000);
        assert!(!motion.advance(560_001_000));
        let middle = motion.project(7, target);
        assert!(middle.y > 0 && middle.y < 48);
        assert!(middle.opacity > 0.0 && middle.opacity < 1.0);
        assert_eq!((middle.width, middle.height), (target.width, target.height));
        assert!(!motion.advance(620_000_999));
        assert!(motion.advance(620_001_000));
        assert_eq!(motion.project(7, target), target);
    }
}
