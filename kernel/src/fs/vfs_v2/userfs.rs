//! ABI-neutral transport for filesystems served by a userspace process.
//!
//! The server capability owns connection lifetime; retaining a mount/client
//! does not keep a departed server alive. Wire formats and descriptor numbers
//! belong to adapters. A delivered request remains charged until its reply,
//! even if the caller leaves, so abandoning requests cannot defeat the bound.
use crate::{
    shield::identity::ActiveIdentity,
    sync::{IrqSpinLock, waker::Waker},
};
use alloc::{
    collections::{BTreeMap, VecDeque},
    string::String,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::sync::atomic::{AtomicBool, Ordering};

pub const MAX_REQUESTS: usize = 128;
pub const MAX_DATA: usize = 128 * 1024;
pub const MAX_NAME: usize = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Disconnected,
    Busy,
    Invalid,
    Stale,
    TooLarge,
    Interrupted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    File,
    Directory,
    Symlink,
}

#[derive(Clone, Debug)]
pub struct Attributes {
    pub node: u64,
    pub generation: u64,
    pub kind: NodeKind,
    pub permissions: u16,
    pub owner: ActiveIdentity,
    pub links: u32,
    pub size: u64,
    pub accessed_ns: u64,
    pub modified_ns: u64,
    pub changed_ns: u64,
}

