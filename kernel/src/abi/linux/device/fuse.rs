//! FUSE 7 wire conversion for the native userspace filesystem transport.
//! Layouts follow Linux v6.1 include/uapi/linux/fuse.h. Only the negotiated
//! baseline is represented here; no cache, splice, ioctl or locking feature
//! is advertised by this codec.
use crate::fs::{
    FileSystemErrorKind,
    vfs_v2::userfs::{self, Attributes, DirectoryItem, NodeKind, Operation, Reply, Request},
};
use crate::shield::identity::{ActiveIdentity, Group, Principal};
use alloc::{string::String, vec, vec::Vec};

pub const INIT_ID: u64 = u64::MAX;
pub const MINOR: u32 = 31;
const HEADER: usize = 40;
const OUT_HEADER: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Malformed,
    Unsupported,
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    let data = bytes
        .get(offset..offset.checked_add(4).ok_or(Error::Malformed)?)
        .ok_or(Error::Malformed)?;
    Ok(u32::from_le_bytes(data.try_into().unwrap()))
}
fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, Error> {
    let data = bytes
        .get(offset..offset.checked_add(8).ok_or(Error::Malformed)?)
        .ok_or(Error::Malformed)?;
    Ok(u64::from_le_bytes(data.try_into().unwrap()))
}
fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
fn packet(
    opcode: u32,
    id: u64,
    node: u64,
    identity: ActiveIdentity,
    pid: u32,
    body: &[u8],
) -> Vec<u8> {
    let mut bytes = vec![0; HEADER + body.len()];
    let size = bytes.len() as u32;
    put32(&mut bytes, 0, size);
    put32(&mut bytes, 4, opcode);
    put64(&mut bytes, 8, id);
    put64(&mut bytes, 16, node);
    put32(&mut bytes, 24, identity.principal.0);
    put32(&mut bytes, 28, identity.group.0);
    put32(&mut bytes, 32, pid);
    bytes[HEADER..].copy_from_slice(body);
    bytes
}

pub fn init_request(identity: ActiveIdentity, pid: u32) -> Vec<u8> {
    let mut body = [0; 16];
    put32(&mut body, 0, 7);
    put32(&mut body, 4, MINOR);
    packet(26, INIT_ID, 0, identity, pid, &body)
}

pub struct Header<'a> {
    pub id: u64,
    pub error: i32,
    pub body: &'a [u8],
}
pub fn header(bytes: &[u8]) -> Result<Header<'_>, Error> {
    if bytes.len() < OUT_HEADER
        || bytes.len() > userfs::MAX_DATA + 160
        || u32_at(bytes, 0)? as usize != bytes.len()
    {
        return Err(Error::Malformed);
    }
    let error = u32_at(bytes, 4)? as i32;
    let id = u64_at(bytes, 8)?;
    if id == 0 || !(-4095..=0).contains(&error) || error != 0 && bytes.len() != OUT_HEADER {
        return Err(Error::Malformed);
    }
    Ok(Header {
        id,
        error,
        body: &bytes[OUT_HEADER..],
    })
}

pub struct Negotiated {
    pub minor: u32,
    pub max_write: usize,
}
pub fn init_reply(bytes: &[u8]) -> Result<Negotiated, Error> {
    let response = header(bytes)?;
    if response.id != INIT_ID || response.error != 0 || response.body.len() != 64 {
        return Err(Error::Malformed);
    }
    let body = response.body;
    let minor = u32_at(body, 4)?;
    if u32_at(body, 0)? != 7
        || !(23..=MINOR).contains(&minor)
        || u32_at(body, 12)? != 0
        || body[28..].iter().any(|byte| *byte != 0)
    {
        return Err(Error::Unsupported);
    }
    let max_write = u32_at(body, 20)? as usize;
    if max_write == 0 {
        return Err(Error::Malformed);
    }
    Ok(Negotiated {
        minor,
        max_write: max_write.min(userfs::MAX_DATA),
    })
}

