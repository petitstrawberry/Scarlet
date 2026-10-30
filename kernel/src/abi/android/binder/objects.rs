//! Binder object identity, reference accounting and wire translation. Native
//! RPC owns delivery; these leases retain adapter references while in flight.
use super::*;
use crate::ipc::rpc::{MAX_MESSAGE_OBJECTS, RpcObject};

pub(super) const HEADER: usize = 24;
const MAX_NODES: usize = 1024;
const MAX_HANDLES: usize = 1024;
const BINDER: u32 = 0x73622a85;
const WEAK_BINDER: u32 = 0x77622a85;
const HANDLE: u32 = 0x73682a85;
const WEAK_HANDLE: u32 = 0x77682a85;
pub(super) const BC_INCREFS: u32 = 0x40046304;
pub(super) const BC_ACQUIRE: u32 = 0x40046305;
pub(super) const BC_RELEASE: u32 = 0x40046306;
pub(super) const BC_DECREFS: u32 = 0x40046307;
pub(super) const BC_INCREFS_DONE: u32 = 0x40106308;
pub(super) const BC_ACQUIRE_DONE: u32 = 0x40106309;
const BR_INCREFS: u32 = 0x80107207;
const BR_ACQUIRE: u32 = 0x80107208;
const BR_RELEASE: u32 = 0x80107209;
const BR_DECREFS: u32 = 0x8010720a;

pub(super) type Registry = IrqSpinLock<BTreeMap<usize, Weak<Node>>>;

#[derive(Default)]
struct References {
    strong: usize,
    weak: usize,
    has_strong: bool,
    has_weak: bool,
    pending_strong: bool,
    pending_weak: bool,
    delivering: bool,
    acquisition_thread: Option<usize>,
    abandoned: bool,
}
impl References {
    fn next(&self, tid: usize) -> Option<u32> {
        if self.delivering {
            return None;
        }
        if self.abandoned {
            return if self.strong == 0 && self.has_strong {
                Some(BR_RELEASE)
            } else if self.strong == 0 && self.weak == 0 && !self.has_strong && self.has_weak {
                Some(BR_DECREFS)
            } else {
                None
            };
        }
        let strong = self.strong != 0 || self.pending_strong;
        let weak = strong || self.weak != 0 || self.pending_weak;
        if weak && !self.has_weak {
            (self.acquisition_thread == Some(tid)).then_some(BR_INCREFS)
        } else if strong && !self.has_strong {
            (self.acquisition_thread == Some(tid)).then_some(BR_ACQUIRE)
        } else if !strong && self.has_strong {
            Some(BR_RELEASE)
        } else if !weak && self.has_weak {
            Some(BR_DECREFS)
        } else {
            None
        }
    }
    fn acquisition_needed(&self) -> bool {
        !self.abandoned
            && (self.pending_strong
                || self.pending_weak
                || (self.strong != 0 && !self.has_strong)
                || ((self.strong != 0 || self.weak != 0) && !self.has_weak))
    }
    fn settle(&mut self) {
        if !self.acquisition_needed() && !self.delivering {
            self.acquisition_thread = None;
        }
    }
    fn idle(&self) -> bool {
        self.strong == 0
            && self.weak == 0
            && !self.has_strong
            && !self.has_weak
            && !self.pending_strong
            && !self.pending_weak
            && !self.delivering
    }
}

pub(super) struct Node {
    pub subject: Option<crate::shield::Subject>,
    pub object: Arc<RpcObject>,
    pub pointer: usize,
    pub cookie: usize,
    flags: u32,
    references: IrqSpinLock<References>,
    waker: Weak<Waker>,
}
impl Node {
    pub(super) fn accepts_fds(&self)->bool {self.flags & 0x100!=0}
    pub(super) fn inherits_rt(&self)->bool {self.flags & 0x800!=0}
    pub(super) fn acquired(&self) -> bool {
        !self.references.lock().acquisition_needed()
    }
    fn notify(&self) {
        if let Some(waker) = self.waker.upgrade() {
            waker.wake_all();
        }
    }
    fn retain(self: &Arc<Self>, strong: bool) -> Lease {
        self.retain_from_thread(strong, None)
    }
    fn retain_from_thread(self: &Arc<Self>, strong: bool, tid: Option<usize>) -> Lease {
        let mut refs = self.references.lock();
        if tid.is_some()
            && refs.acquisition_thread.is_none()
            && (!refs.has_weak || (strong && !refs.has_strong))
        {
            refs.acquisition_thread = tid;
        }
        if strong {
            refs.strong += 1;
        } else {
            refs.weak += 1;
        }
        drop(refs);
        self.notify();
        Lease {
            node: self.clone(),
            strong,
        }
    }
    fn reserve_record(self: &Arc<Self>, tid: usize) -> Option<ReferenceRecord> {
        let mut refs = self.references.lock();
        let command = refs.next(tid)?;
        refs.delivering = true;
        match command {
            BR_INCREFS => {
                refs.has_weak = true;
                refs.pending_weak = true;
            }
            BR_ACQUIRE => {
                refs.has_strong = true;
                refs.pending_strong = true;
            }
            BR_RELEASE => refs.has_strong = false,
            BR_DECREFS => refs.has_weak = false,
            _ => unreachable!(),
        }
        Some(ReferenceRecord {
            node: self.clone(),
            command,
            committed: false,
        })
    }
}
pub(super) struct Lease {
    pub(super) node: Arc<Node>,
    strong: bool,
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut refs = self.node.references.lock();
        if self.strong {
            refs.strong -= 1;
        } else {
            refs.weak -= 1;
        }
        refs.settle();
        drop(refs);
        self.node.notify();
    }
}
pub(super) struct ReferenceRecord {
    node: Arc<Node>,
    command: u32,
    committed: bool,
}
impl ReferenceRecord {
    pub fn write(mut self, task: &Task, output: usize) -> Result<usize, usize> {
        let mut bytes = [0; 20];
        bytes[..4].copy_from_slice(&self.command.to_le_bytes());
        put64(&mut bytes, 4, self.node.pointer);
        put64(&mut bytes, 12, self.node.cookie);
        write(task, output, &bytes)?;
        self.committed = true;
        Ok(bytes.len())
    }
}
impl Drop for ReferenceRecord {
    fn drop(&mut self) {
        let mut refs = self.node.references.lock();
        if !self.committed {
            match self.command {
                BR_INCREFS => {
                    refs.has_weak = false;
                    refs.pending_weak = false;
                }
                BR_ACQUIRE => {
                    refs.has_strong = false;
                    refs.pending_strong = false;
                }
                BR_RELEASE => refs.has_strong = true,
                BR_DECREFS => refs.has_weak = true,
                _ => unreachable!(),
            }
        }
        refs.delivering = false;
        refs.settle();
        drop(refs);
        self.node.notify();
    }
}

