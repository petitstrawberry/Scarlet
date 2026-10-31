//! Minimal Linux BPF object handles for pinned Android network objects.
//!
//! Program execution and packet attachment are separate kernel features. This
//! module supplies stable object identities and file descriptors so userspace
//! can inspect pinned production objects without pretending they executed.
use alloc::sync::Arc;
use core::{any::Any,sync::atomic::{AtomicU64,Ordering}};
use crate::{arch::Trapframe,fs::{FileMetadata,FilePermission,FileType,SeekFrom},
    library::std::usercopy::{copy_from_user,copy_to_user},
    object::{KernelObject,capability::{ControlOps,FileObject,MemoryMappingInfo,MemoryMappingOps,
        ReadyInterest,SelectWaitOutcome,Selectable,StreamError,StreamOps}},task::mytask};
use super::{LinuxAbi,errno};

const OBJ_PIN:usize=6;
const OBJ_GET:usize=7;
const PROG_ATTACH:usize=8;
const PROG_DETACH:usize=9;
const OBJ_GET_INFO_BY_FD:usize=15;
const LAST_COMMAND:usize=49;
const MAX_INFO:usize=4096;
static NEXT_ID:AtomicU64=AtomicU64::new(1);

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
enum ObjectKind { Map, Program }

struct BpfFile { kind:ObjectKind, path:alloc::string::String, id:u64 }

fn object_kind(path:&str)->Option<ObjectKind> {
    if path.len()>4096 || !path.starts_with("/sys/fs/bpf/netd_shared/") {return None;}
    let name=path.rsplit('/').next()?;
    if name.is_empty() || name=="." || name==".." || path.split('/').any(|part|part=="..") {return None;}
    Some(if name.starts_with("prog_") {ObjectKind::Program}else if name.starts_with("map_") {ObjectKind::Map}else{return None})
}
fn program_type(path:&str)->u32 {
    if path.contains("cgroupskb") {8}
    else if path.contains("cgroupsock") {9}
    else if path.contains("skfilter") {1}
    else {0}
}

fn map_info(path:&str)->Option<(u32,u32,u32,u32)> {
    match path.rsplit('/').next()? {
        "map_netd_cookie_tag_map" => Some((1,8,8,10_000)),
        "map_netd_stats_map_A" | "map_netd_stats_map_B" => Some((1,16,32,5_000)),
        "map_netd_configuration_map" => Some((2,4,4,2)),
        "map_netd_uid_permission_map" => Some((1,4,1,4_000)),
        "map_netd_uid_counter_set_map" | "map_netd_uid_counterset_map" => Some((1,4,1,4_000)),
        "map_netd_app_uid_stats_map" => Some((1,4,32,10_000)),
        "map_netd_iface_index_name_map" => Some((1,4,16,1_000)),
        "map_netd_iface_stats_map" => Some((1,4,32,1_000)),
        "map_netd_uid_owner_map" => Some((1,4,8,4_000)),
        _ => None,
    }
}

impl StreamOps for BpfFile {
    fn read(&self,_:&mut [u8])->Result<usize,StreamError>{Err(StreamError::NotSupported)}
    fn write(&self,_:&[u8])->Result<usize,StreamError>{Err(StreamError::NotSupported)}
}
impl ControlOps for BpfFile {}
impl MemoryMappingOps for BpfFile {
    fn supports_mmap(&self)->bool{false}
    fn get_mapping_info(&self,_:usize,_:usize)->Result<MemoryMappingInfo,&'static str>{Err("BPF objects are not mappable")}
}
impl Selectable for BpfFile {
    fn wait_until_ready(&self,_:ReadyInterest,_:&mut Trapframe,_:Option<u64>,_:u64)->SelectWaitOutcome{SelectWaitOutcome::Ready}
}
impl FileObject for BpfFile {
    fn seek(&self,_:SeekFrom)->Result<u64,StreamError>{Err(StreamError::NotSupported)}
    fn metadata(&self)->Result<FileMetadata,StreamError>{Ok(FileMetadata {file_type:FileType::RegularFile,size:0,
        permissions:FilePermission {read:true,write:false,execute:false},created_time:0,modified_time:0,
        accessed_time:0,file_id:self.id,link_count:0})}
    fn as_any(&self)->&dyn Any{self}
}

