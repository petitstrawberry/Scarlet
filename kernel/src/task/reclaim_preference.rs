//! ABI-neutral process memory-reclaim preference.
//!
//! Higher protection makes a process a less attractive memory-reclaim
//! candidate. The bound and textual representation used by a particular ABI
//! belong in that ABI's adapter, outside the task model.

use crate::shield::identity::{Credentials, MANAGE_RECLAIM, Principal};

pub const DEFAULT_PROTECTION: u16 = u16::MAX / 2;
/// A new process may make itself easier to reclaim, but cannot protect itself
/// beyond the default without native reclaim-management authority.
pub const DEFAULT_PROTECTION_CEILING: u16 = DEFAULT_PROTECTION;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateError {
    PermissionDenied,
}

/// Process-shared reclaim state. `protection_ceiling` is inherited by fork so
/// an unprivileged child cannot use its new identity to gain more protection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReclaimPreference {
    protection: u16,
    protection_ceiling: u16,
}

impl Default for ReclaimPreference {
    fn default() -> Self {
        Self {
            protection: DEFAULT_PROTECTION,
            protection_ceiling: DEFAULT_PROTECTION_CEILING,
        }
    }
}

impl ReclaimPreference {
    pub fn protection(&self) -> u16 {
        self.protection
    }

    pub fn protection_ceiling(&self) -> u16 {
        self.protection_ceiling
    }

    /// Update under the caller's current authority and the target's owner.
    /// Numeric identity values do not grant authority.
    pub(crate) fn update(
        &mut self,
        protection: u16,
        caller: &Credentials,
        target_owner: Principal,
    ) -> Result<(), UpdateError> {
        if caller.permits(MANAGE_RECLAIM) {
            self.protection = protection;
            // The manager may revoke protection as well as grant it. The new
            // ceiling, inherited by later children, is the installed value.
            self.protection_ceiling = protection;
            return Ok(());
        }

        if caller.identity().principal != target_owner || protection > self.protection_ceiling {
            return Err(UpdateError::PermissionDenied);
        }

        self.protection = protection;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shield::identity::{Group, Identities};

    #[test_case]
    fn inherited_ceiling_bounds_unprivileged_protection_without_numeric_special_cases() {
        let mut preference = ReclaimPreference::default();
        let mut caller = Credentials::bootstrap();
        caller.set_authority(0, 0).unwrap();
        caller
            .set_principals(Identities::all(Principal(0)))
            .unwrap();

        assert_eq!(
            preference.update(DEFAULT_PROTECTION + 1, &caller, Principal(0)),
            Err(UpdateError::PermissionDenied)
        );
        assert_eq!(
            preference.update(DEFAULT_PROTECTION - 250, &caller, Principal(0)),
            Ok(())
        );
        assert_eq!(
            preference.update(DEFAULT_PROTECTION - 500, &caller, Principal(1)),
            Err(UpdateError::PermissionDenied)
        );
        assert_eq!(preference.protection(), DEFAULT_PROTECTION - 250);

        let mut other_owner = Credentials::bootstrap();
        other_owner
            .set_principals(Identities::all(Principal(1000)))
            .unwrap();
        other_owner
            .set_groups(Identities::all(Group(1000)))
            .unwrap();
        other_owner.set_authority(0, 0).unwrap();
        assert_eq!(
            preference.update(DEFAULT_PROTECTION - 100, &other_owner, Principal(1000)),
            Ok(())
        );
        assert_eq!(
            preference.update(DEFAULT_PROTECTION + 500, &other_owner, Principal(1000)),
            Err(UpdateError::PermissionDenied)
        );
    }

    #[test_case]
    fn explicit_authority_sets_and_can_lower_the_inherited_ceiling() {
        let mut preference = ReclaimPreference::default();
        let mut caller = Credentials::bootstrap();
        assert!(caller.permits(MANAGE_RECLAIM));
        assert_eq!(preference.update(50_000, &caller, Principal(42)), Ok(()));
        assert_eq!(preference.protection(), 50_000);
        assert_eq!(preference.protection_ceiling(), 50_000);
        assert_eq!(preference.update(20_000, &caller, Principal(42)), Ok(()));
        assert_eq!(preference.protection(), 20_000);
        assert_eq!(preference.protection_ceiling(), 20_000);
    }
}