/// visible_tid must be translated in the mount server's PID namespace. The
/// authenticated native task identity remains available on Request.
pub fn encode(request: &Request, visible_tid: u32) -> Vec<u8> {
    let mut body = Vec::new();
    let (opcode, node) = match request.operation() {
        Operation::Lookup { parent, name } => {
            body.extend_from_slice(name.as_bytes());
            body.push(0);
            (1, *parent)
        }
        Operation::Attributes { node } => {
            body.resize(16, 0);
            (3, *node)
        }
        Operation::ReadLink { node } => (5, *node),
        Operation::Open { node, mode } => {
            body.resize(8, 0);
            let flags = if mode.read && mode.write {
                2
            } else if mode.write {
                1
            } else {
                0
            } | if mode.append { 0x400 } else { 0 };
            put32(&mut body, 0, flags);
            (if mode.directory { 27 } else { 14 }, *node)
        }
        Operation::Close {
            node,
            handle,
            directory,
        } => {
            body.resize(24, 0);
            put64(&mut body, 0, *handle);
            (if *directory { 29 } else { 18 }, *node)
        }
        Operation::Flush { node, handle } => {
            body.resize(24, 0);
            put64(&mut body, 0, *handle);
            (25, *node)
        }
        Operation::Read {
            node,
            handle,
            offset,
            size,
        } => {
            body.resize(40, 0);
            put64(&mut body, 0, *handle);
            put64(&mut body, 8, *offset);
            put32(&mut body, 16, *size as u32);
            (15, *node)
        }
        Operation::ReadDirectory {
            node,
            handle,
            cookie,
            size,
        } => {
            body.resize(40, 0);
            put64(&mut body, 0, *handle);
            put64(&mut body, 8, *cookie);
            put32(&mut body, 16, *size as u32);
            (28, *node)
        }
        Operation::Write {
            node,
            handle,
            offset,
            data,
        } => {
            body.resize(40, 0);
            put64(&mut body, 0, *handle);
            put64(&mut body, 8, *offset);
            put32(&mut body, 16, data.len() as u32);
            body.extend_from_slice(data);
            (16, *node)
        }
    };
    packet(
        opcode,
        request.id(),
        node,
        request.identity(),
        visible_tid,
        &body,
    )
}

