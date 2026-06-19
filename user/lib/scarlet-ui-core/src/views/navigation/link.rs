//! NavigationLink - Data structure for navigation items
//!
//! NavigationLink represents a single navigation item in a NavigationView.
//! It is NOT a View - it's a data structure that holds label, icon, and
//! a closure to build the content view.

use crate::view::View;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;

/// Icon type for navigation items
///
/// Minimal geometric icons for MVP. Can be extended with more icons as needed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Icon {
    /// Home icon - house shape
    Home,
    /// Settings icon - gear shape
    Settings,
    /// Info icon - circle with 'i'
    Info,
    /// Search icon - magnifying glass
    Search,
    /// User/Profile icon - person shape
    User,
    /// File/Document icon
    File,
    /// Folder icon
    Folder,
}

/// Navigation link data structure (NOT a View)
///
/// NavigationLink holds the information needed for a single navigation item:
/// - A display label
/// - An optional icon
/// - A closure (wrapped in Rc) that builds the content view when this link is selected
///
/// The closure is wrapped in Rc to allow NavigationLink (and thus NavigationView) to be Clone-able.
/// This is necessary to work with ScarletUI's RenderElement architecture.
///
/// # Type Parameters
///
/// * `V` - View type that the closure returns
///
/// # Examples
///
/// ```ignore
/// let link = NavigationLink::new("Home", Icon::Home, || Text::new("Welcome to Home"));
/// ```
pub struct NavigationLink {
    label: String,
    icon: Icon,
    /// The closure that builds the content view, wrapped in Rc for Clone-ability
    pub(crate) content_builder: Rc<dyn Fn() -> Box<dyn View>>,
}

impl Clone for NavigationLink {
    fn clone(&self) -> Self {
        Self {
            label: self.label.clone(),
            icon: self.icon,
            content_builder: Rc::clone(&self.content_builder),
        }
    }
}

impl NavigationLink {
    /// Create a new NavigationLink
    ///
    /// # Parameters
    ///
    /// * `label` - Display text for the navigation item
    /// * `icon` - Icon to display next to the label
    /// * `content_builder` - Closure that builds the content view when selected (boxed internally)
    ///
    /// # Examples
    ///
    /// ```ignore
    /// NavigationLink::new("Settings", Icon::Settings, || SettingsView::new())
    /// ```
    pub fn new<V>(
        label: impl Into<String>,
        icon: Icon,
        content_builder: impl Fn() -> V + 'static,
    ) -> Self
    where
        V: View + 'static,
    {
        let builder = move || -> Box<dyn View> { Box::new(content_builder()) };
        Self {
            label: label.into(),
            icon,
            content_builder: Rc::new(builder),
        }
    }

    /// Get the label text
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Get the icon
    pub fn icon(&self) -> &Icon {
        &self.icon
    }

    /// Build the content view
    ///
    /// This invokes the stored closure to create the content view.
    pub fn build_content(&self) -> Box<dyn View> {
        // Call the Fn through Rc
        (self.content_builder)()
    }
}
