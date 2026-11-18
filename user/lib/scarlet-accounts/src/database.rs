use alloc::vec::Vec;
use zeroize::Zeroizing;

fn open_at(dir: usize, name: &core::ffi::CStr, directory: bool) -> Result<scarlet_os::Handle, ()> {
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
    let handle = unsafe { scarlet_os::Handle::from_raw(raw as i32) }.map_err(|_| ())?;
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
    Ok(handle)
}

fn load() -> Result<Zeroizing<Vec<u8>>, ()> {
    let root = open_at(scarlet_abi::fs::CURRENT_DIRECTORY, c"/", true)?;
    let etc = open_at(root.as_raw() as usize, c"etc", true)?;
    let file = open_at(etc.as_raw() as usize, c"scarlet-users", false)?;
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

/// Read a fresh protected database for every login or unlock attempt.
pub fn authenticate_local(
    name: &[u8],
    password: &[u8],
) -> Result<Option<super::Account>, &'static str> {
    let data = load().map_err(|_| "cannot read a protected /etc/scarlet-users")?;
    let text = core::str::from_utf8(&data).map_err(|_| "invalid account database")?;
    super::authenticate(text, name, password).map_err(|_| "invalid account database")
}
