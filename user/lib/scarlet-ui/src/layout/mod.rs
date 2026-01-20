use crate::geometry::Size;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LayoutConstraints {
    pub min: Size,
    pub max: Size,
}

impl LayoutConstraints {
    pub const fn tight(size: Size) -> Self {
        Self { min: size, max: size }
    }

    pub const fn loose(max: Size) -> Self {
        Self { min: Size::ZERO, max }
    }

    pub const fn unconstrained() -> Self {
        Self {
            min: Size::ZERO,
            max: Size::INFINITE,
        }
    }

    pub fn clamp(&self, size: Size) -> Size {
        size.clamp(self.min, self.max)
    }

    pub fn satisfies(&self, size: Size) -> bool {
        size.width >= self.min.width
            && size.width <= self.max.width
            && size.height >= self.min.height
            && size.height <= self.max.height
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Alignment {
    #[default]
    Start,
    Center,
    End,
    Stretch,
}
