//! Linux directory-change records over the native VFS notification queue.
use super::{LinuxAbi,errno};
use alloc::sync::Arc;
use core::{any::Any,sync::atomic::{AtomicBool,AtomicU32,Ordering}};
use crate::{arch::Trapframe,fs::{FileMetadata,FilePermission,FileType,SeekFrom},
    fs::vfs_v2::{notify::{self,ChangeQueue,Kind,WatchOptions},PathResolutionOptions},
    object::{KernelObject,capability::{ControlOps,FileObject,MemoryMappingOps,MemoryMappingInfo,StreamOps,StreamError,Selectable,ReadyInterest,ReadySet,SelectWaitOutcome}},
    task::mytask};
const NONBLOCK:u32=0x800;
const CLOEXEC:u32=0x80000;
const ONLYDIR:u32=0x01000000;
const DONT_FOLLOW:u32=0x02000000;
const MASK_CREATE:u32=0x10000000;
const MASK_ADD:u32=0x20000000;
const ONESHOT:u32=0x80000000;
const EVENTS:u32=0x100|0x200|0x40|0x80|0x400|0x800;

struct InotifyFile {queue:Arc<ChangeQueue>,next_tag:AtomicU32,nonblocking:AtomicBool,id:u64}
fn record_size(event:&notify::Event)->usize {16+if event.name.is_empty(){0}else{(event.name.len()+4)&!3}}
fn event_mask(kind:Kind)->u32 {match kind {Kind::Created=>0x100,Kind::Deleted=>0x200,Kind::MovedFrom=>0x40,
    Kind::MovedTo=>0x80,Kind::RemovedSelf=>0x400,Kind::MovedSelf=>0x800,Kind::Ignored=>0x8000,Kind::Overflow=>0x4000}}
