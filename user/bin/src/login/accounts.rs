//! Local console accounts. All records must be valid and unambiguous.
use std::{string::String, vec::Vec};
use zeroize::Zeroizing;

pub struct Account {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub groups: Vec<u32>,
}

fn id(text: &str) -> Option<u32> {
    let id = text.parse().ok()?;
    (id != 0 && id != u32::MAX).then_some(id)
}

pub fn authenticate(data: &str, name: &[u8], password: &[u8]) -> Result<Option<Account>, ()> {
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
                .map(id)
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
    if password.is_empty() || password.len() > 72 || password.contains(&0) {
        return Ok(None);
    }
    Ok(if bcrypt::verify(password, verifier).map_err(|_| ())? {
        selected
    } else {
        None
    })
}

fn open_at(dir: usize, name: &core::ffi::CStr, directory: bool) -> Result<std::fs::File, ()> {
    use scarlet_abi::fs::*;
    let flags = VFS_O_NOFOLLOW | VFS_O_CLOEXEC | if directory { VFS_O_DIRECTORY } else { 0 };
    // SAFETY: name lives through the call, and success transfers one handle.
    let raw = unsafe {
        scarlet_sys::syscall4(
            scarlet_sys::Syscall::VfsOpenAt,
            dir,
            name.as_ptr() as usize,
            flags as usize,
            0,
        )
    } as isize;
    if raw < 0 {
        return Err(());
    }
    let handle = unsafe { std::handle::Handle::from_raw(raw as i32) }.map_err(|_| ())?;
    let access = scarlet_os::shield::inode_access(&handle).map_err(|_| ())?;
    let forbidden = if directory { 0o022 } else { 0o077 };
    if access.owner != 0 || access.has_acl != 0 || access.mode & forbidden != 0 {
        return Err(());
    }
    if !directory
        && !handle
            .as_file()
            .ok()
            .and_then(|f| f.metadata().ok())
            .is_some_and(|m| m.is_file())
    {
        return Err(());
    }
    std::fs::File::from_handle(handle).map_err(|_| ())
}

pub fn load() -> Result<Zeroizing<Vec<u8>>, ()> {
    let root = open_at(scarlet_abi::fs::CURRENT_DIRECTORY, c"/", true)?;
    let etc = open_at(root.as_raw() as usize, c"etc", true)?;
    let mut file = open_at(etc.as_raw() as usize, c"scarlet-users", false)?;
    let mut data = Zeroizing::new(Vec::new());
    let mut chunk = Zeroizing::new([0u8; 1024]);
    loop {
        let n = file.read(&mut *chunk).map_err(|_| ())?;
        if n == 0 {
            return Ok(data);
        }
        if data.len() + n > 64 * 1024 {
            return Err(());
        }
        data.extend_from_slice(&chunk[..n]);
    }
}