struct Handle {
    node: Arc<Node>,
    strong: u32,
    weak: u32,
    temporary_strong: u32,
    temporary_weak: u32,
    strong_lease: Option<Lease>,
    weak_lease: Option<Lease>,
}
impl Handle {
    fn live(&self) -> bool {
        self.strong != 0 || self.weak != 0 || self.temporary_strong != 0 || self.temporary_weak != 0
    }
    fn can_call(&self) -> bool {
        self.strong != 0 || self.temporary_strong != 0
    }
}
pub(super) struct BufferReference {
    handle: Option<u32>,
    lease: Lease,
}
pub(super) struct Objects {
    pub deaths: death::Deaths,
    pub oneway: BTreeMap<usize, RpcRequest>,
    nodes: BTreeMap<usize, Arc<Node>>,
    retired: Vec<Arc<Node>>,
    handles: BTreeMap<u32, Handle>,
    buffers: BTreeMap<usize, Vec<BufferReference>>,
    next_handle: u32,
    context_strong: u32,
    context_weak: u32,
}
impl Default for Objects {
    fn default() -> Self {
        Self {
            deaths: death::Deaths::default(),
            oneway: BTreeMap::new(),
            nodes: BTreeMap::new(),
            retired: Vec::new(),
            handles: BTreeMap::new(),
            buffers: BTreeMap::new(),
            next_handle: 1,
            context_strong: 0,
            context_weak: 0,
        }
    }
}
impl Objects {
    fn local(
        &mut self,
        file: &BinderFile,
        pointer: usize,
        cookie: usize,
        flags: u32,
        strong: bool,
        tid: usize,
    ) -> Result<Lease, usize> {
        self.nodes.retain(|_, node| !node.references.lock().idle());
        self.retired.retain(|node| !node.references.lock().idle());
        if let Some(node) = self.nodes.get(&pointer) {
            if node.cookie != cookie || node.flags != flags {
                return Err(errno::EINVAL);
            }
            return Ok(node.retain_from_thread(strong, Some(tid)));
        }
        if self.nodes.len() + self.retired.len() >= MAX_NODES {
            return Err(errno::ENOSPC);
        }
        let object = file.endpoint.create_object().map_err(|_| errno::ENOMEM)?;
        let node = Arc::new(Node {
            subject: file.subject.clone(),
            object,
            pointer,
            cookie,
            flags,
            references: IrqSpinLock::new(References {
                acquisition_thread: Some(tid),
                ..References::default()
            }),
            waker: Arc::downgrade(&file.endpoint.waker()),
        });
        let mut registry = file.domain.objects.lock();
        registry.retain(|_, node| node.strong_count() != 0);
        registry.insert(Arc::as_ptr(&node.object) as usize, Arc::downgrade(&node));
        self.nodes.insert(pointer, node.clone());
        Ok(node.retain_from_thread(strong, Some(tid)))
    }
    pub fn target(&self, handle: u32) -> Result<Lease, usize> {
        let handle = self.handles.get(&handle).ok_or(errno::EINVAL)?;
        if !handle.can_call() {
            return Err(errno::EOPNOTSUPP);
        }
        Ok(handle.node.retain(true))
    }
    pub fn request_death(
        &mut self,
        number: u32,
        cookie: usize,
        file: &BinderFile,
    ) -> Result<(), usize> {
        let object = if number == 0 {
            if self.context_strong == 0 && self.context_weak == 0 {
                return Err(errno::EINVAL);
            }
            let context = file.domain.context.lock();
            let context = context.as_ref().ok_or(errno::EINVAL)?;
            if !same_policy(file.subject.as_ref(), context.subject.as_ref()) {
                return Err(errno::EACCES);
            }
            context.object.clone()
        } else {
            self.handles
                .get(&number)
                .ok_or(errno::EINVAL)?
                .node
                .object
                .clone()
        };
        self.deaths
            .request(number, cookie, &object, &file.endpoint.waker())
    }
    fn exported(&self, number: u32, strong: bool) -> Result<Lease, usize> {
        let handle = self.handles.get(&number).ok_or(errno::EINVAL)?;
        if strong && !handle.can_call() {
            return Err(errno::EOPNOTSUPP);
        }
        Ok(handle.node.retain(strong))
    }
    pub fn reference_command(&mut self, command: u32, data: &[u8]) -> Result<(), usize> {
        if matches!(command, BC_INCREFS_DONE | BC_ACQUIRE_DONE) {
            let node = self.nodes.get(&u64_at(data, 0)).ok_or(errno::EINVAL)?;
            if node.cookie != u64_at(data, 8) {
                return Err(errno::EINVAL);
            }
            let mut refs = node.references.lock();
            let pending = if command == BC_INCREFS_DONE {
                &mut refs.pending_weak
            } else {
                &mut refs.pending_strong
            };
            if !*pending {
                return Err(errno::EINVAL);
            }
            *pending = false;
            refs.settle();
            drop(refs);
            node.notify();
            return Ok(());
        }
        let number = u32_at(data, 0);
        // Handle zero is the context-manager bootstrap capability. It exists
        // outside this table; libbinder acquires it before its first RPC.
        if number == 0 {
            let count = if matches!(command, BC_ACQUIRE | BC_RELEASE) {
                &mut self.context_strong
            } else {
                &mut self.context_weak
            };
            *count = if matches!(command, BC_ACQUIRE | BC_INCREFS) {
                count.checked_add(1).ok_or(errno::EOVERFLOW)?
            } else {
                count.checked_sub(1).ok_or(errno::EINVAL)?
            };
            if self.context_strong == 0 && self.context_weak == 0 {
                self.deaths.forget(0);
            }
            return Ok(());
        }
        let handle = self.handles.get_mut(&number).ok_or(errno::EINVAL)?;
        match command {
            BC_INCREFS => {
                handle.weak = handle.weak.checked_add(1).ok_or(errno::EOVERFLOW)?;
                if handle.weak == 1 {
                    handle.weak_lease = Some(handle.node.retain(false));
                }
            }
            BC_ACQUIRE => {
                if !handle.can_call() {
                    return Err(errno::EOPNOTSUPP);
                }
                handle.strong = handle.strong.checked_add(1).ok_or(errno::EOVERFLOW)?;
                if handle.strong == 1 {
                    handle.strong_lease = Some(handle.node.retain(true));
                }
            }
            BC_RELEASE => {
                handle.strong = handle.strong.checked_sub(1).ok_or(errno::EINVAL)?;
                if handle.strong == 0 {
                    handle.strong_lease = None;
                }
            }
            BC_DECREFS => {
                handle.weak = handle.weak.checked_sub(1).ok_or(errno::EINVAL)?;
                if handle.weak == 0 {
                    handle.weak_lease = None;
                }
            }
            _ => return Err(errno::EINVAL),
        }
        if !handle.live() {
            self.handles.remove(&number);
            self.deaths.forget(number);
        }
        Ok(())
    }
    pub fn pending_acquisition(&self, tid: usize) -> bool {
        self.nodes.values().any(|node| {
            let refs = node.references.lock();
            refs.acquisition_thread == Some(tid) && refs.acquisition_needed()
        })
    }
    pub fn ready(&self, tid: usize) -> bool {
        self.nodes
            .values()
            .chain(self.retired.iter())
            .any(|node| node.references.lock().next(tid).is_some())
    }
    pub fn pending_local_acquisitions(&self) -> bool {
        self.nodes.values().any(|node| !node.acquired())
    }
    pub fn reference_record(&self, tid: usize) -> Option<ReferenceRecord> {
        self.nodes
            .values()
            .chain(self.retired.iter())
            .find_map(|node| node.reserve_record(tid))
    }
    pub fn abandon_thread(&mut self, tid: usize) -> Vec<Arc<RpcObject>> {
        let mut revoked = Vec::new();
        let retired = &mut self.retired;
        self.nodes.retain(|_, node| {
            let mut refs = node.references.lock();
            if refs.acquisition_thread == Some(tid) && refs.acquisition_needed() {
                // We cannot know whether userspace applied an unacknowledged
                // acquire. Never replay it against a potentially freed cookie.
                refs.abandoned = true;
                // Acknowledged holds are known to exist and must eventually
                // be released. Never release an acquire whose application by
                // userspace is uncertain, or replay it on another thread.
                refs.has_strong &= !refs.pending_strong;
                refs.has_weak &= !refs.pending_weak;
                refs.pending_strong = false;
                refs.pending_weak = false;
                refs.acquisition_thread = None;
                if refs.has_strong || refs.has_weak {
                    retired.push(node.clone());
                }
                revoked.push(node.object.clone());
                false
            } else {
                true
            }
        });
        revoked
    }
    pub fn dispatch(&self, key: u64) -> Result<Lease, usize> {
        self.nodes
            .values()
            .find(|node| node.object.key() == key)
            .map(|node| node.retain(true))
            .ok_or(errno::EINVAL)
    }
    fn imported(
        &mut self,
        file: &BinderFile,
        node: Arc<Node>,
        strong: bool,
    ) -> Result<BufferReference, usize> {
        if node.object.belongs_to(&file.endpoint) {
            return Ok(BufferReference {
                handle: None,
                lease: node.retain(strong),
            });
        }
        let number = if let Some((&number, _)) = self
            .handles
            .iter()
            .find(|(_, handle)| Arc::ptr_eq(&handle.node, &node))
        {
            number
        } else {
            if self.handles.len() >= MAX_HANDLES {
                return Err(errno::ENOSPC);
            }
            let number = self.next_handle;
            self.next_handle = number.checked_add(1).ok_or(errno::ENOSPC)?;
            self.handles.insert(
                number,
                Handle {
                    node: node.clone(),
                    strong: 0,
                    weak: 0,
                    temporary_strong: 0,
                    temporary_weak: 0,
                    strong_lease: None,
                    weak_lease: None,
                },
            );
            number
        };
        let handle = self.handles.get_mut(&number).unwrap();
        let temporary = if strong {
            &mut handle.temporary_strong
        } else {
            &mut handle.temporary_weak
        };
        *temporary += 1;
        Ok(BufferReference {
            handle: Some(number),
            lease: node.retain(strong),
        })
    }
    fn release(&mut self, references: Vec<BufferReference>) {
        for reference in references {
            if let Some(number) = reference.handle {
                let handle = self.handles.get_mut(&number).unwrap();
                if reference.lease.strong {
                    handle.temporary_strong -= 1;
                } else {
                    handle.temporary_weak -= 1;
                }
                if !handle.live() {
                    self.handles.remove(&number);
                    self.deaths.forget(number);
                }
            }
            drop(reference);
        }
    }
    pub fn free_buffer(&mut self, address: usize) {
        if let Some(references) = self.buffers.remove(&address) {
            self.release(references);
        }
    }
}

