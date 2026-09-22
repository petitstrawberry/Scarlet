//! Integer and byte-string printf formatting shared by FILE and buffer sinks.
//!
//! Floating point, positional arguments, wide characters and `%n` are not yet
//! supported; they fail with ENOTSUP instead of consuming an incorrect va_arg.

use core::ffi::{VaList, c_char, c_int, c_long, c_ulong, c_void};
use scarlet_abi::{ERRNO_EINVAL, fs::ERRNO_EOVERFLOW};

const ENOTSUP: c_int = 95;
const MAX_RESULT: usize = c_int::MAX as usize;

/// Padding is separate so a bounded buffer can discard it in constant time.
pub(crate) enum Chunk<'a> {
    Bytes(&'a [u8]),
    Repeat(u8, usize),
}

struct Output<F> {
    emit: F,
    length: usize,
}

impl<F: FnMut(Chunk<'_>) -> Result<(), c_int>> Output<F> {
    fn write(&mut self, chunk: Chunk<'_>) -> Result<(), c_int> {
        let length = match &chunk {
            Chunk::Bytes(bytes) => bytes.len(),
            Chunk::Repeat(_, length) => *length,
        };
        self.length = self
            .length
            .checked_add(length)
            .filter(|length| *length <= MAX_RESULT)
            .ok_or(ERRNO_EOVERFLOW)?;
        if length != 0 {
            (self.emit)(chunk)?;
        }
        Ok(())
    }

    fn bytes(&mut self, bytes: &[u8]) -> Result<(), c_int> {
        self.write(Chunk::Bytes(bytes))
    }

    fn repeat(&mut self, byte: u8, length: usize) -> Result<(), c_int> {
        self.write(Chunk::Repeat(byte, length))
    }
}

