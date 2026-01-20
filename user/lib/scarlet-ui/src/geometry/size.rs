#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const ZERO: Self = Self { width: 0.0, height: 0.0 };
    pub const INFINITE: Self = Self { width: f32::INFINITY, height: f32::INFINITY };

    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    pub fn clamp(&self, min: Size, max: Size) -> Size {
        Size {
            width: self.width.clamp(min.width, max.width),
            height: self.height.clamp(min.height, max.height),
        }
    }
}
