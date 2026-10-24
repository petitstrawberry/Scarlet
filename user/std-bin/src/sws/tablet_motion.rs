//! Compositor-only tablet scene interpolation. Never changes client geometry.
use crate::window::PresentationTransform;

pub(super) struct Motion {
    pub starts: Vec<(u32, PresentationTransform)>,
    pub progress: f32,
    pub started_ns: Option<u64>,
}

impl Motion {
    pub fn advance(&mut self, now: u64) -> bool {
        let Some(start) = self.started_ns else {
            return false;
        };
        let t = (now.saturating_sub(start) as f32 / 220_000_000.0).clamp(0.0, 1.0);
        self.progress = 1.0 - (1.0 - t).powi(3);
        t >= 1.0
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
        let mut motion = Motion {
            starts: vec![(1, full)],
            progress: 0.0,
            started_ns: None,
        };
        assert_eq!(motion.project(1, card), full);
        motion.progress = 0.5;
        assert_eq!(motion.project(1, card).width, 840);
        motion.progress = 1.0;
        assert_eq!(motion.project(1, card), card);
        motion.started_ns = Some(100);
        assert!(!motion.advance(110_000_100));
        assert!(motion.progress > 0.5);
        assert!(motion.advance(220_000_100));
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
        let motion = Motion {
            starts: vec![],
            progress: 0.5,
            started_ns: None,
        };
        assert_eq!(motion.project(2, card).opacity, 0.5);
        assert_eq!(motion.project(2, card).width, card.width);
    }
}
