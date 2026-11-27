//! Local account authentication shared by trusted userland session managers.
#![no_std]

extern crate alloc;
#[cfg(target_os = "scarlet")]
mod database;

use alloc::{string::String, vec::Vec};
#[cfg(target_os = "scarlet")]
pub use database::{
    authenticate_console, authenticate_local, authorized_keys, initial_admin_available,
    login_available, login_names, login_uid_enabled, lookup_local, service_account,
    set_initial_admin_password,
};

#[cfg(any(test, target_os = "scarlet"))]
fn unconfigured_admin(data: &[u8]) -> bool {
    // The image builder emits this exact record for an unprovisioned console
    // root. A missing database, locked accounts and custom images fail closed.
    data == b"root:0:0::\n"
}

pub struct Account {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub groups: Vec<u32>,
}

impl Account {
    pub fn home(&self) -> String {
        if self.uid == 0 {
            String::from("/root")
        } else {
            alloc::format!("/home/{}", self.name)
        }
    }

    /// Join the supervised scope and permanently install this login's credentials.
    ///
    /// # Safety
    /// Run only in an exclusive child about to exec. Apart from the scope, the
    /// caller must relinquish owners of all handles above stderr before calling.
    #[cfg(target_os = "scarlet")]
    pub unsafe fn enter(
        &self,
        scope: scarlet_os::process_scope::ProcessScope,
    ) -> Result<(), &'static str> {
        scope.enter().map_err(|_| "cannot join session scope")?;
        drop(scope);
        // SAFETY: the caller owns an exclusive child and the scope handle was closed.
        unsafe { scarlet_os::shield::enter_user_session(self.uid, self.gid, &self.groups) }
            .map_err(|_| "cannot install session credentials")
    }
}

fn id(text: &str) -> Option<u32> {
    let id = text.parse().ok()?;
    (id != u32::MAX).then_some(id)
}

fn lookup<'a>(data: &'a str, name: &[u8]) -> Result<(Option<Account>, &'a str), ()> {
    lookup_by(data, |login, _| login.as_bytes() == name)
}

fn lookup_by(
    data: &str,
    matches: impl Fn(&str, u32) -> bool,
) -> Result<(Option<Account>, &str), ()> {
    use alloc::collections::BTreeSet;
    let mut names = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut selected = None;
    // A valid, unrelated verifier keeps unknown names on the same bcrypt path.
    let mut verifier = "$2b$12$L6Bc/AlTQHyd9liGgGEZyOFLPHNgyxeEPfgYfBCVxJ7JIlwxyVU3u";
    for line in data.lines() {
        let mut fields = line.split(':');
        let login = fields.next().ok_or(())?;
        let uid = fields.next().and_then(id).ok_or(())?;
        let gid = fields.next().and_then(id).ok_or(())?;
        let groups = fields.next().ok_or(())?;
        let hash = fields.next().ok_or(())?;
        if fields.next().is_some()
            || (login == "root") != (uid == 0)
            || (login == "root") != (gid == 0)
            || login.is_empty()
            || login.len() > 32
            || !login
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
            || !login.as_bytes()[0].is_ascii_lowercase()
            || !names.insert(login)
            || !ids.insert(uid)
        {
            return Err(());
        }
        if !(login == "root" && hash.is_empty())
            && hash != "!"
            && !(hash.len() == 60
                && (hash.starts_with("$2b$12$") || hash.starts_with("$2y$12$"))
                && hash[7..]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'/'))
        {
            return Err(());
        }
        let groups = if groups.is_empty() {
            Vec::new()
        } else {
            groups
                .split(',')
                .map(|group| id(group).filter(|&id| id != 0))
                .collect::<Option<Vec<_>>>()
                .ok_or(())?
        };
        if matches(login, uid) {
            selected = Some(Account {
                name: String::from(login),
                uid,
                gid,
                groups,
            });
            verifier = hash;
        }
    }
    Ok((selected, verifier))
}

#[cfg(any(test, target_os = "scarlet"))]
fn selectable_names(data: &str) -> Result<Vec<String>, ()> {
    lookup(data, b"")?;
    Ok(data
        .lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once(':')?;
            let (_, verifier) = rest.rsplit_once(':')?;
            (!verifier.is_empty() && verifier != "!").then(|| String::from(name))
        })
        .collect())
}

fn authenticate(
    data: &str,
    name: &[u8],
    password: &[u8],
    console: bool,
) -> Result<Option<Account>, ()> {
    let (selected, verifier) = lookup(data, name)?;
    if verifier == "!" {
        return Ok(None);
    }
    if verifier.is_empty() {
        return Ok(selected.filter(|account| console && account.uid == 0 && password.is_empty()));
    }
    if password.is_empty() || password.len() > 72 || password.contains(&0) {
        return Ok(None);
    }
    Ok(if bcrypt::verify(password, verifier).map_err(|_| ())? {
        selected
    } else {
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chooser_includes_only_enabled_graphical_logins() {
        let db = "root:0:0::
service:2:2::!
alice:1000:1000::$2b$12$L6Bc/AlTQHyd9liGgGEZyOFLPHNgyxeEPfgYfBCVxJ7JIlwxyVU3u
";
        assert_eq!(selectable_names(db).unwrap(), ["alice"]);
        assert!(selectable_names(&alloc::format!("{db}bad")).is_err());
    }
    #[test]
    fn empty_verifier_is_only_for_console_root() {
        let db = "root:0:0::\n";
        assert!(authenticate(db, b"root", b"", true).unwrap().is_some());
        assert!(authenticate(db, b"root", b"", false).unwrap().is_none());
        assert!(authenticate(db, b"root", b"wrong", true).unwrap().is_none());
        assert!(
            authenticate("root:0:0::!", b"root", b"", true)
                .unwrap()
                .is_none()
        );
        assert!(authenticate("", b"root", b"", true).unwrap().is_none());
        assert!(authenticate("alice:1:1::", b"alice", b"", true).is_err());
    }

    #[test]
    fn initial_setup_requires_the_factory_account_record() {
        assert!(unconfigured_admin(b"root:0:0::\n"));
        for database in [
            b"".as_slice(),
            b"root:0:0::!\n",
            b"root:0:0::\nservice:2:2::!\n",
            b"root:0:0::\ninvalid\n",
        ] {
            assert!(!unconfigured_admin(database));
        }
    }
}
