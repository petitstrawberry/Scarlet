//! HIDL embedded buffers and descriptor arrays, encoded as bounded native RPC
//! bytes and owned capabilities. Userspace addresses never cross the transport.
use super::*;
use crate::abi::linux::generic::DescriptorReceipt;
use crate::object::handle::HandleMetadata;

pub(super) const FD: u32 = 0x66642a85;
pub(super) const PTR: u32 = 0x70742a85;
pub(super) const FDA: u32 = 0x66646185;
type Capabilities = Vec<(KernelObject, HandleMetadata)>;
fn align(n: usize) -> Result<usize, usize> {
    n.checked_add(7).map(|n| n & !7).ok_or(errno::EINVAL)
}
fn claim(
    claims: &mut Vec<(usize, usize, usize)>,
    parent: usize,
    start: usize,
    size: usize,
    length: usize,
) -> Result<(), usize> {
    let end = start.checked_add(size).ok_or(errno::EINVAL)?;
    if end > length
        || claims
            .iter()
            .any(|&(p, a, b)| p == parent && start < b && a < end)
    {
        return Err(errno::EINVAL);
    }
    claims.push((parent, start, end));
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::abi::linux::generic::fs::FD_CLOEXEC;
    use super::*;
    fn message(task: &Task, abi: &mut LinuxAbi) -> (RpcMessage, usize) {
        let base = crate::environment::USER_STACK_END - crate::environment::PAGE_SIZE;
        task.allocate_stack_pages(base, 1).unwrap();
        let memory = Arc::new(crate::ipc::SharedMemory::new(4096, 1).unwrap());
        let handle = task
            .handle_table
            .insert(KernelObject::SharedMemory(memory))
            .unwrap();
        let fd = abi.allocate_fd(handle).unwrap();
        abi.set_file_status_flags(fd, 2 | 0x800).unwrap();
        let mut bytes = vec![0; objects::HEADER + 136 + 32];
        bytes[12..16].copy_from_slice(&136u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&32u32.to_le_bytes());
        let payload = &mut bytes[objects::HEADER..];
        payload[..4].copy_from_slice(&PTR.to_le_bytes());
        put64(payload, 8, base + 1024);
        put64(payload, 16, 24);
        payload[40..44].copy_from_slice(&PTR.to_le_bytes());
        payload[44..48].copy_from_slice(&1u32.to_le_bytes());
        put64(payload, 48, base + 2048);
        put64(payload, 56, 4);
        put64(payload, 64, 0);
        put64(payload, 72, 0);
        payload[80..84].copy_from_slice(&FDA.to_le_bytes());
        put64(payload, 88, 1);
        put64(payload, 96, 0);
        put64(payload, 104, 8);
        payload[112..116].copy_from_slice(&FD.to_le_bytes());
        payload[116..120].copy_from_slice(&0x17fu32.to_le_bytes());
        put64(payload, 120, fd);
        put64(payload, 128, 1);
        for (i, offset) in [0, 40, 80, 112].into_iter().enumerate() {
            put64(payload, 136 + i * 8, offset);
        }
        let mut root = [0u8; 24];
        root[8..12].copy_from_slice(&(fd as u32).to_le_bytes());
        copy_to_user(task, base + 1024, &root).unwrap();
        copy_to_user(task, base + 2048, b"HIDL").unwrap();
        let capabilities = prepare(task, abi, &mut bytes, &[0, 40, 80, 112], 32).unwrap();
        (
            RpcMessage {
                opcode: 1,
                bytes,
                objects: Vec::new(),
                capabilities,
                retained: None,
            },
            fd,
        )
    }
    #[test_case]
    fn hidl_buffers_relocate_parent_pointers_and_transfer_owned_descriptor_arrays() {
        let mut task = Task::new("hidl-buffer-test".into(), 0, crate::task::TaskType::User);
        task.set_id(60003);
        let mut sender = LinuxAbi::default();
        let receiver = LinuxAbi::default();
        let (message, source) = message(&task, &mut sender);
        assert_eq!(message.capabilities.len(), 2);
        let mut payload = message.bytes[objects::HEADER..message.bytes.len() - 8].to_vec();
        let mut installed = Installed::new(&task, &receiver);
        receive(
            &message,
            &mut payload,
            &[0, 40, 80, 112],
            0x10000,
            136,
            32,
            Some(&mut installed),
        )
        .unwrap();
        assert_eq!(u64_at(&payload, 8), 0x10000 + 168);
        assert_eq!(u64_at(&payload, 48), 0x10000 + 192);
        assert_eq!(u64_at(&payload, 168), 0x10000 + 192);
        assert_eq!(&payload[192..196], b"HIDL");
        let auto_fd = u32_at(&payload, 176) as usize;
        let plain_fd = u64_at(&payload, 120);
        assert_ne!(auto_fd, plain_fd);
        assert_eq!(receiver.get_fd_flags(auto_fd), Some(FD_CLOEXEC));
        assert_eq!(receiver.get_file_status_flags(plain_fd), Some(2 | 0x800));
        let automatic = installed.commit();
        assert_eq!(automatic.len(), 1);
        drop(automatic);
        assert!(receiver.get_handle(auto_fd).is_none());
        assert!(receiver.get_handle(plain_fd).is_some());
        assert!(sender.get_handle(source).is_some());
    }
    #[test_case]
    fn hidl_partial_descriptor_delivery_rolls_back_on_later_malformed_objects() {
        let mut task = Task::new("hidl-rollback-test".into(), 0, crate::task::TaskType::User);
        task.set_id(60004);
        let mut sender = LinuxAbi::default();
        let receiver = LinuxAbi::default();
        let (mut message, _) = message(&task, &mut sender);
        // FDA installs one descriptor before the final flat FD fails validation.
        put64(&mut message.bytes, objects::HEADER + 120, 99);
        let before = task.handle_table.active_handles().len();
        let mut payload = message.bytes[objects::HEADER..message.bytes.len() - 8].to_vec();
        {
            let mut installed = Installed::new(&task, &receiver);
            assert_eq!(
                receive(
                    &message,
                    &mut payload,
                    &[0, 40, 80, 112],
                    0x10000,
                    136,
                    32,
                    Some(&mut installed)
                ),
                Err(errno::EINVAL)
            );
        }
        assert!(receiver.get_handle(0).is_none());
        assert_eq!(task.handle_table.active_handles().len(), before);
        // Parent ranges and pointers cannot alias another fixup or escape the slot.
        for (field, value) in [
            (objects::HEADER + 104, 0usize),
            (objects::HEADER + 96, 3),
            (objects::HEADER + 8, usize::MAX),
        ] {
            let mut bad = message.clone();
            put64(&mut bad.bytes, field, value);
            let mut payload = bad.bytes[objects::HEADER..bad.bytes.len() - 8].to_vec();
            let mut installed = Installed::new(&task, &receiver);
            assert_eq!(
                receive(
                    &bad,
                    &mut payload,
                    &[0, 40, 80, 112],
                    0x10000,
                    136,
                    32,
                    Some(&mut installed)
                ),
                Err(errno::EINVAL)
            );
        }
    }
}
fn cap(
    task: &Task,
    abi: &LinuxAbi,
    fd: u32,
    caps: &mut Capabilities,
    status: &mut Vec<u32>,
) -> Result<usize, usize> {
    if caps.len() >= crate::ipc::rpc::MAX_MESSAGE_OBJECTS {
        return Err(errno::E2BIG);
    }
    if fd > i32::MAX as u32 {
        return Err(errno::EBADF);
    }
    let handle = abi.get_handle(fd as usize).ok_or(errno::EBADF)?;
    caps.push(
        task.handle_table
            .clone_for_dup(handle)
            .ok_or(errno::EBADF)?,
    );
    status.push(abi.get_file_status_flags(fd as usize).ok_or(errno::EBADF)?);
    Ok(caps.len())
}
fn parent<'a>(
    payload: &'a [u8],
    offsets: &[usize],
    index: usize,
    which: usize,
) -> Result<(usize, usize), usize> {
    if which >= index {
        return Err(errno::EINVAL);
    }
    let offset = *offsets.get(which).ok_or(errno::EINVAL)?;
    if u32_at(payload, offset) != PTR {
        return Err(errno::EINVAL);
    }
    Ok((u64_at(payload, offset + 8), u64_at(payload, offset + 16)))
}