#[derive(Clone, Debug)]
pub struct DirectoryItem {
    pub node: u64,
    pub kind: NodeKind,
    pub name: String,
    pub next_cookie: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct OpenMode {
    pub read: bool,
    pub write: bool,
    pub append: bool,
    pub directory: bool,
}

#[derive(Clone, Debug)]
pub enum Operation {
    Lookup {
        parent: u64,
        name: String,
    },
    Attributes {
        node: u64,
    },
    ReadLink {
        node: u64,
    },
    Open {
        node: u64,
        mode: OpenMode,
    },
    Close {
        node: u64,
        handle: u64,
        directory: bool,
    },
    Read {
        node: u64,
        handle: u64,
        offset: u64,
        size: usize,
    },
    Write {
        node: u64,
        handle: u64,
        offset: u64,
        data: Vec<u8>,
    },
    ReadDirectory {
        node: u64,
        handle: u64,
        cookie: u64,
        size: usize,
    },
    Flush {
        node: u64,
        handle: u64,
    },
}

#[derive(Clone, Debug)]
#[must_use]
pub enum Reply {
    Attributes(Attributes),
    Link(String),
    Opened { handle: u64 },
    Data(Vec<u8>),
    Written(usize),
    Directory(Vec<DirectoryItem>),
    Done,
    Failed(crate::fs::FileSystemErrorKind),
}

fn name_valid(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME && !name.contains(['/', '\0'])
}
impl Operation {
    fn validate(&self) -> Result<(), Error> {
        let node = match self {
            Self::Lookup { parent, name } => {
                if !name_valid(name) || name == "." || name == ".." {
                    return Err(Error::Invalid);
                }
                *parent
            }
            Self::Attributes { node }
            | Self::ReadLink { node }
            | Self::Close { node, .. }
            | Self::Flush { node, .. } => *node,
            Self::Open { node, mode } => {
                if !mode.read && !mode.write || mode.append && !mode.write {
                    return Err(Error::Invalid);
                }
                *node
            }
            Self::Read {
                node, offset, size, ..
            } => {
                if *size > MAX_DATA {
                    return Err(Error::TooLarge);
                }
                if offset.checked_add(*size as u64).is_none() {
                    return Err(Error::Invalid);
                }
                *node
            }
            Self::ReadDirectory { node, size, .. } => {
                if *size > MAX_DATA {
                    return Err(Error::TooLarge);
                }
                *node
            }
            Self::Write {
                node, offset, data, ..
            } => {
                if data.len() > MAX_DATA {
                    return Err(Error::TooLarge);
                }
                if offset.checked_add(data.len() as u64).is_none() {
                    return Err(Error::Invalid);
                }
                *node
            }
        };
        if node == 0 {
            Err(Error::Invalid)
        } else {
            Ok(())
        }
    }
    fn accepts(&self, reply: &Reply) -> bool {
        match (self, reply) {
            (_, Reply::Failed(_)) => true,
            (Self::Lookup { .. } | Self::Attributes { .. }, Reply::Attributes(a)) => {
                a.node != 0
                    && a.permissions & !0o7777 == 0
                    && match self {
                        Self::Attributes { node } => *node == a.node,
                        _ => true,
                    }
            }
            (Self::ReadLink { .. }, Reply::Link(target)) => {
                !target.is_empty()
                    && target.len() <= crate::fs::MAX_PATH_LENGTH
                    && !target.contains('\0')
            }
            (Self::Open { .. }, Reply::Opened { .. }) => true,
            (Self::Close { .. } | Self::Flush { .. }, Reply::Done) => true,
            (Self::Read { size, .. }, Reply::Data(data)) => data.len() <= *size,
            (Self::Write { data, .. }, Reply::Written(size)) => *size <= data.len(),
            (Self::ReadDirectory { size, .. }, Reply::Directory(entries)) => {
                let mut bytes = 0usize;
                entries.iter().all(|entry| {
                    bytes = bytes.saturating_add(24 + entry.name.len());
                    entry.node != 0 && name_valid(&entry.name) && bytes <= *size
                })
            }
            _ => false,
        }
    }
}

/// Credentials are captured by the kernel when a request is admitted.
#[derive(Debug)]
pub struct Request {
    id: u64,
    task_id: usize,
    identity: ActiveIdentity,
    operation: Operation,
}
impl Request {
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn native_task_id(&self) -> usize {
        self.task_id
    }
    pub fn identity(&self) -> ActiveIdentity {
        self.identity
    }
    pub fn operation(&self) -> &Operation {
        &self.operation
    }
}

struct Pending {
    request: Arc<Request>,
    delivered: bool,
    replied: bool,
    abandoned: bool,
    completion: Arc<Completion>,
}
struct Completion {
    result: IrqSpinLock<Option<Result<Reply, Error>>>,
    ready: Waker,
}
struct State {
    connected: bool,
    next_id: u64,
    queue: VecDeque<u64>,
    pending: BTreeMap<u64, Pending>,
    abandoned_replies: VecDeque<AbandonedReply>,
}
pub struct Connection {
    state: IrqSpinLock<State>,
    available: Waker,
}

/// Share this capability with Arc for dup/fork. Dropping the last server
/// disconnects all clients, including requests already handed to the server.
pub struct Server {
    connection: Arc<Connection>,
}

pub struct Ticket {
    id: u64,
    connection: Weak<Connection>,
    completion: Arc<Completion>,
    taken: AtomicBool,
}

/// A reply to an abandoned delivered call can own remote resources (an open
/// handle or lookup reference). The adapter must reclaim these before discarding
/// it. Returning the reply, rather than silently dropping it, makes that explicit.
#[must_use]
pub enum ReplyDisposition {
    Completed,
    Abandoned(AbandonedReply),
}
#[must_use]
pub struct AbandonedReply {
    pub request: Arc<Request>,
    pub reply: Reply,
}

impl Connection {
    pub fn new() -> (Arc<Self>, Arc<Server>) {
        let connection = Arc::new(Self {
            state: IrqSpinLock::new(State {
                connected: true,
                next_id: 1,
                queue: VecDeque::new(),
                pending: BTreeMap::new(),
                abandoned_replies: VecDeque::new(),
            }),
            available: Waker::new_interruptible("userfs_requests"),
        });
        let server = Arc::new(Server {
            connection: connection.clone(),
        });
        (connection, server)
    }

    pub fn submit(
        self: &Arc<Self>,
        operation: Operation,
        task: &crate::task::Task,
    ) -> Result<Ticket, Error> {
        let identity = task.credentials.read().identity();
        self.submit_from(operation, identity, task.get_id())
    }

