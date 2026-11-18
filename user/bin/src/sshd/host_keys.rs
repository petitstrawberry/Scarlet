use core::ffi::CStr;
use rand_core::OsRng;
use scarlet_abi::fs::*;
use scarlet_os::Handle;
use ssh_key::{Algorithm, HashAlg, LineEnding, PrivateKey};
use std::{format, println, string::String, vec::Vec};
use sunset::SignKey;
use zeroize::Zeroizing;

fn open_at(parent: usize, name: &CStr, flags: u32) -> Result<Handle, i32> {
    // SAFETY: name is terminated, and a successful open transfers one handle.
    let raw = unsafe {
        scarlet_sys::syscall4(
            scarlet_sys::Syscall::VfsOpenAt,
            parent,
            name.as_ptr() as usize,
            (flags | VFS_O_NOFOLLOW | VFS_O_CLOEXEC) as usize,
            0o600,
        )
    } as isize;
    if raw < 0 {
        return Err(-raw as i32);
    }
    unsafe { Handle::from_raw(raw as i32) }.map_err(|_| ERRNO_EACCES)
}

fn check(handle: &Handle, forbidden: u32) -> Result<(), String> {
    let attr =
        scarlet_os::shield::inode_access(handle).map_err(|_| "cannot inspect host key path")?;
    if attr.owner != 0 || attr.has_acl != 0 || attr.mode & forbidden != 0 {
        return Err("unsafe SSH host key ownership or permissions".into());
    }
    Ok(())
}

fn load(parent: &Handle, name: &CStr) -> Result<Option<SignKey>, String> {
    let file = match open_at(parent.as_raw() as usize, name, VFS_O_RDONLY) {
        Ok(file) => file,
        Err(ERRNO_ENOENT) => return Ok(None),
        Err(e) => return Err(format!("cannot open {:?}: errno {}", name, e)),
    };
    check(&file, 0o077)?;
    let meta = file.as_file().ok().and_then(|f| f.metadata().ok());
    if !meta.is_some_and(|m| m.is_file()) {
        return Err("invalid SSH host key file".into());
    }
    let stream = file
        .as_stream()
        .map_err(|_| "invalid SSH host key handle")?;
    let mut data = Zeroizing::new(Vec::new());
    let mut chunk = Zeroizing::new([0u8; 2048]);
    loop {
        let n = stream
            .read(&mut *chunk)
            .map_err(|_| "host key read failed")?;
        if n == 0 {
            break;
        }
        if data.len() + n > 64 * 1024 {
            return Err("host key too large".into());
        }
        data.extend_from_slice(&chunk[..n]);
    }
    SignKey::from_openssh(&data)
        .map(Some)
        .map_err(|_| format!("invalid host key {:?}", name))
}

const ED25519: &CStr = c"ssh_host_ed25519_key";

fn generate(dir: &Handle) -> Result<SignKey, String> {
    let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519)
        .map_err(|_| "host key generation failed")?;
    let encoded = key
        .to_openssh(LineEnding::LF)
        .map_err(|_| "host key encoding failed")?;
    let signer = SignKey::from_openssh(encoded.as_bytes()).map_err(|_| "unsupported host key")?;
    let tmp = format!("/etc/ssh/.hostkey-{}", std::task::getpid());
    let bytes = std::ffi::str_to_cstr_bytes(&tmp).map_err(|_| "invalid host key path")?;
    let path = CStr::from_bytes_with_nul(&bytes).map_err(|_| "invalid host key path")?;
    let file = match open_at(
        CURRENT_DIRECTORY,
        path,
        VFS_O_WRONLY | VFS_O_CREAT | VFS_O_EXCL,
    ) {
        Ok(file) => file,
        Err(ERRNO_EROFS) => {
            println!("[sshd] read-only filesystem, using an ephemeral host key");
            return Ok(signer);
        }
        Err(e) => return Err(format!("cannot create private host key: errno {}", e)),
    };
    let publish = || -> Result<SignKey, String> {
        check(&file, 0o077)?;
        file.as_stream()
            .map_err(|_| "host key stream unavailable")?
            .write_all(encoded.as_bytes())
            .map_err(|_| "host key write failed")?;
        // SAFETY: file owns this open handle until publication completes.
        if unsafe { scarlet_sys::syscall1(scarlet_sys::Syscall::FileSync, file.as_raw() as usize) }
            != 0
        {
            return Err("host key sync failed".into());
        }
        // SAFETY: both names are NUL-terminated and live through the call.
        let result = unsafe {
            scarlet_sys::syscall3(
                scarlet_sys::Syscall::VfsRenameWithStatus,
                path.as_ptr() as usize,
                c"/etc/ssh/ssh_host_ed25519_key".as_ptr() as usize,
                VFS_RENAME_NOREPLACE,
            )
        } as isize;
        if result == -(ERRNO_EEXIST as isize) {
            return load(dir, ED25519)?.ok_or_else(|| "host key disappeared".into());
        }
        if result != 0 {
            return Err(format!("cannot publish host key: errno {}", -result));
        }
        Ok(signer)
    };
    let result = publish();
    let _ = std::fs::remove_file(&tmp);
    result
}

pub(super) fn load_host_keys() -> Result<Vec<SignKey>, String> {
    let directory = |parent, name| -> Result<Handle, String> {
        let handle = open_at(parent, name, VFS_O_DIRECTORY)
            .map_err(|e| format!("cannot open {:?}: errno {}", name, e))?;
        check(&handle, 0o022)?;
        Ok(handle)
    };
    let root = directory(CURRENT_DIRECTORY, c"/")?;
    let etc = directory(root.as_raw() as usize, c"etc")?;
    let _ = std::fs::create_directory("/etc/ssh");
    let dir = directory(etc.as_raw() as usize, c"ssh")?;
    let mut keys = Vec::new();
    keys.push(match load(&dir, ED25519)? {
        Some(key) => key,
        None => generate(&dir)?,
    });
    if let Some(key) = load(&dir, c"ssh_host_rsa_key")? {
        keys.push(key);
    }
    for key in &keys {
        if let Ok(fp) = key.pubkey().fingerprint(HashAlg::Sha256) {
            println!("[sshd] host key {}", fp);
        }
    }
    Ok(keys)
}
