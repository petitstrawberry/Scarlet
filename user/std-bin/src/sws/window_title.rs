//! Live window-title updates, authorized against compositor-owned metadata.

/// The exact active document window survives temporary shell/menu focus.
/// Application IDs are insufficient because one application can own several
/// documents and transient windows with independent titles and menus.
#[derive(Default)]
pub(super) struct ActiveWindow(Option<u32>);

impl ActiveWindow {
    pub(super) fn focus(&mut self, window_id: u32, active_on_focus: bool) {
        if active_on_focus {
            self.0 = Some(window_id);
        }
    }

    pub(super) fn is_active(&self, window_id: u32) -> bool {
        self.0 == Some(window_id)
    }

    pub(super) fn remove(&mut self, window_id: u32) -> bool {
        if self.is_active(window_id) {
            self.0 = None;
            true
        } else {
            false
        }
    }
}

/// Returns whether a known client's window title changed. Unknown windows and
/// windows owned by another client are rejected without modifying metadata.
pub(super) fn update(
    client_id: usize,
    window: Option<(Option<usize>, &mut Option<Vec<u8>>)>,
    title: Vec<u8>,
) -> Result<bool, ()> {
    let Some((Some(owner), current)) = window else {
        return Err(());
    };
    if owner != client_id {
        return Err(());
    }
    if current.as_deref() == Some(title.as_slice()) {
        return Ok(false);
    }
    *current = Some(title);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{ActiveWindow, update};

    #[test]
    fn background_documents_cannot_publish_or_clear_active_metadata_during_menu_focus() {
        let mut active = ActiveWindow::default();
        active.focus(7, true);
        active.focus(99, false); // A shell menu temporarily has keyboard focus.
        assert!(active.is_active(7));
        assert!(!active.is_active(8)); // Same-app background document metadata.
        assert!(!active.remove(8));
        assert!(!active.remove(99));
        assert!(active.is_active(7));
        active.focus(8, true); // Switching documents changes the exact owner.
        assert!(!active.is_active(7));
        assert!(active.is_active(8));
        assert!(!active.remove(7));
        assert!(active.remove(8));
        assert!(!active.is_active(8));
    }

    #[test]
    fn rejects_unknown_unowned_and_other_clients_windows() {
        let mut current = Some(b"original".to_vec());
        assert_eq!(update(7, None, b"changed".to_vec()), Err(()));
        assert_eq!(
            update(7, Some((None, &mut current)), b"changed".to_vec()),
            Err(())
        );
        assert_eq!(
            update(7, Some((Some(8), &mut current)), b"changed".to_vec()),
            Err(())
        );
        assert_eq!(current.as_deref(), Some(&b"original"[..]));
    }

    #[test]
    fn updates_owned_unicode_titles_and_skips_repeated_values() {
        let mut current = None;
        let title = "日本語.txt — Notepad".as_bytes().to_vec();
        assert_eq!(
            update(7, Some((Some(7), &mut current)), title.clone()),
            Ok(true)
        );
        assert_eq!(
            update(7, Some((Some(7), &mut current)), title.clone()),
            Ok(false)
        );
        assert_eq!(current, Some(title));
        assert_eq!(
            update(7, Some((Some(7), &mut current)), Vec::new()),
            Ok(true)
        );
        assert_eq!(
            update(7, Some((Some(7), &mut current)), Vec::new()),
            Ok(false)
        );
    }
}
