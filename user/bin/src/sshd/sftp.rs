//! SFTP v3 over an authenticated SSH session (no PTY or shell involved).
//! Wire format: draft-ietf-secsh-filexfer-02 and OpenSSH PROTOCOL section 4.
#[path = "sftp/fs.rs"]
mod fs;
use std::{format, string::String, vec, vec::Vec};

const MAX_PACKET: usize = 256 * 1024;
const MAX_READ: usize = 64 * 1024;
const MAX_HANDLES: usize = 64;
const BAD_MESSAGE: Error = Error(5, "Malformed SFTP request");
const UNSUPPORTED: Error = Error(8, "Operation or attributes not supported");
const FAILURE: Error = Error(4, "Filesystem operation failed");
#[derive(Clone, Copy, Debug)]
struct Error(u32, &'static str);
type Result<T> = core::result::Result<T, Error>;

#[derive(Default, Clone, Copy)]
struct Attrs {
    size: Option<u64>,
    uid_gid: Option<(u32, u32)>,
    mode: Option<u32>,
    times: Option<(u32, u32)>,
    extended: bool,
}
impl Attrs {
    fn read(r: &mut Reader<'_>) -> Result<Self> {
        let flags = r.u32()?;
        if flags & !0x8000_000f != 0 {
            return Err(BAD_MESSAGE);
        }
        let size = if flags & 1 != 0 { Some(r.u64()?) } else { None };
        let uid_gid = if flags & 2 != 0 {
            Some((r.u32()?, r.u32()?))
        } else {
            None
        };
        let mode = if flags & 4 != 0 { Some(r.u32()?) } else { None };
        let times = if flags & 8 != 0 {
            Some((r.u32()?, r.u32()?))
        } else {
            None
        };
        let extended = flags & 0x8000_0000 != 0;
        if extended {
            let count = r.u32()?;
            if count > 128 {
                return Err(BAD_MESSAGE);
            }
            for _ in 0..count {
                r.bytes()?;
                r.bytes()?;
            }
        }
        Ok(Self {
            size,
            uid_gid,
            mode,
            times,
            extended,
        })
    }
    fn creation_mode(self, default: u32) -> Result<u32> {
        if self.size.is_some() || self.times.is_some() || self.uid_gid.is_some() || self.extended {
            return Err(UNSUPPORTED);
        }
        Ok(self.mode.unwrap_or(default) & 0o777)
    }
    fn encode(self, out: &mut Vec<u8>) {
        let flags = u32::from(self.size.is_some())
            | (u32::from(self.mode.is_some()) << 2)
            | (u32::from(self.times.is_some()) << 3);
        word(out, flags);
        if let Some(size) = self.size {
            out.extend(size.to_be_bytes());
        }
        if let Some(mode) = self.mode {
            word(out, mode);
        }
        if let Some((atime, mtime)) = self.times {
            word(out, atime);
            word(out, mtime);
        }
    }
}
struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let bytes = self.0.get(..n).ok_or(BAD_MESSAGE)?;
        self.0 = &self.0[n..];
        Ok(bytes)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn bytes(&mut self) -> Result<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn text(&mut self) -> Result<&'a str> {
        let s = core::str::from_utf8(self.bytes()?).map_err(|_| BAD_MESSAGE)?;
        if s.as_bytes().contains(&0) || s.len() > 1023 {
            return Err(BAD_MESSAGE);
        }
        Ok(s)
    }
    fn handle(&mut self) -> Result<u64> {
        let b = self.bytes()?;
        if b.len() != 8 {
            return Err(FAILURE);
        }
        Ok(u64::from_be_bytes(b.try_into().unwrap()))
    }
    fn end(&self) -> Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(BAD_MESSAGE)
        }
    }
}
fn word(out: &mut Vec<u8>, n: u32) {
    out.extend(n.to_be_bytes());
}
fn string(out: &mut Vec<u8>, bytes: &[u8]) {
    word(out, bytes.len() as u32);
    out.extend(bytes);
}
fn response(kind: u8, id: u32) -> Vec<u8> {
    let mut out = vec![kind];
    word(&mut out, id);
    out
}
fn status(id: u32, error: Error) -> Vec<u8> {
    let mut out = response(101, id);
    word(&mut out, error.0);
    string(&mut out, error.1.as_bytes());
    string(&mut out, b"");
    out
}
fn name(out: &mut Vec<u8>, filename: &str, attrs: Attrs) {
    string(out, filename.as_bytes());
    // Long names are display text, never parsed by clients for attributes.
    let mode = attrs.mode.unwrap_or(0);
    let mut perms = [b'-'; 10];
    perms[0] = match mode & 0o170000 {
        0o040000 => b'd',
        0o120000 => b'l',
        _ => b'-',
    };
    for i in 0..9 {
        if mode & (1 << (8 - i)) != 0 {
            perms[i + 1] = b"rwx"[i % 3];
        }
    }
    let long = format!(
        "{} 1 - - {:>10} {}",
        core::str::from_utf8(&perms).unwrap(),
        attrs.size.unwrap_or(0),
        filename
    );
    string(out, long.as_bytes());
    attrs.encode(out);
}

