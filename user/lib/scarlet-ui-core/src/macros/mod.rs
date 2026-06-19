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

/// Compose top-level application scenes.
///
/// # Examples
///
/// ```ignore
/// impl Application for MyApp {
///     fn scenes(&self) -> impl Scene {
///         scenes! {
///             Window::new("Main", self.main_view()),
///             Window::new("Inspector", self.inspector_view())
///                 .scene_key("inspector")
///                 .open_at_launch(false),
///         }
///     }
/// }
/// ```
#[macro_export]
macro_rules! scenes {
    () => {{
        ()
    }};
    ($scene:expr $(,)?) => {{
        $scene
    }};
    ($first:expr, $($rest:expr),+ $(,)?) => {{
        ($first, $crate::scenes!($($rest),+))
    }};
}

/// Conditional view branching for `if`/`else if`/`else` chains.
///
/// Each branch is wrapped in the appropriate `Either` variant automatically.
///
/// # Examples
///
/// ```ignore
/// if_view!(show, settings(), canvas())
///
/// if_view! {
///     page == 0 => home(),
///     page == 1 => settings(),
///     else => about(),
/// }
/// ```
#[macro_export]
macro_rules! if_view {
    ($cond:expr, $a:expr, $b:expr) => {
        if $cond {
            $crate::views::Either::A($a)
        } else {
            $crate::views::Either::B($b)
        }
    };
    ($c1:expr => $a:expr, $c2:expr => $b:expr, else => $c:expr $(,)?) => {
        if $c1 {
            $crate::views::Either3::A($a)
        } else if $c2 {
            $crate::views::Either3::B($b)
        } else {
            $crate::views::Either3::C($c)
        }
    };
    ($c1:expr => $a:expr, $c2:expr => $b:expr, $c3:expr => $c:expr, else => $d:expr $(,)?) => {
        if $c1 {
            $crate::views::Either4::A($a)
        } else if $c2 {
            $crate::views::Either4::B($b)
        } else if $c3 {
            $crate::views::Either4::C($c)
        } else {
            $crate::views::Either4::D($d)
        }
    };
    ($c1:expr => $a:expr, $c2:expr => $b:expr, $c3:expr => $c:expr, $c4:expr => $d:expr, else => $e:expr $(,)?) => {
        if $c1 {
            $crate::views::Either5::A($a)
        } else if $c2 {
            $crate::views::Either5::B($b)
        } else if $c3 {
            $crate::views::Either5::C($c)
        } else if $c4 {
            $crate::views::Either5::D($d)
        } else {
            $crate::views::Either5::E($e)
        }
    };
}

/// Conditional view branching for `match` expressions.
///
/// Each arm is wrapped in the appropriate `Either` variant automatically.
///
/// # Examples
///
/// ```ignore
/// match_view!(show, {
///     true => settings(),
///     false => canvas(),
/// })
///
/// match_view!(page, {
///     0 => home(),
///     1 => settings(),
///     _ => about(),
/// })
/// ```
#[macro_export]
macro_rules! match_view {
    ($scrut:expr, {
        $p1:pat => $e1:expr,
        $p2:pat => $e2:expr $(,)?
    }) => {
        match $scrut {
            $p1 => $crate::views::Either::A($e1),
            $p2 => $crate::views::Either::B($e2),
        }
    };
    ($scrut:expr, {
        $p1:pat => $e1:expr,
        $p2:pat => $e2:expr,
        $p3:pat => $e3:expr $(,)?
    }) => {
        match $scrut {
            $p1 => $crate::views::Either3::A($e1),
            $p2 => $crate::views::Either3::B($e2),
            $p3 => $crate::views::Either3::C($e3),
        }
    };
    ($scrut:expr, {
        $p1:pat => $e1:expr,
        $p2:pat => $e2:expr,
        $p3:pat => $e3:expr,
        $p4:pat => $e4:expr $(,)?
    }) => {
        match $scrut {
            $p1 => $crate::views::Either4::A($e1),
            $p2 => $crate::views::Either4::B($e2),
            $p3 => $crate::views::Either4::C($e3),
            $p4 => $crate::views::Either4::D($e4),
        }
    };
    ($scrut:expr, {
        $p1:pat => $e1:expr,
        $p2:pat => $e2:expr,
        $p3:pat => $e3:expr,
        $p4:pat => $e4:expr,
        $p5:pat => $e5:expr $(,)?
    }) => {
        match $scrut {
            $p1 => $crate::views::Either5::A($e1),
            $p2 => $crate::views::Either5::B($e2),
            $p3 => $crate::views::Either5::C($e3),
            $p4 => $crate::views::Either5::D($e4),
            $p5 => $crate::views::Either5::E($e5),
        }
    };
    ($scrut:expr, {
        $p1:pat => $e1:expr,
        $p2:pat => $e2:expr,
        $p3:pat => $e3:expr,
        $p4:pat => $e4:expr,
        $p5:pat => $e5:expr,
        $p6:pat => $e6:expr $(,)?
    }) => {
        match $scrut {
            $p1 => $crate::views::Either6::A($e1),
            $p2 => $crate::views::Either6::B($e2),
            $p3 => $crate::views::Either6::C($e3),
            $p4 => $crate::views::Either6::D($e4),
            $p5 => $crate::views::Either6::E($e5),
            $p6 => $crate::views::Either6::F($e6),
        }
    };
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
        static NAVIGATION_STATE_KEY: u8 = 0;
        let state_key = (&NAVIGATION_STATE_KEY as *const u8) as usize;
        $crate::views::NavigationView::new_with_state_key(($($link,)*), state_key)
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