#[derive(Default)]
struct Flags {
    left: bool,
    plus: bool,
    space: bool,
    alternate: bool,
    zero: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Length {
    Int,
    Char,
    Short,
    Long,
    LongLong,
    IntMax,
    Size,
    PtrDiff,
    LongDouble,
}

fn decimal(format: &[u8], index: &mut usize) -> Result<usize, c_int> {
    let mut value = 0usize;
    while let Some(digit @ b'0'..=b'9') = format.get(*index) {
        value = value
            .checked_mul(10)
            .and_then(|value| value.checked_add((digit - b'0') as usize))
            .filter(|value| *value <= MAX_RESULT)
            .ok_or(ERRNO_EOVERFLOW)?;
        *index += 1;
    }
    Ok(value)
}

fn integer<F: FnMut(Chunk<'_>) -> Result<(), c_int>>(
    output: &mut Output<F>,
    magnitude: u64,
    negative: bool,
    conversion: u8,
    flags: &Flags,
    width: usize,
    precision: Option<usize>,
) -> Result<(), c_int> {
    let signed = matches!(conversion, b'd' | b'i');
    let radix = match conversion {
        b'o' => 8,
        b'x' | b'X' | b'p' => 16,
        _ => 10,
    };
    let alphabet = if conversion == b'X' {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let mut digits = [0u8; 64];
    let mut start = digits.len();
    let mut remaining = magnitude;
    if remaining != 0 || precision != Some(0) || conversion == b'p' {
        loop {
            start -= 1;
            digits[start] = alphabet[(remaining % radix) as usize];
            remaining /= radix;
            if remaining == 0 {
                break;
            }
        }
    }
    let digits = &digits[start..];
    let mut prefix = [0u8; 2];
    let mut prefix_length = 0;
    if signed {
        let sign = if negative {
            Some(b'-')
        } else if flags.plus {
            Some(b'+')
        } else if flags.space {
            Some(b' ')
        } else {
            None
        };
        if let Some(sign) = sign {
            prefix[0] = sign;
            prefix_length = 1;
        }
    } else if conversion == b'p'
        || (flags.alternate && magnitude != 0 && matches!(conversion, b'x' | b'X'))
    {
        prefix = [b'0', if conversion == b'X' { b'X' } else { b'x' }];
        prefix_length = 2;
    }
    let mut zeros = precision.unwrap_or(0).saturating_sub(digits.len());
    if flags.alternate && conversion == b'o' && zeros == 0 && digits.first() != Some(&b'0') {
        zeros = 1;
    }
    let content_length = prefix_length + zeros + digits.len();
    let padding = width.saturating_sub(content_length);
    if flags.zero && !flags.left && precision.is_none() {
        zeros += padding;
    } else if !flags.left {
        output.repeat(b' ', padding)?;
    }
    output.bytes(&prefix[..prefix_length])?;
    output.repeat(b'0', zeros)?;
    output.bytes(digits)?;
    if flags.left {
        output.repeat(b' ', padding)?;
    }
    Ok(())
}

/// Format a C argument list without allocating. A sink error is propagated.
///
/// # Safety
/// `format` must be a valid NUL-terminated string, and arguments must match its
/// conversions. String arguments must be readable up to NUL or the precision.
pub(crate) unsafe fn format(
    format: *const c_char,
    mut arguments: VaList<'_>,
    emit: impl FnMut(Chunk<'_>) -> Result<(), c_int>,
) -> Result<usize, c_int> {
    if format.is_null() {
        return Err(ERRNO_EINVAL);
    }
    // SAFETY: the caller supplies a NUL-terminated format string.
    let format = unsafe { core::ffi::CStr::from_ptr(format) }.to_bytes();
    let mut output = Output { emit, length: 0 };
    let mut index = 0;
    while index < format.len() {
        let literal_start = index;
        while index < format.len() && format[index] != b'%' {
            index += 1;
        }
        output.bytes(&format[literal_start..index])?;
        if index == format.len() {
            break;
        }
        index += 1;
        if format.get(index) == Some(&b'%') {
            output.bytes(b"%")?;
            index += 1;
            continue;
        }
        let mut flags = Flags::default();
        loop {
            match format.get(index) {
                Some(b'-') => flags.left = true,
                Some(b'+') => flags.plus = true,
                Some(b' ') => flags.space = true,
                Some(b'#') => flags.alternate = true,
                Some(b'0') => flags.zero = true,
                Some(b'\'') => return Err(ENOTSUP),
                _ => break,
            }
            index += 1;
        }
        let width = if format.get(index) == Some(&b'*') {
            index += 1;
            if format.get(index).is_some_and(u8::is_ascii_digit) {
                return Err(ENOTSUP);
            }
            // SAFETY: '*' requires an int argument.
            let width = unsafe { arguments.arg::<c_int>() };
            if width < 0 {
                flags.left = true;
            }
            let width = width.unsigned_abs() as usize;
            if width > MAX_RESULT {
                return Err(ERRNO_EOVERFLOW);
            }
            width
        } else {
            decimal(format, &mut index)?
        };
        if format.get(index) == Some(&b'$') {
            return Err(ENOTSUP);
        }
        let precision = if format.get(index) == Some(&b'.') {
            index += 1;
            if format.get(index) == Some(&b'*') {
                index += 1;
                if format.get(index).is_some_and(u8::is_ascii_digit) {
                    return Err(ENOTSUP);
                }
                // SAFETY: precision '*' requires an int argument.
                let precision = unsafe { arguments.arg::<c_int>() };
                (precision >= 0).then_some(precision as usize)
            } else {
                Some(decimal(format, &mut index)?)
            }
        } else {
            None
        };
        let length = match format.get(index) {
            Some(b'h') => {
                index += 1;
                if format.get(index) == Some(&b'h') {
                    index += 1;
                    Length::Char
                } else {
                    Length::Short
                }
            }
            Some(b'l') => {
                index += 1;
                if format.get(index) == Some(&b'l') {
                    index += 1;
                    Length::LongLong
                } else {
                    Length::Long
                }
            }
            Some(b'j') => {
                index += 1;
                Length::IntMax
            }
            Some(b'z') => {
                index += 1;
                Length::Size
            }
            Some(b't') => {
                index += 1;
                Length::PtrDiff
            }
            Some(b'L') => {
                index += 1;
                Length::LongDouble
            }
            _ => Length::Int,
        };
        let conversion = *format.get(index).ok_or(ERRNO_EINVAL)?;
        index += 1;
        match conversion {
            b'd' | b'i' | b'u' | b'o' | b'x' | b'X' => {
                if length == Length::LongDouble {
                    return Err(ENOTSUP);
                }
                let signed = matches!(conversion, b'd' | b'i');
                // SAFETY: each read uses the promoted C type required by the
                // conversion and length. intmax_t is i64 in the native ABI.
                let (magnitude, negative) = unsafe {
                    if signed {
                        let value = match length {
                            Length::Char => arguments.arg::<c_int>() as i8 as i64,
                            Length::Short => arguments.arg::<c_int>() as i16 as i64,
                            Length::Int => arguments.arg::<c_int>() as i64,
                            Length::Long => arguments.arg::<c_long>() as i64,
                            Length::LongLong | Length::IntMax => arguments.arg::<i64>(),
                            Length::Size | Length::PtrDiff => arguments.arg::<isize>() as i64,
                            Length::LongDouble => unreachable!(),
                        };
                        (value.unsigned_abs(), value < 0)
                    } else {
                        let value = match length {
                            Length::Char => arguments.arg::<c_int>() as u8 as u64,
                            Length::Short => arguments.arg::<c_int>() as u16 as u64,
                            Length::Int => arguments.arg::<u32>() as u64,
                            Length::Long => arguments.arg::<c_ulong>() as u64,
                            Length::LongLong | Length::IntMax => arguments.arg::<u64>(),
                            Length::Size | Length::PtrDiff => arguments.arg::<usize>() as u64,
                            Length::LongDouble => unreachable!(),
                        };
                        (value, false)
                    }
                };
                integer(
                    &mut output,
                    magnitude,
                    negative,
                    conversion,
                    &flags,
                    width,
                    precision,
                )?;
            }
            b'p' if length == Length::Int => {
                // SAFETY: %p consumes a void pointer.
                let pointer = unsafe { arguments.arg::<*const c_void>() };
                integer(
                    &mut output,
                    pointer as usize as u64,
                    false,
                    b'p',
                    &flags,
                    width,
                    precision,
                )?;
            }
            b'c' | b's' if length == Length::Int => {
                let character;
                let bytes = if conversion == b'c' {
                    // SAFETY: a char is promoted to int for varargs.
                    character = [unsafe { arguments.arg::<c_int>() } as u8];
                    &character[..]
                } else {
                    // SAFETY: %s consumes a pointer to readable character data.
                    let pointer = unsafe { arguments.arg::<*const u8>() };
                    let limit = precision.unwrap_or(usize::MAX);
                    if pointer.is_null() && limit != 0 {
                        return Err(ERRNO_EINVAL);
                    }
                    let mut length = 0;
                    // SAFETY: caller supplies readable bytes up to precision
                    // or the first NUL. Zero precision does not read pointer.
                    while length < limit && unsafe { *pointer.add(length) } != 0 {
                        length += 1;
                        if length > MAX_RESULT {
                            return Err(ERRNO_EOVERFLOW);
                        }
                    }
                    if length == 0 {
                        &[]
                    } else {
                        // SAFETY: the scan established this initialized range.
                        unsafe { core::slice::from_raw_parts(pointer, length) }
                    }
                };
                let padding = width.saturating_sub(bytes.len());
                if !flags.left {
                    output.repeat(b' ', padding)?;
                }
                output.bytes(bytes)?;
                if flags.left {
                    output.repeat(b' ', padding)?;
                }
            }
            b'a' | b'A' | b'e' | b'E' | b'f' | b'F' | b'g' | b'G' | b'n' | b'C' | b'S' | b'c'
            | b's' | b'p' | b'$' => return Err(ENOTSUP),
            _ => return Err(ERRNO_EINVAL),
        }
    }
    Ok(output.length)
}

/// # Safety
/// The format/argument contract is that of snprintf. If capacity is nonzero,
/// destination must point to at least capacity writable bytes, disjoint from
/// the format and every string argument read by this call.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn vsnprintf(
    destination: *mut c_char,
    capacity: usize,
    format_string: *const c_char,
    arguments: VaList<'_>,
) -> c_int {
    if capacity != 0 && destination.is_null() {
        return crate::fail(ERRNO_EINVAL);
    }
    let mut written = 0usize;
    // SAFETY: the caller supplies the matching arguments and writable buffer.
    let result = unsafe {
        format(format_string, arguments, |chunk| {
            let room = capacity.saturating_sub(1).saturating_sub(written);
            match chunk {
                Chunk::Bytes(bytes) => {
                    let length = room.min(bytes.len());
                    if length != 0 {
                        core::ptr::copy_nonoverlapping(
                            bytes.as_ptr(),
                            destination.add(written).cast(),
                            length,
                        );
                        written += length;
                    }
                }
                Chunk::Repeat(byte, length) => {
                    let length = room.min(length);
                    if length != 0 {
                        core::ptr::write_bytes(destination.add(written).cast::<u8>(), byte, length);
                        written += length;
                    }
                }
            }
            Ok(())
        })
    };
    if capacity != 0 {
        // SAFETY: at most capacity-1 bytes were written above.
        unsafe { *destination.add(written) = 0 };
    }
    match result {
        Ok(length) => length as c_int,
        Err(errno) => crate::fail(errno),
    }
}

/// # Safety
/// Same buffer and format requirements as vsnprintf, with matching varargs.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn snprintf(
    destination: *mut c_char,
    capacity: usize,
    format_string: *const c_char,
    arguments: ...
) -> c_int {
    // SAFETY: forwarded from the caller's contract.
    unsafe { vsnprintf(destination, capacity, format_string, arguments) }
}

/// # Safety
/// Destination must have space for the complete output and a trailing NUL;
/// all other requirements are those of vsnprintf.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn vsprintf(
    destination: *mut c_char,
    format_string: *const c_char,
    arguments: VaList<'_>,
) -> c_int {
    // SAFETY: the caller guarantees sufficient destination capacity.
    unsafe { vsnprintf(destination, usize::MAX, format_string, arguments) }
}

