//! Native menu surfaces use the same retained paint as the Web menu host.
//! SWS supplies the desktop backdrop; the client paints tint, rim and labels.

use scarlet_ui::buffer::Buffer;
use scarlet_ui::color::Color;
use scarlet_ui::element::{ElementRenderObject, LayoutConstraints};
use scarlet_ui::geometry::{Point, Size};
use scarlet_ui::graphics;
use scarlet_ui::renderer::{CpuPaintRenderer, PaintCommand, PaintContext};
use scarlet_ui::views::MenuItemContent;
use scarlet_ui::views::menu::MenuRenderObject;
use sws_protocol::{WindowGeometry, surface_regions};

pub struct PopupMenuRenderer {
    menu: MenuRenderObject,
    painter: CpuPaintRenderer,
    origin: Point,
    size: Size,
    scale_milli: u32,
}

impl PopupMenuRenderer {
    /// Build a menu at least `min_width` wide that grows to fit its widest
    /// label/shortcut row, capped at `max_width` (the usable screen width).
    pub fn new(
        items: Vec<MenuItemContent>,
        item_height: f32,
        min_width: f32,
        max_width: f32,
        scale_milli: u32,
    ) -> Self {
        graphics::set_current_scale_milli(scale_milli);
        // Measure with the same metrics the menu paints with so labels and
        // right-aligned shortcuts never overlap or get clipped.
        let preferred = MenuRenderObject::new(items.clone(), item_height, min_width)
            .preferred_width()
            .ceil();
        let width = preferred.max(min_width).min(max_width.max(min_width));
        let mut menu = MenuRenderObject::new(items, item_height, width);
        let size = menu.layout(LayoutConstraints {
            min_width: width,
            max_width: width,
            min_height: 0.0,
            max_height: f32::INFINITY,
        });
        let outsets = menu.paint_outsets();
        let origin = Point::new(outsets.left.ceil(), outsets.top.ceil());
        let surface_size = Size::new(
            origin.x + size.width.ceil() + outsets.right.ceil(),
            origin.y + size.height.ceil() + outsets.bottom.ceil(),
        );
        let painter = CpuPaintRenderer::new(surface_size, scale_milli, Color::TRANSPARENT);
        let mut renderer = Self {
            menu,
            painter,
            origin,
            size,
            scale_milli,
        };
        renderer.render();
        renderer
    }

    /// Visible menu extent; shadow padding does not affect popup anchoring.
    pub fn size(&self) -> Size {
        self.size
    }

    pub fn physical_size(&self) -> (u32, u32) {
        (
            self.painter.buffer().width(),
            self.painter.buffer().height(),
        )
    }

    #[cfg(test)]
    pub fn origin(&self) -> Point {
        self.origin
    }

    fn pixels(&self, value: f32) -> u32 {
        (value * self.scale_milli.max(1) as f32 / 1000.0)
            .round()
            .max(0.0) as u32
    }

    pub fn window_geometry(&self) -> WindowGeometry {
        WindowGeometry {
            x: self.pixels(self.origin.x) as i32,
            y: self.pixels(self.origin.y) as i32,
            width: self.pixels(self.size.width).max(1),
            height: self.pixels(self.size.height).max(1),
        }
    }

    /// Derive compositor clipping and diffusion from shared menu paint rather
    /// than maintaining separate native corner and material constants.
    pub fn surface_regions(&self) -> Vec<surface_regions::SurfaceRegion> {
        let mut context = PaintContext::new();
        self.menu.paint(&mut context, self.origin);
        context
            .commands()
            .iter()
            .filter_map(|command| {
                let (rect, corner, blur) = match command {
                    PaintCommand::BackdropFilterRoundedRect {
                        rect,
                        corner_radius,
                        blur_radius,
                        ..
                    } => (*rect, *corner_radius, *blur_radius),
                    // Tint-only quality still needs rounded input geometry.
                    PaintCommand::FillRoundedRect {
                        rect,
                        corner_radius,
                        ..
                    } if rect.origin == self.origin && rect.size == self.size => {
                        (*rect, *corner_radius, 0.0)
                    }
                    _ => return None,
                };
                let width = self.pixels(rect.size.width).max(1);
                let height = self.pixels(rect.size.height).max(1);
                Some(surface_regions::SurfaceRegion {
                    x: self.pixels(rect.origin.x) as i32,
                    y: self.pixels(rect.origin.y) as i32,
                    width,
                    height,
                    corner_radius: self.pixels(corner).min(width.min(height) / 2),
                    blur_radius: self.pixels(blur).min(64),
                    flags: surface_regions::INPUT
                        | if blur > 0.0 {
                            surface_regions::BACKDROP
                        } else {
                            0
                        },
                })
            })
            .collect()
    }

    pub fn handle_move(&mut self, x: f32, y: f32) -> bool {
        let hovered = self.menu.hit_test(x - self.origin.x, y - self.origin.y);
        if hovered != self.menu.hovered() {
            self.menu.set_hovered(hovered);
            true
        } else {
            false
        }
    }

    pub fn handle_click(&self, x: f32, y: f32) {
        if let Some(index) = self.menu.hit_test(x - self.origin.x, y - self.origin.y) {
            self.menu.invoke_item(index);
        }
    }

    pub fn render(&mut self) {
        graphics::set_current_scale_milli(self.scale_milli);
        let mut context = PaintContext::new();
        self.menu.paint(&mut context, self.origin);
        self.painter.execute(&context);
    }

    pub fn buffer(&self) -> Option<&Buffer> {
        Some(self.painter.buffer())
    }
}
