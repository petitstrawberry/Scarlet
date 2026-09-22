//! Byte strings and the ASCII C locale.
//!
//! The matching Rust sysroot's compiler-builtins supplies memcpy, memmove,
//! memset, memcmp and strlen. Do not redefine those symbols here: Rust's own
//! memory operations can lower to them. All other operations below inspect
//! bytes, never UTF-8 characters, and do not allocate except strdup/strndup.

use std::ffi::{c_char, c_int, c_void};
use std::ptr;

/// # Safety
/// `s` must be readable through its terminating NUL.
unsafe fn length(s: *const c_char) -> usize {
    let mut n = 0;
    // SAFETY: the caller supplies a NUL-terminated string.
    while unsafe { *s.add(n) } != 0 {
        n += 1;
    }
    n
}

/// # Safety
/// `s` must be readable through its first NUL or for `maximum` bytes.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strnlen(s: *const c_char, maximum: usize) -> usize {
    let mut n = 0;
    // SAFETY: the bound is checked before accessing the next byte.
    while n < maximum && unsafe { *s.add(n) } != 0 {
        n += 1;
    }
    n
}

/// # Safety
/// Both inputs must be readable NUL-terminated strings.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strcmp(left: *const c_char, right: *const c_char) -> c_int {
    // SAFETY: comparison stops at the first NUL in either input.
    unsafe { strncmp(left, right, usize::MAX) }
}

/// # Safety
/// Each input must be readable through its first NUL or for `maximum` bytes.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strncmp(
    left: *const c_char,
    right: *const c_char,
    maximum: usize,
) -> c_int {
    for i in 0..maximum {
        // SAFETY: the previous iterations saw only nonzero equal bytes.
        let (a, b) = unsafe { (*left.add(i) as u8, *right.add(i) as u8) };
        if a != b || a == 0 {
            return c_int::from(a) - c_int::from(b);
        }
    }
    0
}

/// # Safety
/// `source` is NUL-terminated; `destination` has room for it including NUL.
/// The input and output regions must not overlap.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strcpy(destination: *mut c_char, source: *const c_char) -> *mut c_char {
    let mut i = 0;
    loop {
        // SAFETY: both strings satisfy the caller's size and overlap contract.
        let byte = unsafe { *source.add(i) };
        unsafe { *destination.add(i) = byte };
        if byte == 0 {
            return destination;
        }
        i += 1;
    }
}

/// # Safety
/// The nonoverlapping destination holds `count` bytes; the source is readable
/// for `count` bytes or through an earlier NUL. A truncated result has no NUL.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strncpy(
    destination: *mut c_char,
    source: *const c_char,
    count: usize,
) -> *mut c_char {
    let mut i = 0;
    while i < count {
        // SAFETY: i is bounded and preceding bytes were not NUL.
        let byte = unsafe { *source.add(i) };
        if byte == 0 {
            // SAFETY: the caller supplies the entire destination region.
            unsafe { ptr::write_bytes(destination.add(i), 0, count - i) };
            break;
        }
        unsafe { *destination.add(i) = byte };
        i += 1;
    }
    destination
}

/// # Safety
/// Inputs are nonoverlapping NUL-terminated strings; the destination has room
/// for their combined contents and a final NUL.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strcat(destination: *mut c_char, source: *const c_char) -> *mut c_char {
    // SAFETY: both strings and the final destination satisfy the C contract.
    unsafe { strcpy(destination.add(length(destination)), source) };
    destination
}

/// # Safety
/// The destination is NUL-terminated and holds its current contents plus up to
/// `maximum` source bytes and a final NUL. The source is readable through its
/// NUL or for `maximum` bytes. The regions must not overlap.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strncat(
    destination: *mut c_char,
    source: *const c_char,
    maximum: usize,
) -> *mut c_char {
    // SAFETY: the destination is a terminated string with sufficient capacity.
    let end = unsafe { destination.add(length(destination)) };
    let mut i = 0;
    while i < maximum {
        // SAFETY: bounded by maximum, and earlier source bytes were not NUL.
        let byte = unsafe { *source.add(i) };
        if byte == 0 {
            break;
        }
        unsafe { *end.add(i) = byte };
        i += 1;
    }
    unsafe { *end.add(i) = 0 };
    destination
}

/// # Safety
/// The input must be readable through its terminating NUL.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strchr(s: *const c_char, value: c_int) -> *mut c_char {
    let value = value as u8;
    let mut p = s;
    loop {
        // SAFETY: every prior byte was nonzero.
        let byte = unsafe { *p as u8 };
        if byte == value {
            return p.cast_mut();
        }
        if byte == 0 {
            return ptr::null_mut();
        }
        p = unsafe { p.add(1) };
    }
}

