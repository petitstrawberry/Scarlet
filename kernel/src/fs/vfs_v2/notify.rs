//! Typed, bounded notifications for VFS-mediated directory mutations.
//! Watches pin stable inode identities, not path strings. ABI clients assign
//! opaque tags and must not reuse them while old events may remain queued.
use alloc::{collections::{BTreeMap,VecDeque},string::{String,ToString},sync::{Arc,Weak},vec::Vec};
use core::sync::atomic::{AtomicU32,Ordering};
use crate::{fs::{FileSystemError,FileSystemErrorKind},sync::{IrqSpinLock,Waker}};
use super::core::{FileSystemRef,VfsNode};

pub const CREATED:u32=1;
pub const DELETED:u32=2;
pub const MOVED_FROM:u32=4;
pub const MOVED_TO:u32=8;
pub const REMOVED_SELF:u32=16;
pub const MOVED_SELF:u32=32;
pub const ALL:u32=63;
const MAX_WATCHES:usize=8192;
const MAX_EVENTS:usize=1024;
const MAX_QUEUES:usize=1024;
const MAX_NAME:usize=255;
type NodeKey=(u64,u64);

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum Kind { Created,Deleted,MovedFrom,MovedTo,RemovedSelf,MovedSelf,Ignored,Overflow }
impl Kind {
    fn interest(self)->u32 {match self {Self::Created=>CREATED,Self::Deleted=>DELETED,
        Self::MovedFrom=>MOVED_FROM,Self::MovedTo=>MOVED_TO,Self::RemovedSelf=>REMOVED_SELF,
        Self::MovedSelf=>MOVED_SELF,Self::Ignored|Self::Overflow=>0}}
}
#[derive(Clone,Debug)]
pub struct Event {pub tag:u64,pub kind:Kind,pub name:String,pub directory:bool,pub cookie:u32}
#[derive(Clone,Copy)]
pub struct WatchOptions {pub tag:u64,pub interests:u32,pub additive:bool,pub once:bool,pub create_new:bool}
struct Watch {key:NodeKey,interests:u32,once:bool,_node:Arc<dyn VfsNode>,_filesystem:FileSystemRef}
struct State {watches:BTreeMap<u64,Watch>,events:VecDeque<Event>,overflow:bool}
impl State {
    fn push(&mut self,event:Event) {
        if self.events.len()>=MAX_EVENTS-1 || event.name.len()>MAX_NAME {
            if !self.overflow {self.events.push_back(Event {tag:0,kind:Kind::Overflow,name:String::new(),directory:false,cookie:0});self.overflow=true;}
        } else {self.events.push_back(event);}
    }
}
pub struct ChangeQueue {state:IrqSpinLock<State>,waker:Waker}
static QUEUES:IrqSpinLock<Vec<Weak<ChangeQueue>>>=IrqSpinLock::new(Vec::new());
static MOVE_COOKIES:AtomicU32=AtomicU32::new(1);
fn error(kind:FileSystemErrorKind,message:&str)->FileSystemError {FileSystemError::new(kind,message)}
fn key(node:&Arc<dyn VfsNode>)->Option<NodeKey> {Some((node.filesystem()?.upgrade()?.fs_id().get(),node.id()))}

