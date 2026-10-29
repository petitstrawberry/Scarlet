//! ABI-neutral process identities and delegable authority. Numeric identity
//! zero has no special power here; authority is checked independently.
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Principal(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group(pub u32);

/// An originating identity, its active identity, and an identity retained for
/// a reversible transition. ABI adapters decide the meaning of each slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identities<T: Copy + Eq> {
    pub base: T,
    pub active: T,
    pub retained: T,
}
impl<T: Copy + Eq> Identities<T> {
    pub fn all(id: T) -> Self {
        Self {
            base: id,
            active: id,
            retained: id,
        }
    }
    pub fn contains(&self, id: T) -> bool {
        self.base == id || self.active == id || self.retained == id
    }
    fn admits(&self, next: Self) -> bool {
        self.contains(next.base) && self.contains(next.active) && self.contains(next.retained)
    }
}

pub const CHANGE_PRINCIPAL: u32 = 1;
pub const CHANGE_GROUPS: u32 = 2;
pub const MANAGE_MOUNTS: u32 = 4;
pub const ALL_AUTHORITY: u32 = CHANGE_PRINCIPAL | CHANGE_GROUPS | MANAGE_MOUNTS;
pub const MAX_GROUPS: usize = 65536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActiveIdentity {
    pub principal: Principal,
    pub group: Group,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credentials {
    principals: Identities<Principal>,
    groups: Identities<Group>,
    supplementary: Vec<Group>,
    permitted: u32,
    active: u32,
}
impl Credentials {
    /// Initial kernel-created task authority. Fork/spawn/exec must inherit the
    /// caller's credentials instead of obtaining a fresh bootstrap grant.
    pub(crate) fn bootstrap() -> Self {
        Self {
            principals: Identities::all(Principal(0)),
            groups: Identities::all(Group(0)),
            supplementary: Vec::new(),
            permitted: ALL_AUTHORITY,
            active: ALL_AUTHORITY,
        }
    }
    pub fn principals(&self) -> Identities<Principal> {
        self.principals
    }
    pub fn groups(&self) -> Identities<Group> {
        self.groups
    }
    pub fn supplementary(&self) -> &[Group] {
        &self.supplementary
    }
    pub fn identity(&self) -> ActiveIdentity {
        ActiveIdentity {
            principal: self.principals.active,
            group: self.groups.active,
        }
    }
    pub fn permits(&self, authority: u32) -> bool {
        self.active & authority == authority
    }
    pub fn permitted_authority(&self) -> u32 {
        self.permitted
    }
    pub fn active_authority(&self) -> u32 {
        self.active
    }
    pub fn set_principals(&mut self, next: Identities<Principal>) -> Result<(), ()> {
        if !self.permits(CHANGE_PRINCIPAL) && !self.principals.admits(next) {
            return Err(());
        }
        self.principals = next;
        Ok(())
    }
    pub fn set_groups(&mut self, next: Identities<Group>) -> Result<(), ()> {
        if !self.permits(CHANGE_GROUPS) && !self.groups.admits(next) {
            return Err(());
        }
        self.groups = next;
        Ok(())
    }
    pub fn set_supplementary(&mut self, mut groups: Vec<Group>) -> Result<(), ()> {
        if !self.permits(CHANGE_GROUPS) || groups.len() > MAX_GROUPS {
            return Err(());
        }
        groups.sort_unstable_by_key(|group| group.0);
        self.supplementary = groups;
        Ok(())
    }
    /// Drop the ceiling permanently or reactivate previously delegated power.
    /// Changing an identifier (even to zero) cannot enlarge that ceiling.
    pub fn set_authority(&mut self, permitted: u32, active: u32) -> Result<(), ()> {
        if permitted & !self.permitted != 0 || active & !permitted != 0 {
            return Err(());
        }
        self.permitted = permitted;
        self.active = active;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test_case]
    fn identity_authority_is_not_derived_from_numeric_zero() {
        let mut creds = Credentials::bootstrap();
        creds.set_authority(0, 0).unwrap();
        let saved = creds.clone();
        assert!(
            creds
                .set_principals(Identities::all(Principal(123)))
                .is_err()
        );
        assert!(creds.set_groups(Identities::all(Group(123))).is_err());
        assert!(creds.set_supplementary(alloc::vec![Group(123)]).is_err());
        assert!(creds.set_authority(ALL_AUTHORITY, ALL_AUTHORITY).is_err());
        assert_eq!(creds, saved);
    }
    #[test_case]
    fn identity_retained_transitions_and_fork_copies_do_not_mint_authority() {
        let mut parent = Credentials::bootstrap();
        parent
            .set_principals(Identities {
                base: Principal(1),
                active: Principal(2),
                retained: Principal(3),
            })
            .unwrap();
        parent
            .set_supplementary(alloc::vec![Group(5), Group(4)])
            .unwrap();
        let mut child = parent.clone();
        child.set_authority(0, 0).unwrap();
        child.set_principals(Identities::all(Principal(3))).unwrap();
        assert!(child.set_principals(Identities::all(Principal(2))).is_err());
        assert!(parent.permits(CHANGE_PRINCIPAL));
        assert_eq!(child.supplementary(), &[Group(4), Group(5)]);
        assert_eq!(parent.principals().active, Principal(2));
    }
}