enum Object {
    File(fs::File, u32),
    Directory(fs::Directory),
}
struct Handle {
    id: u64,
    object: Object,
}
pub(super) struct Server {
    home: String,
    input: Vec<u8>,
    initialized: bool,
    next_handle: u64,
    handles: Vec<Handle>,
}
impl Server {
    pub fn new(home: &str) -> Self {
        Self {
            home: String::from(home),
            input: Vec::new(),
            initialized: false,
            next_handle: 1,
            handles: Vec::new(),
        }
    }
    pub fn feed(&mut self, bytes: &[u8]) -> core::result::Result<(), &'static str> {
        if self.input.len().saturating_add(bytes.len()) > MAX_PACKET + 4 + 32 * 1024 {
            return Err("SFTP input limit exceeded");
        }
        self.input.extend_from_slice(bytes);
        Ok(())
    }
    pub fn has_partial_request(&self) -> bool {
        !self.input.is_empty()
    }
    pub fn next_response(&mut self) -> core::result::Result<Option<Vec<u8>>, &'static str> {
        if self.input.len() < 4 {
            return Ok(None);
        }
        let length = u32::from_be_bytes(self.input[..4].try_into().unwrap()) as usize;
        if !(5..=MAX_PACKET).contains(&length) {
            return Err("Invalid SFTP packet length");
        }
        if self.input.len() < length + 4 {
            return Ok(None);
        }
        let packet: Vec<_> = self.input.drain(..length + 4).skip(4).collect();
        let mut r = Reader(&packet[1..]);
        let kind = packet[0];
        let id = r.u32().map_err(|_| "Missing SFTP request ID")?;
        let payload = if kind == 1 {
            if self.initialized || id < 3 {
                return Err("Invalid SFTP initialization");
            }
            // INIT may contain extension name/value pairs.
            while !r.0.is_empty() {
                r.bytes().map_err(|_| "Invalid INIT extension")?;
                r.bytes().map_err(|_| "Invalid INIT extension")?;
            }
            self.initialized = true;
            let mut v = response(2, 3);
            for extension in ["posix-rename@openssh.com", "fsync@openssh.com"] {
                string(&mut v, extension.as_bytes());
                string(&mut v, b"1");
            }
            v
        } else {
            if !self.initialized {
                return Err("SFTP request before initialization");
            }
            match self.request(kind, id, &mut r) {
                Ok(reply) => reply,
                Err(error) => status(id, error),
            }
        };
        let mut framed = Vec::with_capacity(payload.len() + 4);
        word(&mut framed, payload.len() as u32);
        framed.extend(payload);
        Ok(Some(framed))
    }
    fn path(&self, value: &str) -> Result<String> {
        let path = if value.starts_with('/') {
            String::from(value)
        } else if value.is_empty() {
            self.home.clone()
        } else {
            format!("{}/{}", self.home, value)
        };
        if path.len() > 1023 {
            Err(BAD_MESSAGE)
        } else {
            Ok(path)
        }
    }
    fn object(&mut self, id: u64) -> Result<&mut Object> {
        self.handles
            .iter_mut()
            .find(|h| h.id == id)
            .map(|h| &mut h.object)
            .ok_or(Error(4, "Invalid or closed handle"))
    }
    fn insert(&mut self, id: u32, object: Object) -> Result<Vec<u8>> {
        if self.handles.len() >= MAX_HANDLES {
            return Err(Error(4, "Too many open handles"));
        }
        let handle = self.next_handle;
        self.next_handle = self.next_handle.checked_add(1).ok_or(FAILURE)?;
        self.handles.push(Handle { id: handle, object });
        let mut out = response(102, id);
        string(&mut out, &handle.to_be_bytes());
        Ok(out)
    }
    fn request(&mut self, kind: u8, id: u32, r: &mut Reader<'_>) -> Result<Vec<u8>> {
        match kind {
            3 => {
                let path = self.path(r.text()?)?;
                let flags = r.u32()?;
                let attrs = Attrs::read(r)?;
                r.end()?;
                if flags & !63 != 0
                    || flags & 3 == 0
                    || (flags & (4 | 8 | 16) != 0 && flags & 2 == 0)
                    || (flags & 32 != 0 && flags & 8 == 0)
                {
                    return Err(BAD_MESSAGE);
                }
                let mode = attrs.creation_mode(0o600)?;
                // Check admission before opening: rejected requests cannot truncate files.
                if self.handles.len() >= MAX_HANDLES {
                    return Err(Error(4, "Too many open handles"));
                }
                let file = fs::File::open(&path, flags, mode)?;
                self.insert(id, Object::File(file, flags))
            }
            4 => {
                let handle = r.handle()?;
                r.end()?;
                let index = self
                    .handles
                    .iter()
                    .position(|h| h.id == handle)
                    .ok_or(FAILURE)?;
                self.handles.swap_remove(index);
                Ok(status(id, Error(0, "Success")))
            }
            5 => {
                let handle = r.handle()?;
                let offset = r.u64()?;
                let count = r.u32()? as usize;
                r.end()?;
                let Object::File(file, flags) = self.object(handle)? else {
                    return Err(FAILURE);
                };
                if *flags & 1 == 0 {
                    return Err(Error(3, "Handle is not readable"));
                }
                let data = file.read(offset, count.min(MAX_READ))?;
                if data.is_empty() && count != 0 {
                    return Ok(status(id, Error(1, "End of file")));
                }
                let mut out = response(103, id);
                string(&mut out, &data);
                Ok(out)
            }
            6 => {
                let handle = r.handle()?;
                let offset = r.u64()?;
                let data = r.bytes()?;
                r.end()?;
                let Object::File(file, flags) = self.object(handle)? else {
                    return Err(FAILURE);
                };
                if *flags & 2 == 0 {
                    return Err(Error(3, "Handle is not writable"));
                }
                file.write(offset, data, *flags & 4 != 0)?;
                Ok(status(id, Error(0, "Success")))
            }
            7 | 17 => {
                let path = self.path(r.text()?)?;
                r.end()?;
                let attrs = fs::metadata(&path, kind == 7)?;
                let mut out = response(105, id);
                attrs.encode(&mut out);
                Ok(out)
            }
            8 => {
                let handle = r.handle()?;
                r.end()?;
                let attrs = match self.object(handle)? {
                    Object::File(file, _) => file.metadata()?,
                    Object::Directory(dir) => dir.metadata()?,
                };
                let mut out = response(105, id);
                attrs.encode(&mut out);
                Ok(out)
            }
            9 | 10 => {
                if kind == 9 {
                    let path = self.path(r.text()?)?;
                    let attrs = Attrs::read(r)?;
                    r.end()?;
                    fs::set_attrs(&path, attrs)?;
                } else {
                    let handle = r.handle()?;
                    let attrs = Attrs::read(r)?;
                    r.end()?;
                    let Object::File(file, flags) = self.object(handle)? else {
                        return Err(UNSUPPORTED);
                    };
                    if attrs.size.is_some() && *flags & 2 == 0 {
                        return Err(Error(3, "Handle is not writable"));
                    }
                    file.set_attrs(attrs)?;
                }
                Ok(status(id, Error(0, "Success")))
            }
            11 => {
                let path = self.path(r.text()?)?;
                r.end()?;
                if self.handles.len() >= MAX_HANDLES {
                    return Err(Error(4, "Too many open handles"));
                }
                self.insert(id, Object::Directory(fs::Directory::open(&path)?))
            }
            12 => {
                let handle = r.handle()?;
                r.end()?;
                let Object::Directory(dir) = self.object(handle)? else {
                    return Err(FAILURE);
                };
                let mut entries = Vec::new();
                for _ in 0..32 {
                    match dir.next()? {
                        Some(entry) => entries.push(entry),
                        None => break,
                    }
                }
                if entries.is_empty() {
                    return Ok(status(id, Error(1, "End of directory")));
                }
                let mut out = response(104, id);
                word(&mut out, entries.len() as u32);
                for (filename, attrs) in entries {
                    name(&mut out, &filename, attrs);
                }
                Ok(out)
            }
            13 | 15 => {
                let path = self.path(r.text()?)?;
                r.end()?;
                fs::remove(&path, kind == 15)?;
                Ok(status(id, Error(0, "Success")))
            }
            14 => {
                let path = self.path(r.text()?)?;
                let attrs = Attrs::read(r)?;
                r.end()?;
                fs::mkdir(&path, attrs.creation_mode(0o700)?)?;
                Ok(status(id, Error(0, "Success")))
            }
            16 | 19 => {
                let path = self.path(r.text()?)?;
                r.end()?;
                let value = if kind == 16 {
                    fs::canonicalize(&path)?
                } else {
                    fs::readlink(&path)?
                };
                let mut out = response(104, id);
                word(&mut out, 1);
                name(&mut out, &value, Attrs::default());
                Ok(out)
            }
            18 => {
                let from = self.path(r.text()?)?;
                let to = self.path(r.text()?)?;
                r.end()?;
                fs::rename(&from, &to, false)?;
                Ok(status(id, Error(0, "Success")))
            }
            20 => {
                // OpenSSH's established v3 order is target, then link path.
                let target = String::from(r.text()?);
                let path = self.path(r.text()?)?;
                r.end()?;
                fs::symlink(&target, &path)?;
                Ok(status(id, Error(0, "Success")))
            }
            200 => {
                match r.text()? {
                    "posix-rename@openssh.com" => {
                        let from = self.path(r.text()?)?;
                        let to = self.path(r.text()?)?;
                        r.end()?;
                        fs::rename(&from, &to, true)?;
                    }
                    "fsync@openssh.com" => {
                        let handle = r.handle()?;
                        r.end()?;
                        let Object::File(file, _) = self.object(handle)? else {
                            return Err(FAILURE);
                        };
                        file.sync()?;
                    }
                    _ => return Err(UNSUPPORTED),
                }
                Ok(status(id, Error(0, "Success")))
            }
            _ => Err(UNSUPPORTED),
        }
    }
}

#[cfg(test)]
#[path = "sftp/tests.rs"]
mod tests;
