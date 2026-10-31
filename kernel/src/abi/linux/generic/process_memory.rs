//! Safe vectored reads within an explicitly shared address space.
//! Cross-address-space debugging requires a separate native authority model.
use alloc::vec::Vec;
use crate::{arch::Trapframe, task::{Task,mytask}, environment::PAGE_SIZE,
    library::std::usercopy::{copy_from_user,copy_to_user}};
use super::{LinuxAbi,errno};
use scarlet_abi::data_model::AbiDataModel;

fn checked_copy(task:&Task,address:usize,bytes:&mut [u8],writing:bool)->Result<(),usize> {
    AbiDataModel::NATIVE.user_address(address as u64)
        .and_then(|address|address.check_range(bytes.len() as u64)).map_err(|_|errno::EFAULT)?;
    let mut at=0;
    while at<bytes.len() {
        let cursor=address+at;let count=(PAGE_SIZE-cursor%PAGE_SIZE).min(bytes.len()-at);
        let map=task.vm_manager.search_memory_map(cursor).ok_or(errno::EFAULT)?;
        let permissions=8|if writing {2}else{1};
        if map.permissions&permissions!=permissions || cursor+count-1>map.vmarea.end {return Err(errno::EFAULT);}
        let copied=if writing {copy_to_user(task,cursor,&bytes[at..at+count])}
            else {copy_from_user(task,cursor,&mut bytes[at..at+count])};
        copied.map_err(|_|errno::EFAULT)?;at+=count;
    }
    Ok(())
}

fn vectors(task:&Task,address:usize,count:usize)->Result<Vec<(usize,usize)>,usize> {
    if count>1024 {return Err(errno::EINVAL);}
    let mut vectors=Vec::new();vectors.try_reserve_exact(count).map_err(|_|errno::ENOMEM)?;
    let mut total=0usize;
    for index in 0..count {
        let cursor=address.checked_add(index*16).ok_or(errno::EFAULT)?;
        let mut bytes=[0;16];checked_copy(task,cursor,&mut bytes,false)?;
        let base=usize::from_ne_bytes(bytes[..8].try_into().unwrap());
        let length=usize::from_ne_bytes(bytes[8..].try_into().unwrap());
        total=total.checked_add(length).filter(|total|*total<=isize::MAX as usize).ok_or(errno::EINVAL)?;
        if length!=0 && base.checked_add(length-1).is_none() {return Err(errno::EFAULT);}
        vectors.push((base,length));
    }
    Ok(vectors)
}

fn transfer(task:&Task,local:&[(usize,usize)],remote:&[(usize,usize)])->usize {
    let (mut li,mut ri,mut lo,mut ro,mut transferred)=(0,0,0,0,0);
    let mut buffer=[0;PAGE_SIZE];
    while li<local.len() && ri<remote.len() {
        if lo==local[li].1 {li+=1;lo=0;continue;}
        if ro==remote[ri].1 {ri+=1;ro=0;continue;}
        let destination=local[li].0+lo;let source=remote[ri].0+ro;
        let count=(local[li].1-lo).min(remote[ri].1-ro)
            .min(PAGE_SIZE-destination%PAGE_SIZE).min(PAGE_SIZE-source%PAGE_SIZE);
        if checked_copy(task,source,&mut buffer[..count],false).is_err()
            || checked_copy(task,destination,&mut buffer[..count],true).is_err() {
            return if transferred==0 {errno::to_result(errno::EFAULT)}else{transferred};
        }
        lo+=count;ro+=count;transferred+=count;
    }
    transferred
}

pub fn sys_readv(_abi:&mut LinuxAbi,frame:&mut Trapframe)->usize {
    let Some(task)=mytask() else{return errno::to_result(errno::ESRCH);};
    let pid=frame.get_arg(0) as i32;
    let (local,count,remote,remote_count,flags)=(frame.get_arg(1),frame.get_arg(2),frame.get_arg(3),frame.get_arg(4),frame.get_arg(5));
    frame.increment_pc_next(&task);
    if flags!=0 || count>1024 || remote_count>1024 {return errno::to_result(errno::EINVAL);}
    if pid<=0 {return errno::to_result(errno::ESRCH);}
    let target=task.get_namespace().resolve_global_id(pid as usize)
        .and_then(crate::sched::scheduler::get_task_by_id);
    let Some(target)=target else{return errno::to_result(errno::ESRCH);};
    if !task.vm_manager.shares_address_space(&target.vm_manager) {return errno::to_result(errno::EPERM);}
    let local=match vectors(&task,local,count){Ok(v)=>v,Err(error)=>return errno::to_result(error)};
    let remote=match vectors(&task,remote,remote_count){Ok(v)=>v,Err(error)=>return errno::to_result(error)};
    transfer(&task,&local,&remote)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use crate::vm::vmem::{MemoryArea,PhysicalMemoryArea,VirtualMemoryMap};
    use crate::object::capability::memory_mapping::anon_owner::AnonymousZeroPageOwner;
    #[test_case]
    fn self_readv_scatters_and_stops_at_inaccessible_source_or_destination() {
        let task=crate::task::new_user_task("self-readv".into(),0);
        task.vm_manager.set_asid(crate::arch::vm::alloc_virtual_address_space());
        let base=0x21000000;
        task.vm_manager.add_memory_map(VirtualMemoryMap::new(PhysicalMemoryArea::new(0,0),
            MemoryArea::new(base,base+4*PAGE_SIZE-1),11,false,Some(Arc::new(AnonymousZeroPageOwner::new())))).unwrap();
        copy_to_user(&task,base,b"abcdefgh").unwrap();
        let destination=base+3*PAGE_SIZE;
        assert_eq!(transfer(&task,&[(destination,3),(destination+16,5)],&[(base,8)]),8);
        let mut bytes=[0;5];copy_from_user(&task,destination+16,&mut bytes).unwrap();assert_eq!(&bytes,b"defgh");
        task.vm_manager.protect_memory_map_range(MemoryArea::new(base+PAGE_SIZE,base+2*PAGE_SIZE-1),0).unwrap();
        assert_eq!(transfer(&task,&[(destination,16)],&[(base,8),(base+PAGE_SIZE,8)]),8);
        assert_eq!(transfer(&task,&[(destination,8)],&[(base+PAGE_SIZE,8)]),errno::to_result(errno::EFAULT));
        task.vm_manager.protect_memory_map_range(MemoryArea::new(destination,destination+PAGE_SIZE-1),1).unwrap();
        assert_eq!(transfer(&task,&[(destination,8)],&[(base,8)]),errno::to_result(errno::EFAULT));
        assert_eq!(vectors(&task,0,1025).unwrap_err(),errno::EINVAL);
    }
}
