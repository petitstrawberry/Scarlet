//! Whole-machine power transitions, separate from peripheral power/reset lines.
//!
//! Init coordinates process shutdown before entering this layer. A board owns
//! both operations through one controller and composes its PMIC/SoC drivers.
//! With no board controller, the architecture supplies its firmware default.

use alloc::sync::Arc;
use core::convert::Infallible;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    PowerOff,
    Restart,
}

/// Successful hardware transitions cannot return.
pub type PowerResult = Result<Infallible, &'static str>;

pub trait Controller: Send + Sync {
    fn name(&self) -> &'static str;
    /// Called in task context with interrupts and I/O workers available. Bound
    /// hardware waits. Report missing hardware or failure explicitly; neither
    /// error causes a second attempt through a different firmware mechanism.
    fn transition(&self, action: Action) -> PowerResult;
}

static CONTROLLER: crate::sync::Once<Arc<dyn Controller>> = crate::sync::Once::new();

/// Install the board's policy once, before userspace can request shutdown.
/// Resource drivers expose operations; they do not compete for this registry.
pub fn register(controller: Arc<dyn Controller>) -> Result<(), &'static str> {
    CONTROLLER
        .set(controller)
        .map_err(|_| "system power controller already registered")
}

fn dispatch(
    action: Action,
    controller: Option<&dyn Controller>,
    firmware: impl FnOnce(Action) -> PowerResult,
) -> PowerResult {
    match controller {
        Some(controller) => controller.transition(action),
        None => firmware(action),
    }
}

/// Execute the final transition. No registration lock is held across I/O.
/// A failed transition is terminal: init has already stopped user processes.
/// Keep kernel workers available while parked and print the failure reason.
pub fn transition(action: Action) -> ! {
    let controller = CONTROLLER.get().map(|controller| controller.as_ref());
    crate::println!(
        "[system-power] {:?} via {}",
        action,
        controller.map_or("architecture firmware", Controller::name)
    );
    let result = dispatch(action, controller, crate::arch::firmware_system_power);
    match result {
        Ok(never) => match never {},
        Err(error) => crate::println!("[system-power] {:?} failed: {}", action, error),
    }
    crate::arch::instruction::idle()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    struct Board(AtomicUsize);
    impl Controller for Board {
        fn name(&self) -> &'static str {
            "test board"
        }
        fn transition(&self, action: Action) -> PowerResult {
            self.0.store(
                match action {
                    Action::PowerOff => 1,
                    Action::Restart => 2,
                },
                Ordering::SeqCst,
            );
            Err("board hardware failed")
        }
    }

    #[test]
    fn board_owns_both_operations_and_failure_never_falls_through() {
        let board = Board(AtomicUsize::new(0));
        for (action, expected) in [(Action::PowerOff, 1), (Action::Restart, 2)] {
            assert_eq!(
                dispatch(action, Some(&board), |_| panic!(
                    "unexpected firmware fallback"
                )),
                Err("board hardware failed")
            );
            assert_eq!(board.0.load(Ordering::SeqCst), expected);
        }
    }

    #[test]
    fn missing_board_uses_firmware_with_the_requested_operation() {
        for action in [Action::PowerOff, Action::Restart] {
            let result = dispatch(action, None, |received| {
                assert_eq!(received, action);
                Err("firmware unavailable")
            });
            assert_eq!(result, Err("firmware unavailable"));
        }
    }
}