impl StreamOps for InotifyFile {
    fn read(&self,buffer:&mut [u8])->Result<usize,StreamError> {
        let mut offset=0;
        let records=self.queue.drain(|event| {
            let size=record_size(event);if size>buffer.len()-offset{return false;}
            let wd=if event.kind==Kind::Overflow {-1}else{let Ok(wd)=i32::try_from(event.tag) else{return false;};wd};
            let target=&mut buffer[offset..offset+size];target.fill(0);
            target[..4].copy_from_slice(&wd.to_ne_bytes());
            target[4..8].copy_from_slice(&(event_mask(event.kind)|if event.directory {0x40000000}else{0}).to_ne_bytes());
            target[8..12].copy_from_slice(&event.cookie.to_ne_bytes());
            target[12..16].copy_from_slice(&((size-16) as u32).to_ne_bytes());
            target[16..16+event.name.len()].copy_from_slice(event.name.as_bytes());offset+=size;true
        }).map_err(|_|StreamError::InvalidArgument)?;
        if records==0 {Err(StreamError::WouldBlock)}else{Ok(offset)}
    }
    fn write(&self,_:&[u8])->Result<usize,StreamError> {Err(StreamError::InvalidArgument)}
}
impl Selectable for InotifyFile {
    fn current_ready(&self,interest:ReadyInterest)->ReadySet {self.queue.current_ready(interest)}
    fn wait_until_ready(&self,interest:ReadyInterest,tf:&mut Trapframe,timeout:Option<u64>,minimum:u64)->SelectWaitOutcome {self.queue.wait_until_ready(interest,tf,timeout,minimum)}
    fn set_nonblocking(&self,enabled:bool) {self.nonblocking.store(enabled,Ordering::Release);}
    fn is_nonblocking(&self)->bool {self.nonblocking.load(Ordering::Acquire)}
}
impl ControlOps for InotifyFile {}
impl MemoryMappingOps for InotifyFile {
    fn supports_mmap(&self)->bool {false}
    fn get_mapping_info(&self,_:usize,_:usize)->Result<MemoryMappingInfo,&'static str> {Err("inotify is not mappable")}
}
impl FileObject for InotifyFile {
    fn seek(&self,_:SeekFrom)->Result<u64,StreamError> {Err(StreamError::NotSupported)}
    fn metadata(&self)->Result<FileMetadata,StreamError> {Ok(FileMetadata {file_type:FileType::RegularFile,size:0,
        permissions:FilePermission {read:true,write:false,execute:false},created_time:0,modified_time:0,accessed_time:0,file_id:self.id,link_count:0})}
    fn as_any(&self)->&dyn Any {self}
}
pub fn sys_init1(abi:&mut LinuxAbi,tf:&mut Trapframe)->usize {
    let Some(task)=mytask() else{return errno::to_result(errno::ESRCH);};let flags=tf.get_arg(0) as u32;tf.increment_pc_next(&task);
    if flags & !(NONBLOCK|CLOEXEC)!=0 {return errno::to_result(errno::EINVAL);}
    let queue=match ChangeQueue::new(){Ok(queue)=>queue,Err(_)=>return errno::to_result(errno::EMFILE)};
    static IDS:crate::sync::sequence::IdSequence=crate::sync::sequence::IdSequence::new();
    let Some(id)=IDS.reserve() else{return errno::to_result(errno::ENFILE);};
    let file=Arc::new(InotifyFile {queue,next_tag:AtomicU32::new(1),nonblocking:AtomicBool::new(flags&NONBLOCK!=0),id:id.get()});
    let handle=match task.handle_table.insert(KernelObject::File(file)){Ok(handle)=>handle,Err(_)=>return errno::to_result(errno::EMFILE)};
    let fd=match abi.allocate_fd(handle){Ok(fd)=>fd,Err(_)=>{task.handle_table.remove(handle);return errno::to_result(errno::EMFILE);}};
    let _=abi.set_file_status_flags(fd,flags&NONBLOCK);
    if flags&CLOEXEC!=0 {let _=abi.set_fd_flags(fd,super::fs::FD_CLOEXEC);}
    fd
}
pub fn sys_add_watch(abi:&mut LinuxAbi,tf:&mut Trapframe)->usize {
    let Some(task)=mytask() else{return errno::to_result(errno::ESRCH);};let fd=tf.get_arg(0);let path=tf.get_arg(1);let mask=tf.get_arg(2) as u32;tf.increment_pc_next(&task);
    if mask&EVENTS==0 || mask&MASK_ADD!=0 && mask&MASK_CREATE!=0 {return errno::to_result(errno::EINVAL);}
    if mask & !(EVENTS|ONLYDIR|DONT_FOLLOW|MASK_CREATE|MASK_ADD|ONESHOT)!=0 {return errno::to_result(errno::EOPNOTSUPP);}
    let Some(object)=abi.get_handle(fd).and_then(|handle|task.handle_table.get_arc_clone(handle)) else{return errno::to_result(errno::EBADF);};
    let Some(file)=object.as_file().and_then(|file|file.as_any().downcast_ref::<InotifyFile>()) else{return errno::to_result(errno::EINVAL);};
    let path=match crate::library::std::string::parse_c_string_from_userspace(&task,path,4096){Ok(path)=>path,
        Err(crate::library::std::string::StringConversionError::ExceedsMaxLength)=>return errno::to_result(errno::ENAMETOOLONG),Err(_)=>return errno::to_result(errno::EFAULT)};
    if path.is_empty(){return errno::to_result(errno::ENOENT);}
    let Some(vfs)=task.get_vfs() else{return errno::to_result(errno::EIO);};
    let tag=match file.next_tag.fetch_update(Ordering::Relaxed,Ordering::Relaxed,|next|(next<=i32::MAX as u32).then(||next+1)) {
        Ok(tag)=>tag,Err(_)=>return errno::to_result(errno::ENOSPC)};
    let mut interests=0;for (linux,native) in [(0x100,notify::CREATED),(0x200,notify::DELETED),(0x40,notify::MOVED_FROM),(0x80,notify::MOVED_TO),(0x400,notify::REMOVED_SELF),(0x800,notify::MOVED_SELF)] {if mask&linux!=0 {interests|=native;}}
    let options=WatchOptions {tag:tag as u64,interests,additive:mask&MASK_ADD!=0,once:mask&ONESHOT!=0,create_new:mask&MASK_CREATE!=0};
    match vfs.watch_directory(&path,&PathResolutionOptions {no_follow:mask&DONT_FOLLOW!=0},&file.queue,options) {
        Ok(tag)=>tag as usize,
        Err(error)=>errno::to_result(if error.kind==crate::fs::FileSystemErrorKind::NotSupported || error.kind==crate::fs::FileSystemErrorKind::NotADirectory && mask&ONLYDIR==0 {errno::EOPNOTSUPP}else{errno::from_fs_error(&error)})}
}
pub fn sys_rm_watch(abi:&mut LinuxAbi,tf:&mut Trapframe)->usize {
    let Some(task)=mytask() else{return errno::to_result(errno::ESRCH);};let fd=tf.get_arg(0);let tag=tf.get_arg(1) as i32;tf.increment_pc_next(&task);
    let Some(object)=abi.get_handle(fd).and_then(|handle|task.handle_table.get_arc_clone(handle)) else{return errno::to_result(errno::EBADF);};
    let Some(file)=object.as_file().and_then(|file|file.as_any().downcast_ref::<InotifyFile>()) else{return errno::to_result(errno::EINVAL);};
    if tag<=0 || !file.queue.remove(tag as u64) {errno::to_result(errno::EINVAL)}else{0}
}
