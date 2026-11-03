//! Minimal Linux BPF object handles for pinned Android network objects.
//!
//! Program execution and packet attachment are separate kernel features. This
//! module supplies stable object identities and file descriptors so userspace
//! can inspect pinned production objects without pretending they executed.
use alloc::{collections::BTreeMap, string::String, sync::{Arc,Weak}, vec::Vec};
use core::{any::Any,sync::atomic::{AtomicU64,Ordering}};
use crate::{arch::Trapframe,fs::{FileMetadata,FilePermission,FileType,SeekFrom},
    library::std::usercopy::{copy_from_user,copy_to_user},
    object::{KernelObject,capability::{ControlOps,FileObject,MemoryMappingInfo,MemoryMappingOps,
        ReadyInterest,SelectWaitOutcome,Selectable,StreamError,StreamOps}},
    sync::{IrqSpinLock,Once},task::mytask};
use super::{LinuxAbi,errno};

const OBJ_PIN:usize=6;
const OBJ_GET:usize=7;
const PROG_ATTACH:usize=8;
const PROG_DETACH:usize=9;
const OBJ_GET_INFO_BY_FD:usize=15;
const MAP_LOOKUP_AND_DELETE_ELEM:usize=21;
const LAST_COMMAND:usize=49;
const MAX_INFO:usize=4096;
static NEXT_ID:AtomicU64=AtomicU64::new(1);

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
enum ObjectKind { Map, Program }

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
struct MapInfo { map_type:u32, key_size:usize, value_size:usize, max_entries:usize }

struct BpfMap { info:MapInfo, entries:IrqSpinLock<BTreeMap<Vec<u8>,Vec<u8>>> }

struct BpfFile {
    kind:ObjectKind,
    path:String,
    id:u64,
    access:u32,
    map:Option<Arc<BpfMap>>,
}

static MAP_REGISTRY:Once<IrqSpinLock<BTreeMap<String,Weak<BpfMap>>>>=Once::new();

fn map_registry()->&'static IrqSpinLock<BTreeMap<String,Weak<BpfMap>>> {
    MAP_REGISTRY.call_once(||IrqSpinLock::new(BTreeMap::new()))
}

fn object_kind(path:&str)->Option<ObjectKind> {
    if path.len()>4096 {return None;}
    let bpf_path=path.strip_prefix("/sys/fs/bpf/")?;
    let mut components=bpf_path.split('/');
    let namespace=components.next()?;
    let name=path.rsplit('/').next()?;
    if name.is_empty() || name=="." || name==".." || path.split('/').any(|part|part=="..") {return None;}
    match namespace {
        "netd_shared" if name.starts_with("prog_")=>Some(ObjectKind::Program),
        "netd_shared" | "tethering" | "net_shared" if map_definition(path).is_some()=>Some(ObjectKind::Map),
        "net_shared" if name.starts_with("prog_")=>Some(ObjectKind::Program),
        _=>None,
    }
}
fn program_type(path:&str)->u32 {
    if path.contains("cgroupskb") {8}
    else if path.contains("cgroupsock") {9}
    else if path.contains("skfilter") {1}
    else {0}
}

fn map_definition(path:&str)->Option<MapInfo> {
    match path.rsplit('/').next()? {
        "map_netd_cookie_tag_map" => Some(MapInfo {map_type:1,key_size:8,value_size:8,max_entries:10_000}),
        "map_netd_stats_map_A" | "map_netd_stats_map_B" => Some(MapInfo {map_type:1,key_size:16,value_size:32,max_entries:5_000}),
        "map_netd_configuration_map" => Some(MapInfo {map_type:2,key_size:4,value_size:4,max_entries:2}),
        "map_netd_uid_permission_map" => Some(MapInfo {map_type:1,key_size:4,value_size:1,max_entries:4_000}),
        "map_netd_uid_counter_set_map" | "map_netd_uid_counterset_map" => Some(MapInfo {map_type:1,key_size:4,value_size:1,max_entries:4_000}),
        "map_netd_app_uid_stats_map" => Some(MapInfo {map_type:1,key_size:4,value_size:32,max_entries:10_000}),
        "map_netd_iface_index_name_map" => Some(MapInfo {map_type:1,key_size:4,value_size:16,max_entries:1_000}),
        "map_netd_iface_stats_map" => Some(MapInfo {map_type:1,key_size:4,value_size:32,max_entries:1_000}),
        "map_netd_uid_owner_map" => Some(MapInfo {map_type:1,key_size:4,value_size:8,max_entries:4_000}),
        "map_netd_data_saver_enabled_map" => Some(MapInfo {map_type:1,key_size:4,value_size:1,max_entries:1}),
        "map_netd_ingress_discard_map" => Some(MapInfo {map_type:1,key_size:16,value_size:8,max_entries:1_024}),
        "map_offload_tether_downstream4_map" | "map_offload_tether_upstream4_map" => Some(MapInfo {map_type:9,key_size:24,value_size:64,max_entries:4_096}),
        "map_offload_tether_downstream6_map" => Some(MapInfo {map_type:9,key_size:28,value_size:20,max_entries:4_096}),
        "map_offload_tether_upstream6_map" => Some(MapInfo {map_type:9,key_size:24,value_size:20,max_entries:4_096}),
        "map_offload_tether_stats_map" => Some(MapInfo {map_type:1,key_size:4,value_size:48,max_entries:1_024}),
        "map_offload_tether_limit_map" => Some(MapInfo {map_type:1,key_size:4,value_size:8,max_entries:1_024}),
        "map_offload_tether_error_map" => Some(MapInfo {map_type:1,key_size:4,value_size:4,max_entries:1_024}),
        "map_offload_tether_dev_map" => Some(MapInfo {map_type:1,key_size:4,value_size:4,max_entries:64}),
        "map_clatd_clat_ingress6_map" => Some(MapInfo {map_type:9,key_size:36,value_size:24,max_entries:1_024}),
        "map_clatd_clat_egress4_map" => Some(MapInfo {map_type:9,key_size:8,value_size:56,max_entries:1_024}),
        _ => None,
    }
}