pub(super) fn message_empty() -> RpcMessage {
    RpcMessage {
        opcode: 0,
        bytes: Vec::new(),
        objects: Vec::new(),
        capabilities: Vec::new(),
        retained: None,
    }
}

fn offsets(data: &[u8], encoded: &[u8]) -> Result<Vec<usize>, usize> {
    if !encoded.len().is_multiple_of(8) {
        return Err(errno::EINVAL);
    }
    if encoded.len() / 8 > MAX_MESSAGE_OBJECTS {
        return Err(errno::E2BIG);
    }
    let mut result = Vec::new();
    let mut end = 0;
    for offset in encoded.chunks_exact(8) {
        let offset = u64_at(offset, 0);
        if !offset.is_multiple_of(4) || offset < end {
            return Err(errno::EINVAL);
        }
        if offset.checked_add(8).is_none_or(|end|end>data.len()) {
            return Err(errno::EINVAL);
        }
        let kind = u32_at(data, offset);
        let size=match kind {
            BINDER|WEAK_BINDER|HANDLE|WEAK_HANDLE|buffers::FD=>24,
            buffers::PTR=>40,
            buffers::FDA=>32,
            _=>return Err(errno::EOPNOTSUPP),
        };
        end=offset.checked_add(size).ok_or(errno::EINVAL)?;
        if end>data.len() {return Err(errno::EINVAL);}
        let flags = u32_at(data, offset + 4);
        // Recognize libbinder's normal/default minimum-priority encoding.
        // INHERIT_RT is accepted for the Fair-only hwbinder subset; actual RT
        // scheduling policies and security-context delivery remain unsupported.
        if matches!(kind,BINDER|WEAK_BINDER|HANDLE|WEAK_HANDLE) &&
            (flags & !0x9ff != 0 || !matches!(flags & 0xff, 0 | 19)) {
            return Err(errno::EOPNOTSUPP);
        }
        if (kind==buffers::PTR && flags & !1 !=0) || (matches!(kind,buffers::FD|buffers::FDA) && flags!=0) {return Err(errno::EINVAL);}
        result.push(offset);
    }
    Ok(result)
}

