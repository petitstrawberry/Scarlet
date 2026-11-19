use alloc::vec::Vec;
use core::ffi::CStr;
use scarlet_abi::fs::*;
use scarlet_sys::{Syscall, syscall1, syscall2, syscall3, syscall4};
use zeroize::Zeroizing;

fn open_with_flags(
    dir: usize,
    name: &CStr,
    flags: u32,
    mode: usize,
) -> Result<scarlet_os::Handle, ()> {
    // SAFETY: name remains live through the call, and success transfers one handle.
    let raw = unsafe {
        syscall4(
            Syscall::VfsOpenAt,
            dir,
            name.as_ptr() as usize,
            flags as usize,
            mode,
        )
    } as isize;
    if raw < 0 {
        return Err(());
    }
    unsafe { scarlet_os::Handle::from_raw(raw as i32) }.map_err(|_| ())
}

fn open_at(
    dir: usize,
    name: &core::ffi::CStr,
    directory: bool,
    uid: u32,
    forbidden: u32,
) -> Result<scarlet_os::Handle, ()> {
    let flags = VFS_O_NOFOLLOW | VFS_O_CLOEXEC | if directory { VFS_O_DIRECTORY } else { 0 };
    let handle = open_with_flags(dir, name, flags, 0)?;
    let access = scarlet_os::shield::inode_access(&handle).map_err(|_| ())?;
    if (access.owner != 0 && access.owner != uid)
        || access.has_acl != 0
        || access.mode & forbidden != 0
    {
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
    Ok(handle)
}

fn load() -> Result<Zeroizing<Vec<u8>>, ()> {
    load_from(&account_directory()?)
}

fn account_directory() -> Result<scarlet_os::Handle, ()> {
    let root = open_at(scarlet_abi::fs::CURRENT_DIRECTORY, c"/", true, 0, 0o022)?;
    open_at(root.as_raw() as usize, c"etc", true, 0, 0o022)
}

fn load_from(etc: &scarlet_os::Handle) -> Result<Zeroizing<Vec<u8>>, ()> {
    let file = open_at(etc.as_raw() as usize, c"scarlet-users", false, 0, 0o077)?;
    read_bounded(&file)
}

/// Whether the protected database contains only the image's console root placeholder.
pub fn initial_admin_available() -> bool {
    load().is_ok_and(|data| super::unconfigured_admin(&data))
}

/// Provision root on a factory image. The caller must be the trusted local seat.
/// Returns no account if the database has already been provisioned.
pub fn set_initial_admin_password(password: &[u8]) -> Result<Option<super::Account>, ()> {
    let etc = account_directory()?;
    if !super::unconfigured_admin(&load_from(&etc)?) {
        return Ok(None);
    }
    if password.is_empty() || password.len() > 72 || password.contains(&0) {
        return Err(());
    }

    let mut salt = [0u8; 16];
    let mut filled = 0;
    while filled < salt.len() {
        // SAFETY: the buffer suffix is exclusively writable until this call returns.
        let length = unsafe {
            syscall3(
                Syscall::GetRandom,
                salt[filled..].as_mut_ptr() as usize,
                salt.len() - filled,
                scarlet_sys::GET_RANDOM_FLAG_REQUIRE_ENTROPY,
            )
        };
        if length == 0 || length > salt.len() - filled {
            return Err(());
        }
        filled += length;
    }
    let parts = bcrypt::hash_with_salt(password, 12, salt).map_err(|_| ())?;
    let verifier = Zeroizing::new(parts.format_for_version(bcrypt::Version::TwoB));
    let record = Zeroizing::new(alloc::format!(
        "root:0:0::{verifier}\n",
        verifier = &*verifier
    ));

    // Keep this inode stable while replacing the database. The advisory lock
    // follows the open handle and is released even if the broker exits.
    let lock = open_with_flags(
        etc.as_raw() as usize,
        c".scarlet-users.setup-lock",
        VFS_O_RDWR | VFS_O_CREAT | VFS_O_NOFOLLOW | VFS_O_CLOEXEC,
        0o600,
    )?;
    let access = scarlet_os::shield::inode_access(&lock).map_err(|_| ())?;
    if access.owner != 0 || access.has_acl != 0 || access.mode & 0o077 != 0 {
        return Err(());
    }
    const LOCK_EX_NB: usize = 2 | 4;
    // SAFETY: the lock file handle is live and the operation scalar is valid.
    if unsafe { syscall2(Syscall::FileLock, lock.as_raw() as usize, LOCK_EX_NB) } != 0 {
        return Err(());
    }
    if !super::unconfigured_admin(&load_from(&etc)?) {
        return Ok(None);
    }

    let candidate = c"/etc/.scarlet-users.setup-new";
    // A stopped broker may leave a staging file. Only the lock holder uses it.
    // SAFETY: candidate is a static NUL-terminated path in the protected directory.
    unsafe { syscall1(Syscall::VfsRemove, candidate.as_ptr() as usize) };
    let file = open_with_flags(
        etc.as_raw() as usize,
        c".scarlet-users.setup-new",
        VFS_O_WRONLY | VFS_O_CREAT | VFS_O_EXCL | VFS_O_NOFOLLOW | VFS_O_CLOEXEC,
        0o600,
    )?;
    let install = (|| -> Result<(), ()> {
        file.as_stream()
            .map_err(|_| ())?
            .write_all(record.as_bytes())
            .map_err(|_| ())?;
        // SAFETY: the handle remains live until the data and metadata are flushed.
        if unsafe { syscall1(Syscall::FileSync, file.as_raw() as usize) } != 0 {
            return Err(());
        }
        file.close().map_err(|_| ())?;
        // SAFETY: both paths are static and the new file is on the same filesystem.
        let result = unsafe {
            syscall3(
                Syscall::VfsRenameWithStatus,
                candidate.as_ptr() as usize,
                c"/etc/scarlet-users".as_ptr() as usize,
                0,
            )
        };
        (result == 0).then_some(()).ok_or(())
    })();
    if install.is_err() {
        // SAFETY: remove only the reserved, protected staging path.
        unsafe { syscall1(Syscall::VfsRemove, candidate.as_ptr() as usize) };
    }
    install?;
    super::lookup(&record, b"root").map(|(account, _)| account)
}

fn read_bounded(file: &scarlet_os::Handle) -> Result<Zeroizing<Vec<u8>>, ()> {
    let stream = file.as_stream().map_err(|_| ())?;
    let mut data = Zeroizing::new(Vec::new());
    let mut chunk = Zeroizing::new([0u8; 1024]);
    loop {
        let n = stream.read(&mut *chunk).map_err(|_| ())?;
        if n == 0 {
            return Ok(data);
        }
        if data.len() + n > 64 * 1024 {
            return Err(());
        }
        data.extend_from_slice(&chunk[..n]);
    }
}

/// Report whether the seat can offer a login without exposing account names.
pub fn login_available() -> bool {
    let Ok(data) = load() else { return false };
    let Ok(text) = core::str::from_utf8(&data) else {
        return false;
    };
    super::lookup(text, b"").is_ok()
        && text
            .lines()
            .any(|line| !line.ends_with(":!") && !line.ends_with(':'))
}

/// Names available for the graphical account chooser, in image configuration order.
pub fn login_names() -> Vec<alloc::string::String> {
    load()
        .ok()
        .and_then(|data| super::selectable_names(core::str::from_utf8(&data).ok()?).ok())
        .unwrap_or_default()
}

/// Read a fresh protected database for every login or unlock attempt.
pub fn authenticate_local(
    name: &[u8],
    password: &[u8],
) -> Result<Option<super::Account>, &'static str> {
    authenticate_password(name, password, false)
}

