//! NavigationView - SwiftUI-style sidebar navigation with dynamic content switching
//!
//! NavigationView provides a sidebar navigation interface where users can select
//! different items to display different content views.

use core::any::Any;
use crate::view::View;
use crate::state::{State, Listenable};
use crate::views::navigation::tuple::NavigationLinkTuple;

/// NavigationView - Sidebar navigation with dynamic content switching
///
/// NavigationView provides a SwiftUI-style navigation interface with:
/// - A fixed-width sidebar containing navigation items
/// - A content area that displays the selected item's view
/// - Visual feedback for selection and hover states
///
/// # Type Parameters
///
/// * `T` - Tuple of NavigationLink items
///
/// # Important Notes
///
/// - NavigationView does NOT implement Clone (closures don't support Clone)
/// - When selected_index changes, the entire view tree is rebuilt
/// - For state preservation across navigation, use external State<T> passed to closures
///
/// # Examples
///
/// ```ignore
/// // Basic usage with macro (recommended)
/// let nav = navigation! {
///     NavigationLink::new("Home", Icon::Home, || Box::new(Text::new("Home View"))),
///     NavigationLink::new("Settings", Icon::Settings, || Box::new(Text::new("Settings View"))),
/// };
///
/// // With state preservation
/// let home_state = State::new(StateId::new(1), HomeData::default());
/// let nav = navigation! {
///     NavigationLink::new("Home", Icon::Home, || Box::new(HomeView::new(home_state.clone()))),
/// };
///
/// // With modifiers
/// let nav = navigation! {
///     NavigationLink::new("Home", Icon::Home, || Box::new(Text::new("Home"))),
/// }
/// .sidebar_width(250.0)
/// .padding(20.0);
/// ```
pub struct NavigationView<T>
where
    T: NavigationLinkTuple,
{
    /// Tuple of navigation links (stack-only, no heap allocation)
    links: T,
    /// Currently selected link index (tracked via State for reactivity)
    selected_index: State<usize>,
    /// Width of the sidebar (fixed)
    sidebar_width: f32,
}

impl<T> NavigationView<T>
where
    T: NavigationLinkTuple,
{
    /// Create a new NavigationView with the given tuple of links
    ///
    /// # Parameters
    ///
    /// * `links` - Tuple of NavigationLink items (stack-only, no Vec)
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let nav = NavigationView::new((
    ///     NavigationLink::new("Home", Icon::Home, || Box::new(Text::new("Home"))),
    ///     NavigationLink::new("Settings", Icon::Settings, || Box::new(Text::new("Settings"))),
    /// ));
    /// ```
    pub fn new(links: T) -> Self {
        let state_id = crate::state::generate_state_id();

        Self {
            links,
            selected_index: State::new(state_id, 0),
            sidebar_width: 200.0,
        }
    }

    /// Set the sidebar width
    ///
    /// # Parameters
    ///
    /// * `width` - Width of the sidebar in points
    pub fn sidebar_width(mut self, width: f32) -> Self {
        self.sidebar_width = width;
        self
    }

    /// Get the sidebar width
    pub fn get_sidebar_width(&self) -> f32 {
        self.sidebar_width
    }

    /// Get the number of navigation links
    pub fn link_count(&self) -> usize {
        self.links.count()
    }

    /// Get the label for a link at the given index
    pub fn get_label(&self, index: usize) -> &str {
        self.links.get_label(index)
    }

    /// Get the icon for a link at the given index
    pub fn get_icon(&self, index: usize) -> &crate::views::navigation::link::Icon {
        self.links.get_icon(index)
    }

    /// Get the selected index State
    pub fn selected_index_state(&self) -> &State<usize> {
        &self.selected_index
    }

    /// Get the links tuple
    pub fn links(&self) -> &T {
        &self.links
    }
}

// Clone implementation for NavigationView
// NavigationLink is Clone-able because closures are wrapped in Rc
impl<T> Clone for NavigationView<T>
where
    T: NavigationLinkTuple + Clone,
{
    fn clone(&self) -> Self {
        Self {
            links: self.links.clone(),
            selected_index: self.selected_index.clone(),
            sidebar_width: self.sidebar_width,
        }
    }
}

// View trait implementations are in view_impl.rs for each tuple size