pub(super) struct Prepared {
    bytes: Vec<u8>,
    offsets: Vec<usize>,
    pub capabilities: Vec<(KernelObject,crate::object::handle::HandleMetadata)>,
}
pub(super) fn prepare(task: &Task, abi: &LinuxAbi, data: &[u8]) -> Result<Prepared, usize> {
    let len = u64_at(data, 32);
    let offsets_len = u64_at(data, 40);
    if len > MAX_DATA || offsets_len > MAX_MESSAGE_OBJECTS * 8 {
        return Err(errno::E2BIG);
    }
    if !offsets_len.is_multiple_of(8) {
        return Err(errno::EINVAL);
    }
    let aligned = (len + 7) & !7;
    if HEADER + aligned + offsets_len > crate::ipc::rpc::MAX_MESSAGE_SIZE {
        return Err(errno::E2BIG);
    }
    let mut bytes = vec![0; HEADER + aligned + offsets_len];
    bytes[..4].copy_from_slice(
        &(u32_at(data, 20) & (1 | FLAG_COLLECT_NOTED_APP_OPS | 8 | 0x10 | 0x20)).to_le_bytes(),
    );
    let sender = if u32_at(data, 20) & 1 != 0 {
        0
    } else {
        abi.visible_thread_group_id(task) as u32
    };
    bytes[4..8].copy_from_slice(&sender.to_le_bytes());
    bytes[8..12].copy_from_slice(&task.credentials.read().principals().active.0.to_le_bytes());
    bytes[12..16].copy_from_slice(&(len as u32).to_le_bytes());
    bytes[16..20].copy_from_slice(&(offsets_len as u32).to_le_bytes());
    copy_from_user(task, u64_at(data, 48), &mut bytes[HEADER..HEADER + len])
        .map_err(|_| errno::EFAULT)?;
    if offsets_len != 0 {
        copy_from_user(task, u64_at(data, 56), &mut bytes[HEADER + aligned..])
            .map_err(|_| errno::EFAULT)?;
    }
    let offsets = offsets(&bytes[HEADER..HEADER + len], &bytes[HEADER + aligned..])?;
    let budget=if data.len()==72 {u64_at(data,64)}else{0};
    if budget>crate::ipc::rpc::MAX_MESSAGE_SIZE {return Err(errno::E2BIG);}
    let capabilities=buffers::prepare(task,abi,&mut bytes,&offsets,budget)?;
    Ok(Prepared { bytes, offsets, capabilities })
}
impl BinderFile {
    pub(super) fn encode(
        &self,
        task: &Task,
        data: &[u8],
        prepared: Prepared,
        target: Option<Lease>,
    ) -> Result<(RpcMessage, Vec<Arc<Node>>), usize> {
        let Prepared { mut bytes, offsets, capabilities } = prepared;
        let mut leases = Vec::new();
        let mut objects = Vec::new();
        let mut acquisitions = Vec::new();
        let mut table = self.objects.lock();
        for offset in offsets {
            let position = HEADER + offset;
            let kind = u32_at(&bytes, position);
            if matches!(kind,buffers::FD|buffers::PTR|buffers::FDA) {continue;}
            let strong = matches!(kind, BINDER | HANDLE);
            let value = if matches!(kind, BINDER | WEAK_BINDER) {
                u64_at(&bytes, position + 8)
            } else {
                u32_at(&bytes, position + 8) as usize
            };
            let lease = if matches!(kind, BINDER | WEAK_BINDER) {
                if value == 0 {
                    if u64_at(&bytes, position + 16) != 0 {
                        return Err(errno::EINVAL);
                    }
                    continue;
                }
                table.local(
                    self,
                    value,
                    u64_at(&bytes, position + 16),
                    u32_at(&bytes, position + 4),
                    strong,
                    task.get_id(),
                )?
            } else {
                table.exported(value as u32, strong)?
            };
            objects.push(lease.node.object.clone());
            if lease.node.object.belongs_to(&self.endpoint)
                && !acquisitions
                    .iter()
                    .any(|node| Arc::ptr_eq(node, &lease.node))
            {
                acquisitions.push(lease.node.clone());
            }
            leases.push(lease);
            bytes[position..position + 4]
                .copy_from_slice(&(if strong { HANDLE } else { WEAK_HANDLE }).to_le_bytes());
            put64(&mut bytes, position + 8, objects.len());
            put64(&mut bytes, position + 16, 0);
        }
        drop(table);
        if objects.len()+capabilities.len()>MAX_MESSAGE_OBJECTS {return Err(errno::E2BIG);}
        if let Some(target) = target {
            leases.push(target);
        }
        Ok((
            RpcMessage {
                opcode: u32_at(data, 16),
                bytes,
                objects,
                capabilities,
                retained: Some(Arc::new(leases)),
            },
            acquisitions,
        ))
    }