/// Only the trusted local TTY login service uses the console password policy.
pub fn authenticate_console(
    name: &[u8],
    password: &[u8],
) -> Result<Option<super::Account>, &'static str> {
    authenticate_password(name, password, true)
}

fn authenticate_password(
    name: &[u8],
    password: &[u8],
    console: bool,
) -> Result<Option<super::Account>, &'static str> {
    let data = load().map_err(|_| "cannot read a protected /etc/scarlet-users")?;
    let text = core::str::from_utf8(&data).map_err(|_| "invalid account database")?;
    super::authenticate(text, name, password, console).map_err(|_| "invalid account database")
}

/// Return an enabled account for public-key authentication. The caller must
/// still verify both authorization for the key and proof of possession.
pub fn lookup_local(name: &[u8]) -> Result<Option<super::Account>, &'static str> {
    let data = load().map_err(|_| "cannot read a protected /etc/scarlet-users")?;
    let text = core::str::from_utf8(&data).map_err(|_| "invalid account database")?;
    super::lookup(text, name)
        .map(|(account, hash)| account.filter(|_| hash != "!"))
        .map_err(|_| "invalid account database")
}

/// Resolve every component relative to an already checked directory. A user
/// may manage their own keys, but cannot substitute a symlink or writable ancestor.
pub fn authorized_keys(account: &super::Account) -> Option<Zeroizing<Vec<u8>>> {
    let root = open_at(scarlet_abi::fs::CURRENT_DIRECTORY, c"/", true, 0, 0o022).ok()?;
    let home = if account.uid == 0 {
        open_at(root.as_raw() as usize, c"root", true, 0, 0o022).ok()?
    } else {
        let parent = open_at(root.as_raw() as usize, c"home", true, 0, 0o022).ok()?;
        let name = alloc::ffi::CString::new(account.name.as_bytes()).ok()?;
        open_at(parent.as_raw() as usize, &name, true, account.uid, 0o022).ok()?
    };
    let ssh = open_at(home.as_raw() as usize, c".ssh", true, account.uid, 0o022).ok()?;
    let keys = open_at(
        ssh.as_raw() as usize,
        c"authorized_keys",
        false,
        account.uid,
        0o022,
    )
    .ok()?;
    read_bounded(&keys).ok()
}

/// Resolve a preconfigured service identity, including accounts locked for login.
pub fn service_account(name: &[u8]) -> Option<super::Account> {
    let data = load().ok()?;
    super::lookup(core::str::from_utf8(&data).ok()?, name)
        .ok()?
        .0
}
