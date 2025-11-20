//! Linux errno constants for RISC-V 64-bit ABI
//!
//! This module defines the standard Linux error numbers used by system calls.
//! These values match the Linux kernel's errno definitions for RISC-V architecture.

/// Success (no error)
pub const SUCCESS: usize = 0;

/// Operation not permitted
pub const EPERM: usize = 1;

/// No such file or directory  
pub const ENOENT: usize = 2;

/// No such process
pub const ESRCH: usize = 3;

/// Interrupted system call
pub const EINTR: usize = 4;

/// I/O error
pub const EIO: usize = 5;

/// No such device or address
pub const ENXIO: usize = 6;

/// Argument list too long
pub const E2BIG: usize = 7;

/// Exec format error
pub const ENOEXEC: usize = 8;

/// Bad file number
pub const EBADF: usize = 9;

/// No child processes
pub const ECHILD: usize = 10;

/// Try again
pub const EAGAIN: usize = 11;

/// Out of memory
pub const ENOMEM: usize = 12;

/// Permission denied
pub const EACCES: usize = 13;

/// Bad address
pub const EFAULT: usize = 14;

/// Block device required
pub const ENOTBLK: usize = 15;

/// Device or resource busy
pub const EBUSY: usize = 16;

/// File exists
pub const EEXIST: usize = 17;

/// Cross-device link
pub const EXDEV: usize = 18;

/// No such device
pub const ENODEV: usize = 19;

/// Not a directory
pub const ENOTDIR: usize = 20;

/// Is a directory
pub const EISDIR: usize = 21;

/// Invalid argument
pub const EINVAL: usize = 22;

/// File table overflow
pub const ENFILE: usize = 23;

/// Too many open files
pub const EMFILE: usize = 24;

/// Not a typewriter
pub const ENOTTY: usize = 25;

/// Text file busy
pub const ETXTBSY: usize = 26;

/// File too large
pub const EFBIG: usize = 27;

/// No space left on device
pub const ENOSPC: usize = 28;

/// Illegal seek
pub const ESPIPE: usize = 29;

/// Read-only file system
pub const EROFS: usize = 30;

/// Too many links
pub const EMLINK: usize = 31;

/// Broken pipe
pub const EPIPE: usize = 32;

/// Math argument out of domain of func
pub const EDOM: usize = 33;

/// Math result not representable
pub const ERANGE: usize = 34;

/// Resource deadlock would occur
pub const EDEADLK: usize = 35;

/// File name too long
pub const ENAMETOOLONG: usize = 36;

/// No record locks available
pub const ENOLCK: usize = 37;

/// Function not implemented
pub const ENOSYS: usize = 38;

/// Directory not empty
pub const ENOTEMPTY: usize = 39;

/// Too many symbolic links encountered
pub const ELOOP: usize = 40;

/// Operation would block (same as EAGAIN)
pub const EWOULDBLOCK: usize = EAGAIN;

/// No message of desired type
pub const ENOMSG: usize = 42;

/// Identifier removed
pub const EIDRM: usize = 43;

/// Channel number out of range
pub const ECHRNG: usize = 44;

/// Level 2 not synchronized
pub const EL2NSYNC: usize = 45;

/// Level 3 halted
pub const EL3HLT: usize = 46;

/// Level 3 reset
pub const EL3RST: usize = 47;

/// Link number out of range
pub const ELNRNG: usize = 48;

/// Protocol driver not attached
pub const EUNATCH: usize = 49;

/// No CSI structure available
pub const ENOCSI: usize = 50;

/// Level 2 halted
pub const EL2HLT: usize = 51;

/// Invalid exchange
pub const EBADE: usize = 52;

/// Invalid request descriptor
pub const EBADR: usize = 53;

/// Exchange full
pub const EXFULL: usize = 54;

/// No anode
pub const ENOANO: usize = 55;

/// Invalid request code
pub const EBADRQC: usize = 56;

