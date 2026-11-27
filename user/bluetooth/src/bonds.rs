use super::*;

fn check_access(handle: &Handle, directory: bool) -> Result<()> {
    let access = scarlet_os::shield::inode_access(handle)
        .map_err(|_| "cannot verify bond storage ownership, mode and ACL")?;
    let forbidden = if directory { 0o022 } else { 0o7177 };
    if access.owner != 0 || access.has_acl != 0 || access.mode & forbidden != 0 {
        return Err("unsafe bond storage ownership, permissions or ACL".into());
    }
    // O_DIRECTORY already checked directories. A key, stage or lock must be
    // a regular file with no additional hard links.
    if !directory {
        let meta = handle
            .as_file()
            .map_err(|_| "expected a bond storage file")?
            .metadata()
            .map_err(|_| "cannot inspect bond storage file")?;
        if !meta.is_file() || meta.link_count != 1 {
            return Err("bond storage requires a singly linked regular file".into());
        }
    }
    Ok(())
}

fn sync(handle: &Handle) -> Result<()> {
    // SAFETY: the descriptor remains open through this scalar operation.
    let rc = unsafe { syscall1(Syscall::FileSync, handle.as_raw() as usize) } as isize;
    if rc == 0 {
        Ok(())
    } else {
        Err(format!("bond storage sync failed ({rc})"))
    }
}

fn open_file(dir: &Handle, name: &str, flags: u32) -> Result<Option<Handle>> {
    let component = CString::new(name).unwrap();
    match open_at(dir.as_raw() as usize, &component, flags, 0o600) {
        Ok(file) => {
            check_access(&file, false)?;
            Ok(Some(file))
        }
        Err(e) if e.raw_os_error() == Some(vfs::ERRNO_ENOENT) => Ok(None),
        Err(e) => Err(format!("open /etc/{name}: {e}")),
    }
}

fn directory() -> Result<Handle> {
    let root = open_at(vfs::CURRENT_DIRECTORY, c"/", vfs::VFS_O_DIRECTORY, 0)
        .map_err(|e| format!("open bond storage root: {e}"))?;
    check_access(&root, true)?;
    let dir = open_at(root.as_raw() as usize, c"etc", vfs::VFS_O_DIRECTORY, 0)
        .map_err(|e| format!("open /etc for bond storage: {e}"))?;
    check_access(&dir, true)?;
    Ok(dir)
}

fn component(peer: Address) -> String {
    peer.to_string().replace(':', "")
}

fn bonded_peer(name: &str, prefix: &str) -> Option<Address> {
    let mac = name.strip_prefix(prefix)?.strip_suffix(".bond")?;
    if mac.len() != 12
        || !mac
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
    {
        return None;
    }
    let mut bytes = [0; 6];
    for (i, byte) in bytes.iter_mut().rev().enumerate() {
        *byte = u8::from_str_radix(&mac[2 * i..2 * i + 2], 16).ok()?;
    }
    (bytes != [0; 6] && bytes != [255; 6]).then_some(Address(bytes))
}

pub fn list(adapter: bt::Address) -> Result<Vec<Address>> {
    if adapter == [0; 6] {
        return Err("controller has no adapter address for bond storage".into());
    }
    let dir = directory()?;
    let adapter = component(Address(adapter));
    let mut peers = Vec::new();
    // / and /etc were both verified. Reopen each candidate relative to the checked
    // directory and check its metadata without reading a single key octet.
    for_candidates(&adapter, |peer, name| {
        let Some(file) = open_file(&dir, name, vfs::VFS_O_RDONLY)? else {
            return Ok(());
        };
        let size = file
            .as_file()
            .map_err(|_| "invalid bond file")?
            .metadata()
            .map_err(|_| "cannot inspect bond length")?
            .size;
        if size != 17 {
            return Err("invalid bond file length".into());
        }
        peers.push(peer);
        // A JSON address uses at least 13 bytes plus a delimiter.
        if peers.len() > protocol::MAX_REPLY / 14 {
            return Err("bond listing exceeds the service response limit".into());
        }
        Ok(())
    })?;
    peers.sort_unstable();
    Ok(peers)
}

fn for_candidates(adapter: &str, mut visit: impl FnMut(Address, &str) -> Result<()>) -> Result<()> {
    let prefix = format!("bluetooth-{adapter}-");
    for entry in std::fs::read_dir("/etc").map_err(|e| format!("list bond directory: {e}"))? {
        let entry = entry.map_err(|e| format!("read bond directory entry: {e}"))?;
        let name = entry.file_name();
        if let Some(name) = name.to_str()
            && let Some(peer) = bonded_peer(name, &prefix)
        {
            visit(peer, name)?;
        }
    }
    Ok(())
}