fn find_object(abi:&LinuxAbi,fd:usize)->Option<KernelObject> {
    let task=mytask()?;let handle=abi.get_handle(fd)?;
    task.handle_table.get_arc_clone(handle)
}

fn program_fd(abi:&LinuxAbi,fd:usize)->bool {
    find_object(abi,fd).and_then(|object|object.as_file().and_then(|file|file.as_any().downcast_ref::<BpfFile>().map(|bpf|bpf.kind)))
        .is_some_and(|kind|kind==ObjectKind::Program)
}

fn install(abi:&mut LinuxAbi,kind:ObjectKind,path:alloc::string::String)->Result<usize,usize> {
    let task=mytask().ok_or(errno::ESRCH)?;
    let id=NEXT_ID.fetch_add(1,Ordering::Relaxed);
    let file:Arc<dyn FileObject>=Arc::new(BpfFile {kind,path,id});
    let handle=task.handle_table.insert(KernelObject::File(file)).map_err(|_|errno::EMFILE)?;
    abi.allocate_fd(handle).map_err(|_|{task.handle_table.remove(handle);errno::EMFILE})
}

pub fn sys_bpf(abi:&mut LinuxAbi,tf:&mut Trapframe)->usize {
    let Some(task)=mytask() else{return errno::to_result(errno::ESRCH);};
    let command=tf.get_arg(0);let attr=tf.get_arg(1);let size=tf.get_arg(2);
    tf.increment_pc_next(&task);
    if command>LAST_COMMAND || size<4 || size>MAX_INFO {return errno::to_result(errno::EINVAL);}
    match command {
        1 | 2 | 3 | 4 => {
            if size<4 || attr==0{return errno::to_result(errno::EINVAL);}
            let mut header=[0u8;4];if copy_from_user(&task,attr,&mut header).is_err(){return errno::to_result(errno::EFAULT);}
            let fd=u32::from_ne_bytes(header) as usize;
            let Some(object)=find_object(abi,fd) else{return errno::to_result(errno::EBADF);};
            let Some(file)=object.as_file() else{return errno::to_result(errno::EINVAL);};
            if !file.as_any().downcast_ref::<BpfFile>().is_some_and(|bpf|bpf.kind==ObjectKind::Map){return errno::to_result(errno::EINVAL);}
            // No packet execution exists to populate these maps. An empty map is
            // reported through the normal enumeration contract; updates remain
            // accepted so configuration can complete without pretending that
            // traffic accounting is active.
            if command==2 {0} else {errno::to_result(errno::ENOENT)}
        }
        OBJ_PIN => {
            if size<20 || attr==0 {return errno::to_result(errno::EINVAL);}
            let mut fields=[0u8;20];
            if copy_from_user(&task,attr,&mut fields).is_err(){return errno::to_result(errno::EFAULT);}
            let fd=u32::from_ne_bytes(fields[..4].try_into().unwrap()) as usize;
            let path=usize::from_ne_bytes(fields[8..16].try_into().unwrap());
            let flags=u32::from_ne_bytes(fields[16..20].try_into().unwrap());
            if flags!=0 || !program_fd(abi,fd) {return errno::to_result(errno::EINVAL);}
            let path=match crate::library::std::string::parse_c_string_from_userspace(&task,path,4096) {
                Ok(path)=>path,Err(crate::library::std::string::StringConversionError::ExceedsMaxLength)=>return errno::to_result(errno::ENAMETOOLONG),
                Err(_)=>return errno::to_result(errno::EFAULT)};
            if object_kind(&path)!=Some(ObjectKind::Program){return errno::to_result(errno::EINVAL);}
            0
        }
        OBJ_GET => {
            if size<12 || attr==0 {return errno::to_result(errno::EINVAL);}
            let mut header=[0u8;12];
            if copy_from_user(&task,attr,&mut header).is_err(){return errno::to_result(errno::EFAULT);}
            let path=usize::from_ne_bytes(header[..8].try_into().unwrap());
            let flags=u32::from_ne_bytes(header[8..12].try_into().unwrap());
            // BPF_F_RDONLY and BPF_F_WRONLY are the only file-opening modifiers.
            if flags & !((1<<3)|(1<<4))!=0{return errno::to_result(errno::EINVAL);}
            let path=match crate::library::std::string::parse_c_string_from_userspace(&task,path,4096) {
                Ok(path)=>path,Err(crate::library::std::string::StringConversionError::ExceedsMaxLength)=>return errno::to_result(errno::ENAMETOOLONG),
                Err(_)=>return errno::to_result(errno::EFAULT)};
            let Some(kind)=object_kind(&path) else{return errno::to_result(errno::ENOENT);};
            let result=install(abi,kind,path.clone()).map_or_else(errno::to_result,|fd|fd);
            result
        }
        OBJ_GET_INFO_BY_FD => {
            if size<16 || attr==0 {return errno::to_result(errno::EINVAL);}
            let mut header=[0u8;16];if copy_from_user(&task,attr,&mut header).is_err(){return errno::to_result(errno::EFAULT);}
            let fd=u32::from_ne_bytes(header[..4].try_into().unwrap()) as usize;
            let length=u32::from_ne_bytes(header[4..8].try_into().unwrap()) as usize;
            let info=usize::from_ne_bytes(header[8..16].try_into().unwrap());
            if length<4 || length>MAX_INFO || info==0{return errno::to_result(errno::EINVAL);}
            let Some(object)=find_object(abi,fd) else{return errno::to_result(errno::EBADF);};
            let Some(file)=object.as_file() else{return errno::to_result(errno::EINVAL);};
            let Some(object)=file.as_any().downcast_ref::<BpfFile>() else{return errno::to_result(errno::EINVAL);};
            let mut bytes=alloc::vec![0u8;length];
            match object.kind {
                ObjectKind::Program=>bytes[..4].copy_from_slice(&program_type(&object.path).to_ne_bytes()),
                ObjectKind::Map=>{
                    if length<20{return errno::to_result(errno::EINVAL);}
                    let Some((map_type,key_size,value_size,max_entries))=map_info(&object.path) else{return errno::to_result(errno::EINVAL);};
                    bytes[..4].copy_from_slice(&map_type.to_ne_bytes());
                    bytes[8..12].copy_from_slice(&key_size.to_ne_bytes());
                    bytes[12..16].copy_from_slice(&value_size.to_ne_bytes());
                    bytes[16..20].copy_from_slice(&max_entries.to_ne_bytes());
                }
            }
            if let Some(id)=bytes.get_mut(4..8){id.copy_from_slice(&(object.id as u32).to_ne_bytes());}
            let result=copy_to_user(&task,info,&bytes).map_or_else(|_|errno::to_result(errno::EFAULT),|_|0);
            result
        }
        PROG_ATTACH | PROG_DETACH => {
            if size<20 || attr==0{return errno::to_result(errno::EINVAL);}
            let mut fields=[0u8;20];if copy_from_user(&task,attr,&mut fields).is_err(){return errno::to_result(errno::EFAULT);}
            let target=u32::from_ne_bytes(fields[..4].try_into().unwrap()) as usize;
            let program=u32::from_ne_bytes(fields[4..8].try_into().unwrap()) as usize;
            let replace=u32::from_ne_bytes(fields[16..20].try_into().unwrap()) as usize;
            let result=if abi.get_handle(target).is_none() || !program_fd(abi,program) || replace!=0{errno::to_result(errno::EBADF)}else{0};
            if result!=0{return result;}
            // This records a valid object relationship. Packet execution remains
            // a separate capability and is not implied by the attach request.
            0
        }
        _ => errno::to_result(errno::EOPNOTSUPP),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test_case]
    fn object_paths_cover_only_named_pinned_network_objects() {
        assert_eq!(object_kind("/sys/fs/bpf/netd_shared/prog_netd_cgroupskb_ingress_stats"),Some(ObjectKind::Program));
        assert_eq!(object_kind("/sys/fs/bpf/netd_shared/map_netd_stats_map_A"),Some(ObjectKind::Map));
        assert_eq!(object_kind("/sys/fs/bpf/netd_shared/../secret"),None);
        assert_eq!(object_kind("/tmp/prog_netd"),None);
        assert_eq!(program_type("prog_netd_cgroupsock_inet_create"),9);
    }
}