/// Invalid slot
pub const EBADSLT: usize = 57;

/// Resource deadlock would occur (same as EDEADLK)
pub const EDEADLOCK: usize = EDEADLK;

/// Bad font file format
pub const EBFONT: usize = 59;

/// Device not a stream
pub const ENOSTR: usize = 60;

/// No data available
pub const ENODATA: usize = 61;

/// Timer expired
pub const ETIME: usize = 62;

/// Out of streams resources
pub const ENOSR: usize = 63;

/// Machine is not on the network
pub const ENONET: usize = 64;

/// Package not installed
pub const ENOPKG: usize = 65;

/// Object is remote
pub const EREMOTE: usize = 66;

/// Link has been severed
pub const ENOLINK: usize = 67;

/// Advertise error
pub const EADV: usize = 68;

/// Srmount error
pub const ESRMNT: usize = 69;

/// Communication error on send
pub const ECOMM: usize = 70;

/// Protocol error
pub const EPROTO: usize = 71;

/// Multihop attempted
pub const EMULTIHOP: usize = 72;

/// RFS specific error
pub const EDOTDOT: usize = 73;

/// Not a data message
pub const EBADMSG: usize = 74;

/// Value too large for defined data type
pub const EOVERFLOW: usize = 75;

/// Name not unique on network
pub const ENOTUNIQ: usize = 76;

/// File descriptor in bad state
pub const EBADFD: usize = 77;

/// Remote address changed
pub const EREMCHG: usize = 78;

/// Can not access a needed shared library
pub const ELIBACC: usize = 79;

/// Accessing a corrupted shared library
pub const ELIBBAD: usize = 80;

/// .lib section in a.out corrupted
pub const ELIBSCN: usize = 81;

/// Attempting to link in too many shared libraries
pub const ELIBMAX: usize = 82;

/// Cannot exec a shared library directly
pub const ELIBEXEC: usize = 83;

/// Illegal byte sequence
pub const EILSEQ: usize = 84;

/// Interrupted system call should be restarted
pub const ERESTART: usize = 85;

/// Streams pipe error
pub const ESTRPIPE: usize = 86;

/// Too many users
pub const EUSERS: usize = 87;

/// Socket operation on non-socket
pub const ENOTSOCK: usize = 88;

/// Destination address required
pub const EDESTADDRREQ: usize = 89;

/// Message too long
pub const EMSGSIZE: usize = 90;

/// Protocol wrong type for socket
pub const EPROTOTYPE: usize = 91;

/// Protocol not available
pub const ENOPROTOOPT: usize = 92;

/// Protocol not supported
pub const EPROTONOSUPPORT: usize = 93;

/// Socket type not supported
pub const ESOCKTNOSUPPORT: usize = 94;

/// Operation not supported on transport endpoint
pub const EOPNOTSUPP: usize = 95;

/// Protocol family not supported
pub const EPFNOSUPPORT: usize = 96;

/// Address family not supported by protocol
pub const EAFNOSUPPORT: usize = 97;

/// Address already in use
pub const EADDRINUSE: usize = 98;

/// Cannot assign requested address
pub const EADDRNOTAVAIL: usize = 99;

/// Network is down
pub const ENETDOWN: usize = 100;

/// Network is unreachable
pub const ENETUNREACH: usize = 101;

/// Network dropped connection because of reset
pub const ENETRESET: usize = 102;

/// Software caused connection abort
pub const ECONNABORTED: usize = 103;

/// Connection reset by peer
pub const ECONNRESET: usize = 104;

/// No buffer space available
pub const ENOBUFS: usize = 105;

/// Transport endpoint is already connected
pub const EISCONN: usize = 106;

/// Transport endpoint is not connected
pub const ENOTCONN: usize = 107;

/// Cannot send after transport endpoint shutdown
pub const ESHUTDOWN: usize = 108;