pub(super) fn prepare(
    task: &Task,
    abi: &LinuxAbi,
    bytes: &mut Vec<u8>,
    offsets: &[usize],
    budget: usize,
) -> Result<Capabilities, usize> {
    let data = u32_at(bytes, 12) as usize;
    let offset_length = u32_at(bytes, 16) as usize;
    let extra_start = align(data)?
        .checked_add(offset_length)
        .ok_or(errno::EINVAL)?;
    let mut payload = bytes[objects::HEADER..].to_vec();
    let mut caps = Vec::new();
    let mut status = Vec::new();
    let mut claims = Vec::new();
    let mut used = 0usize;
    for (index, &offset) in offsets.iter().enumerate() {
        match u32_at(&payload, offset) {
            PTR => {
                let source = u64_at(&payload, offset + 8);
                let length = u64_at(&payload, offset + 16);
                let padded = align(length)?;
                used = used.checked_add(padded).ok_or(errno::EINVAL)?;
                if used > budget
                    || objects::HEADER + extra_start + used > crate::ipc::rpc::MAX_MESSAGE_SIZE
                {
                    return Err(errno::E2BIG);
                }
                let relative = payload.len();
                payload.resize(relative + padded, 0);
                copy_from_user(task, source, &mut payload[relative..relative + length])
                    .map_err(|_| errno::EFAULT)?;
                put64(&mut payload, offset + 8, relative);
                if u32_at(&payload, offset + 4) & 1 != 0 {
                    let which = u64_at(&payload, offset + 24);
                    let within = u64_at(&payload, offset + 32);
                    if !within.is_multiple_of(8) {
                        return Err(errno::EINVAL);
                    }
                    let (parent, length) = parent(&payload, offsets, index, which)?;
                    claim(&mut claims, which, within, 8, length)?;
                    put64(&mut payload, parent + within, relative);
                }
            }
            FD => {
                let fd = u32_at(&payload, offset + 8);
                let id = cap(task, abi, fd, &mut caps, &mut status)?;
                put64(&mut payload, offset + 8, id);
            }
            FDA => {
                let count = u64_at(&payload, offset + 8);
                let which = u64_at(&payload, offset + 16);
                let within = u64_at(&payload, offset + 24);
                if !within.is_multiple_of(4) || count > crate::ipc::rpc::MAX_MESSAGE_OBJECTS {
                    return Err(errno::EINVAL);
                }
                let (parent, length) = parent(&payload, offsets, index, which)?;
                claim(
                    &mut claims,
                    which,
                    within,
                    count.checked_mul(4).ok_or(errno::EINVAL)?,
                    length,
                )?;
                for n in 0..count {
                    let position = parent + within + n * 4;
                    let fd = u32_at(&payload, position);
                    let id = cap(task, abi, fd, &mut caps, &mut status)?;
                    payload[position..position + 4].copy_from_slice(&(id as u32).to_le_bytes());
                }
            }
            _ => {}
        }
    }
    if objects::HEADER + payload.len() + status.len() * 4 > crate::ipc::rpc::MAX_MESSAGE_SIZE {
        return Err(errno::E2BIG);
    }
    bytes.truncate(objects::HEADER);
    bytes[20..24].copy_from_slice(&(used as u32).to_le_bytes());
    bytes.extend_from_slice(&payload);
    for flags in status {
        bytes.extend_from_slice(&flags.to_le_bytes());
    }
    Ok(caps)
}