    fn submit_as(
        self: &Arc<Self>,
        operation: Operation,
        identity: ActiveIdentity,
    ) -> Result<Ticket, Error> {
        self.submit_from(operation, identity, 0)
    }
    fn submit_from(
        self: &Arc<Self>,
        operation: Operation,
        identity: ActiveIdentity,
        task_id: usize,
    ) -> Result<Ticket, Error> {
        operation.validate()?;
        let completion = Arc::new(Completion {
            result: IrqSpinLock::new(None),
            ready: Waker::new_interruptible("userfs_reply"),
        });
        let id = {
            let mut state = self.state.lock();
            if !state.connected {
                return Err(Error::Disconnected);
            }
            if state.pending.len() + state.abandoned_replies.len() >= MAX_REQUESTS {
                return Err(Error::Busy);
            }
            let id = state.next_id;
            // Never reuse a correlation identifier, even after cancellation.
            state.next_id = id.checked_add(1).ok_or(Error::Busy)?;
            let request = Arc::new(Request {
                id,
                task_id,
                identity,
                operation,
            });
            state.pending.insert(
                id,
                Pending {
                    request,
                    delivered: false,
                    replied: false,
                    abandoned: false,
                    completion: completion.clone(),
                },
            );
            state.queue.push_back(id);
            id
        };
        self.available.wake_all();
        Ok(Ticket {
            id,
            connection: Arc::downgrade(self),
            completion,
            taken: AtomicBool::new(false),
        })
    }

