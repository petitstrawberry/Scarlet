//! Darwin error code definitions and conversion

// Darwin errno values (match macOS /usr/include/sys/errno.h)
pub const EPERM: usize = 1;
pub const ENOENT: usize = 2;
pub const ESRCH: usize = 3;
pub const EINTR: usize = 4;
pub const EIO: usize = 5;
pub const ENXIO: usize = 6;
pub const ENOEXEC: usize = 8;
pub const EBADF: usize = 9;
pub const ENOMEM: usize = 12;
pub const EACCES: usize = 13;
pub const EFAULT: usize = 14;
pub const ENOTBLK: usize = 15;
pub const EBUSY: usize = 16;
pub const EEXIST: usize = 17;
pub const EXDEV: usize = 18;
pub const ENODEV: usize = 19;
pub const ENOTDIR: usize = 20;
pub const EISDIR: usize = 21;
pub const EINVAL: usize = 22;
pub const ENFILE: usize = 23;
pub const EMFILE: usize = 24;
pub const ENOTTY: usize = 25;
pub const ETXTBSY: usize = 26;
pub const EFBIG: usize = 27;
pub const ENOSPC: usize = 28;
pub const ESPIPE: usize = 29;
pub const EROFS: usize = 30;
pub const EMLINK: usize = 31;
pub const EPIPE: usize = 32;
pub const EDOM: usize = 33;
pub const ERANGE: usize = 34;
pub const EDEADLK: usize = 35;
pub const ENAMETOOLONG: usize = 63;
pub const ENOTEMPTY: usize = 66;
pub const ELOOP: usize = 90;
pub const EWOULDBLOCK: usize = 11;
pub const EINPROGRESS: usize = 36;
pub const EALREADY: usize = 37;
pub const ENOTSOCK: usize = 38;
pub const EDESTADDRREQ: usize = 39;
pub const EMSGSIZE: usize = 40;
pub const EPROTOTYPE: usize = 41;
pub const ENOPROTOOPT: usize = 42;
pub const EPROTONOSUPPORT: usize = 43;
pub const ESOCKTNOSUPPORT: usize = 44;
pub const ENOTSUP: usize = 45;
pub const EPFNOSUPPORT: usize = 46;
pub const EAFNOSUPPORT: usize = 47;
pub const EADDRINUSE: usize = 48;
pub const EADDRNOTAVAIL: usize = 49;
pub const ENETDOWN: usize = 50;
pub const ENETUNREACH: usize = 51;
pub const ENETRESET: usize = 52;
pub const ECONNABORTED: usize = 53;
pub const ECONNRESET: usize = 54;
pub const ENOBUFS: usize = 55;
pub const EISCONN: usize = 56;
pub const ENOTCONN: usize = 57;
pub const ESHUTDOWN: usize = 58;
pub const ETIMEDOUT: usize = 60;
pub const ECONNREFUSED: usize = 61;
pub const ENOSYS: usize = 78;

// Mach kernel return codes
pub const KERN_SUCCESS: i64 = 0;
pub const KERN_INVALID_ADDRESS: i64 = 1;
pub const KERN_PROTECTION_FAILURE: i64 = 2;
pub const KERN_NO_SPACE: i64 = 3;
pub const KERN_INVALID_ARGUMENT: i64 = 4;
pub const KERN_FAILURE: i64 = 5;
pub const KERN_RESOURCE_SHORTAGE: i64 = 6;
pub const KERN_NOT_RECEIVER: i64 = 8;
pub const KERN_NO_ACCESS: i64 = 8;
pub const KERN_MEMORY_FAILURE: i64 = 10;
pub const KERN_MEMORY_ERROR: i64 = 11;
pub const KERN_NOT_IN_SET: i64 = 12;
pub const KERN_NAME_EXISTS: i64 = 13;
pub const KERN_ABORTED: i64 = 14;
pub const KERN_INVALID_NAME: i64 = 15;
pub const KERN_INVALID_RIGHT: i64 = 17;
pub const KERN_INVALID_VALUE: i64 = 18;
pub const KERN_UREFS_OVERFLOW: i64 = 19;
pub const KERN_INVALID_CAPABILITY: i64 = 20;
pub const KERN_RIGHT_EXISTS: i64 = 21;
pub const KERN_INVALID_HOST: i64 = 22;
pub const KERN_MEMORY_PRESENT: i64 = 23;
pub const KERN_INVALID_PROCESS: i64 = 31;
pub const KERN_INVALID_TASK: i64 = 31;
pub const KERN_INVALID_THREAD: i64 = 32;

pub type DarwinResult = Result<usize, usize>;
pub type MachResult = Result<i64, i64>;

pub fn from_kernel_error(err: &str) -> usize {
    match err {
        "No such file or directory" => ENOENT,
        "Permission denied" => EPERM,
        "Invalid argument" => EINVAL,
        "Out of memory" => ENOMEM,
        "Bad file descriptor" => EBADF,
        "I/O error" => EIO,
        "File exists" => EEXIST,
        "Is a directory" => EISDIR,
        "Not a directory" => ENOTDIR,
        "Too many open files" => EMFILE,
        "No space left on device" => ENOSPC,
        "Read-only filesystem" => EROFS,
        "Broken pipe" => EPIPE,
        "Connection refused" => ECONNREFUSED,
        "Connection reset" => ECONNRESET,
        "Timed out" => ETIMEDOUT,
        "Address already in use" => EADDRINUSE,
        "Network is unreachable" => ENETUNREACH,
        "Not a socket" => ENOTSOCK,
        "Operation not supported" => ENOTSUP,
        "Function not implemented" => ENOSYS,
        _ => EIO,
    }
}

pub fn to_kern_return(err: &str) -> i64 {
    match err {
        "No such file or directory" => KERN_INVALID_ADDRESS,
        "Permission denied" => KERN_PROTECTION_FAILURE,
        "Invalid argument" => KERN_INVALID_ARGUMENT,
        "Out of memory" => KERN_RESOURCE_SHORTAGE,
        _ => KERN_FAILURE,
    }
}