pub(super) struct Installed<'a> {
    task: &'a Task,
    abi: &'a LinuxAbi,
    all: Vec<(DescriptorReceipt, bool)>,
}
impl<'a> Installed<'a> {
    pub fn new(task: &'a Task, abi: &'a LinuxAbi) -> Self {
        Self {
            task,
            abi,
            all: Vec::new(),
        }
    }
    fn install(
        &mut self,
        message: &RpcMessage,
        index: usize,
        automatic: bool,
        status_start: usize,
    ) -> Result<usize, usize> {
        if self.all.len() >= crate::ipc::rpc::MAX_MESSAGE_OBJECTS {
            return Err(errno::E2BIG);
        }
        let (object, metadata) = message.capabilities.get(index).ok_or(errno::EINVAL)?;
        let status = u32_at(&message.bytes, status_start + index * 4);
        let receipt =
            self.abi
                .receive_capability(self.task, object.clone(), metadata.clone(), status)?;
        let fd = receipt.fd;
        self.all.push((receipt, automatic));
        Ok(fd)
    }
    pub fn commit(self) -> Vec<DescriptorReceipt> {
        let mut automatic = Vec::new();
        for (receipt, auto) in self.all {
            if auto {
                automatic.push(receipt);
            } else {
                receipt.release();
            }
        }
        automatic
    }
}

