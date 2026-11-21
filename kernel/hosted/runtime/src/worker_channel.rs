//! Descriptor transfer for the local worker control channel.

use std::{
    io,
    mem::{size_of, zeroed},
    os::{
        fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    ptr,
};

pub(super) fn send_backing(channel: &UnixStream, backing: BorrowedFd<'_>) -> io::Result<()> {
    let mut tag = [b'F'];
    let mut payload = libc::iovec {
        iov_base: tag.as_mut_ptr().cast(),
        iov_len: tag.len(),
    };
    // The control buffer must have the alignment of a cmsghdr.
    let len = unsafe { libc::CMSG_SPACE(size_of::<libc::c_int>() as u32) };
    let mut control = [0usize; 8];
    let mut msg: libc::msghdr = unsafe { zeroed() };
    msg.msg_iov = &mut payload;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = len;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&msg);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(size_of::<libc::c_int>() as u32);
        ptr::write_unaligned(libc::CMSG_DATA(header).cast(), backing.as_raw_fd());
    }
    retry(|| unsafe { libc::sendmsg(channel.as_raw_fd(), &msg, 0) })?;
    Ok(())
}

pub(super) fn receive_backing(channel: &UnixStream) -> io::Result<OwnedFd> {
    let mut tag = [0u8];
    let mut payload = libc::iovec {
        iov_base: tag.as_mut_ptr().cast(),
        iov_len: tag.len(),
    };
    let mut control = [0usize; 8];
    let mut msg: libc::msghdr = unsafe { zeroed() };
    msg.msg_iov = &mut payload;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = unsafe { libc::CMSG_SPACE(size_of::<libc::c_int>() as u32) };
    if retry(|| unsafe { libc::recvmsg(channel.as_raw_fd(), &mut msg, 0) })? != 1 {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    // Only the broker can send messages to this endpoint. Consume the fd even
    // if the associated tag is invalid, so it cannot leak on an error path.
    let fd = unsafe {
        let header = libc::CMSG_FIRSTHDR(&msg);
        if header.is_null()
            || (*header).cmsg_level != libc::SOL_SOCKET
            || (*header).cmsg_type != libc::SCM_RIGHTS
            || (*header).cmsg_len != libc::CMSG_LEN(size_of::<libc::c_int>() as u32)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Missing worker backing",
            ));
        }
        OwnedFd::from_raw_fd(ptr::read_unaligned(libc::CMSG_DATA(header).cast()))
    };
    if tag != [b'F'] || msg.msg_flags & libc::MSG_CTRUNC != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Bad worker backing message",
        ));
    }
    Ok(fd)
}

fn retry(mut operation: impl FnMut() -> isize) -> io::Result<usize> {
    loop {
        let count = operation();
        if count >= 0 {
            return Ok(count as usize);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}
