//! Focus state transfer helpers for element rebuilds.

use alloc::vec::Vec;

use crate::element::Element;
use crate::event::{Event, FocusEvent, Phase};

/// Return the child-index path to the currently focused descendant.
pub(crate) fn focused_descendant_path(element: &dyn Element) -> Option<Vec<usize>> {
    if element.wants_keyboard_focus() {
        return Some(Vec::new());
    }

    for (index, child) in element.children().iter().enumerate() {
        if let Some(mut path) = focused_descendant_path(child.as_ref()) {
            path.insert(0, index);
            return Some(path);
        }
    }

    None
}

/// Restore focus to a descendant at the supplied child-index path.
pub(crate) fn restore_focus_at_path(element: &mut dyn Element, path: &[usize]) -> bool {
    if path.is_empty() {
        if !element.accepts_keyboard_focus() {
            return false;
        }
        return element.handle_event(&Event::Focus(FocusEvent::Gained), Phase::Target);
    }

    let Some(child) = element.children_mut().get_mut(path[0]) else {
        return false;
    };
    restore_focus_at_path(child.as_mut(), &path[1..])
}