pub(super) fn receive(
    message: &RpcMessage,
    payload: &mut [u8],
    offsets: &[usize],
    address: usize,
    data: usize,
    offset_length: usize,
    installed: Option<&mut Installed<'_>>,
) -> Result<(), usize> {
    if message.capabilities.len() > crate::ipc::rpc::MAX_MESSAGE_OBJECTS {
        return Err(errno::E2BIG);
    }
    let start = align(data)?
        .checked_add(offset_length)
        .ok_or(errno::EINVAL)?;
    let extra = u32_at(&message.bytes, 20) as usize;
    let status_start = objects::HEADER + start + extra;
    if status_start.checked_add(message.capabilities.len() * 4) != Some(message.bytes.len())
        || payload.len() != start + extra
    {
        return Err(errno::EINVAL);
    }
    let mut installed = installed;
    let mut claims = Vec::new();
    // Validate every relative range before rewriting any address in-place.
    let mut buffers = Vec::new();
    let mut used = 0usize;
    for (index, &offset) in offsets.iter().enumerate() {
        if u32_at(payload, offset) == PTR {
            let relative = u64_at(payload, offset + 8);
            let length = u64_at(payload, offset + 16);
            if relative != start + used
                || relative
                    .checked_add(length)
                    .is_none_or(|end| end > payload.len())
            {
                return Err(errno::EINVAL);
            }
            used = used.checked_add(align(length)?).ok_or(errno::EINVAL)?;
            if used > extra {
                return Err(errno::EINVAL);
            }
            if u32_at(payload, offset + 4) & 1 != 0 {
                let which = u64_at(payload, offset + 24);
                let within = u64_at(payload, offset + 32);
                if !within.is_multiple_of(8) {
                    return Err(errno::EINVAL);
                }
                let (parent, length) = parent(payload, offsets, index, which)?;
                claim(&mut claims, which, within, 8, length)?;
                if u64_at(payload, parent + within) != relative {
                    return Err(errno::EINVAL);
                }
            }
            buffers.push((offset, relative));
        } else if u32_at(payload, offset) == FDA {
            let which = u64_at(payload, offset + 16);
            let within = u64_at(payload, offset + 24);
            let count = u64_at(payload, offset + 8);
            if !within.is_multiple_of(4) || count > crate::ipc::rpc::MAX_MESSAGE_OBJECTS {
                return Err(errno::EINVAL);
            }
            let (_, length) = parent(payload, offsets, index, which)?;
            claim(
                &mut claims,
                which,
                within,
                count.checked_mul(4).ok_or(errno::EINVAL)?,
                length,
            )?;
        }
    }
    if used != extra {
        return Err(errno::EINVAL);
    }
    for (index, &offset) in offsets.iter().enumerate() {
        match u32_at(payload, offset) {
            FD => {
                let id = u64_at(payload, offset + 8)
                    .checked_sub(1)
                    .ok_or(errno::EINVAL)?;
                let fd = installed.as_deref_mut().ok_or(errno::EOPNOTSUPP)?.install(
                    message,
                    id,
                    false,
                    status_start,
                )?;
                put64(payload, offset + 8, fd);
            }
            FDA => {
                let which = u64_at(payload, offset + 16);
                let within = u64_at(payload, offset + 24);
                let count = u64_at(payload, offset + 8);
                let (relative, _) = parent(payload, offsets, index, which)?;
                for n in 0..count {
                    let position = relative + within + n * 4;
                    let id = (u32_at(payload, position) as usize)
                        .checked_sub(1)
                        .ok_or(errno::EINVAL)?;
                    let fd = installed.as_deref_mut().ok_or(errno::EOPNOTSUPP)?.install(
                        message,
                        id,
                        true,
                        status_start,
                    )?;
                    payload[position..position + 4].copy_from_slice(&(fd as u32).to_le_bytes());
                }
            }
            _ => {}
        }
    }
    for (offset, relative) in &buffers {
        if u32_at(payload, *offset + 4) & 1 != 0 {
            let which = u64_at(payload, *offset + 24);
            let within = u64_at(payload, *offset + 32);
            // Parents remain relative until all child pointer slots are fixed.
            let parent_offset = offsets[which];
            let parent = u64_at(payload, parent_offset + 8);
            put64(
                payload,
                parent + within,
                address.checked_add(*relative).ok_or(errno::EINVAL)?,
            );
        }
    }
    for (offset, relative) in buffers {
        put64(
            payload,
            offset + 8,
            address.checked_add(relative).ok_or(errno::EINVAL)?,
        );
    }
    Ok(())
}