impl ChangeQueue {
    pub fn new()->Result<Arc<Self>,FileSystemError> {
        let mut queues=QUEUES.lock();queues.retain(|queue|queue.strong_count()!=0);
        if queues.len()>=MAX_QUEUES {return Err(error(FileSystemErrorKind::NoSpace,"VFS notification queue limit"));}
        let queue=Arc::new(Self {state:IrqSpinLock::new(State {watches:BTreeMap::new(),events:VecDeque::new(),overflow:false}),waker:Waker::new_interruptible("vfs_changes")});
        queues.push(Arc::downgrade(&queue));Ok(queue)
    }
    /// Called by VfsManager while namespace mutations are excluded.
    pub(crate) fn watch(&self,node:Arc<dyn VfsNode>,options:WatchOptions)->Result<u64,FileSystemError> {
        let filesystem=node.filesystem().and_then(|fs|fs.upgrade()).ok_or_else(||error(FileSystemErrorKind::NotSupported,"No stable filesystem"))?;
        if !node.supports_directory_notifications() {return Err(error(FileSystemErrorKind::NotSupported,"Node lacks a stable directory watch identity"));}
        if options.tag==0 || options.interests==0 || options.interests & !ALL!=0 {return Err(error(FileSystemErrorKind::InvalidData,"Invalid directory watch"));}
        let key=(filesystem.fs_id().get(),node.id());let mut state=self.state.lock();
        if let Some((&tag,watch))=state.watches.iter_mut().find(|(_,watch)|watch.key==key) {
            if options.create_new {return Err(error(FileSystemErrorKind::AlreadyExists,"Directory already watched"));}
            watch.interests=if options.additive {watch.interests|options.interests}else{options.interests};
            watch.once=if options.additive {watch.once||options.once}else{options.once};return Ok(tag);
        }
        if state.watches.len()>=MAX_WATCHES {return Err(error(FileSystemErrorKind::NoSpace,"VFS directory watch limit"));}
        if state.watches.contains_key(&options.tag) {return Err(error(FileSystemErrorKind::AlreadyExists,"Watch tag already in use"));}
        state.watches.insert(options.tag,Watch {key,interests:options.interests,once:options.once,_node:node,_filesystem:filesystem});
        Ok(options.tag)
    }
    pub fn remove(&self,tag:u64)->bool {
        let removed={let mut state=self.state.lock();let removed=state.watches.remove(&tag);
            if removed.is_some() {state.push(Event {tag,kind:Kind::Ignored,name:String::new(),directory:false,cookie:0});}removed};
        if removed.is_some() {self.waker.wake_all();true}else{false}
    }
    pub fn ready(&self)->bool {!self.state.lock().events.is_empty()}
    /// The callback must be nonblocking and must not reenter this queue.
    /// Refusing the first record preserves it and reports a short buffer.
    pub fn drain(&self,mut accept:impl FnMut(&Event)->bool)->Result<usize,()> {
        let mut state=self.state.lock();let mut count=0;
        while let Some(event)=state.events.front() {
            if !accept(event) {return if count==0 {Err(())}else{Ok(count)};}
            let event=state.events.pop_front().unwrap();if event.kind==Kind::Overflow {state.overflow=false;}count+=1;
        }
        Ok(count)
    }
    fn emit(&self,key:NodeKey,kind:Kind,name:&str,directory:bool,cookie:u32) {
        let mut retired=Vec::new();let mut notify=false;
        {let mut state=self.state.lock();
            let tags:Vec<_>=state.watches.iter().filter(|(_,watch)|watch.key==key).map(|(&tag,_)|tag).collect();
            for tag in tags {
                let watch=&state.watches[&tag];let matches=watch.interests & kind.interest()!=0;
                let retire=kind==Kind::RemovedSelf || matches && watch.once;
                notify|=matches||retire;
                if matches {state.push(Event {tag,kind,name:name.to_string(),directory,cookie});}
                if retire {retired.push(state.watches.remove(&tag).unwrap());state.push(Event {tag,kind:Kind::Ignored,name:String::new(),directory:false,cookie:0});}
            }
        }
        // Node/filesystem destruction must run outside the IRQ-safe queue lock.
        drop(retired);if notify {self.waker.wake_all();}
    }
}

