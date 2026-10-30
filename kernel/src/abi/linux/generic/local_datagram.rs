//! Linux message headers over native local datagram records.
use super::*;
use crate::library::std::usercopy::{copy_from_user, copy_to_user};
use crate::network::{LocalSocketAddress, SocketAddress};
use alloc::vec;
fn address(
    task: &crate::task::Task,
    pointer: usize,
    length: usize,
) -> Result<SocketAddress, usize> {
    if pointer == 0 {
        return Ok(SocketAddress::Unspecified);
    }
    if !(2..=110).contains(&length) {
        return Err(errno::EINVAL);
    }
    let mut bytes = [0; 110];
    copy_from_user(task, pointer, &mut bytes[..length]).map_err(|_| errno::EFAULT)?;
    if u16::from_ne_bytes(bytes[..2].try_into().unwrap()) != AF_UNIX_U16 {
        return Err(errno::EAFNOSUPPORT);
    }
    linux_local_sockaddr_from_user(task, bytes[2..].as_ptr(), length - 2, false)
        .map(|(address, _, _)| address)
}
pub(super) fn send(
    abi: &LinuxAbi,
    task: &crate::task::Task,
    socket: &LocalSocket,
    message: &LinuxMsghdr,
    iovecs: &[IoVec],
    flags: i32,
) -> usize {
    let result = (|| {
        if flags & !(MSG_DONTWAIT | 0x4000) != 0 {
            return Err(errno::EOPNOTSUPP);
        }
        let mut handles = Vec::new();
        if message.msg_controllen > 4096 {
            return Err(errno::EINVAL);
        }
        if message.msg_controllen != 0 && message.msg_control == 0 {
            return Err(errno::EFAULT);
        }
        let mut offset = 0usize;
        while message.msg_control != 0
            && offset + size_of::<LinuxCmsghdr>() <= message.msg_controllen as usize
        {
            let mut raw = [0u8; size_of::<LinuxCmsghdr>()];
            copy_from_user(
                task,
                (message.msg_control as usize)
                    .checked_add(offset)
                    .ok_or(errno::EFAULT)?,
                &mut raw,
            )
            .map_err(|_| errno::EFAULT)?;
            // SAFETY: copied complete POD header; alignment is not required.
            let header = unsafe { core::ptr::read_unaligned(raw.as_ptr().cast::<LinuxCmsghdr>()) };
            if header.cmsg_len < size_of::<LinuxCmsghdr>()
                || header.cmsg_len > message.msg_controllen as usize - offset
            {
                return Err(errno::EINVAL);
            }
            if header.cmsg_level != SOL_SOCKET {
                return Err(errno::EOPNOTSUPP);
            }
            let mut data = vec![0; header.cmsg_len - size_of::<LinuxCmsghdr>()];
            let pointer = (message.msg_control as usize)
                .checked_add(offset + size_of::<LinuxCmsghdr>())
                .ok_or(errno::EFAULT)?;
            copy_from_user(task, pointer, &mut data).map_err(|_| errno::EFAULT)?;
            match header.cmsg_type {
                SCM_RIGHTS => {
                    if !data.len().is_multiple_of(4)
                        || handles.len() + data.len() / 4 > MAX_HANDLE_QUEUE_SIZE
                    {
                        return Err(errno::EINVAL);
                    }
                    for descriptor in data.chunks_exact(4) {
                        let fd = i32::from_ne_bytes(descriptor.try_into().unwrap());
                        let handle = abi.get_handle(fd as usize).ok_or(errno::EBADF)?;
                        handles.push(
                            task.handle_table
                                .clone_for_dup(handle)
                                .ok_or(errno::EBADF)?,
                        );
                    }
                }
                SCM_CREDENTIALS => {
                    if data.len() != size_of::<LinuxUcred>() {
                        return Err(errno::EINVAL);
                    }
                    let ids = task.credentials.read();
                    if i32::from_ne_bytes(data[..4].try_into().unwrap())
                        != task
                            .get_namespace()
                            .resolve_local_id(task.get_thread_group_id())
                            .unwrap_or(0) as i32
                        || u32::from_ne_bytes(data[4..8].try_into().unwrap())
                            != ids.principals().base.0
                        || u32::from_ne_bytes(data[8..12].try_into().unwrap())
                            != ids.groups().base.0
                    {
                        return Err(errno::EPERM);
                    }
                }
                _ => return Err(errno::EOPNOTSUPP),
            }
            offset += (header.cmsg_len + 7) & !7;
        }
        let total = iovecs
            .iter()
            .try_fold(0usize, |n, iov| n.checked_add(iov.iov_len))
            .ok_or(errno::EMSGSIZE)?;
        if total > crate::network::local::MAX_DATAGRAM_BYTES {
            return Err(errno::EMSGSIZE);
        }
        let mut data = vec![0; total];
        let mut offset = 0;
        for iov in iovecs {
            copy_from_user(
                task,
                iov.iov_base as usize,
                &mut data[offset..offset + iov.iov_len],
            )
            .map_err(|_| errno::EFAULT)?;
            offset += iov.iov_len;
        }
        let destination = address(
            task,
            message.msg_name as usize,
            message.msg_namelen as usize,
        )?;
        socket
            .send_datagram(data, handles, &destination, flags & MSG_DONTWAIT != 0)
            .map_err(socket_error_to_errno)
    })();
    result.unwrap_or_else(errno::to_result)
}
fn append_control(control: &mut Vec<u8>, capacity: usize, kind: i32, data: &[u8]) -> bool {
    let aligned = (control.len() + 7) & !7;
    let length = size_of::<LinuxCmsghdr>() + data.len();
    if aligned + length > capacity {
        return false;
    }
    control.resize(aligned + length, 0);
    let header = LinuxCmsghdr {
        cmsg_len: length,
        cmsg_level: SOL_SOCKET,
        cmsg_type: kind,
    };
    // SAFETY: initialized plain C header copied into its reserved byte range.
    unsafe {
        core::ptr::copy_nonoverlapping(
            (&header as *const LinuxCmsghdr).cast::<u8>(),
            control.as_mut_ptr().add(aligned),
            size_of::<LinuxCmsghdr>(),
        );
    }
    control[aligned + size_of::<LinuxCmsghdr>()..aligned + length].copy_from_slice(data);
    true
}
fn source_address(
    task: &crate::task::Task,
    message: &mut LinuxMsghdr,
    source: Option<String>,
) -> Result<(), usize> {
    let address = source
        .map(|name| crate::network::local::datagram_source_address(&name))
        .unwrap_or_else(LocalSocketAddress::unnamed);
    let mut bytes = vec![0u8; 2];
    bytes[..2].copy_from_slice(&AF_UNIX_U16.to_ne_bytes());
    if address.is_abstract() {
        bytes.push(0);
        bytes.extend_from_slice(address.path().as_bytes());
    } else if !address.path().is_empty() {
        bytes.extend_from_slice(address.path().as_bytes());
        bytes.push(0);
    }
    if message.msg_name != 0 {
        copy_to_user(
            task,
            message.msg_name as usize,
            &bytes[..bytes.len().min(message.msg_namelen as usize)],
        )
        .map_err(|_| errno::EFAULT)?;
    }
    message.msg_namelen = bytes.len() as u32;
    Ok(())
}
pub(super) fn receive(
    abi: &mut LinuxAbi,
    task: &crate::task::Task,
    socket: &LocalSocket,
    fd: usize,
    pointer: usize,
    flags: i32,
) -> usize {
    let mut installed = Vec::new();
    let result = (|| {
        if flags & !(MSG_DONTWAIT | MSG_PEEK as i32 | MSG_TRUNC as i32 | 0x100 | 0x4000_0000) != 0 {
            return Err(errno::EOPNOTSUPP);
        }
        let mut header = read_linux_msghdr(task, pointer)?;
        if header.msg_iovlen > 1024 {
            return Err(errno::EINVAL);
        }
        let iovecs = read_linux_iovecs(task, header.msg_iov as usize, header.msg_iovlen as usize)?;
        let capacity = iovecs
            .iter()
            .try_fold(0usize, |n, iov| n.checked_add(iov.iov_len))
            .ok_or(errno::EINVAL)?;
        if capacity > isize::MAX as usize {
            return Err(errno::EINVAL);
        }
        let nonblocking = flags & MSG_DONTWAIT != 0
            || abi
                .get_file_status_flags(fd)
                .is_some_and(|flags| flags as i32 & O_NONBLOCK != 0);
        let packet = socket
            .receive_datagram(flags & (MSG_PEEK as i32) != 0, nonblocking)
            .map_err(socket_error_to_errno)?;
        let length = packet.data.len();
        let amount = length.min(capacity);
        let mut offset = 0;
        for iov in iovecs {
            let n = iov.iov_len.min(amount - offset);
            if n != 0 {
                copy_to_user(
                    task,
                    iov.iov_base as usize,
                    &packet.data[offset..offset + n],
                )
                .map_err(|_| errno::EFAULT)?;
            }
            offset += n;
            if offset == amount {
                break;
            }
        }
        header.msg_flags = if length > capacity {
            MSG_TRUNC as u32
        } else {
            0
        };
        let control_capacity = if header.msg_control == 0 {
            0
        } else {
            header.msg_controllen as usize
        };
        let mut control = Vec::new();
        let maximum = control_capacity.saturating_sub(size_of::<LinuxCmsghdr>()) / size_of::<i32>();
        let count = packet.handles.len();
        let mut rights = Vec::new();
        for (index, (object, metadata)) in packet.handles.into_iter().enumerate() {
            if index >= maximum {
                header.msg_flags |= 8;
                continue;
            }
            let access = metadata.access_mode;
            let handle = task
                .handle_table
                .insert_with_metadata(object, metadata)
                .map_err(|_| errno::EMFILE)?;
            let new_fd = match abi.allocate_fd(handle) {
                Ok(fd) => fd,
                Err(_) => {
                    drop(task.handle_table.remove(handle));
                    return Err(errno::EMFILE);
                }
            };
            installed.push(new_fd as i32);
            let _ = abi.set_fd_flags(
                new_fd,
                if flags & 0x4000_0000 != 0 {
                    FD_CLOEXEC
                } else {
                    0
                },
            );
            let _ = abi.set_file_status_flags(
                new_fd,
                match access {
                    crate::object::handle::AccessMode::ReadOnly => 0,
                    crate::object::handle::AccessMode::WriteOnly => 1,
                    crate::object::handle::AccessMode::ReadWrite => 2,
                },
            );
            rights.extend_from_slice(&(new_fd as i32).to_ne_bytes());
        }
        if !rights.is_empty()
            && !append_control(&mut control, control_capacity, SCM_RIGHTS, &rights)
        {
            return Err(errno::EIO);
        }
        if installed.len() < count {
            header.msg_flags |= 8;
        }
        if socket.passcred() && !packet.end_of_input {
            let credential = LinuxUcred {
                pid: packet.sender.pid_in(&task.get_namespace()) as i32,
                uid: packet.sender.identity.principal.0,
                gid: packet.sender.identity.group.0,
            };
            // SAFETY: LinuxUcred is an initialized fixed-width POD record.
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    (&credential as *const LinuxUcred).cast::<u8>(),
                    size_of::<LinuxUcred>(),
                )
            };
            if !append_control(&mut control, control_capacity, SCM_CREDENTIALS, bytes) {
                header.msg_flags |= 8;
            }
        }
        if !control.is_empty() {
            copy_to_user(task, header.msg_control as usize, &control).map_err(|_| errno::EFAULT)?;
        }
        header.msg_controllen = control.len() as u64;
        if packet.end_of_input {
            header.msg_namelen = 0;
        } else {
            source_address(task, &mut header, packet.source)?;
        }
        // SAFETY: all fields, including padding in LinuxMsghdr, were read from the caller.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                (&header as *const LinuxMsghdr).cast::<u8>(),
                size_of::<LinuxMsghdr>(),
            )
        };
        copy_to_user(task, pointer, bytes).map_err(|_| errno::EFAULT)?;
        Ok(if flags & (MSG_TRUNC as i32) != 0 {
            length
        } else {
            amount
        })
    })();
    match result {
        Ok(n) => n,
        Err(error) => {
            for &fd in &installed {
                if let Some(handle) = abi.remove_fd(fd as usize) {
                    if let Some(object) = task.handle_table.remove(handle) {
                        abi.close_descriptor_object(task, &object);
                    }
                }
            }
            errno::to_result(error)
        }
    }
}
