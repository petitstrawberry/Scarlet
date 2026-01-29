//! ScarletUI Macros - Convenient macros for creating views
//!
//! Provides syntax sugar for common UI patterns.

/// Create a VStack with children
///
/// # Examples
///
/// ```ignore
/// let stack = vstack! {
///     Text::new("Hello"),
///     Text::new("World"),
/// }
/// .spacing(10.0)
/// .alignment(Alignment::Center);
/// ```
#[macro_export]
macro_rules! vstack {
    () => {{
        $crate::views::VStack::new(())
    }};
    ($($view:expr),+ $(,)?) => {{
        $crate::views::VStack::new(($($view,)+))
    }};
}

/// Create an HStack with children
///
/// # Examples
///
/// ```ignore
/// let stack = hstack! {
///     Text::new("Left"),
///     Spacer::new(),
///     Text::new("Right"),
/// }
/// .spacing(10.0);
/// ```
#[macro_export]
macro_rules! hstack {
    () => {{
        $crate::views::HStack::new(())
    }};
    ($($view:expr),+ $(,)?) => {{
        $crate::views::HStack::new(($($view,)+))
    }};
}

/// Create a ZStack with children
///
/// # Examples
///
/// ```ignore
/// let stack = zstack! {
///     Rectangle::new().fill(Color::BLUE),
///     Text::new("Overlay"),
/// }
/// .alignment(Alignment::Center);
/// ```
#[macro_export]
macro_rules! zstack {
    () => {{
        $crate::views::ZStack::new(())
    }};
    ($($view:expr),+ $(,)?) => {{
        $crate::views::ZStack::new(($($view,)+))
    }};
}

/// Create a NavigationView with navigation links
///
/// # Examples
///
/// ```ignore
/// let nav = navigation! {
///     NavigationLink::new("Home", Icon::Home, || Text::new("Home")),
///     NavigationLink::new("Settings", Icon::Settings, || Text::new("Settings")),
/// }
/// .sidebar_width(200.0);
/// ```
#[macro_export]
macro_rules! navigation {
    ($($link:expr),* $(,)?) => {{
        $crate::views::NavigationView::new(($($link,)*))
    }};
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_vstack_macro() {
        // Test that the macro expands correctly
        let _stack = vstack!();
    }

    #[test]
    fn test_hstack_macro() {
        let _stack = hstack!();
    }

    #[test]
    fn test_zstack_macro() {
        let _stack = zstack!();
    }
}