impl crate::object::capability::Selectable for ChangeQueue {
    fn current_ready(&self,interest:crate::object::capability::ReadyInterest)->crate::object::capability::ReadySet {
        let mut ready=crate::object::capability::ReadySet::none();ready.read=interest.read && self.ready();ready
    }
    fn wait_until_ready(&self,interest:crate::object::capability::ReadyInterest,tf:&mut crate::arch::Trapframe,
        timeout:Option<u64>,minimum:u64)->crate::object::capability::SelectWaitOutcome {
        use crate::object::capability::SelectWaitOutcome;
        let Some(task)=crate::task::mytask() else{return SelectWaitOutcome::TimedOut;};
        match self.waker.wait_with_condition(task.get_id(),tf,timeout,minimum,||interest.read && self.ready()) {
            crate::sync::waker::WaitResult::TimedOut=>SelectWaitOutcome::TimedOut,_=>SelectWaitOutcome::Ready}
    }
}
pub(crate) fn emit(node:&Arc<dyn VfsNode>,kind:Kind,name:&str,directory:bool,cookie:u32) {
    let Some(key)=key(node) else{return;};
    let queues:Vec<_>=QUEUES.lock().iter().filter_map(Weak::upgrade).collect();
    for queue in queues {queue.emit(key,kind,name,directory,cookie);}
}
pub(crate) fn move_cookie()->u32 {loop {let value=MOVE_COOKIES.fetch_add(1,Ordering::Relaxed);if value!=0{return value;}}}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::{FileType,vfs_v2::{VfsManager,PathResolutionOptions}};
    fn options(tag:u64)->WatchOptions {WatchOptions {tag,interests:ALL,additive:false,once:false,create_new:false}}
    fn drain(queue:&ChangeQueue)->Vec<Event> {let mut events=Vec::new();queue.drain(|event|{events.push(event.clone());true}).unwrap();events}
    #[test_case]
    fn vfs_create_exclusive_open_rename_and_unlink_emit_ordered_records() {
        let vfs=VfsManager::new();vfs.create_dir("/w").unwrap();let queue=ChangeQueue::new().unwrap();
        vfs.watch_directory("/w",&PathResolutionOptions::default(),&queue,options(1)).unwrap();
        vfs.create_file("/w/a",FileType::RegularFile).unwrap();
        drop(vfs.open("/w/exclusive",0x2|0x40|0x80).unwrap());
        vfs.rename("/w/a","/w/b").unwrap();vfs.remove("/w/b").unwrap();
        let events=drain(&queue);assert_eq!(events.len(),5);
        assert_eq!((events[0].kind,events[0].name.as_str()),(Kind::Created,"a"));
        assert_eq!((events[1].kind,events[1].name.as_str()),(Kind::Created,"exclusive"));
        assert_eq!((events[2].kind,events[2].name.as_str()),(Kind::MovedFrom,"a"));
        assert_eq!((events[3].kind,events[3].name.as_str()),(Kind::MovedTo,"b"));
        assert_eq!((events[4].kind,events[4].name.as_str()),(Kind::Deleted,"b"));
        assert_ne!(events[2].cookie,0);assert_eq!(events[2].cookie,events[3].cookie);
        assert!(events.iter().all(|event|event.tag==1));
    }
    #[test_case]
    fn directory_identity_survives_move_and_deletion_retires_its_watch() {
        let vfs=VfsManager::new();vfs.create_dir("/w").unwrap();let queue=ChangeQueue::new().unwrap();
        vfs.watch_directory("/w",&PathResolutionOptions::default(),&queue,options(7)).unwrap();
        vfs.rename("/w","/moved").unwrap();vfs.create_dir("/moved/child").unwrap();
        vfs.remove("/moved/child").unwrap();vfs.remove("/moved").unwrap();
        vfs.create_dir("/moved").unwrap();vfs.create_dir("/moved/replacement").unwrap();
        let events=drain(&queue);assert_eq!(events.len(),5);
        assert_eq!(events[0].kind,Kind::MovedSelf);assert_eq!(events[1].kind,Kind::Created);
        assert!(events[1].directory);assert_eq!(events[2].kind,Kind::Deleted);
        assert_eq!(events[3].kind,Kind::RemovedSelf);assert_eq!(events[4].kind,Kind::Ignored);
        assert!(!queue.remove(7));assert!(!queue.ready());
    }
    #[test_case]
    fn overlay_directory_watches_retire_before_path_recreation() {
        use crate::fs::vfs_v2::drivers::overlayfs::OverlayFS;
        let upper=Arc::new(VfsManager::new());let lower=Arc::new(VfsManager::new());
        let overlay=OverlayFS::new_from_paths_and_vfs(Some((&upper,"/")),alloc::vec![(&lower,"/")],"notifications").unwrap();
        assert!(!super::super::core::FileSystemOperations::supports_retained_node_identity(overlay.as_ref()));
        let vfs=VfsManager::new_with_root(overlay);let queue=ChangeQueue::new().unwrap();
        vfs.create_dir("/w").unwrap();
        vfs.watch_directory("/w",&PathResolutionOptions::default(),&queue,options(1)).unwrap();
        vfs.create_file("/w/file",FileType::RegularFile).unwrap();vfs.remove("/w/file").unwrap();
        vfs.remove("/w").unwrap();vfs.create_dir("/w").unwrap();vfs.create_dir("/w/new").unwrap();
        let events=drain(&queue);assert_eq!(events.len(),4);
        assert_eq!(events[0].kind,Kind::Created);assert_eq!(events[1].kind,Kind::Deleted);
        assert_eq!(events[2].kind,Kind::RemovedSelf);assert_eq!(events[3].kind,Kind::Ignored);
    }
    #[test_case]
    fn shared_filesystem_aliases_deliver_but_unrelated_views_do_not() {
        let filesystem=crate::fs::vfs_v2::drivers::tmpfs::TmpFS::new(0);
        let first=VfsManager::new_with_root(filesystem.clone());let alias=VfsManager::new_with_root(filesystem);
        let isolated=VfsManager::new();first.create_dir("/w").unwrap();isolated.create_dir("/w").unwrap();
        let queue=ChangeQueue::new().unwrap();first.watch_directory("/w",&PathResolutionOptions::default(),&queue,options(1)).unwrap();
        isolated.create_dir("/w/hidden").unwrap();assert!(!queue.ready());
        alias.create_dir("/w/shared").unwrap();let events=drain(&queue);assert_eq!(events.len(),1);assert_eq!(events[0].name,"shared");
    }
    #[test_case]
    fn overflow_is_bounded_explicit_and_recoverable_and_short_reads_preserve_records() {
        let vfs=VfsManager::new();let queue=ChangeQueue::new().unwrap();
        vfs.watch_directory("/",&PathResolutionOptions::default(),&queue,options(1)).unwrap();
        let node=vfs.resolve_path("/").unwrap().0.node();let key=key(&node).unwrap();
        for _ in 0..MAX_EVENTS+32 {queue.emit(key,Kind::Created,"x",false,0);}
        assert!(queue.drain(|_|false).is_err());assert!(queue.ready());
        let events=drain(&queue);assert_eq!(events.len(),MAX_EVENTS);
        assert_eq!(events.iter().filter(|e|e.kind==Kind::Overflow).count(),1);
        queue.emit(key,Kind::Created,"after",false,0);assert_eq!(drain(&queue)[0].name,"after");
    }
    #[test_case]
    fn one_shot_and_explicit_removal_always_report_ignored() {
        let vfs=VfsManager::new();let queue=ChangeQueue::new().unwrap();
        let mut once=options(1);once.once=true;once.interests=CREATED;
        vfs.watch_directory("/",&PathResolutionOptions::default(),&queue,once).unwrap();
        vfs.create_dir("/first").unwrap();vfs.create_dir("/second").unwrap();
        let events=drain(&queue);assert_eq!(events.len(),2);assert_eq!(events[0].name,"first");assert_eq!(events[1].kind,Kind::Ignored);
        vfs.watch_directory("/",&PathResolutionOptions::default(),&queue,options(2)).unwrap();
        assert!(queue.remove(2));assert!(!queue.remove(2));assert_eq!(drain(&queue)[0].kind,Kind::Ignored);
    }
}
