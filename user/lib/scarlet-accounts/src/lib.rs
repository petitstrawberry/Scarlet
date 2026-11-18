//! Local account authentication shared by trusted userland session managers.
#![no_std]

extern crate alloc;
#[cfg(target_os = "scarlet")]
mod database;

use alloc::{string::String, vec::Vec};
#[cfg(target_os = "scarlet")]
pub use database::{authenticate_local, authorized_keys, lookup_local};

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
        if hash != "!"
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
        if login.as_bytes() == name && hash != "!" {
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

fn authenticate(data: &str, name: &[u8], password: &[u8]) -> Result<Option<Account>, ()> {
    let (selected, verifier) = lookup(data, name)?;
    if password.is_empty() || password.len() > 72 || password.contains(&0) {
        return Ok(None);
    }
    Ok(if bcrypt::verify(password, verifier).map_err(|_| ())? {
        selected
    } else {
        None
    })
}