fn file_kind(mode: u32) -> Result<NodeKind, Error> {
    match mode & 0o170000 {
        0o100000 => Ok(NodeKind::File),
        0o040000 => Ok(NodeKind::Directory),
        0o120000 => Ok(NodeKind::Symlink),
        _ => Err(Error::Unsupported),
    }
}
fn attrs(bytes: &[u8], node: u64, generation: u64) -> Result<Attributes, Error> {
    if bytes.len() != 88 || node == 0 {
        return Err(Error::Malformed);
    }
    let time = |seconds, nanos| -> Result<u64, Error> {
        let nanos = u32_at(bytes, nanos)?;
        if nanos >= 1_000_000_000 {
            return Err(Error::Malformed);
        }
        u64_at(bytes, seconds)?
            .checked_mul(1_000_000_000)
            .and_then(|value| value.checked_add(nanos as u64))
            .ok_or(Error::Malformed)
    };
    let mode = u32_at(bytes, 60)?;
    if u32_at(bytes, 84)? != 0 {
        return Err(Error::Unsupported);
    }
    Ok(Attributes {
        node,
        generation,
        kind: file_kind(mode)?,
        permissions: (mode & 0o7777) as u16,
        owner: ActiveIdentity {
            principal: Principal(u32_at(bytes, 68)?),
            group: Group(u32_at(bytes, 72)?),
        },
        links: u32_at(bytes, 64)?,
        size: u64_at(bytes, 8)?,
        accessed_ns: time(24, 48)?,
        modified_ns: time(32, 52)?,
        changed_ns: time(40, 56)?,
    })
}
fn failure(errno: i32) -> FileSystemErrorKind {
    use FileSystemErrorKind::*;
    match errno {
        1 | 13 => PermissionDenied,
        2 => NotFound,
        16 | 11 => Busy,
        17 => AlreadyExists,
        18 => CrossDevice,
        20 => NotADirectory,
        21 => IsADirectory,
        22 => InvalidOperation,
        28 => NoSpace,
        30 => ReadOnly,
        36 => InvalidPath,
        38 | 95 => NotSupported,
        39 => DirectoryNotEmpty,
        40 => TooManySymlinks,
        75 => ValueOverflow,
        _ => IoError,
    }
}
pub fn decode(request: &Request, bytes: &[u8]) -> Result<Reply, Error> {
    let response = header(bytes)?;
    if response.id != request.id() {
        return Err(Error::Malformed);
    }
    if response.error != 0 {
        return Ok(Reply::Failed(failure(-response.error)));
    }
    let body = response.body;
    let reply = match request.operation() {
        Operation::Lookup { .. } => {
            if body.len() != 128 {
                return Err(Error::Malformed);
            }
            let node = u64_at(body, 0)?;
            if node == 0 {
                return Ok(Reply::Failed(FileSystemErrorKind::NotFound));
            }
            Reply::Attributes(attrs(&body[40..], node, u64_at(body, 8)?)?)
        }
        Operation::Attributes { node } => {
            if body.len() != 104 {
                return Err(Error::Malformed);
            }
            Reply::Attributes(attrs(&body[16..], *node, 0)?)
        }
        Operation::ReadLink { .. } => {
            if body.is_empty() || body.len() > crate::fs::MAX_PATH_LENGTH || body.contains(&0) {
                return Err(Error::Malformed);
            }
            Reply::Link(String::from_utf8(body.to_vec()).map_err(|_| Error::Malformed)?)
        }
        Operation::Open { .. } => {
            if body.len() != 16 {
                return Err(Error::Malformed);
            }
            // DIRECT_IO works with uncached I/O; KEEP_CACHE is merely a hint
            // when there is no data cache. Other semantics need implementation.
            if u32_at(body, 8)? & !3 != 0 {
                return Err(Error::Unsupported);
            }
            Reply::Opened {
                handle: u64_at(body, 0)?,
            }
        }
        Operation::Close { .. } | Operation::Flush { .. } => {
            if !body.is_empty() {
                return Err(Error::Malformed);
            }
            Reply::Done
        }
        Operation::Read { size, .. } => {
            if body.len() > *size {
                return Err(Error::Malformed);
            }
            Reply::Data(body.to_vec())
        }
        Operation::Write { data, .. } => {
            if body.len() != 8 {
                return Err(Error::Malformed);
            }
            let count = u32_at(body, 0)? as usize;
            if count > data.len() {
                return Err(Error::Malformed);
            }
            Reply::Written(count)
        }
        Operation::ReadDirectory { size, cookie, .. } => {
            if body.len() > *size {
                return Err(Error::Malformed);
            }
            let mut entries = Vec::new();
            let mut offset = 0;
            let mut previous = *cookie;
            while offset < body.len() {
                let record = &body[offset..];
                let node = u64_at(record, 0)?;
                let next_cookie = u64_at(record, 8)?;
                let length = u32_at(record, 16)? as usize;
                let kind = match u32_at(record, 20)? {
                    4 => NodeKind::Directory,
                    8 => NodeKind::File,
                    10 => NodeKind::Symlink,
                    _ => return Err(Error::Unsupported),
                };
                let length_aligned =
                    (24usize.checked_add(length).ok_or(Error::Malformed)? + 7) & !7;
                let name = record.get(24..24 + length).ok_or(Error::Malformed)?;
                if length == 0
                    || length > userfs::MAX_NAME
                    || name.contains(&0)
                    || name.contains(&b'/')
                    || node == 0
                    || next_cookie == previous
                    || length_aligned > record.len()
                {
                    return Err(Error::Malformed);
                }
                entries.push(DirectoryItem {
                    node,
                    kind,
                    name: String::from_utf8(name.to_vec()).map_err(|_| Error::Malformed)?,
                    next_cookie,
                });
                previous = next_cookie;
                offset += length_aligned;
            }
            Reply::Directory(entries)
        }
    };
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    fn queued(operation: Operation) -> (Arc<userfs::Server>, userfs::Ticket, Arc<Request>) {
        let mut task = crate::task::Task::new("fuse-codec".into(), 0, crate::task::TaskType::User);
        task.set_id(991);
        let (connection, server) = userfs::Connection::new();
        let ticket = connection.submit(operation, &task).unwrap();
        let request = server.peek().unwrap().unwrap();
        server.deliver(request.id()).unwrap();
        (server, ticket, request)
    }
    fn response(id: u64, body: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; 16 + body.len()];
        let length = bytes.len() as u32;
        put32(&mut bytes, 0, length);
        put64(&mut bytes, 8, id);
        bytes[16..].copy_from_slice(body);
        bytes
    }
    #[test_case]
    fn init_request_uses_linux_header_and_does_not_advertise_unimplemented_features() {
        let bytes = init_request(
            ActiveIdentity {
                principal: Principal(1023),
                group: Group(1023),
            },
            42,
        );
        assert_eq!(bytes.len(), 56);
        assert_eq!(&bytes[..8], &[56, 0, 0, 0, 26, 0, 0, 0]);
        assert_eq!(&bytes[8..16], &[255; 8]);
        assert_eq!(u32_at(&bytes, 24).unwrap(), 1023);
        assert_eq!(u32_at(&bytes, 32).unwrap(), 42);
        assert_eq!(u32_at(&bytes, 40).unwrap(), 7);
        assert_eq!(u32_at(&bytes, 44).unwrap(), 31);
        assert!(bytes[48..].iter().all(|b| *b == 0));
    }
    #[test_case]
    fn init_negotiation_validates_version_flags_and_write_bound() {
        let mut bytes = vec![0; 80];
        put32(&mut bytes, 0, 80);
        put64(&mut bytes, 8, INIT_ID);
        put32(&mut bytes, 16, 7);
        put32(&mut bytes, 20, 31);
        put32(&mut bytes, 36, 1024 * 1024);
        assert_eq!(init_reply(&bytes).unwrap().max_write, userfs::MAX_DATA);
        put32(&mut bytes, 28, 1);
        assert!(matches!(init_reply(&bytes), Err(Error::Unsupported)));
        put32(&mut bytes, 28, 0);
        put32(&mut bytes, 20, 22);
        assert!(matches!(init_reply(&bytes), Err(Error::Unsupported)));
    }
    #[test_case]
    fn reply_header_rejects_short_overlong_and_notification_records() {
        let mut bytes = vec![0; 16];
        put32(&mut bytes, 0, 16);
        put64(&mut bytes, 8, 1);
        assert!(header(&bytes).is_ok());
        assert!(header(&bytes[..15]).is_err());
        bytes.push(0);
        assert!(header(&bytes).is_err());
        bytes.pop();
        put32(&mut bytes, 4, 1);
        assert!(header(&bytes).is_err());
        put32(&mut bytes, 4, (-4096i32) as u32);
        assert!(header(&bytes).is_err());
        put32(&mut bytes, 4, (-2i32) as u32);
        assert_eq!(header(&bytes).unwrap().error, -2);
        put64(&mut bytes, 8, 0);
        assert!(header(&bytes).is_err());
    }
    #[test_case]
    fn read_reply_is_correlated_and_bounded_before_native_completion() {
        let (server, ticket, request) = queued(Operation::Read {
            node: 17,
            handle: 91,
            offset: 8192,
            size: 3,
        });
        let message = encode(&request, 77);
        assert_eq!(message.len(), 80);
        assert_eq!(u32_at(&message, 4).unwrap(), 15);
        assert_eq!(u64_at(&message, 16).unwrap(), 17);
        assert_eq!(u64_at(&message, 40).unwrap(), 91);
        assert_eq!(u64_at(&message, 48).unwrap(), 8192);
        assert_eq!(u32_at(&message, 56).unwrap(), 3);
        assert!(decode(&request, &response(request.id() + 1, b"abc")).is_err());
        assert!(decode(&request, &response(request.id(), b"abcd")).is_err());
        let reply = decode(&request, &response(request.id(), b"abc")).unwrap();
        assert!(matches!(
            server.reply(request.id(), reply),
            Ok(userfs::ReplyDisposition::Completed)
        ));
        assert!(matches!(ticket.try_take(),Some(Ok(Reply::Data(data))) if data==b"abc"));
    }
    #[test_case]
    fn directory_records_require_padding_name_bounds_and_progress() {
        let (server, _ticket, request) = queued(Operation::ReadDirectory {
            node: 1,
            handle: 2,
            cookie: 0,
            size: 64,
        });
        let mut body = vec![0; 32];
        put64(&mut body, 0, 9);
        put64(&mut body, 8, 10);
        put32(&mut body, 16, 3);
        put32(&mut body, 20, 8);
        body[24..27].copy_from_slice(b"apk");
        assert!(
            matches!(decode(&request,&response(request.id(),&body)),Ok(Reply::Directory(items)) if items.len()==1 && items[0].next_cookie==10)
        );
        let reply = decode(&request, &response(request.id(), &body)).unwrap();
        assert!(matches!(
            server.reply(request.id(), reply),
            Ok(userfs::ReplyDisposition::Completed)
        ));
        assert!(decode(&request, &response(request.id(), &body[..27])).is_err());
        put64(&mut body, 8, 0);
        assert!(decode(&request, &response(request.id(), &body)).is_err());
        put64(&mut body, 8, 10);
        put32(&mut body, 16, u32::MAX);
        assert!(decode(&request, &response(request.id(), &body)).is_err());
    }
    #[test_case]
    fn lookup_preserves_remote_generation_and_rejects_invalid_timestamps() {
        let (_server, _ticket, request) = queued(Operation::Lookup {
            parent: 1,
            name: "file".into(),
        });
        let mut body = vec![0; 128];
        put64(&mut body, 0, 99);
        put64(&mut body, 8, 7);
        put64(&mut body, 48, 4096);
        put32(&mut body, 100, 0o100640);
        put32(&mut body, 104, 1);
        put32(&mut body, 108, 1200);
        put32(&mut body, 112, 1300);
        let reply = decode(&request, &response(request.id(), &body)).unwrap();
        assert!(
            matches!(reply,Reply::Attributes(a) if a.node==99 && a.generation==7 && a.size==4096 && a.owner.principal.0==1200)
        );
        put32(&mut body, 88, 1_000_000_000);
        assert!(decode(&request, &response(request.id(), &body)).is_err());
    }
}