fn shared_map(path:&str)->Option<Arc<BpfMap>> {
    let info=map_definition(path)?;
    let mut maps=map_registry().lock();
    maps.retain(|_, map|map.strong_count()!=0);
    if let Some(map)=maps.get(path).and_then(Weak::upgrade) {return Some(map);}
    let map=Arc::new(BpfMap {info,entries:IrqSpinLock::new(BTreeMap::new())});
    maps.insert(String::from(path),Arc::downgrade(&map));
    Some(map)
}

fn bpf_map(abi:&LinuxAbi,fd:usize)->Option<(Arc<BpfMap>,u32)> {
    let object=find_object(abi,fd)?;
    let file=object.as_file()?.as_any().downcast_ref::<BpfFile>()?;
    if file.kind!=ObjectKind::Map {return None;}
    Some((file.map.as_ref()?.clone(),file.access))
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

fn install(abi:&mut LinuxAbi,kind:ObjectKind,path:alloc::string::String,access:u32)->Result<usize,usize> {
    let task=mytask().ok_or(errno::ESRCH)?;
    let id=NEXT_ID.fetch_add(1,Ordering::Relaxed);
    let map=if kind==ObjectKind::Map {Some(shared_map(&path).ok_or(errno::ENOENT)?)}else{None};
    let file:Arc<dyn FileObject>=Arc::new(BpfFile {kind,path,id,access,map});
    let handle=task.handle_table.insert(KernelObject::File(file)).map_err(|_|errno::EMFILE)?;
    abi.allocate_fd(handle).map_err(|_|{task.handle_table.remove(handle);errno::EMFILE})
}

fn elem_attr(task:&crate::task::Task,attr:usize)->Result<(usize,usize,usize,u64),usize> {
    let mut bytes=[0u8;32];
    copy_from_user(task,attr,&mut bytes).map_err(|_|errno::EFAULT)?;
    Ok((u32::from_ne_bytes(bytes[..4].try_into().unwrap()) as usize,
        usize::from_ne_bytes(bytes[8..16].try_into().unwrap()),
        usize::from_ne_bytes(bytes[16..24].try_into().unwrap()),
        u64::from_ne_bytes(bytes[24..32].try_into().unwrap())))
}

fn copy_map_key(task:&crate::task::Task,ptr:usize,size:usize)->Result<Vec<u8>,usize> {
    if ptr==0{return Err(errno::EFAULT);}
    let mut key=alloc::vec![0u8;size];
    copy_from_user(task,ptr,&mut key).map_err(|_|errno::EFAULT)?;
    Ok(key)
}

fn map_lookup(task:&crate::task::Task,map:&BpfMap,access:u32,attr:usize,delete:bool)->usize {
    if access&24==16{return errno::to_result(errno::EPERM);}
    let (_fd,key_ptr,value_ptr,flags)=match elem_attr(task,attr){Ok(v)=>v,Err(e)=>return errno::to_result(e)};
    if flags!=0 || value_ptr==0{return errno::to_result(errno::EINVAL);}
    let key=match copy_map_key(task,key_ptr,map.info.key_size){Ok(k)=>k,Err(e)=>return errno::to_result(e)};
    let value={let mut entries=map.entries.lock();
        match entries.get(&key).cloned() {
            Some(value)=>{if delete{entries.remove(&key);}Some(value)},
            None if map.info.map_type==2 => {
                let index=u32::from_ne_bytes(key[..4].try_into().unwrap()) as usize;
                (index<map.info.max_entries).then(||alloc::vec![0;map.info.value_size])
            }
            None=>None,
        }
    };
    let Some(value)=value else{return errno::to_result(errno::ENOENT);};
    if copy_to_user(task,value_ptr,&value).is_err(){return errno::to_result(errno::EFAULT);}
    0
}

fn map_update(task:&crate::task::Task,map:&BpfMap,access:u32,attr:usize)->usize {
    if access&24==8{return errno::to_result(errno::EPERM);}
    let (_,key_ptr,value_ptr,flags)=match elem_attr(task,attr){Ok(v)=>v,Err(e)=>return errno::to_result(e)};
    if value_ptr==0{return errno::to_result(errno::EINVAL);}
    let key=match copy_map_key(task,key_ptr,map.info.key_size){Ok(k)=>k,Err(e)=>return errno::to_result(e)};
    let mut value=alloc::vec![0u8;map.info.value_size];
    if copy_from_user(task,value_ptr,&mut value).is_err(){return errno::to_result(errno::EFAULT);}
    let operation=flags & !4;
    let mut entries=map.entries.lock();
    let exists=entries.contains_key(&key);
    if map.info.map_type==2 {
        let index=u32::from_ne_bytes(key[..4].try_into().unwrap()) as usize;
        if index>=map.info.max_entries{return errno::to_result(errno::E2BIG);}
    } else if operation==1 && exists {return errno::to_result(errno::EEXIST);}
    else if operation==2 && !exists {return errno::to_result(errno::ENOENT);}
    else if operation>2 {return errno::to_result(errno::EINVAL);}
    if !exists && entries.len()>=map.info.max_entries{return errno::to_result(errno::E2BIG);}
    entries.insert(key,value);
    0
}

fn map_delete(task:&crate::task::Task,map:&BpfMap,access:u32,attr:usize)->usize {
    if access&24==8{return errno::to_result(errno::EPERM);}
    let (_,key_ptr,_,flags)=match elem_attr(task,attr){Ok(v)=>v,Err(e)=>return errno::to_result(e)};
    if flags!=0 || map.info.map_type==2{return errno::to_result(errno::EINVAL);}
    let key=match copy_map_key(task,key_ptr,map.info.key_size){Ok(k)=>k,Err(e)=>return errno::to_result(e)};
    map.entries.lock().remove(&key).ok_or(errno::ENOENT).map_or_else(|e|errno::to_result(e),|_|0)
}

fn map_next_key(task:&crate::task::Task,map:&BpfMap,access:u32,attr:usize)->usize {
    if access&24==16{return errno::to_result(errno::EPERM);}
    let (_,key_ptr,next_ptr,flags)=match elem_attr(task,attr){Ok(v)=>v,Err(e)=>return errno::to_result(e)};
    if flags!=0 || next_ptr==0{return errno::to_result(errno::EINVAL);}
    let next={let entries=map.entries.lock();
        if key_ptr==0 {entries.keys().next().cloned()}
        else {
            let key=match copy_map_key(task,key_ptr,map.info.key_size){Ok(k)=>k,Err(e)=>return errno::to_result(e)};
            let present=entries.contains_key(&key);
            if present {entries.range((core::ops::Bound::Excluded(key),core::ops::Bound::Unbounded)).next().map(|(k,_)|k.clone())}
            else {entries.keys().next().cloned()}
        }
    };
    let Some(next)=next else{return errno::to_result(errno::ENOENT);};
    copy_to_user(task,next_ptr,&next).map_or_else(|_|errno::to_result(errno::EFAULT),|_|0)
}

pub fn sys_bpf(abi:&mut LinuxAbi,tf:&mut Trapframe)->usize {
    let Some(task)=mytask() else{return errno::to_result(errno::ESRCH);};
    let command=tf.get_arg(0);let attr=tf.get_arg(1);let size=tf.get_arg(2);
    tf.increment_pc_next(&task);
    if command>LAST_COMMAND || size<4 || size>MAX_INFO {return errno::to_result(errno::EINVAL);}
    match command {
        1 | 2 | 3 | 4 | 21 => {
            if size<4 || attr==0{return errno::to_result(errno::EINVAL);}
            let mut header=[0u8;4];if copy_from_user(&task,attr,&mut header).is_err(){return errno::to_result(errno::EFAULT);}
            let fd=u32::from_ne_bytes(header) as usize;
            let Some((map,access))=bpf_map(abi,fd) else{return errno::to_result(errno::EBADF);};
            match command {
                1 | 21=>map_lookup(&task,&map,access,attr,command==21),
                2=>map_update(&task,&map,access,attr),
                3=>map_delete(&task,&map,access,attr),
                4=>map_next_key(&task,&map,access,attr),
                _=>errno::to_result(errno::EOPNOTSUPP),
            }
        }
        OBJ_PIN => {
            if size<16 || attr==0 {return errno::to_result(errno::EINVAL);}
            let mut fields=[0u8;16];
            if copy_from_user(&task,attr,&mut fields).is_err(){return errno::to_result(errno::EFAULT);}
            let path=usize::from_ne_bytes(fields[..8].try_into().unwrap());
            let fd=u32::from_ne_bytes(fields[8..12].try_into().unwrap()) as usize;
            let flags=u32::from_ne_bytes(fields[12..16].try_into().unwrap());
            if flags!=0 || !program_fd(abi,fd) {return errno::to_result(errno::EINVAL);}
            let path=match crate::library::std::string::parse_c_string_from_userspace(&task,path,4096) {
                Ok(path)=>path,Err(crate::library::std::string::StringConversionError::ExceedsMaxLength)=>return errno::to_result(errno::ENAMETOOLONG),
                Err(_)=>return errno::to_result(errno::EFAULT)};
            if object_kind(&path)!=Some(ObjectKind::Program){return errno::to_result(errno::EINVAL);}
            0
        }
        OBJ_GET => {
            if size<16 || attr==0 {return errno::to_result(errno::EINVAL);}
            let mut header=[0u8;16];
            if copy_from_user(&task,attr,&mut header).is_err(){return errno::to_result(errno::EFAULT);}
            let path=usize::from_ne_bytes(header[..8].try_into().unwrap());
            let fd=u32::from_ne_bytes(header[8..12].try_into().unwrap());
            let flags=u32::from_ne_bytes(header[12..16].try_into().unwrap());
            if fd!=0{return errno::to_result(errno::EINVAL);}
            // BPF_F_RDONLY and BPF_F_WRONLY are the only file-opening modifiers.
            if flags & !((1<<3)|(1<<4))!=0{return errno::to_result(errno::EINVAL);}
            let path=match crate::library::std::string::parse_c_string_from_userspace(&task,path,4096) {
                Ok(path)=>path,Err(crate::library::std::string::StringConversionError::ExceedsMaxLength)=>return errno::to_result(errno::ENAMETOOLONG),
                Err(_)=>return errno::to_result(errno::EFAULT)};
            let Some(kind)=object_kind(&path) else{return errno::to_result(errno::ENOENT);};
            let result=install(abi,kind,path.clone(),flags).map_or_else(errno::to_result,|fd|fd);
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
                    let Some(info)=map_definition(&object.path) else{return errno::to_result(errno::EINVAL);};
                    bytes[..4].copy_from_slice(&info.map_type.to_ne_bytes());
                    bytes[8..12].copy_from_slice(&(info.key_size as u32).to_ne_bytes());
                    bytes[12..16].copy_from_slice(&(info.value_size as u32).to_ne_bytes());
                    bytes[16..20].copy_from_slice(&(info.max_entries as u32).to_ne_bytes());
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
    #[test_case]
    fn tethering_map_layouts_match_the_stock_structs() {
        let base="/sys/fs/bpf/tethering/";
        assert_eq!(map_definition(&alloc::format!("{base}map_offload_tether_downstream4_map")),
            Some(MapInfo {map_type:9,key_size:24,value_size:64,max_entries:4096}));
        assert_eq!(map_definition(&alloc::format!("{base}map_offload_tether_downstream6_map")),
            Some(MapInfo {map_type:9,key_size:28,value_size:20,max_entries:4096}));
        assert_eq!(map_definition(&alloc::format!("{base}map_offload_tether_stats_map")),
            Some(MapInfo {map_type:1,key_size:4,value_size:48,max_entries:1024}));
        assert_eq!(object_kind(&alloc::format!("{base}map_offload_tether_limit_map")),Some(ObjectKind::Map));
        assert_eq!(map_definition("/sys/fs/bpf/net_shared/map_clatd_clat_ingress6_map"),
            Some(MapInfo {map_type:9,key_size:36,value_size:24,max_entries:1024}));
        assert_eq!(map_definition("/sys/fs/bpf/net_shared/map_clatd_clat_egress4_map"),
            Some(MapInfo {map_type:9,key_size:8,value_size:56,max_entries:1024}));
        assert_eq!(object_kind("/sys/fs/bpf/tethering/prog_arbitrary"),None);
    }
    #[test_case]
    fn repeated_object_gets_share_one_native_map() {
        let path="/sys/fs/bpf/tethering/map_offload_tether_dev_map";
        let first=shared_map(path).unwrap();
        let second=shared_map(path).unwrap();
        assert!(Arc::ptr_eq(&first,&second));
        first.entries.lock().insert(alloc::vec![1,0,0,0],alloc::vec![2,0,0,0]);
        assert_eq!(second.entries.lock().get(&alloc::vec![1,0,0,0]),Some(&alloc::vec![2,0,0,0]));
    }
}
