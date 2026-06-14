//! Test tuple-based containers

#[cfg(test)]
mod tests {
    use crate::view::View;
    use crate::views::Text;
    use crate::views::containers::{HStack, VStack, ZStack};

    #[test]
    fn test_vstack_tuple() {
        // Test creating a VStack with a 2-tuple
        let stack = VStack::new((Text::new("Hello"), Text::new("World"))).spacing(10.0);

        let element = stack.create_element();
        assert!(!element.size().is_zero());
    }

    #[test]
    fn test_vstack_empty() {
        // Test creating an empty VStack
        let stack = VStack::new(());
        let element = stack.create_element();
        assert!(!element.size().is_zero());
    }

    #[test]
    fn test_hstack_tuple() {
        // Test creating an HStack with a 3-tuple
        let stack =
            HStack::new((Text::new("Left"), Text::new("Middle"), Text::new("Right"))).spacing(5.0);

        let element = stack.create_element();
        assert!(!element.size().is_zero());
    }

    #[test]
    fn test_zstack_tuple() {
        // Test creating a ZStack with a 2-tuple
        let stack = ZStack::new((Text::new("Background"), Text::new("Foreground")));

        let element = stack.create_element();
        assert!(!element.size().is_zero());
    }
}