    pub fn disconnect(&self) {
        let pending = {
            let mut state = self.state.lock();
            state.connected = false;
            state.queue.clear();
            // Server teardown invalidates its remote handles and node refs.
            state.abandoned_replies.clear();
            core::mem::take(&mut state.pending)
        };
        for (_, pending) in pending {
            *pending.completion.result.lock() = Some(Err(Error::Disconnected));
            pending.completion.ready.wake_all();
        }
        self.available.wake_all();
    }
}

impl Server {
    pub fn request(&self, id: u64) -> Option<Arc<Request>> {
        self.connection
            .state
            .lock()
            .pending
            .get(&id)
            .map(|pending| pending.request.clone())
    }
    /// Obtain the next request without consuming it. The adapter can size and
    /// encode a read outside the queue lock; a short read leaves it queued.
    pub fn peek(&self) -> Result<Option<Arc<Request>>, Error> {
        let state = self.connection.state.lock();
        if !state.connected {
            return Err(Error::Disconnected);
        }
        Ok(state
            .queue
            .front()
            .map(|id| state.pending[id].request.clone()))
    }
    /// Claim the peeked request after preparing its representation. Competing
    /// readers retry on Stale; no queue lock spans userspace copies or waits.
    pub fn deliver(&self, id: u64) -> Result<(), Error> {
        let mut state = self.connection.state.lock();
        if !state.connected {
            return Err(Error::Disconnected);
        }
        if state.queue.front().copied() != Some(id) {
            return Err(Error::Stale);
        }
        state.queue.pop_front();
        state.pending.get_mut(&id).ok_or(Error::Stale)?.delivered = true;
        Ok(())
    }
    pub fn reply(&self, id: u64, reply: Reply) -> Result<ReplyDisposition, Error> {
        let mut state = self.connection.state.lock();
        if !state.connected {
            return Err(Error::Disconnected);
        }
        let pending = state.pending.get(&id).ok_or(Error::Stale)?;
        if pending.replied {
            return Err(Error::Stale);
        }
        if !pending.delivered || !pending.request.operation.accepts(&reply) {
            return Err(Error::Invalid);
        }
        if pending.abandoned {
            let pending = state.pending.remove(&id).unwrap();
            drop(state);
            return Ok(ReplyDisposition::Abandoned(AbandonedReply {
                request: pending.request,
                reply,
            }));
        }
        let pending = state.pending.get_mut(&id).unwrap();
        pending.replied = true;
        *pending.completion.result.lock() = Some(Ok(reply));
        let completion = pending.completion.clone();
        drop(state);
        completion.ready.wake_all();
        Ok(ReplyDisposition::Completed)
    }
    /// Resource replies abandoned after publication are still charged to the
    /// connection until the adapter accepts them for remote cleanup.
    pub fn take_abandoned_reply(&self) -> Option<AbandonedReply> {
        self.connection.state.lock().abandoned_replies.pop_front()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.connection.disconnect();
    }
}

impl Ticket {
    pub fn try_take(&self) -> Option<Result<Reply, Error>> {
        if self.taken.load(Ordering::Acquire) {
            return Some(Err(Error::Stale));
        }
        // Same order as publication/drop: connection before completion.
        let connection = self.connection.upgrade();
        let mut state = connection
            .as_ref()
            .map(|connection| connection.state.lock());
        if self.taken.load(Ordering::Acquire) {
            return Some(Err(Error::Stale));
        }
        let result = self.completion.result.lock().take();
        if result.is_some() {
            self.taken.store(true, Ordering::Release);
            if let Some(state) = state.as_mut() {
                state.pending.remove(&self.id);
            }
        }
        result
    }
    pub fn wait(
        &self,
        task: &crate::task::Task,
        tf: &mut crate::arch::Trapframe,
    ) -> Result<Reply, Error> {
        loop {
            if let Some(result) = self.try_take() {
                return result;
            }
            let waited =
                self.completion
                    .ready
                    .wait_with_condition(task.get_id(), tf, None, 0, || {
                        self.completion.result.lock().is_some()
                    });
            if waited == crate::sync::waker::WaitResult::Interrupted {
                return Err(Error::Interrupted);
            }
        }
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        let Some(connection) = self.connection.upgrade() else {
            return;
        };
        let mut state = connection.state.lock();
        let Some(pending) = state.pending.get_mut(&self.id) else {
            return;
        };
        if pending.replied {
            let pending = state.pending.remove(&self.id).unwrap();
            if let Some(Ok(reply)) = pending.completion.result.lock().take() {
                if matches!(reply, Reply::Opened { .. })
                    || matches!(pending.request.operation, Operation::Lookup { .. })
                        && matches!(reply, Reply::Attributes(_))
                {
                    state.abandoned_replies.push_back(AbandonedReply {
                        request: pending.request.clone(),
                        reply,
                    });
                }
            }
            drop(state);
            drop(pending);
            connection.available.wake_all();
        } else if pending.delivered {
            pending.abandoned = true;
        } else {
            let pending = state.pending.remove(&self.id);
            state.queue.retain(|id| *id != self.id);
            drop(state);
            drop(pending);
        }
    }
}

impl crate::object::capability::Selectable for Server {
    fn current_ready(
        &self,
        interest: crate::object::capability::ReadyInterest,
    ) -> crate::object::capability::ReadySet {
        let state = self.connection.state.lock();
        let mut ready = crate::object::capability::ReadySet::none();
        ready.read = interest.read
            && (!state.connected || !state.queue.is_empty() || !state.abandoned_replies.is_empty());
        ready
    }
    fn wait_until_ready(
        &self,
        interest: crate::object::capability::ReadyInterest,
        tf: &mut crate::arch::Trapframe,
        timeout: Option<u64>,
        minimum: u64,
    ) -> crate::object::capability::SelectWaitOutcome {
        use crate::object::capability::SelectWaitOutcome;
        let Some(task) = crate::task::mytask() else {
            return SelectWaitOutcome::TimedOut;
        };
        match self.connection.available.wait_with_condition(
            task.get_id(),
            tf,
            timeout,
            minimum,
            || self.current_ready(interest).read,
        ) {
            crate::sync::waker::WaitResult::TimedOut => SelectWaitOutcome::TimedOut,
            _ => SelectWaitOutcome::Ready,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn submit(connection: &Arc<Connection>) -> Ticket {
        connection
            .submit_as(
                Operation::Read {
                    node: 1,
                    handle: 3,
                    offset: 0,
                    size: 4,
                },
                ActiveIdentity {
                    principal: crate::shield::identity::Principal(1200),
                    group: crate::shield::identity::Group(1300),
                },
            )
            .unwrap()
    }
    #[test_case]
    fn correlation_validation_and_short_server_reads_preserve_pending_requests() {
        let (connection, server) = Connection::new();
        let first = submit(&connection);
        let second = submit(&connection);
        let request = server.peek().unwrap().unwrap();
        assert_eq!(request.identity().principal.0, 1200);
        assert_eq!(server.peek().unwrap().unwrap().id(), request.id());
        assert!(matches!(
            server.reply(request.id(), Reply::Data(alloc::vec![1])),
            Err(Error::Invalid)
        ));
        server.deliver(request.id()).unwrap();
        assert_eq!(server.deliver(request.id()), Err(Error::Stale));
        assert!(matches!(
            server.reply(request.id(), Reply::Data(alloc::vec![0;5])),
            Err(Error::Invalid)
        ));
        assert!(matches!(
            server.reply(request.id(), Reply::Written(1)),
            Err(Error::Invalid)
        ));
        assert!(matches!(
            server.reply(request.id(), Reply::Data(alloc::vec![2, 3])),
            Ok(ReplyDisposition::Completed)
        ));
        assert!(matches!(first.try_take(), Some(Ok(Reply::Data(data))) if data == [2,3]));
        assert!(second.try_take().is_none());
        assert!(matches!(
            server.reply(request.id(), Reply::Data(Vec::new())),
            Err(Error::Stale)
        ));
    }
    #[test_case]
    fn server_lifetime_disconnects_queued_and_delivered_calls() {
        let (connection, server) = Connection::new();
        let duplicate = server.clone();
        let first = submit(&connection);
        server
            .deliver(server.peek().unwrap().unwrap().id())
            .unwrap();
        let second = submit(&connection);
        drop(server);
        assert!(first.try_take().is_none());
        drop(duplicate);
        assert!(matches!(first.try_take(), Some(Err(Error::Disconnected))));
        assert!(matches!(second.try_take(), Some(Err(Error::Disconnected))));
        assert!(connection.state.lock().pending.is_empty());
    }
    #[test_case]
    fn abandoned_deliveries_remain_bounded_and_return_resources_to_adapter() {
        let (connection, server) = Connection::new();
        let cancelled = submit(&connection);
        let cancelled_id = server.peek().unwrap().unwrap().id();
        drop(cancelled);
        assert!(server.peek().unwrap().is_none());
        let next = submit(&connection);
        let request = server.peek().unwrap().unwrap();
        assert!(request.id() > cancelled_id);
        server.deliver(request.id()).unwrap();
        drop(next);
        assert_eq!(connection.state.lock().pending.len(), 1);
        assert!(matches!(
            server.reply(request.id(), Reply::Data(alloc::vec![1])),
            Ok(ReplyDisposition::Abandoned(AbandonedReply {
                reply: Reply::Data(_),
                ..
            }))
        ));
        assert!(connection.state.lock().pending.is_empty());
        let requests: Vec<_> = (0..MAX_REQUESTS).map(|_| submit(&connection)).collect();
        let identity = server.peek().unwrap().unwrap().identity();
        assert!(matches!(
            connection.submit_as(Operation::Attributes { node: 1 }, identity),
            Err(Error::Busy)
        ));
        drop(requests);
        assert!(connection.state.lock().pending.is_empty());
    }
    #[test_case]
    fn cancellation_after_reply_publication_retains_remote_open_for_cleanup() {
        let (connection, server) = Connection::new();
        let identity = ActiveIdentity {
            principal: crate::shield::identity::Principal(1),
            group: crate::shield::identity::Group(2),
        };
        let ticket = connection
            .submit_as(
                Operation::Open {
                    node: 1,
                    mode: OpenMode {
                        read: true,
                        write: false,
                        append: false,
                        directory: false,
                    },
                },
                identity,
            )
            .unwrap();
        let id = server.peek().unwrap().unwrap().id();
        server.deliver(id).unwrap();
        assert!(matches!(
            server.reply(id, Reply::Opened { handle: 77 }),
            Ok(ReplyDisposition::Completed)
        ));
        assert!(matches!(
            server.reply(id, Reply::Opened { handle: 88 }),
            Err(Error::Stale)
        ));
        drop(ticket);
        assert_eq!(connection.state.lock().abandoned_replies.len(), 1);
        let abandoned = server.take_abandoned_reply().unwrap();
        assert!(matches!(abandoned.reply, Reply::Opened { handle: 77 }));
        assert!(matches!(
            abandoned.request.operation(),
            Operation::Open { node: 1, .. }
        ));
        assert!(server.take_abandoned_reply().is_none());
    }
}