// Root-owned /etc contains one type + 16-octet .bond and an optional UTF-8
// .name per saved peer. Only the .bond establishes membership.
pub struct Store {
    dir: Handle,
    adapter: Address,
    _lock: Handle,
}

impl Store {
    pub fn locked(adapter: bt::Address) -> Result<Self> {
        let started = Instant::now();
        loop {
            if let Some(store) = Self::try_lock(adapter)? {
                return Ok(store);
            }
            if started.elapsed() >= Duration::from_secs(1) {
                return Err("adapter bond storage is busy. Retry the operation.".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn try_lock(adapter: bt::Address) -> Result<Option<Self>> {
        if adapter == [0; 6] {
            return Err("controller has no adapter address for bond storage".into());
        }
        let dir = directory()?;
        let adapter = Address(adapter);
        let lock = open_file(
            &dir,
            &format!(".bluetooth-{}.lock", component(adapter)),
            vfs::VFS_O_CREAT | vfs::VFS_O_RDWR,
        )?
        .ok_or("cannot create bond storage lock")?;
        // SAFETY: this exclusive nonblocking flock is held through radio completion,
        // storage mutation and directory sync, including for direct recovery clients.
        let rc = unsafe { syscall2(Syscall::FileLock, lock.as_raw() as usize, 2 | 4) } as isize;
        if rc == -(scarlet_abi::ERRNO_EAGAIN as isize) {
            return Ok(None);
        }
        if rc != 0 {
            return Err(format!("cannot lock adapter bond storage ({rc})"));
        }
        Ok(Some(Self {
            dir,
            adapter,
            _lock: lock,
        }))
    }

    fn path(&self, peer: Address, extension: &str) -> String {
        format!(
            "bluetooth-{}-{}.{extension}",
            component(self.adapter),
            component(peer)
        )
    }

    pub fn check_pair(&self, peer: Address) -> Result<()> {
        for ext in ["bond", "name"] {
            let name = self.path(peer, ext);
            open_file(&self.dir, &name, vfs::VFS_O_RDONLY)?;
            open_file(&self.dir, &format!(".{name}.new"), vfs::VFS_O_WRONLY)?;
        }
        Ok(())
    }

    pub fn load_key(&self, peer: Address, request: &mut bt::Record) -> Result<()> {
        let file = open_file(&self.dir, &self.path(peer, "bond"), vfs::VFS_O_RDONLY)?.ok_or_else(
            || format!("no saved key for {peer}. Use btctl pair {peer} to pair the device."),
        )?;
        let stream = file.as_stream().map_err(|_| "cannot read bond file")?;
        let mut bytes = Zeroizing::new([0; 18]);
        stream
            .read_exact(&mut bytes[..17])
            .map_err(|_| "invalid or unreadable bond file")?;
        if stream
            .read(&mut bytes[17..])
            .map_err(|_| "cannot finish reading bond file")?
            != 0
        {
            return Err("invalid bond file length".into());
        }
        request[bt::KEY_TYPE] = bytes[0];
        request[bt::KEY].copy_from_slice(&bytes[1..17]);
        Ok(())
    }

    pub fn register(&self, handle: &Handle, peer: Address) -> Result<()> {
        let mut record = Zeroizing::new([0; bt::RECORD_SIZE]);
        record[bt::ADAPTER].copy_from_slice(&self.adapter.0);
        record[bt::PEER].copy_from_slice(&peer.0);
        self.load_key(peer, &mut record)?;
        control(handle, bt::REGISTER_BOND, &mut record)
    }

    pub fn register_all(&self, handle: &Handle) -> Result<()> {
        let mut first_error = None;
        for_candidates(&component(self.adapter), |peer, _| {
            // A broken bond must not prevent restoration of the other peers.
            if let Err(e) = self.register(handle, peer) {
                first_error.get_or_insert_with(|| format!("register {peer}: {e}"));
            }
            Ok(())
        })?;
        first_error.map_or(Ok(()), Err)
    }

    pub fn save_key(&self, peer: Address, r: &bt::Record) -> Result<()> {
        let mut bytes = Zeroizing::new([0; 17]);
        bytes[0] = r[bt::KEY_TYPE];
        bytes[1..].copy_from_slice(&r[bt::KEY]);
        self.install(&self.path(peer, "bond"), &bytes[..])
    }

    pub fn name(&self, peer: Address) -> Result<Option<String>> {
        let Some(file) = open_file(&self.dir, &self.path(peer, "name"), vfs::VFS_O_RDONLY)? else {
            return Ok(None);
        };
        let size = file
            .as_file()
            .map_err(|_| "invalid saved name file")?
            .metadata()
            .map_err(|_| "cannot inspect saved name")?
            .size;
        // Each invalid byte in a controller name can expand to U+FFFD.
        if size == 0 || size > (bt::NAME.len() * 3) as u64 {
            return Err("invalid saved name length".into());
        }
        let mut bytes = vec![0; size as usize];
        file.as_stream()
            .map_err(|_| "cannot read saved name")?
            .read_exact(&mut bytes)
            .map_err(|_| "cannot read saved name")?;
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| "invalid saved name".into())
    }

    pub fn remember(&self, peer: Address, observed: Option<&str>) -> Result<()> {
        let Some(name) = observed else { return Ok(()) };
        if open_file(&self.dir, &self.path(peer, "bond"), vfs::VFS_O_RDONLY)?.is_some()
            && self.name(peer)?.as_deref() != Some(name)
        {
            self.install(&self.path(peer, "name"), name.as_bytes())?;
        }
        Ok(())
    }

    pub fn remember_connected(&self, rows: &[ConnectedPeer]) -> Result<()> {
        for row in rows {
            self.remember(row.address, row.name.as_deref())?;
        }
        Ok(())
    }

    pub fn forget(&self, peer: Address) -> Result<()> {
        // Remove stages and metadata first. The installed key is the sole source of
        // membership. Retrying after any failure finishes all removals and the sync.
        for ext in ["name", "bond"] {
            let name = self.path(peer, ext);
            self.remove(&format!(".{name}.new"))?;
            self.remove(&name)?;
        }
        sync(&self.dir)
    }

    fn remove(&self, name: &str) -> Result<()> {
        if open_file(&self.dir, name, vfs::VFS_O_RDONLY)?.is_some() {
            let path = CString::new(format!("/etc/{name}")).unwrap();
            // SAFETY: this validated child of root-owned /etc lives through the unlink.
            let rc = unsafe { syscall2(Syscall::VfsRemoveWithStatus, path.as_ptr() as usize, 0) }
                as isize;
            if rc != 0 {
                return Err(format!("cannot remove {name} ({rc})"));
            }
        }
        Ok(())
    }

    fn install(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let stage = format!(".{name}.new");
        let file = open_file(&self.dir, &stage, vfs::VFS_O_WRONLY | vfs::VFS_O_CREAT)?
            .ok_or("cannot create bond staging file")?;
        let old = CString::new(format!("/etc/{stage}")).unwrap();
        let new = CString::new(format!("/etc/{name}")).unwrap();
        let install = (|| -> Result<()> {
            file.as_file()
                .map_err(|_| "invalid staging file")?
                .truncate(0)
                .map_err(|_| "cannot truncate bond staging file")?;
            let stream = file
                .as_stream()
                .map_err(|_| "cannot open bond staging stream")?;
            stream
                .write_all(bytes)
                .map_err(|_| "cannot write bond staging file")?;
            sync(&file)?;
            file.close().map_err(|_| "cannot close bond staging file")?;
            // SAFETY: both C strings live through the call and name children of
            // the validated root-owned /etc. Atomic replacement stays on one filesystem.
            let rc = unsafe {
                syscall3(
                    Syscall::VfsRenameWithStatus,
                    old.as_ptr() as usize,
                    new.as_ptr() as usize,
                    0,
                )
            } as isize;
            if rc != 0 {
                return Err(format!("cannot install bond file ({rc})"));
            }
            Ok(())
        })();
        if install.is_err() {
            self.remove(&stage)?;
        }
        install?;
        sync(&self.dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_installed_canonical_bonds_for_this_adapter_match() {
        let prefix = "bluetooth-020000000001-";
        let parse = |file| bonded_peer(file, prefix);
        assert_eq!(
            parse("bluetooth-020000000001-AABBCCDDEEFF.bond"),
            Some("AA:BB:CC:DD:EE:FF".parse().unwrap())
        );
        for name in [
            "bluetooth-020000000002-AABBCCDDEEFF.bond",
            ".bluetooth-020000000001-AABBCCDDEEFF.bond.new",
            "bluetooth-020000000001-FFFFFFFFFFFF.bond",
            "bluetooth-020000000001-000000000000.bond",
            "bluetooth-020000000001-aaBBCCDDEEFF.bond",
            "bluetooth-020000000001-0123.bond",
        ] {
            assert_eq!(parse(name), None);
        }
    }
}