/// # Safety
/// Same buffer and format requirements as vsprintf, with matching varargs.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn sprintf(
    destination: *mut c_char,
    format_string: *const c_char,
    arguments: ...
) -> c_int {
    // SAFETY: forwarded from the caller's contract.
    unsafe { vsprintf(destination, format_string, arguments) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::__errno_location;

    fn bytes(buffer: &[c_char]) -> &[u8] {
        // SAFETY: tests inspect initialized character storage as bytes.
        unsafe { core::slice::from_raw_parts(buffer.as_ptr().cast(), buffer.len()) }
    }

    #[test]
    fn truncation_and_zero_capacity_count_complete_output() {
        let mut buffer = [0x55; 8];
        unsafe {
            assert_eq!(
                snprintf(
                    buffer.as_mut_ptr(),
                    5,
                    c"%s:%d".as_ptr(),
                    c"abcdef".as_ptr(),
                    12
                ),
                9
            );
            assert_eq!(bytes(&buffer), b"abcd\0UUU");
            assert_eq!(
                snprintf(
                    core::ptr::null_mut(),
                    0,
                    c"%s:%d".as_ptr(),
                    c"abcdef".as_ptr(),
                    12
                ),
                9
            );
            assert_eq!(snprintf(buffer.as_mut_ptr(), 1, c"hello".as_ptr()), 5);
            assert_eq!(buffer[0], 0);
        }
    }

    #[test]
    fn integer_flags_precision_and_signed_minimum() {
        let mut buffer = [0; 160];
        unsafe {
            let result = snprintf(
                buffer.as_mut_ptr(),
                buffer.len(),
                c"%+08d|%#08x|%#.0o|%.0u|% 5d|%lld|%llu".as_ptr(),
                -42,
                0xabu32,
                0u32,
                0u32,
                7,
                i64::MIN,
                u64::MAX,
            );
            let expected = b"-0000042|0x0000ab|0||    7|-9223372036854775808|18446744073709551615";
            assert_eq!(result as usize, expected.len());
            assert_eq!(&bytes(&buffer)[..expected.len()], expected);
            assert_eq!(buffer[expected.len()], 0);
        }
    }

    #[test]
    fn star_width_precision_and_length_modifiers() {
        let mut buffer = [0; 160];
        unsafe {
            let result = snprintf(
                buffer.as_mut_ptr(),
                buffer.len(),
                c"[%*.*s][%0*.*d]|%hhd,%hhu,%hd,%hu,%ld,%ju,%zd,%tu".as_ptr(),
                -6,
                3,
                c"abcdef".as_ptr(),
                7,
                4,
                -12,
                255,
                511,
                65535,
                131071,
                c_long::MIN,
                u64::MAX,
                -9isize,
                10usize,
            );
            let expected = b"[abc   ][  -0012]|-1,255,-1,65535,-9223372036854775808,18446744073709551615,-9,10";
            assert_eq!(result as usize, expected.len());
            assert_eq!(&bytes(&buffer)[..expected.len()], expected);
        }
    }

    #[test]
    fn precision_limits_unterminated_strings_and_negative_precision_is_ignored() {
        let mut buffer = [0; 64];
        let unterminated = [b'a', b'b', b'c'];
        unsafe {
            let result = snprintf(
                buffer.as_mut_ptr(),
                buffer.len(),
                c"%.*s|%.*s|%.0s|%c|%%|%p".as_ptr(),
                3,
                unterminated.as_ptr(),
                -1,
                c"full".as_ptr(),
                core::ptr::null::<c_char>(),
                0x141,
                0x123usize as *const c_void,
            );
            let expected = b"abc|full||A|%|0x123\0";
            assert_eq!(result as usize, expected.len() - 1);
            assert_eq!(&bytes(&buffer)[..expected.len()], expected);
        }
    }

    #[test]
    fn large_padding_is_counted_without_visiting_discarded_bytes() {
        unsafe {
            assert_eq!(
                snprintf(core::ptr::null_mut(), 0, c"%2147483647d".as_ptr(), 1),
                c_int::MAX
            );
            assert_eq!(
                snprintf(core::ptr::null_mut(), 0, c"%2147483647d!".as_ptr(), 1),
                -1
            );
            assert_eq!(*__errno_location(), ERRNO_EOVERFLOW);
            assert_eq!(
                snprintf(core::ptr::null_mut(), 0, c"%*d".as_ptr(), c_int::MIN, 1),
                -1
            );
            assert_eq!(*__errno_location(), ERRNO_EOVERFLOW);
        }
    }

    #[test]
    fn unsupported_and_malformed_formats_fail_and_terminate() {
        let mut buffer = [0x55; 8];
        unsafe {
            for format in [c"x%f", c"x%n", c"x%2$d", c"x%ls", c"x%*2$d"] {
                assert_eq!(
                    snprintf(buffer.as_mut_ptr(), buffer.len(), format.as_ptr()),
                    -1
                );
                assert_eq!(*__errno_location(), ENOTSUP);
                assert_eq!(&bytes(&buffer)[..2], b"x\0");
            }
            assert_eq!(
                snprintf(buffer.as_mut_ptr(), buffer.len(), c"x%".as_ptr()),
                -1
            );
            assert_eq!(*__errno_location(), ERRNO_EINVAL);
            assert_eq!(&bytes(&buffer)[..2], b"x\0");
        }
    }

    #[test]
    fn va_list_forwarding_and_sink_failure() {
        unsafe extern "C" fn forwarded(
            destination: *mut c_char,
            format: *const c_char,
            args: ...
        ) -> c_int {
            unsafe { vsprintf(destination, format, args) }
        }
        unsafe extern "C" fn failing(format_string: *const c_char, args: ...) -> c_int {
            match unsafe { format(format_string, args, |_| Err(5)) } {
                Ok(_) => 0,
                Err(errno) => errno,
            }
        }
        let mut buffer = [0; 64];
        unsafe {
            assert_eq!(
                forwarded(
                    buffer.as_mut_ptr(),
                    c"%d%d%d%d%d%d%d%d%d%d".as_ptr(),
                    0,
                    1,
                    2,
                    3,
                    4,
                    5,
                    6,
                    7,
                    8,
                    9
                ),
                10
            );
            assert_eq!(&bytes(&buffer)[..11], b"0123456789\0");
            assert_eq!(failing(c"literal %d".as_ptr(), 1), 5);
            *__errno_location() = 91;
            assert_eq!(
                sprintf(buffer.as_mut_ptr(), c"%s:%d".as_ptr(), c"ok".as_ptr(), 7),
                4
            );
            assert_eq!(&bytes(&buffer)[..5], b"ok:7\0");
            assert_eq!(*__errno_location(), 91);
        }
    }

    #[cfg(not(target_os = "scarlet"))]
    #[test]
    fn integer_flag_combinations_match_host_libc() {
        unsafe extern "C" {
            #[link_name = "snprintf"]
            fn system_snprintf(
                destination: *mut c_char,
                capacity: usize,
                format: *const c_char,
                ...
            ) -> c_int;
        }
        let mut ours = [0; 128];
        let mut reference = [0; 128];
        for flags in ["", "-", "+", " ", "0", "#", "-0", "+0", "#0"] {
            for width in ["", "1", "23"] {
                for precision in ["", ".0", ".1", ".24"] {
                    for conversion in ['d', 'u', 'o', 'x', 'X'] {
                        if (conversion != 'd' && (flags.contains('+') || flags.contains(' ')))
                            || (matches!(conversion, 'd' | 'u') && flags.contains('#'))
                        {
                            continue;
                        }
                        let format = std::ffi::CString::new(format!(
                            "[%{flags}{width}{precision}ll{conversion}]"
                        ))
                        .unwrap();
                        for bits in [
                            0u64,
                            1,
                            8,
                            42,
                            0xabcd,
                            i64::MAX as u64,
                            1u64 << 63,
                            u64::MAX,
                        ] {
                            // SAFETY: the selected conversion receives exactly
                            // its C type in both implementations. Both buffers
                            // exceed the largest field in this matrix.
                            let (actual, expected) = unsafe {
                                if conversion == 'd' {
                                    (
                                        snprintf(
                                            ours.as_mut_ptr(),
                                            ours.len(),
                                            format.as_ptr(),
                                            bits as i64,
                                        ),
                                        system_snprintf(
                                            reference.as_mut_ptr(),
                                            reference.len(),
                                            format.as_ptr(),
                                            bits as i64,
                                        ),
                                    )
                                } else {
                                    (
                                        snprintf(
                                            ours.as_mut_ptr(),
                                            ours.len(),
                                            format.as_ptr(),
                                            bits,
                                        ),
                                        system_snprintf(
                                            reference.as_mut_ptr(),
                                            reference.len(),
                                            format.as_ptr(),
                                            bits,
                                        ),
                                    )
                                }
                            };
                            assert_eq!(actual, expected, "{format:?}, {bits:#x}");
                            assert!(expected >= 0);
                            assert_eq!(
                                &ours[..actual as usize + 1],
                                &reference[..expected as usize + 1],
                                "{format:?}, {bits:#x}"
                            );
                        }
                    }
                }
            }
        }
    }
}