/// # Safety
/// The input must be readable through its terminating NUL.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strrchr(s: *const c_char, value: c_int) -> *mut c_char {
    let value = value as u8;
    let mut p = s;
    let mut found = ptr::null_mut();
    loop {
        // SAFETY: every prior byte was nonzero.
        let byte = unsafe { *p as u8 };
        if byte == value {
            found = p.cast_mut();
        }
        if byte == 0 {
            return found;
        }
        p = unsafe { p.add(1) };
    }
}

/// # Safety
/// Both inputs must be readable NUL-terminated strings.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strstr(haystack: *const c_char, needle: *const c_char) -> *mut c_char {
    // SAFETY: even an empty needle contains a readable NUL byte.
    if unsafe { *needle } == 0 {
        return haystack.cast_mut();
    }
    let mut start = haystack;
    // SAFETY: the loop advances only after observing a nonzero haystack byte.
    while unsafe { *start } != 0 {
        let mut i = 0;
        loop {
            // SAFETY: all previously examined needle bytes were nonzero.
            let wanted = unsafe { *needle.add(i) };
            if wanted == 0 {
                return start.cast_mut();
            }
            // SAFETY: all previous haystack bytes matched a nonzero byte.
            let actual = unsafe { *start.add(i) };
            if actual == 0 {
                return ptr::null_mut();
            }
            if actual != wanted {
                break;
            }
            i += 1;
        }
        start = unsafe { start.add(1) };
    }
    ptr::null_mut()
}

/// # Safety
/// The input region must be readable for `count` bytes.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn memchr(s: *const c_void, value: c_int, count: usize) -> *mut c_void {
    let bytes = s.cast::<u8>();
    for i in 0..count {
        // SAFETY: each offset is inside the caller-provided readable region.
        if unsafe { *bytes.add(i) } == value as u8 {
            return unsafe { bytes.add(i) }.cast_mut().cast();
        }
    }
    ptr::null_mut()
}

/// # Safety
/// `source` must be readable through its terminating NUL. Free the result with
/// the matching Scarlet libc's free.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strdup(source: *const c_char) -> *mut c_char {
    // SAFETY: the caller guarantees termination.
    unsafe { duplicate(source, length(source)) }
}

/// # Safety
/// `source` must be readable through its NUL or for `maximum` bytes. Free the
/// result with the matching Scarlet libc's free.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strndup(source: *const c_char, maximum: usize) -> *mut c_char {
    // SAFETY: bounded scanning and copying use the caller-provided region.
    unsafe { duplicate(source, strnlen(source, maximum)) }
}

unsafe fn duplicate(source: *const c_char, count: usize) -> *mut c_char {
    let Some(size) = count.checked_add(1) else {
        crate::fail(12); // ENOMEM
        return ptr::null_mut();
    };
    let result = crate::allocation::malloc(size).cast::<c_char>();
    if !result.is_null() {
        // SAFETY: a new allocation cannot overlap the source and has size bytes.
        unsafe {
            ptr::copy_nonoverlapping(source, result, count);
            *result.add(count) = 0;
        }
    }
    result
}

/// Return an immutable, statically allocated C-locale description. The caller
/// must not modify or free it. Unknown values use a stable fallback; errno is
/// left unchanged and no allocation or shared scratch buffer is involved.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn strerror(error: c_int) -> *mut c_char {
    use scarlet_abi::fs::*;
    use scarlet_abi::{
        ERRNO_EAGAIN, ERRNO_EBADF, ERRNO_EINTR, ERRNO_EINVAL, ERRNO_EIO, ERRNO_EOPNOTSUPP,
    };
    let message = match error {
        0 => c"Success",
        ERRNO_ENOENT => c"No such file or directory",
        ERRNO_EINTR => c"Interrupted system call",
        ERRNO_EIO => c"Input/output error",
        ERRNO_EBADF => c"Bad file descriptor",
        ERRNO_EAGAIN => c"Resource temporarily unavailable",
        12 => c"Cannot allocate memory",
        ERRNO_EACCES => c"Permission denied",
        ERRNO_EFAULT => c"Bad address",
        ERRNO_EBUSY => c"Device or resource busy",
        ERRNO_EEXIST => c"File exists",
        ERRNO_EXDEV => c"Invalid cross-device link",
        ERRNO_ENOTDIR => c"Not a directory",
        ERRNO_EISDIR => c"Is a directory",
        ERRNO_EINVAL => c"Invalid argument",
        24 => c"Too many open files",
        27 => c"File too large",
        ERRNO_ENOSPC => c"No space left on device",
        29 => c"Illegal seek",
        ERRNO_EROFS => c"Read-only file system",
        32 => c"Broken pipe",
        ERRNO_ERANGE => c"Numerical result out of range",
        ERRNO_ENAMETOOLONG => c"File name too long",
        38 => c"Function not implemented",
        ERRNO_ENOTEMPTY => c"Directory not empty",
        ERRNO_ELOOP => c"Too many levels of symbolic links",
        ERRNO_EOVERFLOW => c"Value too large for defined data type",
        ERRNO_EOPNOTSUPP => c"Operation not supported",
        _ => c"Unknown error",
    };
    message.as_ptr().cast_mut()
}

