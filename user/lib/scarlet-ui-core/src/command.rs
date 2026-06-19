//! Application command queue for scene-level window actions.

use alloc::vec::Vec;

use crate::os::Mutex;
use crate::scene::SceneWindowKey;

/// Scene-level command emitted by user callbacks and consumed by the runner.
pub enum ApplicationCommand {
    /// Open the single runtime instance for a declared scene window.
    OpenWindow(SceneWindowKey),
    /// Dismiss the runtime instance for a declared scene window.
    DismissWindow(SceneWindowKey),
}

static APPLICATION_COMMANDS: Mutex<Vec<ApplicationCommand>> = Mutex::new(Vec::new());

/// Request that a declared scene window be opened.
///
/// # Arguments
///
/// * `key` - Stable scene window key declared by `Application::scenes()`.
pub fn open_window(key: impl Into<SceneWindowKey>) {
    APPLICATION_COMMANDS
        .lock()
        .push(ApplicationCommand::OpenWindow(key.into()));
}

/// Request that a declared scene window be dismissed.
///
/// # Arguments
///
/// * `key` - Stable scene window key declared by `Application::scenes()`.
pub fn dismiss_window(key: impl Into<SceneWindowKey>) {
    APPLICATION_COMMANDS
        .lock()
        .push(ApplicationCommand::DismissWindow(key.into()));
}

pub(crate) fn take_application_commands() -> Vec<ApplicationCommand> {
    core::mem::take(&mut *APPLICATION_COMMANDS.lock())
}