/// Too many references: cannot splice
pub const ETOOMANYREFS: usize = 109;

/// Connection timed out
pub const ETIMEDOUT: usize = 110;

/// Connection refused
pub const ECONNREFUSED: usize = 111;

/// Host is down
pub const EHOSTDOWN: usize = 112;

/// No route to host
pub const EHOSTUNREACH: usize = 113;

/// Operation already in progress
pub const EALREADY: usize = 114;

/// Operation now in progress
pub const EINPROGRESS: usize = 115;

/// Stale file handle
pub const ESTALE: usize = 116;

/// Structure needs cleaning
pub const EUCLEAN: usize = 117;

/// Not a XENIX named type file
pub const ENOTNAM: usize = 118;

/// No XENIX semaphores available
pub const ENAVAIL: usize = 119;

/// Is a named type file
pub const EISNAM: usize = 120;

/// Remote I/O error
pub const EREMOTEIO: usize = 121;

/// Quota exceeded
pub const EDQUOT: usize = 122;

/// No medium found
pub const ENOMEDIUM: usize = 123;

/// Wrong medium type
pub const EMEDIUMTYPE: usize = 124;

/// Operation Canceled
pub const ECANCELED: usize = 125;

/// Required key not available
pub const ENOKEY: usize = 126;

/// Key has expired
pub const EKEYEXPIRED: usize = 127;

/// Key has been revoked
pub const EKEYREVOKED: usize = 128;

/// Key was rejected by service
pub const EKEYREJECTED: usize = 129;

/// Owner died
pub const EOWNERDEAD: usize = 130;

/// State not recoverable
pub const ENOTRECOVERABLE: usize = 131;

/// Operation not possible due to RF-kill
pub const ERFKILL: usize = 132;

/// Memory page has hardware error
pub const EHWPOISON: usize = 133;

/// Helper function to convert FileSystemErrorKind to Linux errno
pub fn from_fs_error(error: &crate::fs::FileSystemError) -> usize {
    use crate::fs::FileSystemErrorKind;
    
    match error.kind {
        FileSystemErrorKind::NotFound => ENOENT,
        FileSystemErrorKind::PermissionDenied => EACCES,
        FileSystemErrorKind::FileExists => EEXIST,
        FileSystemErrorKind::AlreadyExists => EEXIST,
        FileSystemErrorKind::NotADirectory => ENOTDIR,
        FileSystemErrorKind::IsADirectory => EISDIR,
        FileSystemErrorKind::NotAFile => EISDIR,
        FileSystemErrorKind::DirectoryNotEmpty => ENOTEMPTY,
        FileSystemErrorKind::InvalidPath => EINVAL,
        FileSystemErrorKind::InvalidOperation => EPERM,
        FileSystemErrorKind::CrossDevice => EXDEV,
        FileSystemErrorKind::NoSpace => ENOSPC,
        FileSystemErrorKind::ReadOnly => EROFS,
        FileSystemErrorKind::IoError => EIO,
        FileSystemErrorKind::DeviceError => EIO,
        FileSystemErrorKind::InvalidData => EINVAL,
        FileSystemErrorKind::NotSupported => ENOSYS,
        FileSystemErrorKind::BrokenFileSystem => EIO,
        FileSystemErrorKind::Busy => EBUSY,
    }
}

/// Convert any error to Linux errno, defaulting to EIO for unknown errors
pub fn from_error<E>(_error: E) -> usize {
    EIO // Generic I/O error for unknown error types
}

/// Convert errno to negative value as required by Linux system calls
/// Linux system calls return negative errno values on error
pub fn to_result(errno_val: usize) -> usize {
    if errno_val == SUCCESS {
        SUCCESS
    } else {
        // Convert positive errno to negative value
        // Using two's complement: -errno = !errno + 1
        // But in usize, we use wrapping_neg() which is equivalent to (usize::MAX - errno + 1)
        errno_val.wrapping_neg()
    }
}