// C requires an unsigned-char value or EOF. Inputs outside that domain return
// false (or remain unchanged for case conversion), without indexing a table.
macro_rules! classify {
    ($name:ident, $value:ident, $condition:expr) => {
        #[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
        pub extern "C" fn $name($value: c_int) -> c_int {
            c_int::from($condition)
        }
    };
}
classify!(isalnum, c, matches!(c, 48..=57 | 65..=90 | 97..=122));
classify!(isalpha, c, matches!(c, 65..=90 | 97..=122));
classify!(isblank, c, matches!(c, 9 | 32));
classify!(iscntrl, c, matches!(c, 0..=31 | 127));
classify!(isdigit, c, matches!(c, 48..=57));
classify!(isgraph, c, matches!(c, 33..=126));
classify!(islower, c, matches!(c, 97..=122));
classify!(isprint, c, matches!(c, 32..=126));
classify!(
    ispunct,
    c,
    matches!(c, 33..=47 | 58..=64 | 91..=96 | 123..=126)
);
classify!(isspace, c, matches!(c, 9..=13 | 32));
classify!(isupper, c, matches!(c, 65..=90));
classify!(isxdigit, c, matches!(c, 48..=57 | 65..=70 | 97..=102));

#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn tolower(value: c_int) -> c_int {
    if matches!(value, 65..=90) {
        value + 32
    } else {
        value
    }
}

