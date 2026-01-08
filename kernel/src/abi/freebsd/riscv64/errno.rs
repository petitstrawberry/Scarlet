/// FreeBSD error codes
///
/// These error codes are compatible with FreeBSD's errno.h
#[allow(dead_code)]
#[derive(Copy, Clone)]
#[repr(i32)]
pub enum FreeBsdErrno {
    EPERM = 1,          // Operation not permitted
    ENOENT = 2,         // No such file or directory
    ESRCH = 3,          // No such process
    EINTR = 4,          // Interrupted system call
    EIO = 5,            // I/O error
    ENXIO = 6,          // No such device or address
    E2BIG = 7,          // Argument list too long
    ENOEXEC = 8,        // Exec format error
    EBADF = 9,          // Bad file number
    ECHILD = 10,        // No child processes
    EDEADLK = 11,       // Resource deadlock would occur
    ENOMEM = 12,        // Out of memory
    EACCES = 13,        // Permission denied
    EFAULT = 14,        // Bad address
    ENOTBLK = 15,       // Block device required
    EBUSY = 16,         // Device or resource busy
    EEXIST = 17,        // File exists
    EXDEV = 18,         // Cross-device link
    ENODEV = 19,        // No such device
    ENOTDIR = 20,       // Not a directory
    EISDIR = 21,        // Is a directory
    EINVAL = 22,        // Invalid argument
    ENFILE = 23,        // File table overflow
    EMFILE = 24,        // Too many open files
    ENOTTY = 25,        // Not a typewriter
    ETXTBSY = 26,       // Text file busy
    EFBIG = 27,         // File too large
    ENOSPC = 28,        // No space left on device
    ESPIPE = 29,        // Illegal seek
    EROFS = 30,         // Read-only file system
    EMLINK = 31,        // Too many links
    EPIPE = 32,         // Broken pipe
    EDOM = 33,          // Math argument out of domain of func
    ERANGE = 34,        // Math result not representable
    EAGAIN = 35,        // Try again / Operation would block
    EINPROGRESS = 36,   // Operation now in progress
    EALREADY = 37,      // Operation already in progress
    ENOTSOCK = 38,      // Socket operation on non-socket
    EDESTADDRREQ = 39,  // Destination address required
    EMSGSIZE = 40,      // Message too long
    EPROTOTYPE = 41,    // Protocol wrong type for socket
    ENOPROTOOPT = 42,   // Protocol not available
    EPROTONOSUPPORT = 43, // Protocol not supported
    ESOCKTNOSUPPORT = 44, // Socket type not supported
    EOPNOTSUPP = 45,    // Operation not supported on transport endpoint
    EPFNOSUPPORT = 46,  // Protocol family not supported
    EAFNOSUPPORT = 47,  // Address family not supported by protocol
    EADDRINUSE = 48,    // Address already in use
    EADDRNOTAVAIL = 49, // Cannot assign requested address
    ENETDOWN = 50,      // Network is down
    ENETUNREACH = 51,   // Network is unreachable
    ENETRESET = 52,     // Network dropped connection because of reset
    ECONNABORTED = 53,  // Software caused connection abort
    ECONNRESET = 54,    // Connection reset by peer
    ENOBUFS = 55,       // No buffer space available
    EISCONN = 56,       // Transport endpoint is already connected
    ENOTCONN = 57,      // Transport endpoint is not connected
    ESHUTDOWN = 58,     // Cannot send after transport endpoint shutdown
    ETOOMANYREFS = 59,  // Too many references: cannot splice
    ETIMEDOUT = 60,     // Connection timed out
    ECONNREFUSED = 61,  // Connection refused
    ELOOP = 62,         // Too many symbolic links encountered
    ENAMETOOLONG = 63,  // File name too long
    EHOSTDOWN = 64,     // Host is down
    EHOSTUNREACH = 65,  // No route to host
    ENOTEMPTY = 66,     // Directory not empty
    EPROCLIM = 67,      // Too many processes
    EUSERS = 68,        // Too many users
    EDQUOT = 69,        // Quota exceeded
    ESTALE = 70,        // Stale file handle
    EREMOTE = 71,       // Object is remote
    EBADRPC = 72,       // RPC struct is bad
    ERPCMISMATCH = 73,  // RPC version wrong
    EPROGUNAVAIL = 74,  // RPC prog. not avail
    EPROGMISMATCH = 75, // Program version wrong
    EPROCUNAVAIL = 76,  // Bad procedure for program
    ENOLCK = 77,        // No locks available
    ENOSYS = 78,        // Function not implemented
    EFTYPE = 79,        // Inappropriate file type or format
    EAUTH = 80,         // Authentication error
    ENEEDAUTH = 81,     // Need authenticator
    EIDRM = 82,         // Identifier removed
    ENOMSG = 83,        // No message of desired type
    EOVERFLOW = 84,     // Value too large for defined data type
    ECANCELED = 85,     // Operation Canceled
    EILSEQ = 86,        // Illegal byte sequence
    ENOATTR = 87,       // Attribute not found
    EDOOFUS = 88,       // Programming error
    EBADMSG = 89,       // Bad message
    EMULTIHOP = 90,     // Multihop attempted
    ENOLINK = 91,       // Link has been severed
    EPROTO = 92,        // Protocol error
    ENOTCAPABLE = 93,   // Capabilities insufficient
    ECAPMODE = 94,      // Not permitted in capability mode
    ENOTRECOVERABLE = 95, // State not recoverable
    EOWNERDEAD = 96,    // Previous owner died
    EINTEGRITY = 97,    // Integrity check failed
}

impl FreeBsdErrno {
    /// Convert to i32 error code (negated for syscall return)
    pub fn as_error(&self) -> usize {
        (*self as i32 as isize).wrapping_neg() as usize
    }
}
