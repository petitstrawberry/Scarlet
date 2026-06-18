//! Basic ScarletUI Tests
//!
//! Unit tests for core ScarletUI components.

use scarlet_ui::buffer::Buffer;
use scarlet_ui::color::Color;
use scarlet_ui::element::{ElementTree, LayoutConstraints};
use scarlet_ui::geometry::{Point, Rect, Size};
use scarlet_ui::state::{State, StateId};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_size_creation() {
        let size = Size::new(100.0, 200.0);
        assert_eq!(size.width, 100.0);
        assert_eq!(size.height, 200.0);
    }

    #[test]
    fn test_point_creation() {
        let point = Point::new(10.0, 20.0);
        assert_eq!(point.x, 10.0);
        assert_eq!(point.y, 20.0);
    }

    #[test]
    fn test_rect_contains() {
        let rect = Rect::new(Point::new(0.0, 0.0), Size::new(100.0, 100.0));

        // Point inside
        assert!(rect.contains(Point::new(50.0, 50.0)));

        // Point on boundary
        assert!(rect.contains(Point::new(0.0, 0.0)));
        assert!(!rect.contains(Point::new(100.0, 100.0)));

        // Point outside
        assert!(!rect.contains(Point::new(101.0, 50.0)));
        assert!(!rect.contains(Point::new(50.0, 101.0)));
    }

    #[test]
    fn test_color_creation() {
        let red = Color::rgb(255, 0, 0);
        let bgra = red.to_bgra();
        // Blue channel should be 0 (BGRA format)
        assert_eq!((bgra & 0xFF) as u8, 0);
        // Green channel should be 0
        assert_eq!(((bgra >> 8) & 0xFF) as u8, 0);
        // Red channel should be 255
        assert_eq!(((bgra >> 16) & 0xFF) as u8, 255);
    }

    #[test]
    fn test_state_creation() {
        let state = State::new(StateId::new(1), 42);
        assert_eq!(state.get(), 42);

        state.set(100);
        assert_eq!(state.get(), 100);
    }

    #[test]
    fn test_state_cloning() {
        let state1 = State::new(StateId::new(1), 10);
        let state2 = state1.clone();

        state1.set(20);

        // Both should point to the same underlying state
        assert_eq!(state1.get(), 20);
        assert_eq!(state2.get(), 20);
    }

    #[test]
    fn test_buffer_creation() {
        let buffer = Buffer::new(Size::new(100.0, 100.0));
        assert_eq!(buffer.width(), 100);
        assert_eq!(buffer.height(), 100);
        assert_eq!(buffer.as_slice().len(), 100 * 100);
    }

    #[test]
    fn test_buffer_clear() {
        let mut buffer = Buffer::new(Size::new(10.0, 10.0));
        let red = Color::rgb(255, 0, 0);

        buffer.clear(red);

        // Check all pixels are red
        let pixel = red.to_bgra();
        for &p in buffer.as_slice() {
            assert_eq!(p, pixel);
        }
    }

    #[test]
    fn test_layout_constraints() {
        // Tight constraint
        let tight = LayoutConstraints::tight(100.0, 100.0);
        assert_eq!(tight.min_width, 100.0);
        assert_eq!(tight.max_width, 100.0);
        assert_eq!(tight.min_height, 100.0);
        assert_eq!(tight.max_height, 100.0);

        // Loose constraint
        let loose = LayoutConstraints::loose(200.0, 200.0);
        assert_eq!(loose.min_width, 0.0);
        assert_eq!(loose.max_width, 200.0);
        assert_eq!(loose.min_height, 0.0);
        assert_eq!(loose.max_height, 200.0);
    }

    #[test]
    fn test_element_tree_creation() {
        let tree = ElementTree::new();
        assert!(tree.root().is_none());
    }

    #[test]
    fn test_text_view_creation() {
        use scarlet_ui::views::Text;

        let text = Text::new("Hello, ScarletUI!")
            .font_size(24.0)
            .color(Color::BLACK);

        assert_eq!(text.content(), "Hello, ScarletUI!");
        assert_eq!(text.font_size_value(), 24.0);
    }

    #[test]
    fn test_rectangle_view_creation() {
        use scarlet_ui::views::Rectangle;

        let rect = Rectangle::new()
            .fill(Color::rgb(128, 128, 128))
            .corner_radius(5.0);

        assert_eq!(rect.get_color(), Color::rgb(128, 128, 128));
        assert_eq!(rect.get_corner_radius(), 5.0);
    }

    #[test]
    fn test_window_view_creation() {
        use scarlet_ui::views::Text;
        use scarlet_ui::views::Window;

        let window = Window::new("Test Window", Text::new("Content")).size(Size::new(800.0, 600.0));

        assert_eq!(window.get_title(), "Test Window");
    }

    #[test]
    fn test_padding_modifier() {
        use scarlet_ui::views::Text;
        use scarlet_ui::views::modifiers::Padding;

        let text = Text::new("Padded Text");
        let padded = Padding::new(text, 16.0);

        assert_eq!(padded.insets().top, 16.0);
        assert_eq!(padded.insets().bottom, 16.0);
        assert_eq!(padded.insets().left, 16.0);
        assert_eq!(padded.insets().right, 16.0);
    }

    #[test]
    fn test_frame_modifier() {
        use scarlet_ui::views::Text;
        use scarlet_ui::views::modifiers::Frame;

        let text = Text::new("Framed Text");
        let framed = Frame::new(text, 200.0, 100.0);

        assert_eq!(framed.width_value(), Some(200.0));
        assert_eq!(framed.height_value(), Some(100.0));
    }

    #[test]
    fn test_spacer_creation() {
        use scarlet_ui::views::Spacer;

        let _spacer = Spacer::new();
    }
}