    pub(super) fn decode(
        &self,
        message: &RpcMessage,
        address: usize,
        arena: &Arena,
    ) -> Result<(usize, usize), usize> {
        self.decode_received(message,address,arena,None)
    }
    pub(super) fn decode_received(&self,message:&RpcMessage,address:usize,arena:&Arena,installed:Option<&mut buffers::Installed<'_>>)
        ->Result<(usize,usize),usize> {
        if message.bytes.len() < HEADER {
            return Err(errno::EINVAL);
        }
        let len = u32_at(&message.bytes, 12) as usize;
        let offsets_len = u32_at(&message.bytes, 16) as usize;
        let aligned = len.checked_add(7).ok_or(errno::EINVAL)? & !7;
        let extra=u32_at(&message.bytes,20) as usize;
        if HEADER
            .checked_add(aligned)
            .and_then(|n| n.checked_add(offsets_len))
            .and_then(|n| n.checked_add(extra))
            .and_then(|n| n.checked_add(message.capabilities.len()*4))
            != Some(message.bytes.len())
        {
            return Err(errno::EINVAL);
        }
        let payload_length=aligned+offsets_len+extra;
        let mut payload = message.bytes[HEADER..HEADER+payload_length].to_vec();
        let offsets = offsets(&payload[..len], &payload[aligned..aligned+offsets_len])?;
        buffers::receive(message,&mut payload,&offsets,address,len,offsets_len,installed)?;
        let mut references = Vec::new();
        let mut table = self.objects.lock();
        let result = (|| {
            for offset in offsets {
                let kind = u32_at(&payload, offset);
                if matches!(kind,buffers::FD|buffers::PTR|buffers::FDA) {continue;}
                if matches!(kind, BINDER | WEAK_BINDER) && u64_at(&payload, offset + 8) == 0 {
                    continue;
                }
                let index = u64_at(&payload, offset + 8)
                    .checked_sub(1)
                    .ok_or(errno::EINVAL)?;
                let object = message.objects.get(index).ok_or(errno::EINVAL)?;
                let node = self
                    .domain
                    .objects
                    .lock()
                    .get(&(Arc::as_ptr(object) as usize))
                    .and_then(Weak::upgrade)
                    .ok_or(errno::EINVAL)?;
                if node.references.lock().abandoned {
                    return Err(errno::EPIPE);
                }
                let strong = kind == HANDLE;
                let reference = table.imported(self, node.clone(), strong)?;
                let (kind, value, cookie, flags) = if let Some(handle) = reference.handle {
                    (
                        if strong { HANDLE } else { WEAK_HANDLE },
                        handle as usize,
                        0,
                        0,
                    )
                } else {
                    (
                        if strong { BINDER } else { WEAK_BINDER },
                        node.pointer,
                        node.cookie,
                        node.flags,
                    )
                };
                payload[offset..offset + 4].copy_from_slice(&kind.to_le_bytes());
                payload[offset + 4..offset + 8].copy_from_slice(&flags.to_le_bytes());
                put64(&mut payload, offset + 8, value);
                put64(&mut payload, offset + 16, cookie);
                references.push(reference);
            }
            arena.store(address, &payload)?;
            Ok((len, offsets_len))
        })();
        if result.is_ok() {
            table.buffers.insert(address, references);
        } else {
            table.release(references);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file(domain: Arc<Domain>) -> BinderFile {
        BinderFile {
            subject: None,
            owner: 1,
            domain,
            endpoint: RpcEndpoint::new(),
            send_gate: IrqSpinLock::new(()),
            auto_fds: IrqSpinLock::new(BTreeMap::new()),
            state: IrqSpinLock::new(State {
                threads: BTreeMap::new(),
                arena: None,
                node: (0, 0),
            }),
            objects: IrqSpinLock::new(Objects::default()),
        }
    }
    fn delivered(node: &Arc<Node>) -> u32 {
        let mut record = node.reserve_record(1).unwrap();
        let command = record.command;
        record.committed = true;
        drop(record);
        command
    }
    fn ack(file: &BinderFile, command: u32) -> Result<(), usize> {
        let mut pair = [0; 16];
        put64(&mut pair, 0, 0x1000);
        put64(&mut pair, 8, 0x2000);
        file.objects.lock().reference_command(command, &pair)
    }
    #[test_case]
    fn binder_concurrent_exporters_wait_for_shared_acquisition_before_completion_or_dispatch() {
        let owner = file(domain(Environment::new(), 0));
        let weak = owner
            .objects
            .lock()
            .local(&owner, 0x1000, 0x2000, 0, false, 1)
            .unwrap();
        let strong = owner
            .objects
            .lock()
            .local(&owner, 0x1000, 0x2000, 0, true, 2)
            .unwrap();
        let node = weak.node.clone();
        owner.state.lock().threads.insert(1, Thread::default());
        owner.state.lock().threads.insert(
            2,
            Thread {
                looper: true,
                completed: VecDeque::from([Completed {
                    command: BR_COMPLETE,
                    acquisitions: vec![node.clone()],
                }]),
                ..Thread::default()
            },
        );
        owner.state.lock().threads.insert(
            3,
            Thread {
                looper: true,
                ..Thread::default()
            },
        );
        let call = node.object.send(message_empty()).unwrap();
        assert!(
            !owner.ready(2),
            "strong exporter must retain its userspace object"
        );
        assert!(
            !owner.ready(3),
            "local target must not escape before acquisition"
        );
        assert!(node.reserve_record(2).is_none());
        assert_eq!(delivered(&node), BR_INCREFS);
        ack(&owner, BC_INCREFS_DONE).unwrap();
        assert!(!owner.ready(2));
        assert!(!owner.ready(3));
        assert_eq!(delivered(&node), BR_ACQUIRE);
        assert!(!owner.ready(2));
        ack(&owner, BC_ACQUIRE_DONE).unwrap();
        assert!(owner.ready(2));
        assert!(owner.ready(3));
        drop((call, strong, weak));
    }
    #[test_case]
    fn binder_abandoned_acquisition_revokes_only_its_node_and_releases_known_holds() {
        for acquire_delivered in [false, true] {
            let owner = file(domain(Environment::new(), 0));
            let transit = owner
                .objects
                .lock()
                .local(&owner, 0x1000, 0x2000, 0, true, 1)
                .unwrap();
            let node = transit.node.clone();
            assert_eq!(delivered(&node), BR_INCREFS);
            ack(&owner, BC_INCREFS_DONE).unwrap();
            if acquire_delivered {
                assert_eq!(delivered(&node), BR_ACQUIRE);
            }
            let targeted = node.object.send(message_empty()).unwrap();
            let unrelated = owner.endpoint.send(message_empty()).unwrap();
            owner.state.lock().threads.insert(
                2,
                Thread {
                    looper: true,
                    ..Thread::default()
                },
            );
            assert!(!owner.ready(2));
            owner.thread_exit(1);
            assert!(matches!(
                targeted.take_reply(),
                Some(Err(crate::ipc::rpc::RpcError::PeerClosed))
            ));
            assert!(!unrelated.ready());
            assert!(
                owner.ready(2),
                "unrelated receiver must recover after exporter exit"
            );
            assert!(owner.objects.lock().dispatch(node.object.key()).is_err());
            assert!(
                node.reserve_record(2).is_none(),
                "live leases retain the acknowledged weak hold"
            );
            drop(transit);
            let mut release = owner.objects.lock().reference_record(2).unwrap();
            assert_eq!(
                release.command, BR_DECREFS,
                "release only the acknowledged hold"
            );
            release.committed = true;
            drop(release);
            assert!(node.references.lock().idle());
            assert!(owner.objects.lock().reference_record(2).is_none());
            owner
                .endpoint
                .receive()
                .unwrap()
                .unwrap()
                .reply(message_empty())
                .unwrap();
            assert!(unrelated.take_reply().unwrap().is_ok());
        }
    }
    #[test_case]
    fn binder_failed_transport_admission_leaves_no_acquisition_work() {
        let owner = file(domain(Environment::new(), 0));
        let destination = RpcEndpoint::new();
        let calls: Vec<_> = (0..64)
            .map(|_| destination.send(message_empty()).unwrap())
            .collect();
        let lease = owner
            .objects
            .lock()
            .local(&owner, 0x1000, 0x2000, 0, true, 1)
            .unwrap();
        let node = lease.node.clone();
        let mut message = message_empty();
        message.objects.push(node.object.clone());
        message.retained = Some(Arc::new(vec![lease]));
        assert!(matches!(
            destination.send(message),
            Err(crate::ipc::rpc::RpcError::Busy)
        ));
        assert!(!owner.objects.lock().ready(1));
        assert!(!owner.objects.lock().pending_local_acquisitions());
        assert!(node.references.lock().idle());
        drop(calls);
    }
    #[test_case]
    fn binder_death_subscription_accepts_weak_handles_and_follows_their_lifetime() {
        for explicit_weak in [false, true] {
            let domain = domain(Environment::new(), 0);
            let owner = file(domain.clone());
            let peer = file(domain);
            let transit = owner
                .objects
                .lock()
                .local(&owner, 0x1000, 0x2000, 0, false, 1)
                .unwrap();
            let mut objects = peer.objects.lock();
            let reference = objects
                .imported(&peer, transit.node.clone(), false)
                .unwrap();
            let handle = reference.handle.unwrap();
            objects.buffers.insert(0x3000, vec![reference]);
            assert_eq!(
                objects.request_death(handle + 1, 88, &peer),
                Err(errno::EINVAL)
            );
            objects.request_death(handle, 88, &peer).unwrap();
            if explicit_weak {
                objects
                    .reference_command(BC_INCREFS, &handle.to_le_bytes())
                    .unwrap();
            }
            objects.free_buffer(0x3000);
            owner.endpoint.close();
            assert_eq!(objects.deaths.ready(1, true), explicit_weak);
            if explicit_weak {
                objects
                    .reference_command(BC_DECREFS, &handle.to_le_bytes())
                    .unwrap();
                assert!(!objects.deaths.ready(1, true));
            }
        }
    }
    #[test_case]
    fn binder_buffer_and_handle_references_balance_owner_notifications() {
        let domain = domain(Environment::new(), 0);
        let owner = file(domain.clone());
        let peer = file(domain);
        let transit = owner
            .objects
            .lock()
            .local(&owner, 0x1000, 0x2000, 0x100, true, 1)
            .unwrap();
        let node = transit.node.clone();
        assert_eq!(delivered(&node), BR_INCREFS);
        assert_eq!(delivered(&node), BR_ACQUIRE);
        assert!(owner.objects.lock().pending_acquisition(1));
        ack(&owner, BC_INCREFS_DONE).unwrap();
        ack(&owner, BC_ACQUIRE_DONE).unwrap();
        assert!(!owner.objects.lock().pending_acquisition(1));
        assert_eq!(ack(&owner, BC_ACQUIRE_DONE), Err(errno::EINVAL));
        let mut peer_objects = peer.objects.lock();
        let buffer = peer_objects.imported(&peer, node.clone(), true).unwrap();
        let handle = buffer.handle.unwrap();
        peer_objects.buffers.insert(0x3000, vec![buffer]);
        peer_objects
            .reference_command(BC_INCREFS, &handle.to_le_bytes())
            .unwrap();
        peer_objects
            .reference_command(BC_ACQUIRE, &handle.to_le_bytes())
            .unwrap();
        drop(transit);
        peer_objects.free_buffer(0x3000);
        assert!(peer_objects.target(handle).is_ok());
        peer_objects
            .reference_command(BC_RELEASE, &handle.to_le_bytes())
            .unwrap();
        assert!(matches!(
            peer_objects.target(handle),
            Err(errno::EOPNOTSUPP)
        ));
        assert_eq!(delivered(&node), BR_RELEASE);
        assert!(node.reserve_record(1).is_none());
        peer_objects
            .reference_command(BC_DECREFS, &handle.to_le_bytes())
            .unwrap();
        assert!(peer_objects.handles.is_empty());
        assert_eq!(delivered(&node), BR_DECREFS);
        assert!(node.references.lock().idle());
    }
    #[test_case]
    fn binder_failed_record_copy_restores_notification_and_weak_handles_cannot_promote() {
        let domain = domain(Environment::new(), 0);
        let owner = file(domain.clone());
        let peer = file(domain);
        let transit = owner
            .objects
            .lock()
            .local(&owner, 0x1000, 0x2000, 0, false, 1)
            .unwrap();
        let node = transit.node.clone();
        let task = Task::new("ref-copy-fault".into(), 0, crate::task::TaskType::User);
        assert_eq!(
            node.reserve_record(1).unwrap().write(&task, 0),
            Err(errno::EFAULT)
        );
        assert_eq!(node.references.lock().next(1), Some(BR_INCREFS));
        assert_eq!(delivered(&node), BR_INCREFS);
        assert!(node.reserve_record(1).is_none());
        ack(&owner, BC_INCREFS_DONE).unwrap();
        let mut peer_objects = peer.objects.lock();
        let buffer = peer_objects.imported(&peer, node.clone(), false).unwrap();
        let number = buffer.handle.unwrap();
        assert_eq!(
            peer_objects.reference_command(BC_ACQUIRE, &number.to_le_bytes()),
            Err(errno::EOPNOTSUPP)
        );
        assert!(matches!(
            peer_objects.target(number),
            Err(errno::EOPNOTSUPP)
        ));
        assert_eq!(
            peer_objects.reference_command(BC_RELEASE, &number.to_le_bytes()),
            Err(errno::EINVAL)
        );
        drop(transit);
        peer_objects.release(vec![buffer]);
        assert_eq!(delivered(&node), BR_DECREFS);
        assert!(node.references.lock().idle());
    }
    #[test_case]
    fn binder_offsets_reject_overlap_alignment_bounds_and_unknown_types() {
        let mut data = [0; 48];
        data[..4].copy_from_slice(&BINDER.to_le_bytes());
        data[24..28].copy_from_slice(&HANDLE.to_le_bytes());
        let mut encoded = [0; 16];
        put64(&mut encoded, 8, 24);
        assert_eq!(offsets(&data, &encoded), Ok(vec![0, 24]));
        put64(&mut encoded, 8, 0);
        assert_eq!(offsets(&data, &encoded), Err(errno::EINVAL));
        put64(&mut encoded, 8, 25);
        assert_eq!(offsets(&data, &encoded), Err(errno::EINVAL));
        put64(&mut encoded, 8, usize::MAX - 3);
        assert_eq!(offsets(&data, &encoded), Err(errno::EINVAL));
        assert_eq!(offsets(&data, &[0; 7]), Err(errno::EINVAL));
        data[..4].copy_from_slice(&0x66642a85u32.to_le_bytes());
        assert_eq!(offsets(&data, &[0; 8]), Ok(vec![0]));
        data[..4].copy_from_slice(&0xdeadbeefu32.to_le_bytes());
        assert_eq!(offsets(&data, &[0; 8]), Err(errno::EOPNOTSUPP));
        data[..4].copy_from_slice(&BINDER.to_le_bytes());
        data[4..8].copy_from_slice(&0x900u32.to_le_bytes());
        assert_eq!(offsets(&data,&[0;8]),Ok(vec![0]));
        for flags in [0xb00u32,0xd00,0x1900,0x901] {
            data[4..8].copy_from_slice(&flags.to_le_bytes());
            assert_eq!(offsets(&data,&[0;8]),Err(errno::EOPNOTSUPP));
        }
    }
    #[test_case]
    fn binder_forwarded_handles_keep_identity_and_do_not_reuse_released_numbers() {
        let domain = domain(Environment::new(), 0);
        let owner = file(domain.clone());
        let peer = file(domain.clone());
        let third = file(domain);
        let local = owner
            .objects
            .lock()
            .local(&owner, 0x1000, 0x2000, 0, true, 1)
            .unwrap();
        let mut peer_objects = peer.objects.lock();
        let first = peer_objects
            .imported(&peer, local.node.clone(), true)
            .unwrap();
        let second = peer_objects
            .imported(&peer, local.node.clone(), false)
            .unwrap();
        assert_eq!(first.handle, second.handle);
        let number = first.handle.unwrap();
        let forwarded = peer_objects.exported(number, true).unwrap();
        let third_buffer = third
            .objects
            .lock()
            .imported(&third, forwarded.node.clone(), true)
            .unwrap();
        let back = owner
            .objects
            .lock()
            .imported(&owner, forwarded.node.clone(), true)
            .unwrap();
        assert!(back.handle.is_none());
        peer_objects.release(vec![first, second]);
        let renewed = peer_objects
            .imported(&peer, local.node.clone(), true)
            .unwrap();
        assert_ne!(renewed.handle, Some(number));
        peer_objects.release(vec![renewed]);
        drop(owner);
        assert!(matches!(
            forwarded.node.object.send(message_empty()),
            Err(crate::ipc::rpc::RpcError::PeerClosed)
        ));
        third.objects.lock().release(vec![third_buffer]);
        drop(back);
    }

    #[test_case]
    fn binder_transaction_handle_ignores_unused_union_upper_word() {
        let domain = domain(Environment::new(), 0);
        let owner = file(domain.clone());
        let peer = file(domain);
        let local = owner
            .objects
            .lock()
            .local(&owner, 0x1000, 0x2000, 0, true, 1)
            .unwrap();
        let buffer = peer
            .objects
            .lock()
            .imported(&peer, local.node.clone(), true)
            .unwrap();
        let number = buffer.handle.unwrap();
        let mut task = Task::new("handle-union".into(), 0, crate::task::TaskType::User);
        task.set_id(1);
        task.set_namespace_id(1);
        peer.ensure_thread(task.get_id()).unwrap();
        let mut tx = [0; 64];
        put64(&mut tx, 0, (0xa5a5a5a5usize << 32) | number as usize);
        peer.command(&task, &LinuxAbi::default(), BC_TRANSACTION, &tx)
            .unwrap();
        let request = owner.endpoint.receive().unwrap().unwrap();
        assert_eq!(request.object_key(), Some(local.node.object.key()));
        drop(request);
        peer.objects.lock().release(vec![buffer]);
    }
}