#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn toupper(value: c_int) -> c_int {
    if matches!(value, 97..=122) {
        value - 32
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_operations_do_not_need_a_terminator() {
        let source = [b'a', b'b', b'c'];
        let mut output = [0x55u8; 5];
        unsafe {
            assert_eq!(strnlen(source.as_ptr().cast(), 3), 3);
            assert_eq!(strnlen(source.as_ptr().cast(), 0), 0);
            assert_eq!(strncmp(source.as_ptr().cast(), c"abd".as_ptr(), 2), 0);
            assert!(strncmp(source.as_ptr().cast(), c"abd".as_ptr(), 3) < 0);
            assert_eq!(
                strncpy(output.as_mut_ptr().cast(), source.as_ptr().cast(), 3),
                output.as_mut_ptr().cast()
            );
            assert_eq!(output, [b'a', b'b', b'c', 0x55, 0x55]);
            strncpy(output.as_mut_ptr().cast(), c"a".as_ptr(), 4);
            assert_eq!(output, [b'a', 0, 0, 0, 0x55]);
        }
    }

    #[test]
    fn comparisons_and_searches_use_unsigned_bytes_and_include_nul() {
        let high = [0xffu8, 0];
        let low = [0x7fu8, 0];
        let text = c"abaca".as_ptr();
        unsafe {
            assert!(strcmp(high.as_ptr().cast(), low.as_ptr().cast()) > 0);
            assert_eq!(strchr(text, b'a'.into()), text.cast_mut());
            assert_eq!(strrchr(text, b'a'.into()), text.add(4).cast_mut());
            assert_eq!(strchr(text, 256), text.add(5).cast_mut());
            assert_eq!(strrchr(text, 0), text.add(5).cast_mut());
            assert!(strchr(text, b'x'.into()).is_null());
            assert_eq!(
                memchr(high.as_ptr().cast(), -1, 2),
                high.as_ptr().cast_mut().cast()
            );
            assert!(memchr(high.as_ptr().cast(), 0xff, 0).is_null());
        }
    }

    #[test]
    fn concatenation_and_substrings_preserve_boundaries() {
        let mut output = [0x55u8; 16];
        let p = output.as_mut_ptr().cast();
        unsafe {
            assert_eq!(strcpy(p, c"ab".as_ptr()), p);
            assert_eq!(strcat(p, c"cd".as_ptr()), p);
            assert_eq!(strncat(p, c"efgh".as_ptr(), 2), p);
            assert_eq!(&output[..8], b"abcdef\0U");
            strncat(p, c"ignored".as_ptr(), 0);
            assert_eq!(&output[..8], b"abcdef\0U");
            let text = c"abababc".as_ptr();
            assert_eq!(strstr(text, c"ababc".as_ptr()), text.add(2).cast_mut());
            assert_eq!(strstr(text, c"".as_ptr()), text.cast_mut());
            assert!(strstr(text, c"abababcd".as_ptr()).is_null());
            assert!(strstr(c"".as_ptr(), c"x".as_ptr()).is_null());
        }
    }

    #[test]
    fn duplicate_is_owned_bounded_terminated_and_preserves_errno() {
        unsafe {
            *crate::__errno_location() = 73;
            let original = c"hello".as_ptr();
            let copy = strdup(original);
            assert!(!copy.is_null());
            assert_ne!(copy.cast_const(), original);
            assert_eq!(strcmp(copy, original), 0);
            *copy = b'j' as c_char;
            assert_eq!(*original, b'h' as c_char);
            crate::allocation::free(copy.cast());
            let unterminated = *b"abcd";
            let copy = strndup(unterminated.as_ptr().cast(), 3);
            assert_eq!(strcmp(copy, c"abc".as_ptr()), 0);
            crate::allocation::free(copy.cast());
            let empty = strndup(original, 0);
            assert!(!empty.is_null());
            assert_eq!(*empty, 0);
            crate::allocation::free(empty.cast());
            assert_eq!(*crate::__errno_location(), 73);
        }
    }

    #[test]
    fn error_descriptions_have_stable_storage_and_preserve_errno() {
        unsafe {
            *crate::__errno_location() = 73;
            let description = strerror(scarlet_abi::ERRNO_EINVAL);
            assert_eq!(strcmp(description, c"Invalid argument".as_ptr()), 0);
            assert_eq!(strcmp(strerror(-123), c"Unknown error".as_ptr()), 0);
            assert_eq!(
                strcmp(
                    strerror(scarlet_abi::ERRNO_EINTR),
                    c"Interrupted system call".as_ptr()
                ),
                0
            );
            assert_eq!(strcmp(description, c"Invalid argument".as_ptr()), 0);
            assert_eq!(*crate::__errno_location(), 73);
        }
    }

    #[test]
    fn c_locale_classification_covers_every_byte_and_eof() {
        for value in -1..=255 {
            let byte = u8::try_from(value).ok();
            assert_eq!(
                isalnum(value) != 0,
                byte.is_some_and(|b| b.is_ascii_alphanumeric())
            );
            assert_eq!(
                isalpha(value) != 0,
                byte.is_some_and(|b| b.is_ascii_alphabetic())
            );
            assert_eq!(isblank(value) != 0, value == 9 || value == 32);
            assert_eq!(
                iscntrl(value) != 0,
                byte.is_some_and(|b| b.is_ascii_control())
            );
            assert_eq!(
                isdigit(value) != 0,
                byte.is_some_and(|b| b.is_ascii_digit())
            );
            assert_eq!(
                isgraph(value) != 0,
                byte.is_some_and(|b| b.is_ascii_graphic())
            );
            assert_eq!(
                islower(value) != 0,
                byte.is_some_and(|b| b.is_ascii_lowercase())
            );
            assert_eq!(
                isprint(value) != 0,
                byte.is_some_and(|b| b.is_ascii_graphic() || b == b' ')
            );
            assert_eq!(
                ispunct(value) != 0,
                byte.is_some_and(|b| b.is_ascii_punctuation())
            );
            // Rust's is_ascii_whitespace deliberately omits the C vertical tab.
            assert_eq!(
                isspace(value) != 0,
                byte.is_some_and(|b| b.is_ascii_whitespace() || b == 11)
            );
            assert_eq!(
                isupper(value) != 0,
                byte.is_some_and(|b| b.is_ascii_uppercase())
            );
            assert_eq!(
                isxdigit(value) != 0,
                byte.is_some_and(|b| b.is_ascii_hexdigit())
            );
            assert_eq!(
                tolower(value),
                byte.map_or(value, |b| b.to_ascii_lowercase().into())
            );
            assert_eq!(
                toupper(value),
                byte.map_or(value, |b| b.to_ascii_uppercase().into())
            );
        }
    }
}
